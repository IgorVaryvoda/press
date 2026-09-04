//! Read what a folder of images actually contains.
//!
//! Everything here is header-only. Decoding a 6000px JPEG to learn it is 6000px wide
//! costs a hundred times what reading its header costs, and a shoot folder has
//! thousands of them.

use std::{
    borrow::Cow,
    ops::{ControlFlow, Range},
    path::{Path, PathBuf},
    sync::{
        Arc,
        atomic::{AtomicBool, Ordering},
    },
    time::Duration,
};

use image::{
    AnimationDecoder, DynamicImage, ImageDecoder, ImageFormat, ImageReader,
    codecs::{gif::GifDecoder, png::PngDecoder, webp::WebPDecoder},
    metadata::Orientation,
};
use walkdir::WalkDir;

/// Camera raw formats. Most are TIFF containers, so a plain header read reports the
/// embedded preview — a 6000x4000 NEF comes back as a 160x120 TIFF, which makes every
/// derived number a lie. They are also not web delivery candidates. Counted, not listed.
const RAW_EXTENSIONS: [&str; 9] = [
    "nef", "cr2", "cr3", "arw", "dng", "orf", "rw2", "raf", "srw",
];

/// Apple's HEIC and the HEIF family it sits in — what every recent iPhone writes by
/// default, and what mirrorless bodies increasingly offer. Nothing here links a HEIC
/// decoder, so these cannot be measured. Counted like raw rather than dropped: a
/// folder straight off a phone otherwise audits as empty and the app looks broken.
const HEIC_EXTENSIONS: [&str; 5] = ["heic", "heif", "hif", "avci", "heix"];

fn has_extension(path: &Path, extensions: &[&str]) -> bool {
    path.extension()
        .and_then(|extension| extension.to_str())
        .is_some_and(|extension| extensions.contains(&extension.to_ascii_lowercase().as_str()))
}

pub fn is_raw(path: &Path) -> bool {
    has_extension(path, &RAW_EXTENSIONS)
}

pub fn is_heic(path: &Path) -> bool {
    has_extension(path, &HEIC_EXTENSIONS)
}

/// Converted files land here, inside the folder being audited. A second run would
/// otherwise list its own output and offer to convert it again.
pub const OUTPUT_DIR: &str = "optimized";

/// Replace mode moves every original here, mirroring the audited tree. Skipped by
/// the walk like `OUTPUT_DIR`: these are the files the folder used to hold, and
/// offering to convert them again would undo the run that put them here.
pub const BACKUP_DIR: &str = "press-originals";

/// The backup is not a folder to browse into either. Auditing it and converting
/// what is inside would rewrite the originals in place of themselves, and no
/// restore could put back what it no longer has.
fn is_backup_folder(path: &Path) -> bool {
    path.file_name() == Some(std::ffi::OsStr::new(BACKUP_DIR))
}

/// A half-written output belonging to a run in flight. It is never an input, and
/// a browser that offers it offers a file that is about to be renamed away.
fn is_partial(path: &Path) -> bool {
    path.extension() == Some(std::ffi::OsStr::new("part"))
}

/// Extensions macOS keeps as opaque packages. Some are permission-walled, and the
/// rest are directory trees whose internal images are not web-delivery candidates.
/// Skipped by design like camera raw, counted for the same reason.
const PACKAGE_EXTENSIONS: [&str; 13] = [
    "photoslibrary",
    "photolibrary",
    "aplibrary",
    "lrdata",
    "app",
    "bundle",
    "framework",
    "plugin",
    "kext",
    "xpc",
    "appex",
    "wdgt",
    "docset",
];

/// Enough rows for the first viewport, then larger updates so a fast scan does not
/// spend its win rebuilding the table hundreds of times.
const FIRST_SCAN_BATCH: usize = 32;
const NEXT_SCAN_BATCH: usize = 256;
const PROGRESSIVE_SCAN_BATCHES: [usize; 5] =
    [FIRST_SCAN_BATCH, NEXT_SCAN_BATCH, 1_024, 4_096, 8_192];

/// Publishes enough rows for the first viewport, then grows each update until the
/// table has a useful body. Larger scans retain the final size so their callbacks
/// stay bounded rather than returning to per-row work.
struct ProductionBatchState {
    published: usize,
    next_size: usize,
    size_index: usize,
}

impl ProductionBatchState {
    fn new() -> Self {
        Self {
            published: 0,
            next_size: PROGRESSIVE_SCAN_BATCHES[0],
            size_index: 0,
        }
    }

    fn ready_range(&mut self, completed: usize) -> Option<Range<usize>> {
        if completed - self.published != self.next_size {
            return None;
        }
        let range = self.published..completed;
        self.published = completed;
        self.size_index = (self.size_index + 1).min(PROGRESSIVE_SCAN_BATCHES.len() - 1);
        self.next_size = PROGRESSIVE_SCAN_BATCHES[self.size_index];
        Some(range)
    }

    fn final_tail(&mut self, completed: usize) -> Option<Range<usize>> {
        (self.published < completed).then(|| {
            let range = self.published..completed;
            self.published = completed;
            range
        })
    }
}

/// True when this directory is one macOS keeps opaque. Packages are a macOS
/// concept — on other systems these names are just folders, so they keep being
/// walked there.
fn is_opaque_package(path: &Path) -> bool {
    if !cfg!(target_os = "macos") {
        return false;
    }
    has_extension(path, &PACKAGE_EXTENSIONS)
}

/// What a folder holds.
pub struct Scan {
    pub entries: Vec<Entry>,
    /// Camera raw files left out of the list, so the total is not silently short.
    /// A count, not names: raw is excluded by design and a photographer knows they
    /// have it. Nothing in the window would act on the list.
    pub skipped_raw: usize,
    /// HEIC/HEIF files left out of the list for want of a decoder, counted like raw
    /// so an iPhone folder says what it holds instead of showing nothing.
    pub skipped_heic: usize,
    /// macOS packages the walk never entered, counted like raw for the same
    /// reason: they are excluded by design and the total says so.
    pub skipped_packages: usize,
    /// Files that look like images by extension but would not decode. Counted in
    /// the lane, named in the scan toast: "3 would not decode" tells you a folder
    /// has a problem, and the toast says where to look for it.
    pub unreadable: Vec<PathBuf>,
    /// Directories the walk could not enter, named like `unreadable`: "permission
    /// denied" somewhere in the tree means every number above is short, and a count
    /// alone would leave the user no place to look.
    pub walk_errors: Vec<PathBuf>,
    /// Files already sitting in this root's own `OUTPUT_DIR`. The walk steps over them
    /// anyway, so counting them is free, and a second run is otherwise silent about
    /// what it is about to write over.
    ///
    /// Only this root's output folder counts. The walk skips every path with an
    /// `optimized` component in it, wherever it sits, but a run rooted at `~/Pictures`
    /// would not touch `~/Pictures/Screenshots/optimized`, so warning about it would
    /// name the wrong 5,415 files.
    pub existing_output: usize,
}

/// One file-browser page: direct child folders plus direct image files. Unlike
/// `scan`, this never walks into a child directory.
pub struct Browse {
    pub folders: Vec<PathBuf>,
    pub scan: Scan,
}

/// A cancelled scan never exposes its partial facts as a completed audit.
pub(crate) enum ScanOutcome {
    Complete(Scan),
    Cancelled,
}

#[derive(Clone, Debug, PartialEq)]
pub struct Entry {
    pub path: PathBuf,
    pub format: FileFormat,
    pub width: u32,
    pub height: u32,
    /// Bytes on disk, not decoded size.
    pub bytes: u64,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum FileFormat {
    Image(ImageFormat),
    JpegXl,
}

impl FileFormat {
    fn extensions_str(self) -> &'static [&'static str] {
        match self {
            Self::Image(format) => format.extensions_str(),
            Self::JpegXl => &["jxl"],
        }
    }
}

impl From<ImageFormat> for FileFormat {
    fn from(format: ImageFormat) -> Self {
        Self::Image(format)
    }
}

impl Entry {
    pub fn name(&self) -> String {
        self.name_lossy().into_owned()
    }

    pub fn name_lossy(&self) -> Cow<'_, str> {
        self.path
            .file_name()
            .map(|name| name.to_string_lossy())
            .unwrap_or_default()
    }

    /// Bytes per pixel of output. The number that says whether a file is carrying
    /// weight it does not need — a photographic JPEG lands near 0.2, a screenshot
    /// saved as PNG can be ten times that.
    pub fn bytes_per_pixel(&self) -> f32 {
        let pixels = (self.width as u64) * (self.height as u64);
        if pixels == 0 {
            return 0.;
        }
        self.bytes as f32 / pixels as f32
    }

    /// True when the extension disagrees with the bytes inside the file. The first
    /// folder this was ever pointed at held 169 files named `.webp`, 59 of which
    /// were PNG — the sort of thing an audit should say out loud rather than leave
    /// for someone to notice in a column.
    pub fn extension_lies(&self) -> bool {
        let Some(extension) = self.path.extension().and_then(|name| name.to_str()) else {
            // No extension is not a lie, just an omission.
            return false;
        };
        // `jpg` and `jpeg` are one format under two spellings, as are `tif` and
        // `tiff`; `extensions_str` lists every name the format answers to.
        !self
            .format
            .extensions_str()
            .iter()
            .any(|expected| extension.eq_ignore_ascii_case(expected))
    }
}

/// Read one file's header. `None` when it is not an image we can read.
pub fn probe(path: &Path) -> Option<Entry> {
    let bytes = std::fs::metadata(path).ok()?.len();
    if let Ok(reader) = ImageReader::open(path).and_then(ImageReader::with_guessed_format)
        && let Some(format) = reader.format()
        && let Ok(mut decoder) = reader.into_decoder()
    {
        let (mut width, mut height) = decoder.dimensions();
        if orientation_swaps_dimensions(decoder.orientation().unwrap_or(Orientation::NoTransforms))
        {
            std::mem::swap(&mut width, &mut height);
        }
        return Some(Entry {
            path: path.to_path_buf(),
            format: format.into(),
            width,
            height,
            bytes,
        });
    }

    let info = crate::jxl::probe(path)?;
    Some(Entry {
        path: path.to_path_buf(),
        format: FileFormat::JpegXl,
        width: info.width,
        height: info.height,
        bytes,
    })
}

/// Decode a file, choosing the decoder by what is inside it rather than by what it
/// is called.
///
/// `image::open` picks its decoder from the extension, which is the one thing this
/// app already knows it cannot trust — `probe` reads the format from the magic bytes
/// precisely because extensions lie. Using both meant the files the audit flagged as
/// mislabelled were exactly the files it then failed to convert, thumbnail or open,
/// with no error beyond a missing row.
pub fn decode(path: &Path) -> Option<DynamicImage> {
    if let Ok(reader) = ImageReader::open(path).and_then(ImageReader::with_guessed_format)
        && let Ok(mut decoder) = reader.into_decoder()
    {
        let orientation = decoder.orientation().unwrap_or(Orientation::NoTransforms);
        if let Ok(mut image) = DynamicImage::from_decoder(decoder) {
            image.apply_orientation(orientation);
            return Some(image);
        }
    }
    crate::jxl::decode_path(path).map(|(image, _)| image)
}

pub fn decode_bytes(bytes: &[u8]) -> Option<DynamicImage> {
    image::load_from_memory(bytes)
        .ok()
        .or_else(|| crate::jxl::decode_bytes(bytes))
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum ConversionDecodeError {
    Failed,
    AnimatedGif,
    AnimatedPng,
    AnimatedWebP,
    AnimatedJpegXl,
}

/// Decode one still image for conversion, with the source's ICC profile beside it.
///
/// GIF, APNG and animated WebP all have to be refused by name. Each of their
/// decoders hands back a single frame as a `DynamicImage` — frame zero for WebP, the
/// default image for APNG — so accepting one here would report "converted, -80%" and
/// quietly return a still.
///
/// The profile travels with the pixels because it is the only thing that says these
/// pixels are Display P3 or Adobe RGB rather than sRGB. Written without it, a wide
/// gamut photo is rendered as sRGB by every browser and its colours shift.
///
/// `max_edge` is a hint about the size the caller is heading for, not a promise about
/// the size that comes back: only JPEG can act on it, and only in factors of two. The
/// caller still runs `MaxEdge::apply` on the result to reach the exact edge.
pub fn decode_for_conversion(
    path: &Path,
    max_edge: crate::convert::MaxEdge,
) -> Result<(DynamicImage, Option<Vec<u8>>), ConversionDecodeError> {
    if let Ok(reader) = ImageReader::open(path).and_then(ImageReader::with_guessed_format) {
        if reader.format() == Some(ImageFormat::Gif) {
            let file = std::fs::File::open(path).map_err(|_| ConversionDecodeError::Failed)?;
            let decoder = GifDecoder::new(std::io::BufReader::new(file))
                .map_err(|_| ConversionDecodeError::Failed)?;
            let mut frames = decoder.into_frames();
            let first = frames
                .next()
                .ok_or(ConversionDecodeError::Failed)?
                .map_err(|_| ConversionDecodeError::Failed)?;
            if frames.next().is_some() {
                return Err(ConversionDecodeError::AnimatedGif);
            }
            return Ok((DynamicImage::ImageRgba8(first.into_buffer()), None));
        }

        if reader.format() == Some(ImageFormat::Png) {
            let decoder =
                PngDecoder::new(reader.into_inner()).map_err(|_| ConversionDecodeError::Failed)?;
            if decoder.is_apng().unwrap_or(false) {
                return Err(ConversionDecodeError::AnimatedPng);
            }
            return still(decoder);
        }

        if reader.format() == Some(ImageFormat::WebP) {
            let decoder =
                WebPDecoder::new(reader.into_inner()).map_err(|_| ConversionDecodeError::Failed)?;
            if decoder.has_animation() {
                return Err(ConversionDecodeError::AnimatedWebP);
            }
            return still(decoder);
        }

        if reader.format() == Some(ImageFormat::Jpeg)
            && let Some(decoded) = scaled_jpeg(path, max_edge)
        {
            return Ok(decoded);
        }

        if let Ok(decoder) = reader.into_decoder() {
            return still(decoder);
        }
    }

    let info = crate::jxl::probe(path).ok_or(ConversionDecodeError::Failed)?;
    if info.animated {
        return Err(ConversionDecodeError::AnimatedJpegXl);
    }
    crate::jxl::decode_path(path)
        .map(|(image, profile)| (image, rgb_profile(profile)))
        .ok_or(ConversionDecodeError::Failed)
}

/// Decode a JPEG with libjpeg-turbo, stopping the inverse DCT at the smallest
/// scale that still covers `edge`.
///
/// Conversion used to decode every source at full size and throw most of it away:
/// exporting 1600px out of a 6000px JPEG allocated the whole 6000px image first, once
/// per worker, and eight workers over 45MP sources is where the resident set went.
/// libjpeg-turbo can finish the DCT at a half, a quarter or an eighth, so the
/// intermediate is now at most twice the exported edge.
///
/// Full-size sources take the same path rather than the image crate's decoder:
/// `jpeg-decoder` is scalar while turbojpeg is SIMD, so a full-size photo decodes
/// several times faster for pixels nobody can tell apart. `None` means "decode
/// this the old way": a JPEG libjpeg-turbo will not hand back as packed pixels —
/// CMYK and YCCK, which it only decompresses to CMYK, along with lossless and
/// twelve-bit ones. Arithmetic coding it does read, so those take this path like
/// any other.
///
/// `MaxEdge::apply` still finishes the job with Lanczos, so the only difference from
/// a full decode is that scaled-DCT step; the exported pixels are not bit-identical
/// to what a full decode produced, and they are not meant to be.
fn scaled_jpeg(
    path: &Path,
    max_edge: crate::convert::MaxEdge,
) -> Option<(DynamicImage, Option<Vec<u8>>)> {
    let bytes = std::fs::read(path).ok()?;
    let mut decompressor = turbojpeg::Decompressor::new().ok()?;
    let header = decompressor.read_header(&bytes).ok()?;
    // `ONE` when the run wants full size or nothing smaller covers the edge.
    // Falling back there would hand every full-size photo to the scalar decoder.
    let factor = max_edge.0.map_or(turbojpeg::ScalingFactor::ONE, |edge| {
        crate::thumbs::jpeg_scaling_factor(header.width.max(header.height), edge)
    });
    decompressor.set_scaling_factor(factor).ok()?;
    let (width, height) = (factor.scale(header.width), factor.scale(header.height));
    // A grayscale JPEG has to leave here with one channel, the way the full decode
    // leaves it. Read as RGB it would reach the encoders as three components where the
    // source had one, and a grayscale photo re-encoded as JPEG would grow.
    //
    // Otherwise RGB rather than RGBA: a JPEG has no alpha, and the fourth channel would
    // be a quarter of this allocation spent carrying 255.
    let gray = header.subsamp == turbojpeg::Subsamp::Gray;
    let channels = if gray { 1 } else { 3 };
    let mut pixels = vec![0u8; width.checked_mul(height)?.checked_mul(channels)?];
    decompressor
        .decompress(
            &bytes,
            turbojpeg::Image {
                pixels: &mut pixels,
                width,
                pitch: width.checked_mul(channels)?,
                height,
                format: if gray {
                    turbojpeg::PixelFormat::GRAY
                } else {
                    turbojpeg::PixelFormat::RGB
                },
            },
        )
        .ok()?;
    let (width, height) = (width.try_into().ok()?, height.try_into().ok()?);

    // turbojpeg hands back the stored pixels in the stored order and knows nothing
    // about EXIF, so orientation and profile still come off the image crate's header
    // read, and the rotation is applied after the scaled decode — the same order as a
    // full decode, and the only order in which the dimensions come out swapped.
    let mut decoder = ImageReader::with_format(std::io::Cursor::new(&bytes), ImageFormat::Jpeg)
        .into_decoder()
        .ok()?;
    let orientation = decoder.orientation().unwrap_or(Orientation::NoTransforms);
    let profile = rgb_profile(decoder.icc_profile().ok().flatten());
    let mut image = if gray {
        DynamicImage::ImageLuma8(image::GrayImage::from_raw(width, height, pixels)?)
    } else {
        DynamicImage::ImageRgb8(image::RgbImage::from_raw(width, height, pixels)?)
    };
    image.apply_orientation(orientation);
    Some((image, profile))
}

/// Orientation and colour profile both have to be read off the decoder before
/// `from_decoder` consumes it.
fn still<D: ImageDecoder>(
    mut decoder: D,
) -> Result<(DynamicImage, Option<Vec<u8>>), ConversionDecodeError> {
    let orientation = decoder.orientation().unwrap_or(Orientation::NoTransforms);
    let profile = rgb_profile(decoder.icc_profile().ok().flatten());
    let mut image =
        DynamicImage::from_decoder(decoder).map_err(|_| ConversionDecodeError::Failed)?;
    image.apply_orientation(orientation);
    Ok((image, profile))
}

/// The source's RGB profile, read from its header alone.
///
/// The native decoders behind the preview are fast because they do one job; neither
/// libjpeg-turbo nor libwebp hands back an ICC profile. A comparison built from those
/// pixels still has to write the profile the file carried, or the size it reports is
/// short by exactly the bytes the writer would attach.
pub fn icc_profile(path: &Path) -> Option<Vec<u8>> {
    let reader = ImageReader::open(path)
        .and_then(ImageReader::with_guessed_format)
        .ok()?;
    let mut decoder = reader.into_decoder().ok()?;
    rgb_profile(decoder.icc_profile().ok().flatten())
}

/// Keep a profile only when it describes the pixels the encoders are handed.
///
/// A decoder answers with the profile the *file* carried, not the one its pixels come
/// back in: the JPEG decoder converts CMYK and YCCK to RGB and still reports the CMYK
/// profile, and a grayscale source keeps a GRAY profile that the encoders' `to_rgb8`
/// then contradicts. Every encoder here is handed RGB or RGBA, so anything else
/// describes the wrong thing, and a wrong profile is worse than none.
fn rgb_profile(profile: Option<Vec<u8>>) -> Option<Vec<u8>> {
    let profile = profile?;
    // ICC header: profile size big-endian at 0, data colour space at 16, 128 bytes
    // before the tag table starts. A header that disagrees with the bytes around it
    // is not a profile anyone should copy forward.
    let size = u32::from_be_bytes(profile.get(0..4)?.try_into().ok()?) as usize;
    (profile.len() >= 128 && size == profile.len() && &profile[16..20] == b"RGB ")
        .then_some(profile)
}

fn orientation_swaps_dimensions(orientation: Orientation) -> bool {
    matches!(
        orientation,
        Orientation::Rotate90
            | Orientation::Rotate270
            | Orientation::Rotate90FlipH
            | Orientation::Rotate270FlipH
    )
}

/// Walk a folder and probe every image in it, subfolders included.
/// Walk `root` for images. `output_root` is where this audit writes its output;
/// files under it are counted, never audited — otherwise the second scan of a
/// folder offers you last run's WebPs as candidates for conversion.
pub fn scan(root: &Path, output_root: &Path) -> Scan {
    let cancelled = Arc::new(AtomicBool::new(false));
    match scan_progressive_cancellable(root, output_root, cancelled, |_| ControlFlow::Continue(()))
    {
        ScanOutcome::Complete(scan) => scan,
        ScanOutcome::Cancelled => unreachable!("a private token is never cancelled"),
    }
}

/// Probe exactly the files the user chose, without walking their folder and pulling
/// unrelated neighbours into the audit.
pub fn scan_files(paths: &[PathBuf]) -> Scan {
    let mut entries = Vec::new();
    let mut skipped_raw = 0;
    let mut skipped_heic = 0;
    let mut unreadable = Vec::new();
    for path in paths {
        if is_raw(path) {
            skipped_raw += 1;
        } else if is_heic(path) {
            skipped_heic += 1;
        } else if let Some(entry) = probe(path) {
            entries.push(entry);
        } else {
            unreadable.push(path.clone());
        }
    }
    entries.sort_by_key(|entry| std::cmp::Reverse(entry.bytes));
    Scan {
        entries,
        skipped_raw,
        skipped_heic,
        skipped_packages: 0,
        unreadable,
        walk_errors: Vec::new(),
        existing_output: 0,
    }
}

fn is_hidden_folder(path: &Path) -> bool {
    cfg!(unix)
        && path
            .file_name()
            .is_some_and(|name| name.to_string_lossy().starts_with('.'))
}

fn folder_sort_key(path: &Path) -> (String, std::ffi::OsString) {
    let name = path.file_name().unwrap_or_default().to_os_string();
    (name.to_string_lossy().to_lowercase(), name)
}

fn lexical_boundary(path: &Path) -> std::io::Result<PathBuf> {
    let base = std::env::current_dir()?;
    crate::output::lexical_normalize_against(path, &base)
        .map_err(|error| std::io::Error::new(std::io::ErrorKind::InvalidInput, error.to_string()))
}

pub(crate) fn canonical_boundary(path: &Path) -> std::io::Result<PathBuf> {
    let normalized = lexical_boundary(path)?;
    let mut existing = normalized;
    let mut missing = Vec::new();
    loop {
        match std::fs::canonicalize(&existing) {
            Ok(mut canonical) => {
                for component in missing.iter().rev() {
                    canonical.push(component);
                }
                return Ok(canonical);
            }
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => {
                let Some(component) = existing.file_name().map(ToOwned::to_owned) else {
                    return Err(error);
                };
                missing.push(component);
                existing.pop();
            }
            Err(error) => return Err(error),
        }
    }
}

/// The one identity a browsed folder has, refused when it sits inside the output:
/// auditing last run's WebPs offers them back as candidates for conversion.
fn input_root(root: &Path, output_root: &Path) -> std::io::Result<PathBuf> {
    let root = canonical_boundary(root)?;
    let output = canonical_boundary(output_root).or_else(|_| lexical_boundary(output_root))?;
    if root.starts_with(output) {
        return Err(std::io::Error::new(
            std::io::ErrorKind::InvalidInput,
            "the output folder cannot also be an input folder",
        ));
    }
    Ok(root)
}

fn browse_page(
    root: &Path,
    output_root: &Path,
    cancelled: Option<&AtomicBool>,
) -> std::io::Result<Option<Browse>> {
    if cancelled.is_some_and(|flag| flag.load(Ordering::Acquire)) {
        return Ok(None);
    }
    let root = input_root(root, output_root)?;
    let mut folders = Vec::new();
    let mut entries = Vec::new();
    let mut skipped_raw = 0;
    let mut skipped_heic = 0;
    let mut skipped_packages = 0;
    let mut unreadable = Vec::new();
    let mut walk_errors = Vec::new();

    for item in std::fs::read_dir(&root)? {
        if cancelled.is_some_and(|flag| flag.load(Ordering::Acquire)) {
            return Ok(None);
        }
        let item = match item {
            Ok(item) => item,
            Err(_) => {
                walk_errors.push(root.to_path_buf());
                continue;
            }
        };
        let path = item.path();
        let file_type = match item.file_type() {
            Ok(file_type) => file_type,
            Err(_) => {
                walk_errors.push(path);
                continue;
            }
        };

        if file_type.is_dir() {
            if is_hidden_folder(&path) || is_backup_folder(&path) {
                continue;
            }
            if is_opaque_package(&path) {
                skipped_packages += 1;
            } else {
                folders.push(path);
            }
            continue;
        }
        if !file_type.is_file() || is_partial(&path) {
            continue;
        }
        if is_raw(&path) {
            skipped_raw += 1;
        } else if is_heic(&path) {
            skipped_heic += 1;
        } else if let Some(entry) = probe(&path) {
            entries.push(entry);
        } else if looks_like_an_image(&path) {
            unreadable.push(path);
        }
    }

    folders.sort_by_cached_key(|path| folder_sort_key(path));
    entries.sort_by_key(|entry| std::cmp::Reverse(entry.bytes));
    let existing_output = if is_default_output(&root, output_root) {
        let Some(count) = count_files(output_root, cancelled) else {
            return Ok(None);
        };
        count
    } else {
        0
    };
    Ok(Some(Browse {
        folders,
        scan: Scan {
            entries,
            skipped_raw,
            skipped_heic,
            skipped_packages,
            unreadable,
            walk_errors,
            existing_output,
        },
    }))
}

fn count_files(root: &Path, cancelled: Option<&AtomicBool>) -> Option<usize> {
    let mut count = 0;
    for item in WalkDir::new(root).min_depth(1) {
        if cancelled.is_some_and(|flag| flag.load(Ordering::Acquire)) {
            return None;
        }
        // The run record sits with the outputs and is not one of them, so a folder
        // holding one converted image still reports one file.
        if item.is_ok_and(|item| {
            item.file_type().is_file() && item.file_name() != crate::manifest::NAME
        }) {
            count += 1;
        }
    }
    Some(count)
}

fn is_default_output(root: &Path, output_root: &Path) -> bool {
    matches!(
        (
            canonical_boundary(&root.join(OUTPUT_DIR)),
            canonical_boundary(output_root),
        ),
        (Ok(default), Ok(output)) if default == output
    )
}

/// Read one directory level for the window's file browser. Header probes keep
/// the existing audit rows truthful without decoding image pixels or walking the
/// directory tree. The command line uses it for `--no-subfolders`.
pub fn browse(root: &Path, output_root: &Path) -> std::io::Result<Browse> {
    Ok(browse_page(root, output_root, None)?.expect("an uncancellable browse completes"))
}

pub(crate) fn browse_cancellable(
    root: &Path,
    output_root: &Path,
    cancelled: &AtomicBool,
) -> std::io::Result<Option<Browse>> {
    browse_page(root, output_root, Some(cancelled))
}

pub(crate) fn browse_folders_cancellable(
    roots: &[PathBuf],
    output_root: &Path,
    cancelled: &AtomicBool,
) -> std::io::Result<Option<Scan>> {
    browse_folders_inner(roots, output_root, Some(cancelled))
}

fn browse_folders_inner(
    roots: &[PathBuf],
    output_root: &Path,
    cancelled: Option<&AtomicBool>,
) -> std::io::Result<Option<Scan>> {
    let mut scan = Scan {
        entries: Vec::new(),
        skipped_raw: 0,
        skipped_heic: 0,
        skipped_packages: 0,
        unreadable: Vec::new(),
        walk_errors: Vec::new(),
        existing_output: 0,
    };
    for root in roots {
        let Some(browsed) = browse_page(root, output_root, cancelled)? else {
            return Ok(None);
        };
        let browsed = browsed.scan;
        scan.entries.extend(browsed.entries);
        scan.skipped_raw += browsed.skipped_raw;
        scan.skipped_heic += browsed.skipped_heic;
        scan.skipped_packages += browsed.skipped_packages;
        scan.unreadable.extend(browsed.unreadable);
        scan.walk_errors.extend(browsed.walk_errors);
    }
    if let Some(parent) = roots.first().and_then(|root| root.parent())
        && roots.iter().all(|root| root.parent() == Some(parent))
        && is_default_output(parent, output_root)
    {
        let Some(count) = count_files(output_root, cancelled) else {
            return Ok(None);
        };
        scan.existing_output = count;
    }
    scan.entries
        .sort_by_key(|entry| std::cmp::Reverse(entry.bytes));
    Ok(Some(scan))
}

/// The window's whole-tree page: the same walk the command line does, with the
/// direct child folders kept beside it so the folder tree still navigates. Batches
/// reach `publish` as headers are probed, which is how the window shows a count.
pub(crate) fn browse_tree_cancellable(
    root: &Path,
    output_root: &Path,
    cancelled: Arc<AtomicBool>,
    publish: impl FnMut(&[Entry]) -> ControlFlow<()>,
) -> std::io::Result<Option<Browse>> {
    let root = input_root(root, output_root)?;
    let folders = child_folders(&root)?;
    Ok(
        match scan_progressive_cancellable(&root, output_root, cancelled, publish) {
            ScanOutcome::Complete(scan) => Some(Browse { folders, scan }),
            ScanOutcome::Cancelled => None,
        },
    )
}

/// Folder-only listing used when a tree node expands.
pub(crate) fn child_folders(root: &Path) -> std::io::Result<Vec<PathBuf>> {
    let mut folders = Vec::new();
    for item in std::fs::read_dir(canonical_boundary(root)?)? {
        let item = item?;
        let path = item.path();
        if item.file_type()?.is_dir()
            && !is_hidden_folder(&path)
            && !is_backup_folder(&path)
            && !is_opaque_package(&path)
        {
            folders.push(path);
        }
    }
    folders.sort_by_cached_key(|path| folder_sort_key(path));
    Ok(folders)
}

/// The same complete scan, publishing small groups as their headers become ready.
/// The window can draw those rows while the remaining files are still being probed;
/// callers that need one final result keep using `scan`.
#[cfg(test)]
pub fn scan_progressive(
    root: &Path,
    output_root: &Path,
    mut publish: impl FnMut(&[Entry]),
) -> Scan {
    let cancelled = Arc::new(AtomicBool::new(false));
    match scan_progressive_cancellable(root, output_root, cancelled, |batch| {
        publish(batch);
        ControlFlow::Continue(())
    }) {
        ScanOutcome::Complete(scan) => scan,
        ScanOutcome::Cancelled => unreachable!("a private token is never cancelled"),
    }
}

/// The cancellable form is deliberately internal until the window can make its
/// handoff return promptly. An admitted synchronous callback may finish, but no
/// later callback is admitted after cancellation is observed by the collector.
#[cfg(test)]
pub(crate) fn scan_progressive_cancellable(
    root: &Path,
    output_root: &Path,
    cancelled: Arc<AtomicBool>,
    publish: impl FnMut(&[Entry]) -> ControlFlow<()>,
) -> ScanOutcome {
    let hooks = TEST_HOOKS.with(|hooks| hooks.borrow().clone().unwrap_or_default());
    scan_progressive_cancellable_with_hooks(root, output_root, cancelled, publish, hooks)
}

#[cfg(test)]
thread_local! {
    /// Seams for a window test that reaches the walk through its production entry
    /// point. Thread-local because gpui's test executor runs the walk on the test's
    /// own thread, and tests run in parallel.
    pub(crate) static TEST_HOOKS: std::cell::RefCell<Option<ScanHooks>> =
        const { std::cell::RefCell::new(None) };
}

#[cfg(not(test))]
pub(crate) fn scan_progressive_cancellable(
    root: &Path,
    output_root: &Path,
    cancelled: Arc<AtomicBool>,
    publish: impl FnMut(&[Entry]) -> ControlFlow<()>,
) -> ScanOutcome {
    scan_progressive_cancellable_inner(root, output_root, cancelled, publish)
}

#[cfg(test)]
#[derive(Clone)]
pub(crate) struct ScanHooks {
    queue_capacity: Option<usize>,
    before_walk_next: Arc<dyn Fn() + Send + Sync>,
    before_probe: Arc<dyn Fn(&Path) + Send + Sync>,
    before_callback: Arc<dyn Fn() + Send + Sync>,
    before_path_receive: Arc<dyn Fn() + Send + Sync>,
    pub(crate) before_path_send: Arc<dyn Fn() + Send + Sync>,
    before_result_receive: Arc<dyn Fn() + Send + Sync>,
    before_result_send: Arc<dyn Fn() + Send + Sync>,
}

#[cfg(test)]
impl Default for ScanHooks {
    fn default() -> Self {
        Self {
            queue_capacity: None,
            before_walk_next: Arc::new(|| {}),
            before_probe: Arc::new(|_| {}),
            before_callback: Arc::new(|| {}),
            before_path_receive: Arc::new(|| {}),
            before_path_send: Arc::new(|| {}),
            before_result_receive: Arc::new(|| {}),
            before_result_send: Arc::new(|| {}),
        }
    }
}

#[cfg(test)]
fn scan_progressive_cancellable_with_hooks(
    root: &Path,
    output_root: &Path,
    cancelled: Arc<AtomicBool>,
    publish: impl FnMut(&[Entry]) -> ControlFlow<()>,
    hooks: ScanHooks,
) -> ScanOutcome {
    scan_progressive_cancellable_inner(root, output_root, cancelled, publish, hooks)
}

fn scan_progressive_cancellable_inner(
    root: &Path,
    output_root: &Path,
    cancelled: Arc<AtomicBool>,
    mut publish: impl FnMut(&[Entry]) -> ControlFlow<()>,
    #[cfg(test)] hooks: ScanHooks,
) -> ScanOutcome {
    let threads = std::thread::available_parallelism().map_or(4, |count| count.get());
    let mut entries = Vec::new();
    let mut unreadable = Vec::new();
    let mut batches = ProductionBatchState::new();
    let mut was_cancelled = false;
    let summary = std::thread::scope(|scope| {
        enum Probed {
            Entry(Entry),
            Unreadable(PathBuf),
        }

        #[derive(Default)]
        struct WalkSummary {
            skipped_raw: usize,
            skipped_heic: usize,
            skipped_packages: usize,
            walk_errors: Vec<PathBuf>,
            existing_output: usize,
        }

        // The walker can get only one path ahead of each worker. Downloads-style
        // folders may hold a million non-images; retaining all of their PathBufs
        // before probing makes memory scale with the folder instead of useful rows.
        #[cfg(test)]
        let capacity = hooks.queue_capacity.unwrap_or(threads);
        #[cfg(not(test))]
        let capacity = threads;
        let (path_sender, path_receiver) = std::sync::mpsc::sync_channel(capacity);
        let path_receiver = std::sync::Arc::new(std::sync::Mutex::new(path_receiver));
        let walker_cancelled = cancelled.clone();
        #[cfg(test)]
        let walk_hook = hooks.before_walk_next.clone();
        #[cfg(test)]
        let path_send_hook = hooks.before_path_send.clone();
        let walker = scope.spawn(move || {
            let mut summary = WalkSummary::default();
            let counted_packages = &mut summary.skipped_packages;

            // `filter_entry` prunes: a package directory is never descended into, so
            // its unreadable interior is never even attempted. Walk errors pass the
            // predicate untouched — a locked folder still names itself.
            let mut walk = WalkDir::new(root)
                .follow_links(false)
                .into_iter()
                .filter_entry(|entry| {
                    if entry.depth() == 0 {
                        return true;
                    }
                    // Pruned rather than walked: the backup is not output this run
                    // could replace, it is the originals themselves.
                    if entry
                        .path()
                        .components()
                        .any(|part| part.as_os_str() == BACKUP_DIR)
                    {
                        return false;
                    }
                    // Output is excluded below, but still walked so its count remains
                    // truthful. A package there is not skipped input.
                    if entry
                        .path()
                        .components()
                        .any(|part| part.as_os_str() == OUTPUT_DIR)
                        || entry.path().starts_with(output_root)
                    {
                        return true;
                    }
                    if entry.file_type().is_dir() && is_opaque_package(entry.path()) {
                        *counted_packages += 1;
                        return false;
                    }
                    true
                });
            loop {
                // A walk step itself is not interruptible, so cancellation is checked
                // on both sides of the one `next` call rather than claimed inside it.
                if walker_cancelled.load(Ordering::Acquire) {
                    break;
                }
                #[cfg(test)]
                walk_hook();
                let Some(entry) = walk.next() else {
                    break;
                };
                if walker_cancelled.load(Ordering::Acquire) {
                    break;
                }
                let Ok(file) = entry else {
                    if let Some(path) = entry.unwrap_err().path() {
                        summary.walk_errors.push(path.to_path_buf());
                    }
                    continue;
                };
                if !file.file_type().is_file() {
                    continue;
                }
                let relative = file.path().strip_prefix(root).unwrap_or(file.path());
                let in_output = file.path().starts_with(output_root);
                if in_output
                    || relative
                        .components()
                        .any(|part| part.as_os_str() == OUTPUT_DIR)
                {
                    // The run record is not an image the next run would replace.
                    if in_output && file.file_name() != crate::manifest::NAME {
                        summary.existing_output += 1;
                    }
                    continue;
                }
                // A run in flight stages its output beside the target. It is a
                // whole image and it is about to be renamed away, so auditing it
                // would offer to convert a file that will not be there.
                if is_partial(file.path()) {
                    continue;
                }
                if is_raw(file.path()) {
                    summary.skipped_raw += 1;
                    continue;
                }
                if is_heic(file.path()) {
                    summary.skipped_heic += 1;
                    continue;
                }
                if walker_cancelled.load(Ordering::Acquire) {
                    break;
                }
                #[cfg(test)]
                path_send_hook();
                if path_sender.send(file.into_path()).is_err() {
                    break;
                }
            }
            summary
        });

        let (result_sender, result_receiver) = std::sync::mpsc::sync_channel(capacity);
        let workers: Vec<_> = (0..threads)
            .map(|_| {
                let path_receiver = path_receiver.clone();
                let result_sender = result_sender.clone();
                let worker_cancelled = cancelled.clone();
                #[cfg(test)]
                let probe_hook = hooks.before_probe.clone();
                #[cfg(test)]
                let path_receive_hook = hooks.before_path_receive.clone();
                #[cfg(test)]
                let result_send_hook = hooks.before_result_send.clone();
                scope.spawn(move || {
                    loop {
                        if worker_cancelled.load(Ordering::Acquire) {
                            return;
                        }
                        #[cfg(test)]
                        path_receive_hook();
                        let path = {
                            let receiver = path_receiver
                                .lock()
                                .unwrap_or_else(std::sync::PoisonError::into_inner);
                            receiver.recv_timeout(Duration::from_millis(10))
                        };
                        let path = match path {
                            Ok(path) => path,
                            Err(std::sync::mpsc::RecvTimeoutError::Timeout) => continue,
                            Err(std::sync::mpsc::RecvTimeoutError::Disconnected) => return,
                        };
                        if worker_cancelled.load(Ordering::Acquire) {
                            return;
                        }
                        // A probe is one whole cooperative unit. It may finish after
                        // cancellation, but its result is never sent afterwards.
                        #[cfg(test)]
                        probe_hook(&path);
                        let probed = probe(&path);
                        if worker_cancelled.load(Ordering::Acquire) {
                            return;
                        }
                        match probed {
                            Some(entry) => {
                                if worker_cancelled.load(Ordering::Acquire) {
                                    return;
                                }
                                #[cfg(test)]
                                result_send_hook();
                                if result_sender.send(Probed::Entry(entry)).is_err() {
                                    return;
                                }
                            }
                            // Only report things that claimed to be images. A README is
                            // not a failure.
                            None if looks_like_an_image(&path) => {
                                if worker_cancelled.load(Ordering::Acquire) {
                                    return;
                                }
                                #[cfg(test)]
                                result_send_hook();
                                if result_sender.send(Probed::Unreadable(path)).is_err() {
                                    return;
                                }
                            }
                            None => {}
                        }
                    }
                })
            })
            .collect();
        drop(path_receiver);
        drop(result_sender);

        loop {
            if cancelled.load(Ordering::Acquire) {
                was_cancelled = true;
                break;
            }
            #[cfg(test)]
            (hooks.before_result_receive)();
            let result = match result_receiver.recv_timeout(Duration::from_millis(10)) {
                Ok(result) => result,
                Err(std::sync::mpsc::RecvTimeoutError::Timeout) => continue,
                Err(std::sync::mpsc::RecvTimeoutError::Disconnected) => break,
            };
            if cancelled.load(Ordering::Acquire) {
                was_cancelled = true;
                break;
            }
            match result {
                Probed::Entry(entry) => {
                    entries.push(entry);
                    if let Some(range) = batches.ready_range(entries.len()) {
                        if cancelled.load(Ordering::Acquire) {
                            was_cancelled = true;
                            break;
                        }
                        #[cfg(test)]
                        (hooks.before_callback)();
                        if cancelled.load(Ordering::Acquire) {
                            was_cancelled = true;
                            break;
                        }
                        if publish(&entries[range]).is_break() || cancelled.load(Ordering::Acquire)
                        {
                            cancelled.store(true, Ordering::Release);
                            was_cancelled = true;
                            break;
                        }
                    }
                }
                Probed::Unreadable(path) => unreadable.push(path),
            }
        }
        // Dropping the result receiver wakes workers blocked sending a result. They
        // then drop their path receivers, which wakes a walker blocked on a path.
        drop(result_receiver);
        for worker in workers {
            if let Err(panic) = worker.join() {
                std::panic::resume_unwind(panic);
            }
        }
        match walker.join() {
            Ok(summary) => summary,
            Err(panic) => std::panic::resume_unwind(panic),
        }
    });
    if was_cancelled || cancelled.load(Ordering::Acquire) {
        return ScanOutcome::Cancelled;
    }
    if let Some(range) = batches.final_tail(entries.len()) {
        if cancelled.load(Ordering::Acquire) {
            return ScanOutcome::Cancelled;
        }
        #[cfg(test)]
        (hooks.before_callback)();
        if cancelled.load(Ordering::Acquire) {
            return ScanOutcome::Cancelled;
        }
        if publish(&entries[range]).is_break() || cancelled.load(Ordering::Acquire) {
            cancelled.store(true, Ordering::Release);
            return ScanOutcome::Cancelled;
        }
    }
    if cancelled.load(Ordering::Acquire) {
        return ScanOutcome::Cancelled;
    }

    // Heaviest first: the top of the list is the work worth doing.
    entries.sort_by_key(|entry| std::cmp::Reverse(entry.bytes));
    ScanOutcome::Complete(Scan {
        entries,
        skipped_raw: summary.skipped_raw,
        skipped_heic: summary.skipped_heic,
        skipped_packages: summary.skipped_packages,
        unreadable,
        walk_errors: summary.walk_errors,
        existing_output: summary.existing_output,
    })
}

/// Extension-only guess, used to decide whether a decode failure is worth reporting.
fn looks_like_an_image(path: &Path) -> bool {
    const EXTENSIONS: [&str; 10] = [
        "jpg", "jpeg", "png", "webp", "avif", "jxl", "gif", "tif", "tiff", "bmp",
    ];
    path.extension()
        .and_then(|extension| extension.to_str())
        .is_some_and(|extension| EXTENSIONS.contains(&extension.to_ascii_lowercase().as_str()))
}

/// Human-readable file size. Deliberately not exact: the point is comparison.
pub fn format_bytes(bytes: u64) -> String {
    const UNITS: [(u64, &str); 3] = [(1 << 30, "GB"), (1 << 20, "MB"), (1 << 10, "KB")];
    for (scale, unit) in UNITS {
        if bytes >= scale {
            return format!("{:.1} {unit}", bytes as f64 / scale as f64);
        }
    }
    format!("{bytes} B")
}

/// The short name shown in the format column.
pub fn format_name(format: FileFormat) -> &'static str {
    match format {
        FileFormat::JpegXl => "JPEG XL",
        FileFormat::Image(ImageFormat::Jpeg) => "JPEG",
        FileFormat::Image(ImageFormat::Png) => "PNG",
        FileFormat::Image(ImageFormat::WebP) => "WebP",
        FileFormat::Image(ImageFormat::Avif) => "AVIF",
        FileFormat::Image(ImageFormat::Gif) => "GIF",
        FileFormat::Image(ImageFormat::Tiff) => "TIFF",
        FileFormat::Image(ImageFormat::Bmp) => "BMP",
        FileFormat::Image(other) => other.extensions_str().first().copied().unwrap_or("?"),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use image::{Frame, ImageBuffer, Rgb, Rgba, codecs::gif::GifEncoder};
    #[cfg(unix)]
    use std::os::unix::fs::PermissionsExt;
    use std::sync::{Condvar, Mutex, mpsc};
    use std::time::Duration;

    struct Gate {
        open: Mutex<bool>,
        changed: Condvar,
    }

    impl Gate {
        fn new() -> Self {
            Self {
                open: Mutex::new(false),
                changed: Condvar::new(),
            }
        }

        fn wait(&self) {
            let open = self.open.lock().unwrap();
            drop(
                self.changed
                    .wait_while(open, |open| !*open)
                    .unwrap_or_else(std::sync::PoisonError::into_inner),
            );
        }

        fn open(&self) {
            *self.open.lock().unwrap() = true;
            self.changed.notify_all();
        }
    }

    fn worker_count() -> usize {
        std::thread::available_parallelism().map_or(4, |count| count.get())
    }

    fn cancellable(
        root: &Path,
        cancelled: Arc<AtomicBool>,
        publish: impl FnMut(&[Entry]) -> ControlFlow<()>,
    ) -> ScanOutcome {
        scan_progressive_cancellable(root, &root.join(OUTPUT_DIR), cancelled, publish)
    }

    fn write_sample(dir: &Path, name: &str, width: u32, height: u32) -> PathBuf {
        let path = dir.join(name);
        let buffer = ImageBuffer::from_fn(width, height, |x, y| {
            Rgb([(x % 256) as u8, (y % 256) as u8, 128])
        });
        buffer.save(&path).unwrap();
        path
    }

    fn temp_dir(tag: &str) -> PathBuf {
        let dir = std::env::temp_dir().join(format!("imageguide-test-{tag}"));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();
        std::fs::canonicalize(dir).unwrap()
    }

    #[test]
    fn probes_dimensions_and_format_without_decoding() {
        let dir = temp_dir("probe");
        let path = write_sample(&dir, "sample.png", 40, 25);

        let entry = probe(&path).expect("png is readable");
        assert_eq!(entry.format, FileFormat::Image(ImageFormat::Png));
        assert_eq!((entry.width, entry.height), (40, 25));
        assert_eq!(entry.bytes, std::fs::metadata(&path).unwrap().len());
    }

    #[test]
    fn exif_orientation_is_applied_to_dimensions_and_pixels() {
        const EXIF_ROTATE_90: [u8; 36] = [
            0xff, 0xe1, 0x00, 0x22, b'E', b'x', b'i', b'f', 0, 0, b'M', b'M', 0, 0x2a, 0, 0, 0, 8,
            0, 1, 0x01, 0x12, 0, 3, 0, 0, 0, 1, 0, 6, 0, 0, 0, 0, 0, 0,
        ];

        let dir = temp_dir("orientation");
        let path = write_sample(&dir, "camera.jpg", 40, 20);
        let jpeg = std::fs::read(&path).unwrap();
        assert_eq!(&jpeg[..2], &[0xff, 0xd8]);
        let mut oriented = Vec::with_capacity(jpeg.len() + EXIF_ROTATE_90.len());
        oriented.extend_from_slice(&jpeg[..2]);
        oriented.extend_from_slice(&EXIF_ROTATE_90);
        oriented.extend_from_slice(&jpeg[2..]);
        std::fs::write(&path, oriented).unwrap();

        let entry = probe(&path).expect("oriented JPEG is probed");
        let decoded = decode(&path).expect("oriented JPEG is decoded");
        assert_eq!((entry.width, entry.height), (20, 40));
        assert_eq!((decoded.width(), decoded.height()), (20, 40));
    }

    /// Exporting a smaller image must not allocate the larger one first. The saving
    /// has to be visible in what comes back rather than only in a stopwatch: the
    /// intermediate is the largest DCT scale that still covers the target edge.
    #[test]
    fn a_downscaled_jpeg_decodes_at_a_reduced_dct_scale() {
        let dir = temp_dir("scaled-jpeg");
        let path = dir.join("big.jpg");
        crate::convert::tests::photo(4000, 1000)
            .save(&path)
            .unwrap();

        // A quarter of 4000 is exactly the target, so Lanczos has nothing left to do.
        let (quarter, _) = decode_for_conversion(&path, crate::convert::MaxEdge(Some(1000)))
            .expect("the JPEG decodes");
        assert_eq!((quarter.width(), quarter.height()), (1000, 250));

        // A quarter would land under 1200, so the decode stops at a half and
        // `MaxEdge::apply` finishes the step down.
        let (half, _) = decode_for_conversion(&path, crate::convert::MaxEdge(Some(1200)))
            .expect("the JPEG decodes");
        assert_eq!((half.width(), half.height()), (2000, 500));
        let applied = crate::convert::MaxEdge(Some(1200)).apply(half);
        assert_eq!((applied.width(), applied.height()), (1200, 300));

        // Full size still decodes every pixel, as it always did.
        let (full, _) =
            decode_for_conversion(&path, crate::convert::MaxEdge::FULL).expect("the JPEG decodes");
        assert_eq!((full.width(), full.height()), (4000, 1000));

        // And a budget larger than the source never reaches for a smaller scale.
        let (untouched, _) = decode_for_conversion(&path, crate::convert::MaxEdge(Some(6000)))
            .expect("the JPEG decodes");
        assert_eq!((untouched.width(), untouched.height()), (4000, 1000));
    }

    /// libjpeg-turbo hands CMYK and YCCK back only as CMYK, so the scaled path has to
    /// decline them and let the general decoder do the conversion. Getting that wrong
    /// is not a failure anyone would notice quickly: four channels read as three come
    /// out as a picture, just the wrong one.
    ///
    /// The fixture is an Adobe CMYK JPEG, red on the left and blue on the right.
    /// Nothing here writes CMYK, so it is bytes rather than something built in the
    /// test.
    #[test]
    fn a_cmyk_jpeg_falls_back_to_the_general_decoder_with_its_colours_intact() {
        const CMYK_JPEG: [u8; 377] = [
            0xff, 0xd8, 0xff, 0xee, 0x00, 0x0e, 0x41, 0x64, 0x6f, 0x62, 0x65, 0x00, 0x64, 0x00,
            0x00, 0x00, 0x00, 0x00, 0xff, 0xdb, 0x00, 0x43, 0x00, 0x02, 0x01, 0x01, 0x01, 0x01,
            0x01, 0x02, 0x01, 0x01, 0x01, 0x02, 0x02, 0x02, 0x02, 0x02, 0x04, 0x03, 0x02, 0x02,
            0x02, 0x02, 0x05, 0x04, 0x04, 0x03, 0x04, 0x06, 0x05, 0x06, 0x06, 0x06, 0x05, 0x06,
            0x06, 0x06, 0x07, 0x09, 0x08, 0x06, 0x07, 0x09, 0x07, 0x06, 0x06, 0x08, 0x0b, 0x08,
            0x09, 0x0a, 0x0a, 0x0a, 0x0a, 0x0a, 0x06, 0x08, 0x0b, 0x0c, 0x0b, 0x0a, 0x0c, 0x09,
            0x0a, 0x0a, 0x0a, 0xff, 0xc0, 0x00, 0x14, 0x08, 0x00, 0x10, 0x00, 0x10, 0x04, 0x43,
            0x11, 0x00, 0x4d, 0x11, 0x00, 0x59, 0x11, 0x00, 0x4b, 0x11, 0x00, 0xff, 0xc4, 0x00,
            0x1f, 0x00, 0x00, 0x01, 0x05, 0x01, 0x01, 0x01, 0x01, 0x01, 0x01, 0x00, 0x00, 0x00,
            0x00, 0x00, 0x00, 0x00, 0x00, 0x01, 0x02, 0x03, 0x04, 0x05, 0x06, 0x07, 0x08, 0x09,
            0x0a, 0x0b, 0xff, 0xc4, 0x00, 0xb5, 0x10, 0x00, 0x02, 0x01, 0x03, 0x03, 0x02, 0x04,
            0x03, 0x05, 0x05, 0x04, 0x04, 0x00, 0x00, 0x01, 0x7d, 0x01, 0x02, 0x03, 0x00, 0x04,
            0x11, 0x05, 0x12, 0x21, 0x31, 0x41, 0x06, 0x13, 0x51, 0x61, 0x07, 0x22, 0x71, 0x14,
            0x32, 0x81, 0x91, 0xa1, 0x08, 0x23, 0x42, 0xb1, 0xc1, 0x15, 0x52, 0xd1, 0xf0, 0x24,
            0x33, 0x62, 0x72, 0x82, 0x09, 0x0a, 0x16, 0x17, 0x18, 0x19, 0x1a, 0x25, 0x26, 0x27,
            0x28, 0x29, 0x2a, 0x34, 0x35, 0x36, 0x37, 0x38, 0x39, 0x3a, 0x43, 0x44, 0x45, 0x46,
            0x47, 0x48, 0x49, 0x4a, 0x53, 0x54, 0x55, 0x56, 0x57, 0x58, 0x59, 0x5a, 0x63, 0x64,
            0x65, 0x66, 0x67, 0x68, 0x69, 0x6a, 0x73, 0x74, 0x75, 0x76, 0x77, 0x78, 0x79, 0x7a,
            0x83, 0x84, 0x85, 0x86, 0x87, 0x88, 0x89, 0x8a, 0x92, 0x93, 0x94, 0x95, 0x96, 0x97,
            0x98, 0x99, 0x9a, 0xa2, 0xa3, 0xa4, 0xa5, 0xa6, 0xa7, 0xa8, 0xa9, 0xaa, 0xb2, 0xb3,
            0xb4, 0xb5, 0xb6, 0xb7, 0xb8, 0xb9, 0xba, 0xc2, 0xc3, 0xc4, 0xc5, 0xc6, 0xc7, 0xc8,
            0xc9, 0xca, 0xd2, 0xd3, 0xd4, 0xd5, 0xd6, 0xd7, 0xd8, 0xd9, 0xda, 0xe1, 0xe2, 0xe3,
            0xe4, 0xe5, 0xe6, 0xe7, 0xe8, 0xe9, 0xea, 0xf1, 0xf2, 0xf3, 0xf4, 0xf5, 0xf6, 0xf7,
            0xf8, 0xf9, 0xfa, 0xff, 0xda, 0x00, 0x0e, 0x04, 0x43, 0x00, 0x4d, 0x00, 0x59, 0x00,
            0x4b, 0x00, 0x00, 0x3f, 0x00, 0xfd, 0xfc, 0xaf, 0xe7, 0xfe, 0xbf, 0x9f, 0xfa, 0xfd,
            0xfc, 0xaf, 0xe0, 0x0e, 0x8a, 0xfe, 0xff, 0x00, 0x28, 0xaf, 0xef, 0xf2, 0x8a, 0xfe,
            0x00, 0xe8, 0xaf, 0xe0, 0x0e, 0x8a, 0xfe, 0xff, 0x00, 0x28, 0xaf, 0xff, 0xd9,
        ];

        let dir = temp_dir("cmyk-jpeg");
        let path = dir.join("press.jpg");
        std::fs::write(&path, CMYK_JPEG).unwrap();

        // Half of sixteen, so the scaled path is asked for and has to decline.
        let (image, _) = decode_for_conversion(&path, crate::convert::MaxEdge(Some(8)))
            .expect("a CMYK JPEG decodes");
        assert_eq!((image.width(), image.height()), (16, 16));

        let pixels = image.to_rgb8();
        let left = pixels.get_pixel(2, 2).0;
        let right = pixels.get_pixel(13, 2).0;
        assert!(
            left[0] > 200 && left[2] < 60,
            "the left half is not red: {left:?}"
        );
        assert!(
            right[2] > 200 && right[0] < 60,
            "the right half is not blue: {right:?}"
        );
    }

    /// The rotation belongs after the scaled decode. turbojpeg hands back the stored
    /// pixels in the stored order, so applying EXIF to the header first would swap the
    /// dimensions of an image that was never that shape, and the marked corner would
    /// come out on the wrong side.
    #[test]
    fn a_rotated_jpeg_keeps_its_orientation_through_a_scaled_decode() {
        const EXIF_ROTATE_90: [u8; 36] = [
            0xff, 0xe1, 0x00, 0x22, b'E', b'x', b'i', b'f', 0, 0, b'M', b'M', 0, 0x2a, 0, 0, 0, 8,
            0, 1, 0x01, 0x12, 0, 3, 0, 0, 0, 1, 0, 6, 0, 0, 0, 0, 0, 0,
        ];

        let dir = temp_dir("scaled-jpeg-orientation");
        let path = dir.join("camera.jpg");
        // A white block in the stored top-left corner. Rotated 90 degrees clockwise it
        // belongs in the displayed top-right one.
        let mut marked = crate::convert::tests::photo(2000, 1000).into_rgb8();
        for y in 0..150 {
            for x in 0..150 {
                marked.put_pixel(x, y, Rgb([255, 255, 255]));
            }
        }
        DynamicImage::ImageRgb8(marked).save(&path).unwrap();
        let jpeg = std::fs::read(&path).unwrap();
        let mut oriented = Vec::with_capacity(jpeg.len() + EXIF_ROTATE_90.len());
        oriented.extend_from_slice(&jpeg[..2]);
        oriented.extend_from_slice(&EXIF_ROTATE_90);
        oriented.extend_from_slice(&jpeg[2..]);
        std::fs::write(&path, oriented).unwrap();

        let entry = probe(&path).expect("the oriented JPEG is probed");
        assert_eq!((entry.width, entry.height), (1000, 2000));

        let (scaled, _) = decode_for_conversion(&path, crate::convert::MaxEdge(Some(500)))
            .expect("the JPEG decodes");
        assert_eq!((scaled.width(), scaled.height()), (250, 500));
        let pixels = scaled.to_rgb8();
        assert!(
            pixels.get_pixel(230, 20).0[1] > 200,
            "the marked corner did not land top-right"
        );
        assert!(
            pixels.get_pixel(20, 20).0[1] < 200,
            "the marked corner is still top-left, so nothing rotated"
        );
    }

    /// The mislabelled files are the whole point, so they have to survive every path
    /// and not just the one that counts them. Decoding by extension meant the folder
    /// this app was built for — 169 files named `.webp`, 59 of them PNG — listed
    /// those 59 and then silently failed to convert, thumbnail or preview any of them.
    #[test]
    fn a_file_decodes_by_its_contents_not_its_name() {
        let dir = temp_dir("decode-liar");
        let honest = write_sample(&dir, "honest.png", 24, 16);
        let liar = dir.join("liar.webp");
        std::fs::copy(&honest, &liar).unwrap();

        let decoded = decode(&liar).expect("a PNG named .webp still decodes");
        assert_eq!((decoded.width(), decoded.height()), (24, 16));
        // And the audit agrees about what it actually is.
        assert_eq!(
            probe(&liar).unwrap().format,
            FileFormat::Image(ImageFormat::Png)
        );
    }

    #[test]
    fn jpeg_xl_is_probed_and_decoded_by_its_contents() {
        let dir = temp_dir("jpeg-xl");
        let path = dir.join("photo.png");
        let image = DynamicImage::ImageRgb8(ImageBuffer::from_fn(18, 12, |x, y| {
            Rgb([(x * 9) as u8, (y * 13) as u8, 80])
        }));
        let encoded = crate::convert::encode(
            &image,
            crate::convert::Format::JpegXl,
            crate::convert::Quality::lossy(80.),
            None,
        )
        .expect("JPEG XL encodes");
        std::fs::write(&path, encoded).unwrap();

        let entry = probe(&path).expect("JPEG XL header is readable");
        assert_eq!(entry.format, FileFormat::JpegXl);
        assert_eq!((entry.width, entry.height), (18, 12));
        assert!(entry.extension_lies());

        let decoded = decode(&path).expect("JPEG XL named .png still decodes");
        assert_eq!((decoded.width(), decoded.height()), (18, 12));
    }

    #[test]
    fn an_animated_gif_is_not_decoded_as_a_still_for_conversion() {
        let dir = temp_dir("animated-gif");
        let path = dir.join("moving.gif");
        let file = std::fs::File::create(&path).unwrap();
        GifEncoder::new(file)
            .encode_frames([
                Frame::new(ImageBuffer::from_pixel(2, 2, Rgba([255, 0, 0, 255]))),
                Frame::new(ImageBuffer::from_pixel(2, 2, Rgba([0, 0, 255, 255]))),
            ])
            .unwrap();

        assert_eq!(
            decode_for_conversion(&path, crate::convert::MaxEdge::FULL),
            Err(ConversionDecodeError::AnimatedGif)
        );
    }

    /// An APNG is a PNG with an `acTL` chunk in front of the still the image crate
    /// hands back, so the fixture is exactly that: a real PNG with the chunk spliced
    /// in after IHDR. Nothing here writes APNGs, and only the chunk is read.
    fn write_animated_png(path: &Path) {
        let still = {
            let mut bytes = Vec::new();
            DynamicImage::ImageRgb8(ImageBuffer::from_fn(4, 4, |x, y| {
                Rgb([(x * 60) as u8, (y * 60) as u8, 20])
            }))
            .write_to(&mut std::io::Cursor::new(&mut bytes), ImageFormat::Png)
            .unwrap();
            bytes
        };
        // Two frames, looping forever.
        let mut actl = Vec::new();
        actl.extend_from_slice(&8u32.to_be_bytes());
        actl.extend_from_slice(b"acTL");
        actl.extend_from_slice(&2u32.to_be_bytes());
        actl.extend_from_slice(&0u32.to_be_bytes());
        let mut crc = flate2::Crc::new();
        crc.update(&actl[4..]);
        actl.extend_from_slice(&crc.sum().to_be_bytes());

        // 8 signature bytes, then IHDR: 4 length + 4 name + 13 payload + 4 CRC.
        let after_ihdr = 8 + 25;
        let mut animated = still[..after_ihdr].to_vec();
        animated.extend_from_slice(&actl);
        animated.extend_from_slice(&still[after_ihdr..]);
        std::fs::write(path, animated).unwrap();
    }

    /// An animated WebP is the extended container with the animation flag, an ANIM
    /// chunk and one ANMF frame per still. libwebp will write one but the crate's
    /// bindings stop at the still encoder, so the container is assembled by hand
    /// around a still this app just produced.
    fn write_animated_webp(path: &Path, width: u32, height: u32) {
        fn chunk(name: &[u8; 4], payload: &[u8]) -> Vec<u8> {
            let mut chunk = name.to_vec();
            chunk.extend_from_slice(&(payload.len() as u32).to_le_bytes());
            chunk.extend_from_slice(payload);
            if payload.len() % 2 == 1 {
                chunk.push(0);
            }
            chunk
        }
        fn u24(value: u32) -> [u8; 3] {
            let bytes = value.to_le_bytes();
            [bytes[0], bytes[1], bytes[2]]
        }

        let still = crate::convert::encode(
            &DynamicImage::ImageRgb8(ImageBuffer::from_fn(width, height, |x, y| {
                Rgb([(x * 7) as u8, (y * 11) as u8, 90])
            })),
            crate::convert::Format::WebP,
            crate::convert::Quality::lossy(80.),
            None,
        )
        .expect("the still frame encodes");
        // Everything after "RIFF<size>WEBP" is the image chunk, header included.
        let image_chunk = &still[12..];

        let mut vp8x = vec![0b0000_0010, 0, 0, 0];
        vp8x.extend_from_slice(&u24(width - 1));
        vp8x.extend_from_slice(&u24(height - 1));

        let mut anim = vec![0, 0, 0, 0];
        anim.extend_from_slice(&0u16.to_le_bytes());

        let mut frame = Vec::new();
        frame.extend_from_slice(&u24(0));
        frame.extend_from_slice(&u24(0));
        frame.extend_from_slice(&u24(width - 1));
        frame.extend_from_slice(&u24(height - 1));
        frame.extend_from_slice(&u24(100));
        frame.push(0);
        frame.extend_from_slice(image_chunk);

        let mut body = b"WEBP".to_vec();
        body.extend_from_slice(&chunk(b"VP8X", &vp8x));
        body.extend_from_slice(&chunk(b"ANIM", &anim));
        body.extend_from_slice(&chunk(b"ANMF", &frame));
        body.extend_from_slice(&chunk(b"ANMF", &frame));

        let mut animated = b"RIFF".to_vec();
        animated.extend_from_slice(&(body.len() as u32).to_le_bytes());
        animated.extend_from_slice(&body);
        std::fs::write(path, animated).unwrap();
    }

    #[test]
    fn an_animated_png_is_not_decoded_as_a_still_for_conversion() {
        let dir = temp_dir("animated-png");
        let path = dir.join("moving.png");
        write_animated_png(&path);

        assert_eq!(
            decode_for_conversion(&path, crate::convert::MaxEdge::FULL),
            Err(ConversionDecodeError::AnimatedPng)
        );
    }

    #[test]
    fn an_animated_webp_is_not_decoded_as_a_still_for_conversion() {
        let dir = temp_dir("animated-webp");
        let path = dir.join("moving.webp");
        write_animated_webp(&path, 16, 16);

        assert_eq!(
            decode_for_conversion(&path, crate::convert::MaxEdge::FULL),
            Err(ConversionDecodeError::AnimatedWebP)
        );
    }

    /// The refusal has to be about the animation, not about the format: a still WebP
    /// and a still PNG both still convert.
    #[test]
    fn a_still_webp_and_a_still_png_are_still_decoded_for_conversion() {
        let dir = temp_dir("still-webp-and-png");
        let png = write_sample(&dir, "still.png", 12, 9);
        let webp = dir.join("still.webp");
        let encoded = crate::convert::encode(
            &decode(&png).unwrap(),
            crate::convert::Format::WebP,
            crate::convert::Quality::lossy(80.),
            None,
        )
        .unwrap();
        std::fs::write(&webp, encoded).unwrap();

        for path in [png, webp] {
            let (image, _) = decode_for_conversion(&path, crate::convert::MaxEdge::FULL)
                .expect("a still decodes");
            assert_eq!((image.width(), image.height()), (12, 9));
        }
    }

    #[test]
    fn an_animated_jpeg_xl_is_not_decoded_as_a_still_for_conversion() {
        const TWO_FRAME_JXL: &[u8] = &[
            0xff, 0x0a, 0x08, 0x10, 0x41, 0x00, 0x02, 0x8a, 0x4b, 0x02, 0x08, 0x00, 0x2a, 0x00,
            0x00, 0x44, 0x00, 0x4b, 0x12, 0xa5, 0x42, 0x85, 0x24, 0xd6, 0x68, 0x60, 0xfb, 0xc6,
            0x07, 0x20, 0xc7, 0x7d, 0xac, 0x03, 0x08, 0x00, 0x2a, 0x04, 0x00, 0x44, 0x00, 0x4b,
            0x12, 0xa5, 0x42, 0x85, 0x24, 0xd6, 0x68, 0x60, 0xfb, 0xc6, 0x07, 0x20, 0xc7, 0x7b,
            0xac, 0x03,
        ];
        let dir = temp_dir("animated-jpeg-xl");
        let path = dir.join("moving.jxl");
        std::fs::write(&path, TWO_FRAME_JXL).unwrap();

        assert_eq!(
            decode_for_conversion(&path, crate::convert::MaxEdge::FULL),
            Err(ConversionDecodeError::AnimatedJpegXl)
        );
    }

    /// The audit's best finding is a file whose name disagrees with its bytes, so
    /// the check has to be right about which disagreements are real. `jpg` and
    /// `jpeg` naming the same format is not a finding.
    #[test]
    fn an_extension_only_lies_when_it_names_another_format() {
        let png = Entry {
            path: PathBuf::from("/photos/promo.png"),
            format: ImageFormat::Png.into(),
            width: 10,
            height: 10,
            bytes: 100,
        };
        assert!(!png.extension_lies());

        let liar = Entry {
            path: PathBuf::from("/photos/promo.webp"),
            ..png.clone()
        };
        assert!(liar.extension_lies(), "a PNG named .webp is the finding");

        let shouty = Entry {
            path: PathBuf::from("/photos/PROMO.PNG"),
            ..png.clone()
        };
        assert!(!shouty.extension_lies(), "case is not a disagreement");

        let jpeg = Entry {
            path: PathBuf::from("/photos/shot.jpg"),
            format: ImageFormat::Jpeg.into(),
            ..png.clone()
        };
        assert!(!jpeg.extension_lies(), "jpg and jpeg are one format");

        let bare = Entry {
            path: PathBuf::from("/photos/shot"),
            ..png
        };
        assert!(!bare.extension_lies(), "no extension is not a claim");
    }

    #[test]
    fn skips_files_that_are_not_images() {
        let dir = temp_dir("skip");
        std::fs::write(dir.join("notes.txt"), "not an image").unwrap();
        write_sample(&dir, "real.png", 8, 8);

        let scanned = scan(&dir, &dir.join(OUTPUT_DIR));
        assert_eq!(scanned.entries.len(), 1);
        assert_eq!(scanned.entries[0].name(), "real.png");
    }

    #[test]
    fn scan_reaches_subfolders_and_sorts_heaviest_first() {
        let dir = temp_dir("walk");
        std::fs::create_dir_all(dir.join("nested")).unwrap();
        write_sample(&dir, "small.png", 8, 8);
        write_sample(&dir.join("nested"), "big.png", 300, 300);

        let scanned = scan(&dir, &dir.join(OUTPUT_DIR));
        assert_eq!(scanned.entries.len(), 2);
        assert_eq!(
            scanned.entries[0].name(),
            "big.png",
            "heaviest file sorts first"
        );
        assert!(scanned.entries[0].bytes > scanned.entries[1].bytes);
    }

    #[test]
    fn browser_lists_child_folders_without_auditing_their_images() {
        let dir = temp_dir("browse-shallow");
        let nested = dir.join("nested");
        let prior_output = dir.join(OUTPUT_DIR).join("nested");
        std::fs::create_dir_all(&nested).unwrap();
        std::fs::create_dir_all(&prior_output).unwrap();
        write_sample(&dir, "direct.png", 8, 8);
        write_sample(&nested, "descendant.png", 16, 16);
        write_sample(&prior_output, "direct.webp", 8, 8);

        let browsed = browse(&dir, &dir.join(OUTPUT_DIR)).unwrap();
        assert_eq!(browsed.folders, vec![nested, dir.join(OUTPUT_DIR)]);
        assert_eq!(browsed.scan.entries.len(), 1);
        assert_eq!(browsed.scan.entries[0].name(), "direct.png");
        assert_eq!(browsed.scan.existing_output, 1);
    }

    #[test]
    fn folder_sorting_breaks_case_folded_ties_by_the_original_name() {
        let mut folders = vec![PathBuf::from("alpha"), PathBuf::from("Alpha")];
        folders.sort_by_cached_key(|path| folder_sort_key(path));
        assert_eq!(
            folders,
            vec![PathBuf::from("Alpha"), PathBuf::from("alpha")]
        );
    }

    #[test]
    fn browser_refuses_to_audit_the_output_folder_as_input() {
        let dir = temp_dir("browse-output-as-input");
        let output = dir.join("output");
        let nested = output.join("nested");
        std::fs::create_dir_all(&nested).unwrap();
        write_sample(&nested, "existing.webp", 8, 8);

        let error = browse(&nested, &output).err().unwrap();
        assert_eq!(error.kind(), std::io::ErrorKind::InvalidInput);
    }

    #[test]
    fn a_cancelled_shallow_browse_starts_no_work() {
        let dir = temp_dir("browse-cancelled");
        write_sample(&dir, "direct.png", 8, 8);
        let cancelled = AtomicBool::new(true);

        assert!(
            browse_cancellable(&dir, &dir.join(OUTPUT_DIR), &cancelled)
                .unwrap()
                .is_none()
        );
    }

    #[test]
    fn browser_does_not_walk_an_external_output_tree() {
        let source = temp_dir("browse-external-source");
        let output = temp_dir("browse-external-output");
        std::fs::create_dir_all(output.join("deep")).unwrap();
        write_sample(&output.join("deep"), "existing.webp", 8, 8);

        let browsed = browse(&source, &output).unwrap();
        assert_eq!(browsed.scan.existing_output, 0);
    }

    #[test]
    fn an_unavailable_output_does_not_block_a_read_only_browse() {
        let source = temp_dir("browse-unavailable-output");
        write_sample(&source, "direct.png", 8, 8);
        let blocker = source.join("not-a-directory");
        std::fs::write(&blocker, "occupied").unwrap();

        let browsed = browse(&source, &blocker.join("offline-output")).unwrap();

        assert_eq!(browsed.scan.entries.len(), 1);
        assert_eq!(browsed.scan.entries[0].name(), "direct.png");
    }

    #[cfg(unix)]
    #[test]
    fn browser_resolves_output_aliases_before_accepting_input() {
        use std::os::unix::fs::symlink;

        let dir = temp_dir("browse-output-alias");
        let output = dir.join("output");
        let alias = dir.join("alias");
        std::fs::create_dir_all(&output).unwrap();
        symlink(&output, &alias).unwrap();

        let error = browse(&output, &alias).err().unwrap();
        assert_eq!(error.kind(), std::io::ErrorKind::InvalidInput);
    }

    #[cfg(unix)]
    #[test]
    fn browser_and_tree_hide_dot_folders_on_unix() {
        let dir = temp_dir("browse-hidden");
        let visible = dir.join("visible");
        let hidden = dir.join(".hidden");
        let optimized = dir.join(OUTPUT_DIR);
        std::fs::create_dir_all(&visible).unwrap();
        std::fs::create_dir_all(&hidden).unwrap();
        std::fs::create_dir_all(&optimized).unwrap();

        let browsed = browse(&dir, &dir.join(OUTPUT_DIR)).unwrap();
        assert_eq!(browsed.folders, vec![optimized.clone(), visible.clone()]);
        assert_eq!(child_folders(&dir).unwrap(), vec![optimized, visible]);
    }

    /// Replace mode leaves two things in the audited folder that are not input: the
    /// originals it moved aside, and the record of what it wrote. Listing either one
    /// would offer to convert a backup, or count a JSON file as an output image.
    #[test]
    fn the_walk_never_lists_the_backup_tree_or_the_run_record() {
        let dir = temp_dir("backup-tree");
        write_sample(&dir, "shot.webp", 16, 16);
        let backups = dir.join(BACKUP_DIR).join("album");
        std::fs::create_dir_all(&backups).unwrap();
        write_sample(&backups, "shot.png", 16, 16);
        write_sample(&dir.join(BACKUP_DIR), "loose.png", 8, 8);
        std::fs::write(dir.join(crate::manifest::NAME), b"{\"version\":1}").unwrap();
        std::fs::create_dir_all(dir.join(OUTPUT_DIR)).unwrap();
        std::fs::write(
            dir.join(OUTPUT_DIR).join(crate::manifest::NAME),
            b"{\"version\":1}",
        )
        .unwrap();

        let scanned = scan(&dir, &dir.join(OUTPUT_DIR));

        assert_eq!(
            scanned
                .entries
                .iter()
                .map(|entry| entry.name())
                .collect::<Vec<_>>(),
            vec!["shot.webp".to_string()],
            "only the folder's own images are listed"
        );
        assert!(scanned.unreadable.is_empty(), "the record is not an image");
        assert_eq!(
            scanned.existing_output, 0,
            "the record is not an output waiting to be replaced"
        );
    }

    /// The browser must not offer the backup either. Navigating into it and
    /// converting would rewrite the originals in place of themselves, and no
    /// restore could put back what it no longer has.
    #[test]
    fn the_browser_never_lists_the_backup_tree_or_a_half_written_output() {
        let dir = temp_dir("browse-backup");
        write_sample(&dir, "shot.png", 16, 16);
        std::fs::create_dir_all(dir.join(BACKUP_DIR)).unwrap();
        write_sample(&dir.join(BACKUP_DIR), "shot.png", 16, 16);
        let album = dir.join("album");
        std::fs::create_dir_all(&album).unwrap();
        let partial = dir.join(format!("shot.webp.{}.part", std::process::id()));
        std::fs::copy(dir.join("shot.png"), &partial).unwrap();

        let browsed = browse(&dir, &dir.join(OUTPUT_DIR)).unwrap();

        assert_eq!(browsed.folders, vec![album.clone()]);
        assert_eq!(
            browsed
                .scan
                .entries
                .iter()
                .map(|entry| entry.name())
                .collect::<Vec<_>>(),
            vec!["shot.png".to_string()],
            "the half-written output is not an input"
        );
        assert_eq!(child_folders(&dir).unwrap(), vec![album]);
        // The walk that feeds the audit has to agree with the browser: a staged
        // leftover it listed would be converted as though it were an original.
        assert_eq!(
            scan(&dir, &dir.join(OUTPUT_DIR))
                .entries
                .iter()
                .map(|entry| entry.name())
                .collect::<Vec<_>>(),
            vec!["shot.png".to_string()]
        );
        std::fs::remove_dir_all(dir).unwrap();
    }

    #[test]
    fn the_output_folder_is_not_audited_as_input() {
        let dir = temp_dir("output");
        write_sample(&dir, "source.png", 16, 16);
        std::fs::create_dir_all(dir.join(OUTPUT_DIR)).unwrap();
        write_sample(&dir.join(OUTPUT_DIR), "source.png", 16, 16);

        let scanned = scan(&dir, &dir.join(OUTPUT_DIR));
        assert_eq!(
            scanned.entries.len(),
            1,
            "a second run must not offer to convert its own output"
        );
        assert_eq!(
            scanned.existing_output, 1,
            "and it says what it would replace"
        );
    }

    #[test]
    fn a_root_named_optimized_is_still_audited() {
        let parent = temp_dir("root-named");
        let dir = parent.join(OUTPUT_DIR);
        std::fs::create_dir_all(&dir).unwrap();
        write_sample(&dir, "source.png", 16, 16);

        let scanned = scan(&dir, &dir.join(OUTPUT_DIR));
        assert_eq!(scanned.entries.len(), 1);
    }

    /// Every `optimized` folder is skipped as input, but only this root's own is what a
    /// run would write over. Counting a nested one told a real folder that converting
    /// would replace 5,415 files it was never going to touch.
    #[test]
    fn only_this_roots_output_folder_counts_as_what_a_run_would_replace() {
        let dir = temp_dir("nested-output");
        write_sample(&dir, "source.png", 16, 16);
        let nested = dir.join("screenshots").join(OUTPUT_DIR);
        std::fs::create_dir_all(&nested).unwrap();
        write_sample(&nested, "old.png", 8, 8);
        write_sample(&nested, "older.png", 8, 8);

        let scanned = scan(&dir, &dir.join(OUTPUT_DIR));
        assert_eq!(
            scanned.entries.len(),
            1,
            "the nested output is still skipped"
        );
        assert_eq!(
            scanned.existing_output, 0,
            "a run rooted here would not touch screenshots/optimized"
        );
    }

    #[test]
    fn camera_raw_is_counted_but_not_listed() {
        let dir = temp_dir("raw");
        write_sample(&dir, "keep.png", 8, 8);
        // A raw file is skipped on its name, before anything tries to decode it.
        std::fs::write(dir.join("DSC_0001.NEF"), b"not really a nef").unwrap();
        std::fs::write(dir.join("DSC_0002.cr2"), b"nor this").unwrap();

        let scanned = scan(&dir, &dir.join(OUTPUT_DIR));
        assert_eq!(scanned.entries.len(), 1);
        assert_eq!(
            scanned.skipped_raw, 2,
            "raw is counted, not silently dropped"
        );
    }

    #[test]
    fn heic_is_counted_but_not_listed() {
        let dir = temp_dir("heic");
        write_sample(&dir, "keep.png", 8, 8);
        // Nothing here decodes HEIC, so the files are skipped on their name like raw.
        // Bytes that no decoder will read still have to be admitted to.
        std::fs::write(dir.join("IMG_0001.HEIC"), b"not really a heic").unwrap();
        std::fs::write(dir.join("IMG_0002.heif"), b"nor this").unwrap();

        let scanned = scan(&dir, &dir.join(OUTPUT_DIR));
        assert_eq!(scanned.entries.len(), 1);
        assert_eq!(
            scanned.skipped_heic, 2,
            "HEIC is counted, not silently dropped"
        );
        assert!(
            scanned.unreadable.is_empty(),
            "a format we never claimed to read is not a read failure"
        );
        assert!(is_heic(Path::new("a/IMG_1.HIF")));
        assert!(!is_heic(Path::new("a/b.png")));
    }

    /// The walk streams files to one worker per core, so every count and the sort order
    /// have to survive being assembled from the bounded queue instead of one loop.
    #[test]
    fn a_streamed_folder_reports_every_file_once() {
        let dir = temp_dir("parallel");
        for index in 0..40u32 {
            write_sample(&dir, &format!("s{index:02}.png"), 8 + index, 8);
        }
        std::fs::write(dir.join("broken.png"), b"not a png").unwrap();

        let scanned = scan(&dir, &dir.join(OUTPUT_DIR));
        assert_eq!(scanned.entries.len(), 40, "no file is probed twice or lost");
        assert_eq!(scanned.unreadable.len(), 1, "failures survive the join");
        assert!(
            scanned
                .entries
                .windows(2)
                .all(|pair| pair[0].bytes >= pair[1].bytes),
            "heaviest first still holds across chunks"
        );
    }

    #[test]
    fn a_broken_image_is_counted_not_dropped() {
        let dir = temp_dir("unreadable");
        write_sample(&dir, "good.png", 8, 8);
        std::fs::write(dir.join("truncated.png"), b"not a png at all").unwrap();
        std::fs::write(dir.join("notes.txt"), b"plain text").unwrap();

        let scanned = scan(&dir, &dir.join(OUTPUT_DIR));
        assert_eq!(scanned.entries.len(), 1);
        assert_eq!(
            scanned.unreadable.len(),
            1,
            "a broken png is named; a text file is not a failure"
        );
        assert!(
            scanned.unreadable[0].ends_with("truncated.png"),
            "the report says which file, not how many"
        );
    }

    #[cfg(unix)]
    #[test]
    fn a_folder_it_cannot_enter_is_named_not_swallowed() {
        let dir = temp_dir("walk-error");
        let locked = dir.join("locked");
        std::fs::create_dir_all(&locked).unwrap();
        write_sample(&dir, "keep.png", 8, 8);
        // No read or execute permission: readdir on it fails.
        std::fs::set_permissions(&locked, std::fs::Permissions::from_mode(0o000)).unwrap();

        let scanned = scan(&dir, &dir.join(OUTPUT_DIR));

        // Restore before asserting so cleanup cannot fail on the locked folder.
        std::fs::set_permissions(&locked, std::fs::Permissions::from_mode(0o755)).unwrap();

        if scanned.entries.len() == 2 {
            // Root read the locked folder anyway; there is nothing to assert.
            return;
        }
        assert_eq!(scanned.entries.len(), 1);
        assert_eq!(
            scanned.walk_errors,
            vec![locked],
            "the place the walk stopped is named, not swallowed"
        );
    }

    /// A Photos library inside the audited folder is skipped whole and counted,
    /// not reported as a place the walk failed. Its interior is permission-walled
    /// by macOS and never held web-delivery images anyway.
    #[test]
    #[cfg(target_os = "macos")]
    fn a_macos_package_is_skipped_by_design_and_counted() {
        let dir = temp_dir("package");
        let library = dir.join("Photos Library.photoslibrary");
        std::fs::create_dir_all(&library).unwrap();
        write_sample(&library, "master.png", 8, 8);
        write_sample(&dir, "keep.png", 8, 8);

        let scanned = scan(&dir, &dir.join(OUTPUT_DIR));
        assert_eq!(
            scanned.entries.len(),
            1,
            "the sibling file is still audited"
        );
        assert_eq!(scanned.entries[0].name(), "keep.png");
        assert_eq!(
            scanned.skipped_packages, 1,
            "the package is counted like camera raw"
        );
        assert!(scanned.walk_errors.is_empty());
        assert!(scanned.unreadable.is_empty());
    }

    /// A folder explicitly handed to the app is entered even when its name says
    /// package: the user asked for it, and nothing else would be shown at all.
    #[test]
    #[cfg(target_os = "macos")]
    fn the_folder_the_app_was_pointed_at_is_entered_even_when_it_is_a_package() {
        let dir = temp_dir("package-root");
        write_sample(&dir, "inside.png", 8, 8);

        let scanned = scan(&dir, &dir.join(OUTPUT_DIR));
        assert_eq!(scanned.entries.len(), 1);
        assert_eq!(scanned.skipped_packages, 0);
    }

    /// The extension says nothing about a regular file's contents. Package pruning
    /// is for directories only, or a mislabelled but valid image disappears before
    /// the content-based probe can identify it.
    #[test]
    #[cfg(target_os = "macos")]
    fn a_regular_file_with_a_package_suffix_is_still_probed() {
        let dir = temp_dir("package-file");
        let png = write_sample(&dir, "image.png", 8, 8);
        let disguised = dir.join("image.app");
        std::fs::rename(png, &disguised).unwrap();

        let scanned = scan(&dir, &dir.join(OUTPUT_DIR));
        assert_eq!(scanned.entries.len(), 1);
        assert_eq!(scanned.entries[0].path, disguised);
        assert_eq!(
            scanned.entries[0].format,
            FileFormat::Image(ImageFormat::Png)
        );
        assert_eq!(scanned.skipped_packages, 0);
    }

    /// Existing output is not input, so a package-shaped folder there must neither
    /// count as skipped input nor stop the existing-output count at its boundary.
    #[test]
    #[cfg(target_os = "macos")]
    fn a_package_inside_output_is_counted_only_as_existing_output() {
        let dir = temp_dir("output-package");
        let package = dir.join(OUTPUT_DIR).join("Archive.app");
        std::fs::create_dir_all(&package).unwrap();
        write_sample(&package, "inside.png", 8, 8);

        let scanned = scan(&dir, &dir.join(OUTPUT_DIR));
        assert!(scanned.entries.is_empty());
        assert_eq!(scanned.existing_output, 1);
        assert_eq!(scanned.skipped_packages, 0);
    }

    #[test]
    fn raw_detection_ignores_extension_case() {
        assert!(is_raw(Path::new("a/DSC_1.NEF")));
        assert!(is_raw(Path::new("a/DSC_1.nef")));
        assert!(is_raw(Path::new("a/b.ArW")));
        assert!(!is_raw(Path::new("a/b.png")));
        assert!(!is_raw(Path::new("a/b")));
    }

    #[test]
    fn bytes_per_pixel_is_zero_for_an_empty_image() {
        let entry = Entry {
            path: PathBuf::from("x.png"),
            format: ImageFormat::Png.into(),
            width: 0,
            height: 0,
            bytes: 100,
        };
        assert_eq!(entry.bytes_per_pixel(), 0.);
    }

    #[test]
    fn sizes_read_in_the_nearest_unit() {
        assert_eq!(format_bytes(512), "512 B");
        assert_eq!(format_bytes(2048), "2.0 KB");
        assert_eq!(format_bytes(5 * (1 << 20)), "5.0 MB");
    }

    /// A destination the user chose inside the audited folder is still output:
    /// counted, never offered back as a candidate. Without this the second scan
    /// of a folder hands you last run's WebPs to convert again.
    #[test]
    fn a_chosen_output_folder_inside_the_root_is_not_audited() {
        let dir = temp_dir("chosen-output");
        write_sample(&dir, "source.png", 16, 16);
        let chosen = dir.join("exports");
        std::fs::create_dir_all(&chosen).unwrap();
        write_sample(&chosen, "source.webp", 16, 16);

        let scanned = scan(&dir, &chosen);
        assert_eq!(scanned.entries.len(), 1, "only the original is audited");
        assert!(scanned.entries[0].path.ends_with("source.png"));
        assert_eq!(scanned.existing_output, 1);

        // Pointed elsewhere, the same folder is ordinary input again.
        let elsewhere = scan(&dir, &dir.join("optimized"));
        assert_eq!(elsewhere.entries.len(), 2);
        let _ = std::fs::remove_dir_all(dir);
    }

    #[test]
    fn production_batches_grow_without_losing_order_or_tail() {
        let completions: Vec<_> = (0..100_000).collect();
        let mut batches = ProductionBatchState::new();
        let mut ranges = Vec::new();
        let mut published = Vec::new();

        for completed in 1..=completions.len() {
            if let Some(range) = batches.ready_range(completed) {
                published.extend_from_slice(&completions[range.clone()]);
                ranges.push(range);
            }
        }
        if let Some(range) = batches.final_tail(completions.len()) {
            published.extend_from_slice(&completions[range.clone()]);
            ranges.push(range);
        }

        assert_eq!(
            ranges,
            [
                0..32,
                32..288,
                288..1_312,
                1_312..5_408,
                5_408..13_600,
                13_600..21_792,
                21_792..29_984,
                29_984..38_176,
                38_176..46_368,
                46_368..54_560,
                54_560..62_752,
                62_752..70_944,
                70_944..79_136,
                79_136..87_328,
                87_328..95_520,
                95_520..100_000,
            ]
        );
        assert_eq!(ranges.len(), 16);
        assert_eq!(ranges.last(), Some(&(95_520..100_000)));
        assert_eq!(published, completions);

        assert_eq!(ProductionBatchState::new().final_tail(0), None);

        let mut exact_first = ProductionBatchState::new();
        assert_eq!(exact_first.ready_range(32), Some(0..32));
        assert_eq!(exact_first.final_tail(32), None);

        let mut exact_second = ProductionBatchState::new();
        assert_eq!(exact_second.ready_range(32), Some(0..32));
        assert_eq!(exact_second.ready_range(288), Some(32..288));
        assert_eq!(exact_second.final_tail(288), None);
    }

    #[test]
    fn a_progressive_scan_publishes_each_image_once() {
        let dir = temp_dir("progressive");
        for index in 0..35 {
            write_sample(&dir, &format!("image-{index}.png"), 8, 8);
        }
        let mut published = Vec::new();

        let scanned = scan_progressive(&dir, &dir.join(OUTPUT_DIR), |batch| {
            published.extend(batch.iter().map(|entry| entry.path.clone()));
        });

        published.sort();
        let mut complete: Vec<_> = scanned
            .entries
            .iter()
            .map(|entry| entry.path.clone())
            .collect();
        complete.sort();
        assert_eq!(published, complete);
        let _ = std::fs::remove_dir_all(dir);
    }

    #[test]
    fn a_tree_browse_finds_what_the_whole_folder_scan_finds() {
        let dir = temp_dir("tree-browse");
        let child = dir.join("child");
        let grandchild = child.join("grandchild");
        std::fs::create_dir_all(&grandchild).unwrap();
        write_sample(&dir, "direct.png", 8, 8);
        write_sample(&child, "nested.png", 8, 8);
        write_sample(&grandchild, "deep.png", 8, 8);
        let mut published = 0;

        let browsed = browse_tree_cancellable(
            &dir,
            &dir.join(OUTPUT_DIR),
            Arc::new(AtomicBool::new(false)),
            |batch| {
                published += batch.len();
                ControlFlow::Continue(())
            },
        )
        .unwrap()
        .expect("an uncancelled tree browse completes");

        let whole = scan(&dir, &dir.join(OUTPUT_DIR));
        assert_eq!(browsed.scan.entries.len(), 3);
        assert_eq!(browsed.scan.entries.len(), whole.entries.len());
        assert_eq!(published, 3);
        assert_eq!(browsed.folders, vec![child]);
        let flat = browse(&dir, &dir.join(OUTPUT_DIR)).unwrap();
        assert_eq!(flat.scan.entries.len(), 1);
        let _ = std::fs::remove_dir_all(dir);
    }

    #[test]
    fn pre_cancelled_scan_starts_no_work() {
        let dir = temp_dir("pre-cancelled");
        write_sample(&dir, "image.png", 8, 8);
        let cancelled = Arc::new(AtomicBool::new(true));
        let walks = Arc::new(std::sync::atomic::AtomicUsize::new(0));
        let probes = Arc::new(std::sync::atomic::AtomicUsize::new(0));
        let callbacks = Arc::new(std::sync::atomic::AtomicUsize::new(0));
        let hooks = ScanHooks {
            before_walk_next: {
                let walks = walks.clone();
                Arc::new(move || {
                    walks.fetch_add(1, Ordering::Relaxed);
                })
            },
            before_probe: {
                let probes = probes.clone();
                Arc::new(move |_| {
                    probes.fetch_add(1, Ordering::Relaxed);
                })
            },
            before_callback: {
                let callbacks = callbacks.clone();
                Arc::new(move || {
                    callbacks.fetch_add(1, Ordering::Relaxed);
                })
            },
            ..ScanHooks::default()
        };

        assert!(matches!(
            scan_progressive_cancellable_with_hooks(
                &dir,
                &dir.join(OUTPUT_DIR),
                cancelled,
                |_| ControlFlow::Continue(()),
                hooks,
            ),
            ScanOutcome::Cancelled
        ));
        assert_eq!(walks.load(Ordering::Relaxed), 0);
        assert_eq!(probes.load(Ordering::Relaxed), 0);
        assert_eq!(callbacks.load(Ordering::Relaxed), 0);
    }

    #[test]
    fn cancelled_scan_never_returns_partial_complete() {
        let dir = temp_dir("partial-cancelled");
        for index in 0..32 {
            write_sample(&dir, &format!("image-{index}.png"), 8, 8);
        }
        let cancelled = Arc::new(AtomicBool::new(false));
        let published = Arc::new(std::sync::atomic::AtomicUsize::new(0));

        let outcome = cancellable(&dir, cancelled.clone(), {
            let published = published.clone();
            move |batch| {
                published.fetch_add(batch.len(), Ordering::Relaxed);
                cancelled.store(true, Ordering::Release);
                ControlFlow::Continue(())
            }
        });

        assert!(matches!(outcome, ScanOutcome::Cancelled));
        assert_eq!(published.load(Ordering::Relaxed), 32);
    }

    #[test]
    fn cancellation_during_callback_never_returns_complete() {
        let dir = temp_dir("callback-cancelled");
        for index in 0..32 {
            write_sample(&dir, &format!("image-{index}.png"), 8, 8);
        }
        let cancelled = Arc::new(AtomicBool::new(false));
        let callbacks = Arc::new(std::sync::atomic::AtomicUsize::new(0));

        let outcome = cancellable(&dir, cancelled.clone(), {
            let callbacks = callbacks.clone();
            move |_| {
                callbacks.fetch_add(1, Ordering::Relaxed);
                cancelled.store(true, Ordering::Release);
                ControlFlow::Continue(())
            }
        });

        assert!(matches!(outcome, ScanOutcome::Cancelled));
        assert_eq!(callbacks.load(Ordering::Relaxed), 1);
    }

    #[test]
    fn observed_cancellation_suppresses_final_tail() {
        let dir = temp_dir("tail-cancelled");
        for index in 0..33 {
            write_sample(&dir, &format!("image-{index}.png"), 8, 8);
        }
        let cancelled = Arc::new(AtomicBool::new(false));
        let callbacks = Arc::new(std::sync::atomic::AtomicUsize::new(0));
        let hooks = ScanHooks {
            before_callback: {
                let cancelled = cancelled.clone();
                Arc::new(move || cancelled.store(true, Ordering::Release))
            },
            ..ScanHooks::default()
        };

        let outcome = scan_progressive_cancellable_with_hooks(
            &dir,
            &dir.join(OUTPUT_DIR),
            cancelled,
            {
                let callbacks = callbacks.clone();
                move |_| {
                    callbacks.fetch_add(1, Ordering::Relaxed);
                    ControlFlow::Continue(())
                }
            },
            hooks,
        );

        assert!(matches!(outcome, ScanOutcome::Cancelled));
        assert_eq!(callbacks.load(Ordering::Relaxed), 0);
    }

    #[test]
    fn cancellation_wakes_a_blocked_path_sender() {
        let dir = temp_dir("blocked-path-send");
        write_sample(&dir, "one.png", 8, 8);
        write_sample(&dir, "two.png", 8, 8);
        let cancelled = Arc::new(AtomicBool::new(false));
        let workers_ready = Arc::new(Gate::new());
        let walker_ready = Arc::new(Gate::new());
        let (workers_entered_sender, workers_entered_receiver) = mpsc::channel();
        let (path_attempt_sender, path_attempt_receiver) = mpsc::channel();
        let path_sends = Arc::new(std::sync::atomic::AtomicUsize::new(0));
        let mut hooks = ScanHooks {
            queue_capacity: Some(1),
            ..ScanHooks::default()
        };
        hooks.before_path_receive = {
            let workers_ready = workers_ready.clone();
            Arc::new(move || {
                workers_entered_sender.send(()).unwrap();
                workers_ready.wait();
            })
        };
        hooks.before_walk_next = {
            let walker_ready = walker_ready.clone();
            Arc::new(move || walker_ready.wait())
        };
        hooks.before_path_send = {
            let path_sends = path_sends.clone();
            Arc::new(move || {
                if path_sends.fetch_add(1, Ordering::Relaxed) == 1 {
                    path_attempt_sender.send(()).unwrap();
                }
            })
        };
        let (outcome_sender, outcome_receiver) = mpsc::channel();
        let root = dir.clone();
        let run_cancelled = cancelled.clone();
        std::thread::spawn(move || {
            outcome_sender
                .send(scan_progressive_cancellable_with_hooks(
                    &root,
                    &root.join(OUTPUT_DIR),
                    run_cancelled,
                    |_| ControlFlow::Continue(()),
                    hooks,
                ))
                .unwrap();
        });

        for _ in 0..worker_count() {
            workers_entered_receiver
                .recv_timeout(Duration::from_secs(5))
                .expect("every worker waits before receiving a path");
        }
        walker_ready.open();
        path_attempt_receiver
            .recv_timeout(Duration::from_secs(5))
            .expect("the second path send reaches its full queue");
        cancelled.store(true, Ordering::Release);
        workers_ready.open();
        assert!(matches!(
            outcome_receiver
                .recv_timeout(Duration::from_secs(5))
                .expect("cancellation tears down the blocked path send"),
            ScanOutcome::Cancelled
        ));
    }

    #[test]
    fn cancellation_wakes_a_blocked_result_sender() {
        let dir = temp_dir("blocked-result-send");
        write_sample(&dir, "one.png", 8, 8);
        write_sample(&dir, "two.png", 8, 8);
        let cancelled = Arc::new(AtomicBool::new(false));
        let collector_ready = Arc::new(Gate::new());
        let (collector_entered_sender, collector_entered_receiver) = mpsc::channel();
        let (result_attempt_sender, result_attempt_receiver) = mpsc::channel();
        let result_sends = Arc::new(std::sync::atomic::AtomicUsize::new(0));
        let mut hooks = ScanHooks {
            queue_capacity: Some(1),
            ..ScanHooks::default()
        };
        hooks.before_result_receive = {
            let collector_ready = collector_ready.clone();
            Arc::new(move || {
                collector_entered_sender.send(()).unwrap();
                collector_ready.wait();
            })
        };
        hooks.before_result_send = {
            let result_sends = result_sends.clone();
            Arc::new(move || {
                if result_sends.fetch_add(1, Ordering::Relaxed) == 1 {
                    result_attempt_sender.send(()).unwrap();
                }
            })
        };
        let (outcome_sender, outcome_receiver) = mpsc::channel();
        let root = dir.clone();
        let run_cancelled = cancelled.clone();
        std::thread::spawn(move || {
            outcome_sender
                .send(scan_progressive_cancellable_with_hooks(
                    &root,
                    &root.join(OUTPUT_DIR),
                    run_cancelled,
                    |_| ControlFlow::Continue(()),
                    hooks,
                ))
                .unwrap();
        });

        collector_entered_receiver
            .recv_timeout(Duration::from_secs(5))
            .expect("collector waits before receiving a result");
        result_attempt_receiver
            .recv_timeout(Duration::from_secs(5))
            .expect("the second result send reaches its full queue");
        cancelled.store(true, Ordering::Release);
        collector_ready.open();
        assert!(matches!(
            outcome_receiver
                .recv_timeout(Duration::from_secs(5))
                .expect("cancellation tears down the blocked result send"),
            ScanOutcome::Cancelled
        ));
    }

    #[test]
    fn cancellation_stops_trailing_non_images() {
        let dir = temp_dir("non-image-tail");
        for index in 0..32 {
            write_sample(&dir, &format!("image-{index}.png"), 8, 8);
        }
        for index in 0..1_000 {
            std::fs::write(dir.join(format!("tail-{index}.txt")), b"not an image").unwrap();
        }
        let cancelled = Arc::new(AtomicBool::new(false));
        let callbacks = Arc::new(std::sync::atomic::AtomicUsize::new(0));
        let probes = Arc::new(std::sync::atomic::AtomicUsize::new(0));
        let probes_at_cancellation = Arc::new(std::sync::atomic::AtomicUsize::new(0));
        let hooks = ScanHooks {
            before_probe: {
                let probes = probes.clone();
                Arc::new(move |_| {
                    probes.fetch_add(1, Ordering::Relaxed);
                })
            },
            ..ScanHooks::default()
        };

        let outcome = scan_progressive_cancellable_with_hooks(
            &dir,
            &dir.join(OUTPUT_DIR),
            cancelled.clone(),
            {
                let callbacks = callbacks.clone();
                let probes = probes.clone();
                let probes_at_cancellation = probes_at_cancellation.clone();
                move |_| {
                    callbacks.fetch_add(1, Ordering::Relaxed);
                    probes_at_cancellation.store(probes.load(Ordering::Relaxed), Ordering::Relaxed);
                    cancelled.store(true, Ordering::Release);
                    ControlFlow::Continue(())
                }
            },
            hooks,
        );

        assert!(matches!(outcome, ScanOutcome::Cancelled));
        assert_eq!(callbacks.load(Ordering::Relaxed), 1);
        assert!(
            probes.load(Ordering::Relaxed)
                <= probes_at_cancellation.load(Ordering::Relaxed) + worker_count(),
            "only probes admitted before the callback can finish after cancellation"
        );
    }
}
