//! Read what a folder of images actually contains.
//!
//! Everything here is header-only. Decoding a 6000px JPEG to learn it is 6000px wide
//! costs a hundred times what reading its header costs, and a shoot folder has
//! thousands of them.

use std::path::{Path, PathBuf};

use image::{
    AnimationDecoder, DynamicImage, ImageDecoder, ImageFormat, ImageReader,
    codecs::gif::GifDecoder, metadata::Orientation,
};
use walkdir::WalkDir;

/// Camera raw formats. Most are TIFF containers, so a plain header read reports the
/// embedded preview — a 6000x4000 NEF comes back as a 160x120 TIFF, which makes every
/// derived number a lie. They are also not web delivery candidates. Counted, not listed.
const RAW_EXTENSIONS: [&str; 9] = [
    "nef", "cr2", "cr3", "arw", "dng", "orf", "rw2", "raf", "srw",
];

pub fn is_raw(path: &Path) -> bool {
    path.extension()
        .and_then(|extension| extension.to_str())
        .is_some_and(|extension| RAW_EXTENSIONS.contains(&extension.to_ascii_lowercase().as_str()))
}

/// Converted files land here, inside the folder being audited. A second run would
/// otherwise list its own output and offer to convert it again.
pub const OUTPUT_DIR: &str = "optimized";

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

/// True when this directory is one macOS keeps opaque. Packages are a macOS
/// concept — on other systems these names are just folders, so they keep being
/// walked there.
fn is_opaque_package(path: &Path) -> bool {
    if !cfg!(target_os = "macos") {
        return false;
    }
    path.extension()
        .and_then(|extension| extension.to_str())
        .is_some_and(|extension| {
            PACKAGE_EXTENSIONS.contains(&extension.to_ascii_lowercase().as_str())
        })
}

/// What a folder holds.
pub struct Scan {
    pub entries: Vec<Entry>,
    /// Camera raw files left out of the list, so the total is not silently short.
    /// A count, not names: raw is excluded by design and a photographer knows they
    /// have it. Nothing in the window would act on the list.
    pub skipped_raw: usize,
    /// macOS packages the walk never entered, counted like raw for the same
    /// reason: they are excluded by design and the total says so.
    pub skipped_packages: usize,
    /// Files that look like images by extension but would not decode. Named, not
    /// counted: "3 would not decode" tells you a folder has a problem and gives you
    /// nowhere to look for it.
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

#[derive(Clone, Debug, PartialEq)]
pub struct Entry {
    pub path: PathBuf,
    pub format: ImageFormat,
    pub width: u32,
    pub height: u32,
    /// Bytes on disk, not decoded size.
    pub bytes: u64,
}

impl Entry {
    pub fn name(&self) -> String {
        self.path
            .file_name()
            .map(|name| name.to_string_lossy().into_owned())
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
            .contains(&extension.to_ascii_lowercase().as_str())
    }
}

/// Read one file's header. `None` when it is not an image we can read.
pub fn probe(path: &Path) -> Option<Entry> {
    let bytes = std::fs::metadata(path).ok()?.len();
    let reader = ImageReader::open(path).ok()?.with_guessed_format().ok()?;
    let format = reader.format()?;
    let mut decoder = reader.into_decoder().ok()?;
    let (mut width, mut height) = decoder.dimensions();
    if orientation_swaps_dimensions(decoder.orientation().unwrap_or(Orientation::NoTransforms)) {
        std::mem::swap(&mut width, &mut height);
    }

    Some(Entry {
        path: path.to_path_buf(),
        format,
        width,
        height,
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
    let reader = ImageReader::open(path).ok()?.with_guessed_format().ok()?;
    let mut decoder = reader.into_decoder().ok()?;
    let orientation = decoder.orientation().unwrap_or(Orientation::NoTransforms);
    let mut image = DynamicImage::from_decoder(decoder).ok()?;
    image.apply_orientation(orientation);
    Some(image)
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum ConversionDecodeError {
    Failed,
    AnimatedGif,
}

/// Decode one still image for conversion. GIF is the exception to the generic
/// decoder: it exposes only the first frame as a `DynamicImage`, so accepting an
/// animation here would silently replace it with a still.
pub fn decode_for_conversion(path: &Path) -> Result<DynamicImage, ConversionDecodeError> {
    let reader = ImageReader::open(path)
        .map_err(|_| ConversionDecodeError::Failed)?
        .with_guessed_format()
        .map_err(|_| ConversionDecodeError::Failed)?;
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
        return Ok(DynamicImage::ImageRgba8(first.into_buffer()));
    }

    let mut decoder = reader
        .into_decoder()
        .map_err(|_| ConversionDecodeError::Failed)?;
    let orientation = decoder.orientation().unwrap_or(Orientation::NoTransforms);
    let mut image =
        DynamicImage::from_decoder(decoder).map_err(|_| ConversionDecodeError::Failed)?;
    image.apply_orientation(orientation);
    Ok(image)
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
pub fn scan(root: &Path) -> Scan {
    let mut candidates = Vec::new();
    let mut skipped_raw = 0;
    let mut existing_output = 0;
    let mut walk_errors = Vec::new();
    let mut skipped_packages = 0;
    let counted_packages = &mut skipped_packages;
    let output_root = root.join(OUTPUT_DIR);

    // `filter_entry` prunes: a package directory is never descended into, so its
    // unreadable interior is never even attempted. Walk errors pass the predicate
    // untouched — a folder that is locked and not a package still names itself.
    for entry in WalkDir::new(root)
        .follow_links(false)
        .into_iter()
        .filter_entry(|entry| {
            // The folder the user asked for is entered whatever it is called.
            if entry.depth() == 0 {
                return true;
            }
            // Output is already excluded below, but it still has to be walked so the
            // root output count remains truthful. A package there is not skipped input.
            if entry
                .path()
                .components()
                .any(|part| part.as_os_str() == OUTPUT_DIR)
            {
                return true;
            }
            if entry.file_type().is_dir() && is_opaque_package(entry.path()) {
                *counted_packages += 1;
                return false;
            }
            true
        })
    {
        // A directory whose readdir fails yields an Err here. Dropping it would
        // report a folder that was never fully looked at as fully audited, so the
        // path is kept and named in the same way decode failures are.
        let Ok(entry) = entry else {
            if let Some(path) = entry.unwrap_err().path() {
                walk_errors.push(path.to_path_buf());
            }
            continue;
        };
        let file = entry;
        if !file.file_type().is_file() {
            continue;
        }
        let relative = file.path().strip_prefix(root).unwrap_or(file.path());
        if relative
            .components()
            .any(|part| part.as_os_str() == OUTPUT_DIR)
        {
            if file.path().starts_with(&output_root) {
                existing_output += 1;
            }
            continue;
        }
        if is_raw(file.path()) {
            skipped_raw += 1;
            continue;
        }
        candidates.push(file.into_path());
    }

    // A probe is an open and a header read, so a few thousand of them are waiting on
    // the disk rather than arithmetic. Split the list across the cores and read them
    // at the same time: the folder a photographer points this at holds thousands, and
    // one at a time is the whole "Scanning…" wait.
    let threads = std::thread::available_parallelism().map_or(4, |count| count.get());
    let chunk = candidates.len().div_ceil(threads).max(1);
    let mut entries = Vec::with_capacity(candidates.len());
    let mut unreadable = Vec::new();
    std::thread::scope(|scope| {
        // Chunks are contiguous and joined in order, so the walk order survives into
        // the stable sort below and ties still break the way they always did.
        let workers: Vec<_> = candidates
            .chunks(chunk)
            .map(|chunk| {
                scope.spawn(move || {
                    let mut found = Vec::new();
                    let mut missed = Vec::new();
                    for path in chunk {
                        match probe(path) {
                            Some(entry) => found.push(entry),
                            // Only report things that claimed to be images. A README is
                            // not a failure.
                            None if looks_like_an_image(path) => missed.push(path.clone()),
                            None => {}
                        }
                    }
                    (found, missed)
                })
            })
            .collect();
        for worker in workers {
            let (found, missed) = worker.join().unwrap_or_default();
            entries.extend(found);
            unreadable.extend(missed);
        }
    });

    // Heaviest first: the top of the list is the work worth doing.
    entries.sort_by_key(|entry| std::cmp::Reverse(entry.bytes));
    Scan {
        entries,
        skipped_raw,
        skipped_packages,
        unreadable,
        walk_errors,
        existing_output,
    }
}

/// Extension-only guess, used to decide whether a decode failure is worth reporting.
fn looks_like_an_image(path: &Path) -> bool {
    const EXTENSIONS: [&str; 9] = [
        "jpg", "jpeg", "png", "webp", "avif", "gif", "tif", "tiff", "bmp",
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
pub fn format_name(format: ImageFormat) -> &'static str {
    match format {
        ImageFormat::Jpeg => "JPEG",
        ImageFormat::Png => "PNG",
        ImageFormat::WebP => "WebP",
        ImageFormat::Avif => "AVIF",
        ImageFormat::Gif => "GIF",
        ImageFormat::Tiff => "TIFF",
        ImageFormat::Bmp => "BMP",
        other => other.extensions_str().first().copied().unwrap_or("?"),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use image::{Frame, ImageBuffer, Rgb, Rgba, codecs::gif::GifEncoder};
    use std::os::unix::fs::PermissionsExt;

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
        dir
    }

    #[test]
    fn probes_dimensions_and_format_without_decoding() {
        let dir = temp_dir("probe");
        let path = write_sample(&dir, "sample.png", 40, 25);

        let entry = probe(&path).expect("png is readable");
        assert_eq!(entry.format, ImageFormat::Png);
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
        assert_eq!(probe(&liar).unwrap().format, ImageFormat::Png);
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
            decode_for_conversion(&path),
            Err(ConversionDecodeError::AnimatedGif)
        );
    }

    /// The audit's best finding is a file whose name disagrees with its bytes, so
    /// the check has to be right about which disagreements are real. `jpg` and
    /// `jpeg` naming the same format is not a finding.
    #[test]
    fn an_extension_only_lies_when_it_names_another_format() {
        let png = Entry {
            path: PathBuf::from("/photos/promo.png"),
            format: ImageFormat::Png,
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
            format: ImageFormat::Jpeg,
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

        let scanned = scan(&dir);
        assert_eq!(scanned.entries.len(), 1);
        assert_eq!(scanned.entries[0].name(), "real.png");
    }

    #[test]
    fn scan_reaches_subfolders_and_sorts_heaviest_first() {
        let dir = temp_dir("walk");
        std::fs::create_dir_all(dir.join("nested")).unwrap();
        write_sample(&dir, "small.png", 8, 8);
        write_sample(&dir.join("nested"), "big.png", 300, 300);

        let scanned = scan(&dir);
        assert_eq!(scanned.entries.len(), 2);
        assert_eq!(
            scanned.entries[0].name(),
            "big.png",
            "heaviest file sorts first"
        );
        assert!(scanned.entries[0].bytes > scanned.entries[1].bytes);
    }

    #[test]
    fn the_output_folder_is_not_audited_as_input() {
        let dir = temp_dir("output");
        write_sample(&dir, "source.png", 16, 16);
        std::fs::create_dir_all(dir.join(OUTPUT_DIR)).unwrap();
        write_sample(&dir.join(OUTPUT_DIR), "source.png", 16, 16);

        let scanned = scan(&dir);
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

        let scanned = scan(&dir);
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

        let scanned = scan(&dir);
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

        let scanned = scan(&dir);
        assert_eq!(scanned.entries.len(), 1);
        assert_eq!(
            scanned.skipped_raw, 2,
            "raw is counted, not silently dropped"
        );
    }

    /// The walk hands its files to one thread per core, so every count and the sort
    /// order have to survive being assembled from several chunks instead of one loop.
    #[test]
    fn a_folder_larger_than_one_chunk_reports_every_file_once() {
        let dir = temp_dir("parallel");
        for index in 0..40u32 {
            write_sample(&dir, &format!("s{index:02}.png"), 8 + index, 8);
        }
        std::fs::write(dir.join("broken.png"), b"not a png").unwrap();

        let scanned = scan(&dir);
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

        let scanned = scan(&dir);
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

    #[test]
    fn a_folder_it_cannot_enter_is_named_not_swallowed() {
        let dir = temp_dir("walk-error");
        let locked = dir.join("locked");
        std::fs::create_dir_all(&locked).unwrap();
        write_sample(&dir, "keep.png", 8, 8);
        // No read or execute permission: readdir on it fails.
        std::fs::set_permissions(&locked, std::fs::Permissions::from_mode(0o000)).unwrap();

        let scanned = scan(&dir);

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

        let scanned = scan(&dir);
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

        let scanned = scan(&dir);
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

        let scanned = scan(&dir);
        assert_eq!(scanned.entries.len(), 1);
        assert_eq!(scanned.entries[0].path, disguised);
        assert_eq!(scanned.entries[0].format, ImageFormat::Png);
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

        let scanned = scan(&dir);
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
            format: ImageFormat::Png,
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
}
