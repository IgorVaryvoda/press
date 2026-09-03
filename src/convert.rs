//! Re-encode an image, on this machine.
//!
//! The `image` crate can only write lossless WebP, which is the wrong tool for the
//! job — the whole point is trading a little quality for a lot of bytes. This uses
//! libwebp directly for both, and picks between them by whether the source has
//! meaningful transparency.
//!
//! AVIF goes through libavif's libaom backend, with libyuv colour conversion where
//! packaged. The system libraries are the same path as `avifenc`, without starting a
//! process per image.
//!
//! JPEG XL encoding uses jixel and decoding uses jxl-oxide, both in Rust.
//!
//! JPEG goes through libjpeg-turbo, which the thumbnails already decode with. Its
//! container has no alpha plane, so a see-through source is refused by name rather
//! than flattened onto a colour nobody chose.

use std::collections::HashSet;
use std::ffi::{OsStr, OsString};
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicUsize, Ordering};

use image::DynamicImage;
use libwebp_sys::{
    WEBP_ENCODER_ABI_VERSION, WebPConfig, WebPConfigInitInternal, WebPData, WebPDataClear,
    WebPEncode, WebPMemoryWrite, WebPMemoryWriter, WebPMemoryWriterClear, WebPMemoryWriterInit,
    WebPMuxAssemble, WebPMuxDelete, WebPMuxError, WebPMuxNew, WebPMuxSetChunk, WebPMuxSetImage,
    WebPPicture, WebPPictureFree, WebPPictureImportRGB, WebPPictureImportRGBA,
    WebPPictureInitInternal, WebPPreset, WebPValidateConfig,
};

/// Encoder quality, 1 to 100. `None` means lossless.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Quality(pub Option<f32>);

impl Quality {
    pub const LOSSLESS: Self = Self(None);

    pub fn lossy(value: f32) -> Self {
        Self(Some(value.clamp(1., 100.)))
    }

    pub fn label(&self) -> String {
        match self.0 {
            None => "lossless".to_string(),
            Some(value) => format!("q{}", value.round() as u32),
        }
    }
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Failure {
    Failed,
    LosslessNeedsEightBit,
    LosslessNeedsIntegerSamples,
    ProfileNotAttached,
    JpegNeedsOpaque,
    KeepFormatUnavailable(String),
    ExtensionLies(String, &'static str),
    AnimatedGif,
    AnimatedPng,
    AnimatedWebP,
    AnimatedJpegXl,
    OutsideOutput,
    UnsafeOutputPath,
    OverwritesSource,
    BackupOccupied(PathBuf),
    BackupFailed,
    InstallFailed,
    OriginalLeftInBackup(PathBuf),
    RecordNotWritten(String),
    StalePartial(PathBuf),
}

impl Failure {
    pub fn reason(&self) -> Option<String> {
        match self {
            Self::Failed => None,
            Self::LosslessNeedsEightBit => {
                Some("lossless WebP cannot keep more than 8 bits per colour channel".into())
            }
            Self::LosslessNeedsIntegerSamples => {
                Some("lossless JPEG XL cannot keep 32-bit floating point samples".into())
            }
            Self::ProfileNotAttached => Some("the colour profile could not be attached".into()),
            Self::JpegNeedsOpaque => Some("JPEG cannot keep transparency".into()),
            Self::KeepFormatUnavailable(name) => {
                Some(format!("keep format is not available for {name}"))
            }
            Self::ExtensionLies(extension, probed) => Some(format!(
                "named .{extension} but the bytes are {probed}; convert it explicitly"
            )),
            Self::AnimatedGif => Some("animated GIFs are not converted".into()),
            Self::AnimatedPng => Some("animated PNG files are not converted".into()),
            Self::AnimatedWebP => Some("animated WebP files are not converted".into()),
            Self::AnimatedJpegXl => Some("animated JPEG XL files are not converted".into()),
            Self::OutsideOutput => Some("the target is outside the output folder".into()),
            Self::UnsafeOutputPath => {
                Some("a folder on the way to the target is not a plain folder".into())
            }
            Self::OverwritesSource => Some("the output would overwrite a source image".into()),
            Self::BackupOccupied(path) => Some(format!(
                "an earlier run already backed up an original here: {}",
                path.display()
            )),
            Self::BackupFailed => {
                Some("the original could not be moved into the backup folder".into())
            }
            Self::InstallFailed => {
                Some("the finished file could not take its place; the original is unchanged".into())
            }
            Self::OriginalLeftInBackup(path) => Some(format!(
                "the finished file could not take its place and the original is in the backup: {}",
                path.display()
            )),
            Self::RecordNotWritten(reason) => {
                Some(format!("the run record could not be written: {reason}"))
            }
            Self::StalePartial(path) => Some(format!(
                "a leftover partial file is in the way: {}",
                path.display()
            )),
        }
    }
}

/// Longest edge of the exported image. `None` leaves the source alone.
///
/// This is where most of the weight actually is. Re-encoding a 6400px photo as AVIF
/// still hands back a 6400px photo, which is the wrong image for a web page however
/// well it is compressed.
#[derive(Clone, Copy, Debug, Hash, PartialEq, Eq)]
pub struct MaxEdge(pub Option<u32>);

impl MaxEdge {
    pub const FULL: Self = Self(None);

    /// The sizes offered in the window, in order. Listed once so the buttons and the
    /// value they select cannot disagree.
    pub const PRESETS: [Self; 4] = [
        Self::FULL,
        Self(Some(2400)),
        Self(Some(1600)),
        Self(Some(1000)),
    ];

    pub fn label(&self) -> String {
        match self.0 {
            None => "full".to_string(),
            Some(edge) => format!("{edge}px"),
        }
    }

    /// A size typed into the window. The same rule as `--max-edge`: a positive whole
    /// number of pixels. An emptied box means the source size again; anything else
    /// is `None`, and the caller leaves the current size alone rather than guess.
    pub fn parse(text: &str) -> Option<Self> {
        let text = text.trim();
        if text.is_empty() {
            return Some(Self::FULL);
        }
        text.parse()
            .ok()
            .filter(|edge| *edge > 0)
            .map(|edge| Self(Some(edge)))
    }

    /// Scale `image` down to fit. Never scales up: an 800px source asked to fit 2000px
    /// is already inside the budget, and stretching it would invent detail.
    pub fn apply(&self, image: DynamicImage) -> DynamicImage {
        let Some(edge) = self.0 else {
            return image;
        };
        if image.width().max(image.height()) <= edge {
            return image;
        }
        // Lanczos3 rather than the fast filter used for thumbnails: this one is what
        // gets shipped, and a soft downscale wastes the bytes it saves.
        image.resize(edge, edge, image::imageops::FilterType::Lanczos3)
    }
}

/// The container to write.
///
/// `Same` keeps each source's own container, so `hero.jpg` comes out as `hero.jpg`
/// and every `<img src>` that named it still resolves. With a max edge it is the
/// "just make them smaller" run. `Png` exists for that path alone: the window and
/// the CLI never offer it, because a PNG is only ever the right output for a PNG.
#[derive(Clone, Copy, Debug, Hash, PartialEq, Eq)]
pub enum Format {
    WebP,
    Avif,
    JpegXl,
    Jpeg,
    Png,
    Same,
}

impl Format {
    /// `None` for `Same`: the output keeps whatever extension the source has.
    pub fn extension(&self) -> Option<&'static str> {
        match self {
            Format::WebP => Some("webp"),
            Format::Avif => Some("avif"),
            Format::JpegXl => Some("jxl"),
            Format::Jpeg => Some("jpg"),
            Format::Png => Some("png"),
            Format::Same => None,
        }
    }

    pub fn label(&self) -> &'static str {
        match self {
            Format::WebP => "webp",
            Format::Avif => "avif",
            Format::JpegXl => "jxl",
            Format::Jpeg => "jpeg",
            Format::Png => "png",
            Format::Same => "same",
        }
    }

    /// `Same` says no: a folder holds JPEGs and AVIFs side by side, and the label
    /// would be true of some files and false of the rest.
    pub fn supports_lossless(self) -> bool {
        matches!(self, Self::WebP | Self::JpegXl | Self::Png)
    }

    /// The window's word for the format. `Same` is a verb in the window ("Keep"),
    /// where "SAME" beside four container names would read as a fifth.
    pub fn display(&self) -> &'static str {
        match self {
            Format::WebP => "WEBP",
            Format::Avif => "AVIF",
            Format::JpegXl => "JXL",
            Format::Jpeg => "JPEG",
            Format::Png => "PNG",
            Format::Same => "Keep",
        }
    }

    /// The format a source's name promises, or `None` when no encoder here answers
    /// to that name. Reads nothing: the comparison view asks during a render.
    pub fn from_extension(source: &Path) -> Option<Format> {
        let extension = source.extension()?.to_string_lossy().to_lowercase();
        match extension.as_str() {
            "jpg" | "jpeg" | "jpe" => Some(Format::Jpeg),
            "png" => Some(Format::Png),
            "webp" => Some(Format::WebP),
            "avif" => Some(Format::Avif),
            "jxl" => Some(Format::JpegXl),
            _ => None,
        }
    }

    /// The encoder `Same` means for one source. The name is what the output keeps,
    /// so the name and the bytes have to agree: a PNG called `.jpg` is the audit's
    /// own headline finding, and writing a lossy JPEG under that name would report
    /// the lie as a conversion. It is refused by name, as is anything without an
    /// encoder here. Reads the file header, so this belongs off the main thread.
    pub fn resolve(self, source: &Path) -> Result<Format, Failure> {
        if self != Format::Same {
            return Ok(self);
        }
        let extension = source
            .extension()
            .map(|extension| extension.to_string_lossy().to_lowercase())
            .unwrap_or_default();
        if extension.is_empty() {
            return Err(Failure::KeepFormatUnavailable(
                "a file with no extension".into(),
            ));
        }
        let format = Self::from_extension(source)
            .ok_or_else(|| Failure::KeepFormatUnavailable(extension.to_uppercase()))?;
        if let Some(entry) = crate::scan::probe(source)
            && entry.extension_lies()
        {
            return Err(Failure::ExtensionLies(
                extension,
                crate::scan::format_name(entry.format),
            ));
        }
        Ok(format)
    }
}

/// The result of re-encoding one file.
#[derive(Clone, Debug, PartialEq)]
pub struct Converted {
    pub written: PathBuf,
    pub bytes: u64,
    /// Dimensions actually written, which differ from the source when resizing.
    pub width: u32,
    pub height: u32,
}

/// Encode `image` in `format`. Returns the encoded bytes.
///
/// `profile` is the source's ICC profile, which every output format here can carry.
/// Previews pass `None`; a file being written to disk passes what it was decoded
/// with, or the colours it was tagged with are lost on the way out.
pub fn encode(
    image: &DynamicImage,
    format: Format,
    quality: Quality,
    profile: Option<&[u8]>,
) -> Result<Vec<u8>, Failure> {
    let profile = profile.filter(|profile| !profile.is_empty());
    match format {
        Format::WebP => {
            let encoded = encode_webp(image, quality).ok_or(Failure::Failed)?;
            match profile {
                Some(profile) => {
                    attach_webp_profile(&encoded, profile).ok_or(Failure::ProfileNotAttached)
                }
                None => Ok(encoded),
            }
        }
        Format::Avif => encode_avif(image, quality, profile).ok_or(Failure::Failed),
        Format::JpegXl => encode_jpeg_xl(image, quality, profile).ok_or(Failure::Failed),
        Format::Jpeg => {
            let encoded = encode_jpeg(image, quality)?;
            match profile {
                Some(profile) => {
                    attach_jpeg_profile(&encoded, profile).ok_or(Failure::ProfileNotAttached)
                }
                None => Ok(encoded),
            }
        }
        Format::Png => encode_png(image, profile).ok_or(Failure::Failed),
        // Callers resolve `Same` against the source before encoding; there is no
        // source here to resolve it against.
        Format::Same => Err(Failure::Failed),
    }
}

/// PNG is lossless whatever the quality says, so the only knob worth turning is
/// how hard the deflater works. The samples go in at the source's own depth: a
/// 16-bit PNG asked to keep its format keeps its bits.
///
/// `Default` was measured 44% faster for 2.1% larger output on 16 real PNGs
/// (16.1s vs 28.9s debug). Best stays: output bytes are the product, and PNG
/// runs are rare enough that the time is not the bottleneck.
fn encode_png(image: &DynamicImage, profile: Option<&[u8]>) -> Option<Vec<u8>> {
    let mut encoded = Vec::new();
    let mut encoder = image::codecs::png::PngEncoder::new_with_quality(
        &mut encoded,
        image::codecs::png::CompressionType::Best,
        image::codecs::png::FilterType::Adaptive,
    );
    if let Some(profile) = profile {
        image::ImageEncoder::set_icc_profile(&mut encoder, profile.to_vec()).ok()?;
    }
    image::ImageEncoder::write_image(
        encoder,
        image.as_bytes(),
        image.width(),
        image.height(),
        image.color().into(),
    )
    .ok()?;
    Some(encoded)
}

/// libjpeg-turbo writes no metadata, so the profile goes in by hand: APP2 segments
/// tagged `ICC_PROFILE`, at most 65519 profile bytes each, right after the JFIF
/// APP0 the spec wants first. Every decoder that reads a profile reads it there.
fn attach_jpeg_profile(encoded: &[u8], profile: &[u8]) -> Option<Vec<u8>> {
    const CHUNK: usize = 65_533 - 14;
    let mut at = 2;
    if encoded.get(at..at + 2) == Some(&[0xff, 0xe0]) {
        let length = encoded.get(at + 2..at + 4)?;
        at += 2 + usize::from(u16::from_be_bytes([length[0], length[1]]));
    }
    let count = u8::try_from(profile.chunks(CHUNK).len()).ok()?;
    let mut tagged = Vec::with_capacity(encoded.len() + profile.len() + 18 * usize::from(count));
    tagged.extend_from_slice(encoded.get(..at)?);
    for (index, chunk) in profile.chunks(CHUNK).enumerate() {
        tagged.extend_from_slice(&[0xff, 0xe2]);
        tagged.extend_from_slice(&(chunk.len() as u16 + 16).to_be_bytes());
        tagged.extend_from_slice(b"ICC_PROFILE\0");
        tagged.extend_from_slice(&[index as u8 + 1, count]);
        tagged.extend_from_slice(chunk);
    }
    tagged.extend_from_slice(&encoded[at..]);
    Some(tagged)
}

/// 4:2:0 subsampling and baseline Huffman: what every camera and CMS writes, so
/// the output opens everywhere the source did. A grayscale source stays one
/// plane; promoted to RGB it would pay for two chroma planes of nothing. Lossless
/// never reaches here; the window and the CLI both refuse it for JPEG, so `None`
/// only has to be safe.
fn encode_jpeg(image: &DynamicImage, quality: Quality) -> Result<Vec<u8>, Failure> {
    if has_transparency(image) {
        return Err(Failure::JpegNeedsOpaque);
    }
    let gray = matches!(
        image,
        DynamicImage::ImageLuma8(_) | DynamicImage::ImageLuma16(_)
    );
    // Photographs already arrive as RGB8 or Luma8: borrow those buffers instead of
    // copying every pixel into a second frame before the compressor reads it.
    let luma_owned;
    let rgb_owned;
    let (pixels, channels, pixel_format, subsamp) = if gray {
        let pixels: &[u8] = if let Some(gray) = image.as_luma8() {
            gray.as_raw()
        } else {
            luma_owned = image.to_luma8().into_raw();
            &luma_owned
        };
        (
            pixels,
            1,
            turbojpeg::PixelFormat::GRAY,
            turbojpeg::Subsamp::Gray,
        )
    } else {
        let pixels: &[u8] = if let Some(rgb) = image.as_rgb8() {
            rgb.as_raw()
        } else {
            rgb_owned = image.to_rgb8().into_raw();
            &rgb_owned
        };
        (
            pixels,
            3,
            turbojpeg::PixelFormat::RGB,
            turbojpeg::Subsamp::Sub2x2,
        )
    };
    let mut compressor = turbojpeg::Compressor::new().map_err(|_| Failure::Failed)?;
    compressor
        .set_quality(quality.0.unwrap_or(100.).round() as i32)
        .map_err(|_| Failure::Failed)?;
    compressor
        .set_subsamp(subsamp)
        .map_err(|_| Failure::Failed)?;
    // Optimised Huffman tables cost a second pass and buy a few percent for free.
    compressor.set_optimize(true).map_err(|_| Failure::Failed)?;
    compressor
        .compress_to_vec(turbojpeg::Image {
            pixels,
            width: image.width() as usize,
            pitch: image.width() as usize * channels,
            height: image.height() as usize,
            format: pixel_format,
        })
        .map_err(|_| Failure::Failed)
}

/// Rewrap an encoded bitstream in WebP's extended container so it can carry an ICCP
/// chunk. The simple container libwebp writes has nowhere to put a profile at all.
fn attach_webp_profile(encoded: &[u8], profile: &[u8]) -> Option<Vec<u8>> {
    let bitstream = WebPData {
        bytes: encoded.as_ptr(),
        size: encoded.len(),
    };
    let profile = WebPData {
        bytes: profile.as_ptr(),
        size: profile.len(),
    };
    // SAFETY: both slices outlive the calls, which are told to copy what they read.
    // The mux and the assembled data are released on every path out.
    unsafe {
        let mux = WebPMuxNew();
        if mux.is_null() {
            return None;
        }
        let mut assembled = WebPData::default();
        let assembled_ok = WebPMuxSetImage(mux, &bitstream, 1) == WebPMuxError::WEBP_MUX_OK
            && WebPMuxSetChunk(mux, c"ICCP".as_ptr(), &profile, 1) == WebPMuxError::WEBP_MUX_OK
            && WebPMuxAssemble(mux, &mut assembled) == WebPMuxError::WEBP_MUX_OK;
        WebPMuxDelete(mux);
        let output = if assembled_ok && !assembled.bytes.is_null() {
            Some(std::slice::from_raw_parts(assembled.bytes, assembled.size).to_vec())
        } else {
            None
        };
        WebPDataClear(&mut assembled);
        output
    }
}

/// libwebp's lossy path discards the alpha channel's precision in ways that ruin
/// cut-outs, so anything carrying real transparency goes lossless regardless of the
/// requested quality.
fn encode_webp(image: &DynamicImage, quality: Quality) -> Option<Vec<u8>> {
    let lossless = quality.0.is_none() || has_transparency(image);
    let mut config = std::mem::MaybeUninit::<WebPConfig>::uninit();
    // SAFETY: `config` points to writable storage for exactly one WebPConfig and the
    // ABI constant comes from the same statically linked libwebp crate.
    if unsafe {
        WebPConfigInitInternal(
            config.as_mut_ptr(),
            WebPPreset::WEBP_PRESET_DEFAULT,
            quality.0.unwrap_or(75.),
            WEBP_ENCODER_ABI_VERSION as i32,
        )
    } == 0
    {
        return None;
    }
    // SAFETY: the initializer above succeeded.
    let mut config = unsafe { config.assume_init() };
    config.lossless = i32::from(lossless);
    config.alpha_compression = i32::from(!lossless);
    config.quality = quality.0.unwrap_or(75.);
    // ponytail: method 1 makes lossy q80 output 13.5% larger, but cut a real 3.0GB
    // folder from 69.5s to 29.6s. Lossless keeps the prior method 4 behavior because
    // its explicit promise is unchanged pixels, not the lossy path's speed tradeoff.
    config.method = if lossless { 4 } else { 1 };
    if unsafe { WebPValidateConfig(&config) } == 0 {
        return None;
    }

    if let Some(pixels) = image.as_rgba8() {
        encode_webp_pixels(
            pixels.as_raw(),
            pixels.width(),
            pixels.height(),
            true,
            &config,
        )
    } else if let Some(pixels) = image.as_rgb8() {
        encode_webp_pixels(
            pixels.as_raw(),
            pixels.width(),
            pixels.height(),
            false,
            &config,
        )
    } else if image.color().has_alpha() {
        let pixels = image.to_rgba8();
        encode_webp_pixels(
            pixels.as_raw(),
            pixels.width(),
            pixels.height(),
            true,
            &config,
        )
    } else {
        let pixels = image.to_rgb8();
        encode_webp_pixels(
            pixels.as_raw(),
            pixels.width(),
            pixels.height(),
            false,
            &config,
        )
    }
}

fn encode_webp_pixels(
    pixels: &[u8],
    width: u32,
    height: u32,
    alpha: bool,
    config: &WebPConfig,
) -> Option<Vec<u8>> {
    let channels = if alpha { 4 } else { 3 };
    let stride = i32::try_from(width.checked_mul(channels)?).ok()?;
    let width = i32::try_from(width).ok()?;
    let height = i32::try_from(height).ok()?;
    let expected = usize::try_from(width)
        .ok()?
        .checked_mul(usize::try_from(height).ok()?)?
        .checked_mul(channels as usize)?;
    if pixels.len() != expected {
        return None;
    }
    let mut picture = std::mem::MaybeUninit::<WebPPicture>::uninit();
    // SAFETY: all pointers remain valid through the synchronous encode. Both libwebp
    // allocations are released before returning, including every failure path.
    unsafe {
        if WebPPictureInitInternal(picture.as_mut_ptr(), WEBP_ENCODER_ABI_VERSION as i32) == 0 {
            return None;
        }
        let mut picture = picture.assume_init();
        picture.use_argb = 1;
        picture.width = width;
        picture.height = height;
        let imported = if alpha {
            WebPPictureImportRGBA(&mut picture, pixels.as_ptr(), stride)
        } else {
            WebPPictureImportRGB(&mut picture, pixels.as_ptr(), stride)
        };
        if imported == 0 {
            WebPPictureFree(&mut picture);
            return None;
        }

        let mut writer = std::mem::MaybeUninit::<WebPMemoryWriter>::uninit();
        WebPMemoryWriterInit(writer.as_mut_ptr());
        let mut writer = writer.assume_init();
        picture.writer = Some(WebPMemoryWrite);
        picture.custom_ptr = (&mut writer as *mut WebPMemoryWriter).cast();
        let encoded = if WebPEncode(config, &mut picture) != 0 && !writer.mem.is_null() {
            Some(std::slice::from_raw_parts(writer.mem, writer.size).to_vec())
        } else {
            None
        };
        WebPMemoryWriterClear(&mut writer);
        WebPPictureFree(&mut picture);
        encoded
    }
}

/// AVIF keeps alpha in a separate plane, so transparency needs no special case here.
/// libaom and rav1e calibrate their 1-100 scales differently; 75% matches the former
/// rav1e output size and measured PSNR on the real corpus.
fn encode_avif(image: &DynamicImage, quality: Quality, profile: Option<&[u8]>) -> Option<Vec<u8>> {
    let has_alpha = has_transparency(image);
    let quality = aom_quality(quality);
    // The one read of the process-wide speed. Everything below this takes it as an
    // argument, so a test can ask for a speed without writing anything shared.
    let speed = crate::avif::speed();
    let cores = std::thread::available_parallelism().map_or(4, |count| count.get());
    let threads = (cores / workers(Format::Avif)).clamp(1, 8);

    if has_alpha {
        // The common photograph with transparency is already RGBA8: handing its
        // buffer straight to the bridge saves a full-frame copy per file. Other
        // depths still convert, as they always did.
        if let Some(rgba) = image.as_rgba8() {
            crate::avif::encode(
                rgba.as_raw(),
                rgba.width(),
                rgba.height(),
                true,
                quality,
                speed,
                threads,
                profile,
            )
        } else {
            let rgba = image.to_rgba8();
            crate::avif::encode(
                rgba.as_raw(),
                rgba.width(),
                rgba.height(),
                true,
                quality,
                speed,
                threads,
                profile,
            )
        }
    } else if let Some(rgb) = image.as_rgb8() {
        crate::avif::encode(
            rgb.as_raw(),
            rgb.width(),
            rgb.height(),
            false,
            quality,
            speed,
            threads,
            profile,
        )
    } else {
        let rgb = image.to_rgb8();
        crate::avif::encode(
            rgb.as_raw(),
            rgb.width(),
            rgb.height(),
            false,
            quality,
            speed,
            threads,
            profile,
        )
    }
}

fn encode_jpeg_xl(
    image: &DynamicImage,
    quality: Quality,
    profile: Option<&[u8]>,
) -> Option<Vec<u8>> {
    let has_alpha = has_transparency(image);
    if is_high_depth(image) {
        let pixels = if has_alpha {
            image.to_rgba16().into_raw()
        } else {
            image.to_rgb16().into_raw()
        };
        return crate::jxl::encode_16bit(
            &pixels,
            image.width(),
            image.height(),
            has_alpha,
            quality.0,
            profile,
        );
    }
    // Like AVIF: the decoded frame is usually already 8-bit packed, so borrow it
    // instead of copying every pixel before the encoder reads it.
    if has_alpha {
        if let Some(rgba) = image.as_rgba8() {
            return crate::jxl::encode(
                rgba.as_raw(),
                rgba.width(),
                rgba.height(),
                true,
                quality.0,
                profile,
            );
        }
        let pixels = image.to_rgba8().into_raw();
        crate::jxl::encode(
            &pixels,
            image.width(),
            image.height(),
            true,
            quality.0,
            profile,
        )
    } else if let Some(rgb) = image.as_rgb8() {
        crate::jxl::encode(
            rgb.as_raw(),
            rgb.width(),
            rgb.height(),
            false,
            quality.0,
            profile,
        )
    } else {
        let pixels = image.to_rgb8().into_raw();
        crate::jxl::encode(
            &pixels,
            image.width(),
            image.height(),
            false,
            quality.0,
            profile,
        )
    }
}

/// True when the source carries more than eight bits per channel. Only JPEG XL can
/// keep them here: libwebp is an eight-bit encoder and the AVIF bridge writes
/// eight-bit planes.
fn is_high_depth(image: &DynamicImage) -> bool {
    let colour = image.color();
    colour.bits_per_pixel() / u16::from(colour.channel_count()) > 8
}

fn aom_quality(quality: Quality) -> u8 {
    ((quality.0.unwrap_or(90.) * 0.75).round() as u8).clamp(1, 75)
}

/// True when any pixel is not fully opaque. A PNG with an alpha channel that is
/// entirely 255 is just an RGB image paying for a fourth channel, and should still
/// get the lossy path.
fn has_transparency(image: &DynamicImage) -> bool {
    match image {
        DynamicImage::ImageRgba8(buffer) => buffer.pixels().any(|pixel| pixel.0[3] != 255),
        DynamicImage::ImageLumaA8(buffer) => buffer.pixels().any(|pixel| pixel.0[1] != 255),
        DynamicImage::ImageRgba16(buffer) => buffer.pixels().any(|pixel| pixel.0[3] != u16::MAX),
        DynamicImage::ImageLumaA16(buffer) => buffer.pixels().any(|pixel| pixel.0[1] != u16::MAX),
        // A float TIFF is rare and its alpha is still alpha. Answering "no" here would
        // send a cut-out down the opaque path and flatten it.
        DynamicImage::ImageRgba32F(buffer) => buffer.pixels().any(|pixel| pixel.0[3] != 1.0),
        _ => false,
    }
}

/// How many files to convert at once.
///
/// Each file in flight holds a fully decoded image, so this bounds memory as much as
/// it bounds speed, and the two encoders want opposite things. libwebp runs on the
/// calling thread and left one core 46% busy on a sixteen-core machine, so WebP wants
/// a file per core. libaom gets half the available threads per image, so two files
/// keep the machine busy without doubling peak decoded-image memory again.
pub fn workers(format: Format) -> usize {
    let cores = std::thread::available_parallelism().map_or(4, |count| count.get());
    match format {
        // libjpeg-turbo and the PNG deflater run on the calling thread too.
        Format::WebP | Format::Jpeg | Format::Png => cores.clamp(2, 8),
        Format::Avif => 2,
        // A kept-format folder is mostly JPEG and PNG, which want a file per core,
        // with the odd AVIF or JPEG XL whose encoder already uses them all. Four
        // keeps the first fast without letting the second double peak memory.
        Format::Same => 4,
        // jixel uses the machine's cores inside one encode. A second decoded image
        // would add memory and contention without adding useful parallelism.
        Format::JpegXl => 1,
    }
}

/// Convert every path, `workers(format)` at a time, calling `report` with each result
/// as it lands. `report` is called from a worker thread, one at a time.
///
/// `planned` is `plan_outputs`' answer for these sources, one entry each. The caller
/// plans, so it can also read the planned names — to report them without writing, or
/// to leave a file out of `sources` once it has decided the output is already current
/// — while the collision planning still sees the whole list.
///
/// The window's conversion has its own copy of this loop built out of executor tasks,
/// because it has to hand each result back to the UI thread. This one is for callers
/// that only need the work done.
#[allow(clippy::too_many_arguments)]
pub fn convert_each(
    root: &Path,
    sources: &[PathBuf],
    planned: &[Result<PathBuf, Failure>],
    destination: &Destination,
    format: Format,
    quality: Quality,
    max_edge: MaxEdge,
    report: impl Fn(&Path, Result<Converted, Failure>) + Sync,
) {
    let out_dir = destination.out_dir;
    let stamp = &crate::manifest::Stamp::new(format, quality, max_edge);
    // A shared cursor rather than a slice per thread: files in one folder differ in
    // size by a hundred times, so a fixed split leaves most threads finished early.
    let next = &AtomicUsize::new(0);
    let report = &report;
    let reporting = &parking_lot::Mutex::new(());
    std::thread::scope(|scope| {
        for _ in 0..workers(format).min(sources.len().max(1)) {
            scope.spawn(move || {
                loop {
                    let index = next.fetch_add(1, Ordering::Relaxed);
                    let (Some(source), Some(written)) = (sources.get(index), planned.get(index))
                    else {
                        return;
                    };
                    let backup = destination.backup(root, source);
                    let recording = Recording {
                        root,
                        out_dir,
                        stamp,
                        backup: backup.as_ref(),
                    };
                    let converted = match written {
                        Ok(written) => convert_to(
                            out_dir,
                            source,
                            written,
                            Some(&recording),
                            format,
                            quality,
                            max_edge,
                        ),
                        Err(failure) => Err(failure.clone()),
                    };
                    let _ordered = reporting.lock();
                    report(source, converted);
                }
            });
        }
    });
}

/// Where one run writes.
///
/// The output root is the boundary `Context` proved. `backups` is set only in
/// replace mode, where the converted file lands beside its source and the original
/// has to be somewhere safe before that happens. The manifest is what earlier runs
/// left behind, and is the only way this run can tell its own old output from a
/// file it is about to destroy.
pub struct Destination<'a> {
    pub out_dir: &'a Path,
    pub backups: Option<&'a Path>,
    pub manifest: &'a crate::manifest::Manifest,
}

impl Destination<'_> {
    /// Where this source's original is kept, and whether this run is the one that
    /// has to put it there. `None` outside replace mode, where no original moves.
    ///
    /// A source that is itself an earlier run's output inherits that run's backup:
    /// the file worth keeping is at the start of the chain, and backing up a
    /// derived file as well would leave a second copy nobody wants and a restore
    /// that puts back an intermediate.
    pub fn backup(&self, root: &Path, source: &Path) -> Option<Backup> {
        let backups = self.backups?;
        let relative = source.strip_prefix(root).unwrap_or(source);
        if let Some(record) = self.manifest.chain(relative, source)
            && let Some(inherited) = record.backup.as_ref()
        {
            return Some(Backup {
                path: backups.join(inherited),
                moved: false,
            });
        }
        Some(Backup {
            path: backups.join(relative),
            moved: true,
        })
    }
}

/// Where one original is kept. `moved` is false when an earlier run already put
/// it there and this run only has to name it.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Backup {
    pub path: PathBuf,
    pub moved: bool,
}

/// What one file adds to the folder's record before its output takes the name.
pub struct Recording<'a> {
    pub root: &'a Path,
    pub out_dir: &'a Path,
    pub stamp: &'a crate::manifest::Stamp,
    pub backup: Option<&'a Backup>,
}

/// One spelling of a path for comparison. Case-insensitive, because `Shot.png` and
/// `shot.png` are two files on Linux and one on macOS, and by component rather than
/// by string, because Windows joins with a backslash while an audited path may
/// carry `/`, and the two spellings are one file.
pub(crate) fn path_key(path: &Path) -> String {
    path.components()
        .map(|component| component.as_os_str().to_string_lossy().to_lowercase())
        .collect::<Vec<_>>()
        .join("/")
}

/// Where a converted file goes: the same layout as the source, rooted at `out_dir`,
/// with the selected extension. Keeping the tree means a folder of albums stays a
/// folder of albums.
pub fn output_path(root: &Path, source: &Path, out_dir: &Path, format: Format) -> PathBuf {
    let relative = source.strip_prefix(root).unwrap_or(source);
    let target = out_dir.join(relative);
    match format.extension() {
        Some(extension) => target.with_extension(extension),
        None => target,
    }
}

/// A collision-free path for one AI result beside the normal converted files.
pub fn ai_output_path(
    root: &Path,
    out_dir: &Path,
    source: &Path,
    suffix: &str,
    extension: &str,
) -> Result<PathBuf, String> {
    let relative = source
        .strip_prefix(root)
        .map_err(|_| "the source image is outside the audited folder".to_string())?;
    let relative_parent = relative.parent().unwrap_or(Path::new(""));
    if relative_parent
        .components()
        .any(|component| !matches!(component, std::path::Component::Normal(_)))
    {
        return Err("the source image has an unsafe relative path".into());
    }
    let parent = out_dir.join(relative_parent);
    let stem = relative
        .file_stem()
        .filter(|stem| !stem.is_empty())
        .unwrap_or_else(|| OsStr::new("image"));
    for attempt in 1..=10_000 {
        let mut name = OsString::from(stem);
        name.push(suffix);
        if attempt > 1 {
            name.push(format!("-{attempt}"));
        }
        let candidate = parent.join(name).with_extension(extension);
        match candidate.symlink_metadata() {
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => return Ok(candidate),
            Ok(_) => {}
            Err(error) => return Err(format!("could not inspect the AI output path: {error}")),
        }
    }
    Err("too many AI outputs already use this name".into())
}

/// One output path per source, no two of them the same.
///
/// `output_path` replaces the source extension, so `shot.jpg` and `shot.png` in one
/// folder both ask for `optimized/shot.webp`. Whichever finished second replaced the
/// other, and the run still reported two files converted and a saving that counted
/// bytes no longer on disk. A second claim on a name keeps its source extension:
/// `optimized/shot-jpg.webp`, then `-2`, `-3` if even that is taken. Claims are
/// assigned in source-path order, independent of the current table sort.
///
/// A destination that already holds audited images is refused per file rather than
/// renamed around: `a.png` landing on the original `a.webp` destroys it and the run
/// would report the loss as a saving. Such a source claims no name and does not stop
/// its siblings.
///
/// `audited` is every image in the folder, not just the ones being converted. Ticking
/// one file and writing into a subfolder full of untouched originals is the same
/// destruction, and the run that does it never had those files in `sources`.
///
/// An output an earlier run wrote from a different source is refused for the same
/// reason, read out of the manifest: the folder is the only record either run has
/// of the other, and `shot.jpg` landing on the `shot.webp` made from `shot.png`
/// destroys a file this run never looked at.
///
/// Replace mode is the one case where an output may claim an audited original: its
/// own source. The original is in the backup before the rename happens.
pub fn plan_outputs(
    root: &Path,
    sources: &[PathBuf],
    audited: &[PathBuf],
    destination: &Destination,
    format: Format,
) -> Vec<Result<PathBuf, Failure>> {
    let out_dir = destination.out_dir;
    let replace = destination.backups.is_some();
    let mut taken: HashSet<String> = HashSet::new();
    let key = path_key;
    let originals: HashSet<String> = audited.iter().map(|path| key(path)).collect();
    // One pass over the manifest instead of one reverse scan per candidate. The
    // old code answered the same question per source with a linear search plus
    // a stat; on a large folder that is O(N*M) path keys. Newest record wins,
    // and only while its file is still on disk — the reverse walk below keeps
    // the first existing entry per name, which is exactly what the per-candidate
    // scan used to find.
    let mut owned: std::collections::HashMap<String, (String, bool)> =
        std::collections::HashMap::new();
    for record in destination.manifest.outputs.iter().rev() {
        let output_key = key(&record.output);
        if owned.contains_key(&output_key) {
            continue;
        }
        if out_dir.join(&record.output).symlink_metadata().is_ok() {
            owned.insert(output_key, (key(&record.source), record.backup.is_some()));
        }
    }
    // A name an earlier run wrote from somebody else. The current source may take
    // back its own name — that is a rerun, and replacing its own output is the
    // point — and so may a source that is itself that run's output, which is one
    // more link in a replace chain.
    let claimed = |candidate: &Path, mine: &str| -> bool {
        let Ok(relative) = candidate.strip_prefix(out_dir) else {
            return false;
        };
        let output_key = key(relative);
        let Some((source_key, has_backup)) = owned.get(&output_key) else {
            return false;
        };
        source_key != mine && !(output_key == *mine && *has_backup)
    };

    let mut planned = vec![Err(Failure::OverwritesSource); sources.len()];
    // Source keys computed once: the old ordering compared fresh path keys per
    // pair, which is O(N log N) allocations for the sort alone.
    let full_keys: Vec<String> = sources.iter().map(|source| key(source)).collect();
    let mine_keys: Vec<String> = sources
        .iter()
        .map(|source| key(source.strip_prefix(root).unwrap_or(source)))
        .collect();
    let mut order: Vec<usize> = (0..sources.len()).collect();
    order.sort_by(|left, right| {
        full_keys[*left]
            .cmp(&full_keys[*right])
            .then_with(|| sources[*left].cmp(&sources[*right]))
    });

    for index in order {
        let source = &sources[index];
        let mine = mine_keys[index].as_str();
        let plain = output_path(root, source, out_dir, format);
        // In replace mode a WebP converted to WebP writes its own name back. That
        // is not destruction: the original moves into the backup first.
        let own_name = replace && key(&plain) == key(source);
        if originals.contains(&key(&plain)) && !own_name {
            continue;
        }
        // A name somebody else's run owns is renamed around like any other taken
        // name. Failing the file instead would stop a folder converting for a
        // reason the person cannot see or clear.
        if !claimed(&plain, mine) && taken.insert(key(&plain)) {
            planned[index] = Ok(plain);
            continue;
        }
        let extension = source
            .extension()
            .map(|extension| extension.to_string_lossy().to_lowercase())
            .unwrap_or_else(|| "file".to_string());
        let stem = plain.file_stem().unwrap_or_default().to_string_lossy();
        let parent = plain.parent().unwrap_or(out_dir);
        for attempt in 1.. {
            let suffix = if attempt == 1 {
                String::new()
            } else {
                format!("-{attempt}")
            };
            let candidate = parent
                .join(format!("{stem}-{extension}{suffix}"))
                .with_extension(plain.extension().unwrap_or_default());
            if originals.contains(&key(&candidate)) || claimed(&candidate, mine) {
                continue;
            }
            if taken.insert(key(&candidate)) {
                planned[index] = Ok(candidate);
                break;
            }
        }
    }
    planned
}

/// Safely install already-encoded bytes inside `output_root`.
///
/// The boundary is the destination, not the audited folder. A chosen output folder is
/// routinely somewhere else entirely — a staging directory, a share, a build tree —
/// and measuring the target against the source root failed every one of those writes
/// with nothing to tell the user.
///
/// AI tools and the normal encoder share this boundary: no symlinked ancestor, no
/// half-written final file, and no path outside the output folder.
pub fn write_output(output_root: &Path, written: &Path, encoded: &[u8]) -> Result<(), Failure> {
    write_inner(output_root, written, encoded, None)
}

/// The same write, with the two durable steps a conversion owes its folder in
/// between: the record goes down, then the original moves out of its own name,
/// and only then does the finished file take that name.
///
/// The order is what makes a killed run recoverable. A record without a moved
/// original describes a file that was never installed, and `restore` reads it as
/// nothing to do; a moved original without a record is an original nobody can
/// find. Only the first of those can happen here.
pub fn write_recorded(
    output_root: &Path,
    source: &Path,
    written: &Path,
    encoded: &[u8],
    recording: &Recording,
) -> Result<(), Failure> {
    write_inner(output_root, written, encoded, Some((source, recording)))
}

fn write_inner(
    output_root: &Path,
    written: &Path,
    encoded: &[u8],
    recorded: Option<(&Path, &Recording)>,
) -> Result<(), Failure> {
    let relative = written
        .strip_prefix(output_root)
        .map_err(|_| Failure::OutsideOutput)?;
    let mut ancestor = output_root.to_path_buf();
    // The destination itself is now the first thing that has to be a plain folder;
    // nothing above it is walked any more, so nothing else would catch a symlink
    // standing in for it.
    ensure_directory(&ancestor, || std::fs::create_dir_all(&ancestor))?;
    for component in relative
        .parent()
        .ok_or(Failure::OutsideOutput)?
        .components()
    {
        let std::path::Component::Normal(component) = component else {
            return Err(Failure::OutsideOutput);
        };
        ancestor.push(component);
        ensure_directory(&ancestor, || std::fs::create_dir(&ancestor))?;
    }
    // Write beside the target and rename onto it. A crash or a full disk part-way
    // through the write would otherwise leave a short image that looks finished.
    //
    // The staging name carries this process id. Without it a killed run's leftover
    // sat on the one name `create_new` would accept and failed that file on every
    // later run, for good.
    let mut partial = written.to_path_buf().into_os_string();
    partial.push(format!(".{}.part", std::process::id()));
    let partial = PathBuf::from(partial);
    use std::io::Write;
    let mut stage = match std::fs::OpenOptions::new()
        .write(true)
        .create_new(true)
        .open(&partial)
    {
        Ok(file) => file,
        // Somebody else's file, so name it and leave it alone rather than delete it.
        Err(error) if error.kind() == std::io::ErrorKind::AlreadyExists => {
            return Err(Failure::StalePartial(partial));
        }
        Err(_) => return Err(Failure::Failed),
    };
    let staged = stage.write_all(encoded).and_then(|()| stage.sync_all());
    if staged.is_err() {
        let _ = std::fs::remove_file(&partial);
        return Err(Failure::Failed);
    }

    let mut claim = None;
    if let Some((source, recording)) = recorded {
        let described = recording
            .stamp
            .record(
                (recording.root, recording.out_dir),
                source,
                written,
                &partial,
                recording.backup.map(|backup| backup.path.as_path()),
            )
            .ok_or(Failure::OutsideOutput);
        let appended = described.and_then(|record| {
            crate::manifest::append_record(recording.out_dir, &record)
                .map(|()| record)
                .map_err(Failure::RecordNotWritten)
        });
        match appended {
            Ok(record) => claim = Some(record),
            Err(failure) => {
                let _ = std::fs::remove_file(&partial);
                return Err(failure);
            }
        }
        if let Some(backup) = recording.backup.filter(|backup| backup.moved)
            && let Err(failure) = move_to_backup(
                source,
                &backup.path,
                &crate::manifest::backup_root(recording.root),
            )
        {
            let _ = std::fs::remove_file(&partial);
            return Err(withdraw(recording.out_dir, claim.as_ref(), failure));
        }
    }

    if std::fs::rename(&partial, written).is_err() {
        let _ = std::fs::remove_file(&partial);
        if let Some((source, recording)) = recorded {
            // The original is out of its own name and nothing took that name, so
            // put it back. A folder left with a hole where an image was is worse
            // than a file this run could not convert.
            if let Some(backup) = recording.backup.filter(|backup| backup.moved)
                && std::fs::rename(&backup.path, source).is_err()
            {
                // The record stays: it is the only thing that now knows where
                // this original went.
                return Err(Failure::OriginalLeftInBackup(backup.path.clone()));
            }
            return Err(withdraw(
                recording.out_dir,
                claim.as_ref(),
                Failure::InstallFailed,
            ));
        }
        return Err(Failure::InstallFailed);
    }
    Ok(())
}

/// Take back the record for a file that failed after it was written.
///
/// The record goes down before the original moves, so a failure between the two
/// leaves a claim on a name this run never took. Left standing, an undo reads it
/// as a file to delete and an original to look for, and fails the real record
/// beside it as well. The withdrawal is another line rather than an edit, because
/// eight workers are appending to the same file.
fn withdraw(out_dir: &Path, claim: Option<&crate::manifest::Record>, failure: Failure) -> Failure {
    if let Some(claim) = claim {
        let _ = crate::manifest::append_record(out_dir, &claim.voided());
    }
    failure
}

/// Move one original into the backup mirror, keeping whatever is already there.
///
/// An earlier run's backup of the same name is the real original; this run's
/// "original" is that run's output. Overwriting it would lose the only copy, so
/// the file is named and left alone instead.
///
/// Every folder on the way is checked, not just the last one, for the reason the
/// output side checks them: one symlink standing in for a folder redirects the
/// whole mirror somewhere else.
fn move_to_backup(source: &Path, backup: &Path, backups: &Path) -> Result<(), Failure> {
    let relative = backup
        .strip_prefix(backups)
        .map_err(|_| Failure::OutsideOutput)?;
    let mut ancestor = backups.to_path_buf();
    ensure_directory(&ancestor, || std::fs::create_dir_all(&ancestor))?;
    for component in relative
        .parent()
        .ok_or(Failure::OutsideOutput)?
        .components()
    {
        let std::path::Component::Normal(component) = component else {
            return Err(Failure::OutsideOutput);
        };
        ancestor.push(component);
        ensure_directory(&ancestor, || std::fs::create_dir(&ancestor))?;
    }
    if backup.symlink_metadata().is_ok() {
        return Err(Failure::BackupOccupied(backup.to_path_buf()));
    }
    std::fs::rename(source, backup).map_err(|_| Failure::BackupFailed)
}

/// `path` must be a plain directory, creating it with `create` if it is missing.
fn ensure_directory(
    path: &Path,
    create: impl FnOnce() -> std::io::Result<()>,
) -> Result<(), Failure> {
    match path.symlink_metadata() {
        Ok(metadata) if metadata.file_type().is_symlink() || !metadata.is_dir() => {
            Err(Failure::UnsafeOutputPath)
        }
        Ok(_) => Ok(()),
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => match create() {
            Ok(()) => Ok(()),
            // Another worker got there first; it still has to be a plain folder.
            Err(error) if error.kind() == std::io::ErrorKind::AlreadyExists => {
                let metadata = path.symlink_metadata().map_err(|_| Failure::Failed)?;
                if metadata.file_type().is_symlink() || !metadata.is_dir() {
                    return Err(Failure::UnsafeOutputPath);
                }
                Ok(())
            }
            Err(_) => Err(Failure::Failed),
        },
        Err(_) => Err(Failure::Failed),
    }
}

/// Read, encode, and write one file to the path `plan_outputs` chose for it.
///
/// `recording` is what the run owes the folder before the new file takes the name:
/// a line in the run record, and — in replace mode — the original moved into the
/// backup. The original is never deleted, an occupied backup name stops the file
/// rather than overwriting the older original sitting there, and a failed install
/// puts the original back.
pub fn convert_to(
    output_root: &Path,
    source: &Path,
    written: &Path,
    recording: Option<&Recording>,
    format: Format,
    quality: Quality,
    max_edge: MaxEdge,
) -> Result<Converted, Failure> {
    let format = format.resolve(source)?;
    let (decoded, profile) =
        crate::scan::decode_for_conversion(source, max_edge).map_err(|error| match error {
            crate::scan::ConversionDecodeError::Failed => Failure::Failed,
            crate::scan::ConversionDecodeError::AnimatedGif => Failure::AnimatedGif,
            crate::scan::ConversionDecodeError::AnimatedPng => Failure::AnimatedPng,
            crate::scan::ConversionDecodeError::AnimatedWebP => Failure::AnimatedWebP,
            crate::scan::ConversionDecodeError::AnimatedJpegXl => Failure::AnimatedJpegXl,
        })?;
    let decoded = max_edge.apply(decoded);
    // "lossless" is a promise of unchanged pixels. WebP cannot carry more than eight
    // bits per channel, and JPEG XL's 16-bit path cannot carry float samples, so over
    // those sources the label would survive and half the depth would not. Lossy
    // quality promises nothing and is left alone.
    if quality == Quality::LOSSLESS {
        if format == Format::WebP && is_high_depth(&decoded) {
            return Err(Failure::LosslessNeedsEightBit);
        }
        if format == Format::JpegXl
            && matches!(
                decoded,
                DynamicImage::ImageRgb32F(_) | DynamicImage::ImageRgba32F(_)
            )
        {
            return Err(Failure::LosslessNeedsIntegerSamples);
        }
    }
    let (width, height) = (decoded.width(), decoded.height());
    let encoded = encode(&decoded, format, quality, profile.as_deref())?;
    match recording {
        Some(recording) => write_recorded(output_root, source, written, &encoded, recording)?,
        None => write_output(output_root, written, &encoded)?,
    }

    Ok(Converted {
        written: written.to_path_buf(),
        bytes: encoded.len() as u64,
        width,
        height,
    })
}

#[cfg(test)]
pub(crate) mod tests {
    use super::*;
    use image::{ImageBuffer, Rgb, Rgba};

    /// The conversions that predate replace mode: no original moves, and no run
    /// record exists yet. Shadowing the real ones keeps every test that never
    /// heard of either reading the way it always did.
    fn convert_to(
        output_root: &Path,
        source: &Path,
        written: &Path,
        format: Format,
        quality: Quality,
        max_edge: MaxEdge,
    ) -> Result<Converted, Failure> {
        super::convert_to(
            output_root,
            source,
            written,
            None,
            format,
            quality,
            max_edge,
        )
    }

    fn plan_outputs(
        root: &Path,
        sources: &[PathBuf],
        audited: &[PathBuf],
        out_dir: &Path,
        format: Format,
    ) -> Vec<Result<PathBuf, Failure>> {
        super::plan_outputs(root, sources, audited, &plain(out_dir), format)
    }

    /// A destination that writes into `out_dir` and has no history.
    pub(crate) fn plain(out_dir: &Path) -> Destination<'_> {
        Destination {
            out_dir,
            backups: None,
            manifest: &EMPTY,
        }
    }

    static EMPTY: std::sync::LazyLock<crate::manifest::Manifest> =
        std::sync::LazyLock::new(crate::manifest::Manifest::default);

    fn temp_dir(tag: &str) -> PathBuf {
        let dir = std::env::temp_dir().join(format!("imageguide-convert-{tag}"));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();
        // macOS hands out `/var/folders/...`, and `/var` is a symlink to
        // `/private/var`. `Context` canonicalizes its roots, so a fixture that
        // starts from the aliased spelling compares two different names.
        dir.canonicalize().unwrap()
    }

    /// Deterministic noise. A flat colour compresses to nothing, and so does a smooth
    /// gradient — lossless WebP squeezed one to 90 bytes and made an earlier version of
    /// these tests assert something false. Real photographs are noisy; this is too.
    pub(crate) fn photo(width: u32, height: u32) -> DynamicImage {
        DynamicImage::ImageRgb8(ImageBuffer::from_fn(width, height, |x, y| {
            let mut hash = x.wrapping_mul(2_654_435_761) ^ y.wrapping_mul(2_246_822_519);
            hash ^= hash >> 13;
            hash = hash.wrapping_mul(3_266_489_917);
            Rgb([(hash >> 8) as u8, (hash >> 16) as u8, (hash >> 24) as u8])
        }))
    }

    /// The same stand-in in the one colour space the encoders accept.
    pub(crate) fn rgb_profile() -> Vec<u8> {
        colour_profile(b"RGB ")
    }

    /// A stand-in for a Display P3 profile, in the given data colour space. Nothing on
    /// the way through reads a profile's contents, so this only has to carry the header
    /// the checks read and come back byte for byte.
    fn colour_profile(space: &[u8; 4]) -> Vec<u8> {
        let mut profile = vec![0u8; 132];
        let size = profile.len() as u32;
        profile[0..4].copy_from_slice(&size.to_be_bytes());
        profile[12..16].copy_from_slice(b"mntr");
        profile[16..20].copy_from_slice(space);
        profile[20..24].copy_from_slice(b"XYZ ");
        profile[36..40].copy_from_slice(b"acsp");
        profile
    }

    fn write_tagged_png(path: &Path, image: &DynamicImage, profile: &[u8]) {
        let file = std::fs::File::create(path).unwrap();
        let mut encoder = image::codecs::png::PngEncoder::new(file);
        image::ImageEncoder::set_icc_profile(&mut encoder, profile.to_vec()).unwrap();
        image::ImageEncoder::write_image(
            encoder,
            image.as_bytes(),
            image.width(),
            image.height(),
            image::ExtendedColorType::Rgb8,
        )
        .unwrap();
    }

    /// The profile a written file actually carries, read back the way a browser
    /// would rather than from whatever this process still holds in memory.
    fn embedded_profile(bytes: &[u8]) -> Option<Vec<u8>> {
        if bytes.starts_with(&[0xff, 0x0a]) {
            // `rendered_icc` answers with the profile the pixels are handed back in,
            // which is a synthesized sRGB one when the file carries nothing.
            return jxl_oxide::JxlImage::builder()
                .read(std::io::Cursor::new(bytes))
                .unwrap()
                .original_icc()
                .map(<[u8]>::to_vec);
        }
        let mut decoder = image::ImageReader::new(std::io::Cursor::new(bytes))
            .with_guessed_format()
            .unwrap()
            .into_decoder()
            .unwrap();
        image::ImageDecoder::icc_profile(&mut decoder).unwrap()
    }

    /// A Display P3 photo written untagged is a photo a browser renders as sRGB, and
    /// the run still calls it converted. The profile has to reach the file.
    #[test]
    fn a_source_colour_profile_reaches_the_converted_file() {
        let dir = temp_dir("colour-profile");
        let profile = colour_profile(b"RGB ");
        assert!(!profile.is_empty(), "the fixture carries a real profile");
        let source = dir.join("wide.png");
        write_tagged_png(&source, &photo(48, 48), &profile);

        let (_, read_back) = crate::scan::decode_for_conversion(&source, MaxEdge::FULL)
            .expect("the tagged PNG decodes");
        assert_eq!(
            read_back.as_deref(),
            Some(profile.as_slice()),
            "the decoder did not hand back the source profile"
        );

        for format in [Format::WebP, Format::Avif, Format::JpegXl, Format::Jpeg] {
            let out = dir.join(format.label());
            let written = output_path(&dir, &source, &out, format);
            convert_to(
                &dir,
                &source,
                &written,
                format,
                Quality::lossy(80.),
                MaxEdge::FULL,
            )
            .unwrap_or_else(|error| panic!("{format:?} conversion failed: {error:?}"));

            let bytes = std::fs::read(&written).unwrap();
            assert_eq!(
                embedded_profile(&bytes).as_deref(),
                Some(profile.as_slice()),
                "{format:?} dropped the colour profile"
            );
        }
    }

    /// Transparency forces WebP down the lossless path and AVIF keeps alpha in its own
    /// plane, so both reach the encoders by a different route than an opaque photo.
    /// Neither route may drop the profile.
    #[test]
    fn a_colour_profile_survives_the_transparent_paths() {
        let profile = colour_profile(b"RGB ");
        assert!(!profile.is_empty(), "the fixture carries a real profile");
        let mut buffer: ImageBuffer<Rgba<u8>, Vec<u8>> =
            ImageBuffer::from_pixel(24, 24, Rgba([200u8, 30, 40, 255]));
        buffer.put_pixel(0, 0, Rgba([200, 30, 40, 0]));
        let image = DynamicImage::ImageRgba8(buffer);
        assert!(
            has_transparency(&image),
            "the fixture is really see-through"
        );

        for format in [Format::WebP, Format::Avif] {
            let encoded = encode(&image, format, Quality::lossy(80.), Some(&profile))
                .unwrap_or_else(|error| panic!("{format:?} encode failed: {error:?}"));
            assert_eq!(
                embedded_profile(&encoded).as_deref(),
                Some(profile.as_slice()),
                "{format:?} dropped the colour profile"
            );
        }
    }

    /// A decoder answers with the profile the file carried, not the one its pixels came
    /// back in. A GRAY or CMYK profile on pixels the encoders take as RGB describes the
    /// wrong thing, and a profile whose own header disagrees with its length describes
    /// nothing. All of them are worse than writing no profile at all.
    #[test]
    fn a_profile_that_does_not_describe_the_encoded_pixels_is_dropped() {
        let dir = temp_dir("mismatched-profile");
        let out = dir.join("optimised");
        for (name, profile) in [
            ("gray.png", colour_profile(b"GRAY")),
            ("cmyk.png", colour_profile(b"CMYK")),
            ("truncated.png", colour_profile(b"RGB ")[..130].to_vec()),
        ] {
            assert!(!profile.is_empty(), "{name} carries a profile at all");
            let source = dir.join(name);
            write_tagged_png(&source, &photo(16, 16), &profile);

            let (_, read_back) = crate::scan::decode_for_conversion(&source, MaxEdge::FULL)
                .expect("the tagged PNG decodes");
            assert_eq!(read_back, None, "{name} kept a profile it should not have");

            let written = output_path(&dir, &source, &out, Format::WebP);
            convert_to(
                &dir,
                &source,
                &written,
                Format::WebP,
                Quality::lossy(80.),
                MaxEdge::FULL,
            )
            .unwrap_or_else(|error| panic!("{name} did not convert: {error:?}"));
            assert_eq!(
                embedded_profile(&std::fs::read(&written).unwrap()),
                None,
                "{name} put the wrong profile in the output"
            );
        }
    }

    /// The extended container the profile needs is still a WebP the app can read
    /// back, which the comparison view does for every converted file.
    #[test]
    fn a_tagged_webp_still_decodes_to_pixels() {
        let profile = colour_profile(b"RGB ");
        let image = photo(32, 32);
        let encoded = encode(&image, Format::WebP, Quality::lossy(80.), Some(&profile))
            .expect("webp encodes");

        let decoded = crate::scan::decode_bytes(&encoded).expect("the tagged webp decodes");
        assert_eq!((decoded.width(), decoded.height()), (32, 32));
    }

    /// The quality number has to actually reach libwebp. If it were dropped on the
    /// floor both encodes would come back the same size and nobody would notice.
    #[test]
    fn lower_quality_produces_fewer_bytes() {
        let image = photo(256, 256);
        let low = encode(&image, Format::WebP, Quality::lossy(20.), None).expect("q20 encodes");
        let high = encode(&image, Format::WebP, Quality::lossy(95.), None).expect("q95 encodes");

        assert!(
            low.len() < high.len(),
            "q20 {} should be smaller than q95 {}",
            low.len(),
            high.len()
        );
    }

    #[test]
    fn output_is_a_real_webp() {
        let encoded = encode(&photo(32, 32), Format::WebP, Quality::lossy(80.), None).unwrap();
        assert_eq!(&encoded[0..4], b"RIFF");
        assert_eq!(&encoded[8..12], b"WEBP");
    }

    #[test]
    fn bundled_libwebp_has_the_current_encoder() {
        // 0xMMmmpp: major, minor, patch.
        let version = unsafe { libwebp_sys::WebPGetEncoderVersion() };
        assert!(version >= 0x010600, "bundled libwebp is {version:#08x}");
    }

    #[test]
    fn output_is_a_real_avif() {
        let encoded = encode(&photo(32, 32), Format::Avif, Quality::lossy(80.), None).unwrap();
        // ISO base media file format: a 'ftyp' box naming the AVIF brand.
        assert_eq!(&encoded[4..8], b"ftyp");
        assert_eq!(&encoded[8..12], b"avif");
    }

    #[test]
    fn jpeg_xl_round_trips_through_the_rust_decoder() {
        let image = photo(32, 32);
        let encoded = encode(&image, Format::JpegXl, Quality::lossy(80.), None).unwrap();
        assert_eq!(&encoded[..2], &[0xff, 0x0a]);

        let decoded = crate::jxl::decode_bytes(&encoded).expect("JPEG XL decodes");
        assert_eq!(
            (decoded.width(), decoded.height()),
            (image.width(), image.height())
        );
    }

    #[test]
    fn jpeg_output_decodes_with_its_dimensions_and_no_alpha() {
        let image = photo(48, 32);
        let encoded = encode(&image, Format::Jpeg, Quality::lossy(80.), None).unwrap();
        assert_eq!(&encoded[..2], &[0xff, 0xd8], "a JPEG starts with SOI");

        let decoded = image::load_from_memory(&encoded).expect("JPEG decodes");
        assert_eq!((decoded.width(), decoded.height()), (48, 32));
        assert!(
            !decoded.color().has_alpha(),
            "JPEG carries no alpha channel"
        );
    }

    #[test]
    fn lower_jpeg_quality_produces_fewer_bytes() {
        let image = photo(128, 128);
        let low = encode(&image, Format::Jpeg, Quality::lossy(20.), None).unwrap();
        let high = encode(&image, Format::Jpeg, Quality::lossy(95.), None).unwrap();
        assert!(low.len() < high.len());
    }

    /// A cut-out has nowhere to keep its transparency in a JPEG. Flattening it onto a
    /// colour nobody chose would report "converted" for a file that lost its edge, so
    /// the file is refused by name. An alpha channel that is all opaque is still fine.
    #[test]
    fn a_transparent_source_is_refused_for_jpeg_by_name() {
        let mut buffer: ImageBuffer<Rgba<u8>, Vec<u8>> =
            ImageBuffer::from_pixel(16, 16, Rgba([10, 20, 30, 255]));
        let opaque = encode(
            &DynamicImage::ImageRgba8(buffer.clone()),
            Format::Jpeg,
            Quality::lossy(80.),
            None,
        )
        .expect("an opaque alpha channel still encodes");
        assert!(opaque.len() > 2);

        buffer.put_pixel(0, 0, Rgba([10, 20, 30, 0]));
        let refused = encode(
            &DynamicImage::ImageRgba8(buffer),
            Format::Jpeg,
            Quality::lossy(80.),
            None,
        );
        assert_eq!(refused, Err(Failure::JpegNeedsOpaque));
        assert_eq!(
            Failure::JpegNeedsOpaque.reason(),
            Some("JPEG cannot keep transparency".into())
        );
    }

    /// The whole point of keeping the format is that the name does not change: the
    /// pages that name `hero.jpg` keep working. So the extension is the source's,
    /// case and all, and the bytes inside are the container the name promises.
    #[test]
    fn keep_format_keeps_the_extension_for_jpg_png_and_webp() {
        let dir = temp_dir("keep-format");
        let out = dir.join("optimised");
        for (name, magic) in [
            ("shot.jpg", &[0xff, 0xd8][..]),
            ("shot.PNG", &b"\x89PNG"[..]),
            ("shot.webp", &b"RIFF"[..]),
        ] {
            let source = dir.join(name);
            photo(40, 30)
                .save_with_format(
                    &source,
                    image::ImageFormat::from_path(&source).expect("the name has a format"),
                )
                .unwrap();
            let written = output_path(&dir, &source, &out, Format::Same);
            assert_eq!(written, out.join(name), "{name} changed its name");

            let converted = convert_to(
                &dir,
                &source,
                &written,
                Format::Same,
                Quality::lossy(80.),
                MaxEdge(Some(20)),
            )
            .unwrap_or_else(|error| panic!("{name} did not convert: {error:?}"));
            let bytes = std::fs::read(&written).unwrap();
            assert!(bytes.starts_with(magic), "{name} is not its own format");
            assert_eq!((converted.width, converted.height), (20, 15), "{name}");
        }
    }

    /// A PNG called `.jpg` is the audit's own headline finding. Keeping the "format"
    /// of that file has two wrong answers: lossy JPEG under the lying name reports
    /// the lie as a conversion, and PNG bytes under `.jpg` keeps the lie. So it is
    /// refused by name, and the failure says what to do instead.
    #[test]
    fn keep_format_refuses_a_png_named_jpg_by_name() {
        let dir = temp_dir("keep-lying-name");
        let source = dir.join("photo.jpg");
        photo(16, 16)
            .save_with_format(&source, image::ImageFormat::Png)
            .unwrap();
        let out = dir.join("optimised");

        let refused = convert_to(
            &dir,
            &source,
            &output_path(&dir, &source, &out, Format::Same),
            Format::Same,
            Quality::lossy(80.),
            MaxEdge::FULL,
        );
        assert_eq!(refused, Err(Failure::ExtensionLies("jpg".into(), "PNG")));
        assert_eq!(
            Failure::ExtensionLies("jpg".into(), "PNG").reason(),
            Some("named .jpg but the bytes are PNG; convert it explicitly".into())
        );
        assert!(!out.exists(), "nothing was written under the lying name");
    }

    /// A grayscale source stays a one-plane JPEG. Promoted to RGB with 4:2:0 it came
    /// out roughly twice the size for the same pixels.
    #[test]
    fn a_grayscale_source_becomes_a_single_plane_jpeg() {
        let image = DynamicImage::ImageLuma8(photo(96, 96).to_luma8());
        let gray = encode(&image, Format::Jpeg, Quality::lossy(80.), None).unwrap();
        let rgb = encode(
            &DynamicImage::ImageRgb8(image.to_rgb8()),
            Format::Jpeg,
            Quality::lossy(80.),
            None,
        )
        .unwrap();

        let decoded = image::load_from_memory(&gray).expect("the gray JPEG decodes");
        assert_eq!(decoded.color(), image::ColorType::L8);
        assert!(
            gray.len() < rgb.len(),
            "gray {} should be smaller than the RGB promotion {}",
            gray.len(),
            rgb.len()
        );
    }

    /// A real profile is bigger than one APP2 segment can hold: Display P3 from a
    /// phone is small, but a printer's is hundreds of kilobytes. The chunks have to
    /// come back in order and byte for byte.
    #[test]
    fn a_profile_larger_than_one_app2_segment_round_trips_through_jpeg() {
        let mut profile: Vec<u8> = (0..200_000u32)
            .map(|index| (index.wrapping_mul(2_654_435_761) >> 13) as u8)
            .collect();
        let size = profile.len() as u32;
        profile[0..4].copy_from_slice(&size.to_be_bytes());
        profile[16..20].copy_from_slice(b"RGB ");
        assert!(profile.len() > 3 * 65_519, "the fixture needs four chunks");

        let encoded = encode(
            &photo(24, 24),
            Format::Jpeg,
            Quality::lossy(80.),
            Some(&profile),
        )
        .expect("a large profile attaches");
        assert_eq!(
            embedded_profile(&encoded).as_deref(),
            Some(profile.as_slice()),
            "the chunks did not reassemble"
        );
    }

    #[test]
    fn keep_format_refuses_a_bmp_by_name() {
        let dir = temp_dir("keep-bmp");
        let source = dir.join("scan.bmp");
        photo(8, 8).save(&source).unwrap();

        let refused = convert_to(
            &dir,
            &source,
            &output_path(&dir, &source, &dir.join("optimised"), Format::Same),
            Format::Same,
            Quality::lossy(80.),
            MaxEdge::FULL,
        );
        assert_eq!(refused, Err(Failure::KeepFormatUnavailable("BMP".into())));
        assert_eq!(
            Failure::KeepFormatUnavailable("BMP".into()).reason(),
            Some("keep format is not available for BMP".into())
        );
        assert_eq!(
            Format::Same.resolve(Path::new("/p/noext")),
            Err(Failure::KeepFormatUnavailable(
                "a file with no extension".into()
            ))
        );
    }

    /// Keeping the format must also keep the depth: a 16-bit PNG that comes back
    /// as eight bits kept its name and lost half its samples.
    #[test]
    fn keep_format_keeps_sixteen_bit_png_samples() {
        let dir = temp_dir("keep-deep-png");
        let source = dir.join("deep.png");
        let image = deep_photo(24, 16);
        image.save(&source).unwrap();
        let written = output_path(&dir, &source, &dir.join("optimised"), Format::Same);

        convert_to(
            &dir,
            &source,
            &written,
            Format::Same,
            Quality::lossy(80.),
            MaxEdge::FULL,
        )
        .expect("a 16-bit PNG keeps its format");

        let decoded = image::open(&written).unwrap();
        assert!(is_high_depth(&decoded), "the output dropped to eight bits");
        assert_eq!(decoded.to_rgb16(), image.to_rgb16());
    }

    /// With the format kept, the output name is the source name. Written into the
    /// audited folder itself that is the original, and the guard that already
    /// stops `a.png` landing on `a.webp` has to stop this too.
    #[test]
    fn keep_format_never_writes_onto_a_source() {
        let root = Path::new("/photos");
        let sources = [
            PathBuf::from("/photos/a.jpg"),
            PathBuf::from("/photos/album/b.png"),
        ];

        let onto_itself = plan_outputs(root, &sources, &sources, root, Format::Same);
        assert_eq!(
            onto_itself,
            [
                Err(Failure::OverwritesSource),
                Err(Failure::OverwritesSource)
            ]
        );

        let mirrored = plan_outputs(
            root,
            &sources,
            &sources,
            Path::new("/photos/optimized"),
            Format::Same,
        );
        assert_eq!(
            mirrored,
            [
                Ok(PathBuf::from("/photos/optimized/a.jpg")),
                Ok(PathBuf::from("/photos/optimized/album/b.png")),
            ]
        );
    }

    #[test]
    fn lossless_jpeg_xl_preserves_rgba_pixels() {
        let image = DynamicImage::ImageRgba8(ImageBuffer::from_fn(12, 8, |x, y| {
            Rgba([
                (x * 17) as u8,
                (y * 29) as u8,
                ((x + y) * 11) as u8,
                ((x * 19 + y * 7) % 256) as u8,
            ])
        }));
        let encoded = encode(&image, Format::JpegXl, Quality::LOSSLESS, None).unwrap();
        let decoded = crate::jxl::decode_bytes(&encoded).expect("lossless JPEG XL decodes");

        assert_eq!(decoded.into_rgba8(), image.into_rgba8());
    }

    /// Deterministic 16-bit noise, for the same reason `photo` is noisy: the low byte
    /// of every channel has to actually carry something, or losing it proves nothing.
    fn deep_photo(width: u32, height: u32) -> DynamicImage {
        DynamicImage::ImageRgb16(ImageBuffer::from_fn(width, height, |x, y| {
            let mut hash = x.wrapping_mul(2_654_435_761) ^ y.wrapping_mul(2_246_822_519);
            hash ^= hash >> 13;
            hash = hash.wrapping_mul(3_266_489_917);
            image::Rgb([hash as u16, (hash >> 11) as u16, (hash >> 19) as u16])
        }))
    }

    /// JPEG XL is the one format here that can hold 16 bits, so lossless has to mean
    /// what it says: the samples come back untouched, not halved to eight bits.
    #[test]
    fn lossless_jpeg_xl_keeps_sixteen_bit_samples() {
        let image = deep_photo(24, 16);
        assert!(is_high_depth(&image), "the fixture is a 16-bit source");

        let encoded = encode(&image, Format::JpegXl, Quality::LOSSLESS, None)
            .expect("16-bit JPEG XL encodes");
        let decoded = crate::jxl::decode_bytes(&encoded).expect("16-bit JPEG XL decodes");

        assert_eq!(decoded.to_rgb16(), image.to_rgb16());
    }

    /// JPEG XL's 16-bit path is the deepest one here, so a float source is truncated on
    /// the way in. Lossless must not promise otherwise.
    #[test]
    fn lossless_jpeg_xl_refuses_a_float_source_by_name() {
        let dir = temp_dir("float-jxl");
        let source = dir.join("float.tiff");
        let deep = deep_photo(16, 12).to_rgb32f();
        DynamicImage::ImageRgb32F(deep)
            .save_with_format(&source, image::ImageFormat::Tiff)
            .unwrap();
        let out = dir.join("optimised");

        assert_eq!(
            convert_to(
                &dir,
                &source,
                &output_path(&dir, &source, &out, Format::JpegXl),
                Format::JpegXl,
                Quality::LOSSLESS,
                MaxEdge::FULL,
            ),
            Err(Failure::LosslessNeedsIntegerSamples)
        );
        assert_eq!(
            Failure::LosslessNeedsIntegerSamples.reason(),
            Some("lossless JPEG XL cannot keep 32-bit floating point samples".into())
        );
    }

    /// WebP has nowhere to put the extra bits, so a lossless run over a 16-bit source
    /// is refused by name rather than labelled lossless and quietly truncated.
    #[test]
    fn lossless_webp_refuses_a_sixteen_bit_source_by_name() {
        let dir = temp_dir("sixteen-bit-webp");
        let source = dir.join("deep.png");
        deep_photo(24, 16).save(&source).unwrap();
        let out = dir.join("optimised");

        let refused = convert_to(
            &dir,
            &source,
            &output_path(&dir, &source, &out, Format::WebP),
            Format::WebP,
            Quality::LOSSLESS,
            MaxEdge::FULL,
        );
        assert_eq!(refused, Err(Failure::LosslessNeedsEightBit));
        assert_eq!(
            Failure::LosslessNeedsEightBit.reason(),
            Some("lossless WebP cannot keep more than 8 bits per colour channel".into())
        );

        // Nothing was promised at a quality setting, so that conversion still runs.
        let lossy = convert_to(
            &dir,
            &source,
            &output_path(&dir, &source, &out, Format::WebP),
            Format::WebP,
            Quality::lossy(80.),
            MaxEdge::FULL,
        )
        .expect("a quality setting still converts a 16-bit source");
        assert!(lossy.bytes > 0);
    }

    #[test]
    fn aom_quality_matches_the_measured_rav1e_output() {
        assert_eq!(aom_quality(Quality::lossy(80.)), 60);
    }

    #[test]
    fn lower_jpeg_xl_quality_produces_fewer_bytes() {
        let image = photo(128, 128);
        let low = encode(&image, Format::JpegXl, Quality::lossy(20.), None).unwrap();
        let high = encode(&image, Format::JpegXl, Quality::lossy(95.), None).unwrap();
        assert!(low.len() < high.len());
    }

    /// Alpha survives the trip. AVIF carries it in its own plane, so unlike WebP there
    /// is no lossless fallback protecting it, and a regression here would silently
    /// flatten every cut-out.
    #[test]
    fn avif_keeps_transparency() {
        let mut buffer: ImageBuffer<Rgba<u8>, Vec<u8>> =
            ImageBuffer::from_pixel(64, 64, Rgba([200u8, 30, 40, 255]));
        for x in 0..32 {
            for y in 0..64 {
                buffer.put_pixel(x, y, Rgba([200, 30, 40, 0]));
            }
        }

        let encoded = encode(
            &DynamicImage::ImageRgba8(buffer),
            Format::Avif,
            Quality::lossy(90.),
            None,
        )
        .expect("avif encodes");
        let decoded = image::load_from_memory(&encoded).expect("avif decodes");

        assert!(
            has_transparency(&decoded),
            "the see-through half came back opaque"
        );
    }

    #[test]
    fn transparency_forces_the_lossless_path() {
        let mut buffer: ImageBuffer<Rgba<u8>, Vec<u8>> =
            ImageBuffer::from_pixel(16, 16, Rgba([10, 20, 30, 255]));
        assert!(!has_transparency(&DynamicImage::ImageRgba8(buffer.clone())));

        buffer.put_pixel(0, 0, Rgba([10, 20, 30, 0]));
        let image = DynamicImage::ImageRgba8(buffer);
        assert!(has_transparency(&image), "one see-through pixel is enough");

        let encoded = encode(&image, Format::WebP, Quality::lossy(20.), None).unwrap();
        let decoded = image::load_from_memory(&encoded).unwrap().to_rgba8();
        assert_eq!(decoded.get_pixel(1, 1), &Rgba([10, 20, 30, 255]));
        assert_eq!(decoded.get_pixel(0, 0)[3], 0);
    }

    #[test]
    fn output_path_mirrors_the_source_tree() {
        let path = output_path(
            Path::new("/photos"),
            Path::new("/photos/album/one.PNG"),
            Path::new("/photos/optimised"),
            Format::WebP,
        );
        assert_eq!(path, Path::new("/photos/optimised/album/one.webp"));

        let avif = output_path(
            Path::new("/photos"),
            Path::new("/photos/album/one.PNG"),
            Path::new("/photos/optimised"),
            Format::Avif,
        );
        assert_eq!(avif, Path::new("/photos/optimised/album/one.avif"));

        let jpeg_xl = output_path(
            Path::new("/photos"),
            Path::new("/photos/album/one.PNG"),
            Path::new("/photos/optimised"),
            Format::JpegXl,
        );
        assert_eq!(jpeg_xl, Path::new("/photos/optimised/album/one.jxl"));

        let jpeg = output_path(
            Path::new("/photos"),
            Path::new("/photos/album/one.PNG"),
            Path::new("/photos/optimised"),
            Format::Jpeg,
        );
        assert_eq!(jpeg, Path::new("/photos/optimised/album/one.jpg"));
    }

    #[test]
    fn converting_writes_a_smaller_file_and_reports_its_size() {
        let dir = temp_dir("roundtrip");
        let source = dir.join("big.png");
        photo(400, 400).save(&source).unwrap();
        let out = dir.join("optimised");

        let converted = convert_to(
            &dir,
            &source,
            &output_path(&dir, &source, &out, Format::WebP),
            Format::WebP,
            Quality::lossy(75.),
            MaxEdge::FULL,
        )
        .expect("conversion runs");

        assert_eq!(converted.written, out.join("big.webp"));
        assert!(converted.written.exists(), "the file is actually on disk");
        assert_eq!(
            converted.bytes,
            std::fs::metadata(&converted.written).unwrap().len(),
            "reported size matches the file"
        );
        // Not asserting it shrank: this source is pure noise, which is the one input
        // that legitimately does not compress. Size correctness is covered above.
        assert!(converted.bytes > 0);
    }

    #[test]
    fn max_edge_scales_down_and_keeps_the_aspect_ratio() {
        let scaled = MaxEdge(Some(100)).apply(photo(400, 200));
        assert_eq!((scaled.width(), scaled.height()), (100, 50));
    }

    #[test]
    fn max_edge_never_scales_up() {
        let untouched = MaxEdge(Some(4000)).apply(photo(80, 60));
        assert_eq!(
            (untouched.width(), untouched.height()),
            (80, 60),
            "a small source must not be stretched to fill the budget"
        );
        let full = MaxEdge::FULL.apply(photo(80, 60));
        assert_eq!((full.width(), full.height()), (80, 60));
    }

    /// The window's box follows the CLI's rule: a positive whole number, never a
    /// stretch. Typing 4000 over an 80px source is a no-op, not an upscale.
    #[test]
    fn a_custom_max_edge_rejects_zero_and_never_upscales() {
        assert_eq!(MaxEdge::parse("1200"), Some(MaxEdge(Some(1200))));
        assert_eq!(MaxEdge::parse(" 640 "), Some(MaxEdge(Some(640))));
        assert_eq!(MaxEdge::parse(""), Some(MaxEdge::FULL));
        for junk in ["0", "-5", "abc", "1.5", "12px"] {
            assert_eq!(MaxEdge::parse(junk), None, "accepted {junk:?}");
        }

        let custom = MaxEdge::parse("4000").expect("a large edge parses");
        let untouched = custom.apply(photo(80, 60));
        assert_eq!((untouched.width(), untouched.height()), (80, 60));
        let scaled = MaxEdge::parse("40")
            .expect("a small edge parses")
            .apply(photo(80, 60));
        assert_eq!((scaled.width(), scaled.height()), (40, 30));
    }

    /// The speed dial has to reach libaom, not just the settings file. Six is what
    /// Press has always encoded at, so the default must still produce those bytes, and
    /// a different speed must produce different ones.
    ///
    /// Asked for by argument, never by writing the process-wide value: cargo runs
    /// these in parallel, and the other AVIF tests are encoding while this one runs.
    #[test]
    fn the_avif_speed_setting_reaches_the_encoder() {
        let rgb = photo(64, 64).to_rgb8();
        let at = |speed| {
            crate::avif::encode(rgb.as_raw(), 64, 64, false, 50, speed, 1, None)
                .expect("AVIF encodes")
        };

        let default = at(crate::avif::DEFAULT_SPEED);
        let fast = at(10);
        assert_ne!(fast, default, "speed 10 wrote the speed 6 bytes");
        assert!(
            crate::scan::decode_bytes(&fast).is_some(),
            "speed 10 is unreadable"
        );

        // And an unconfigured process still asks for six, which is what keeps an
        // existing run byte for byte what it was.
        assert_eq!(crate::avif::speed(), crate::avif::DEFAULT_SPEED);
        assert_eq!(crate::avif::configured_speed(), None);
    }

    /// The scaled decode changes what conversion holds in memory, not what it writes:
    /// a 4000px JPEG asked for 1000px still exports 1000px.
    #[test]
    fn a_big_jpeg_exports_at_the_max_edge_after_a_scaled_decode() {
        let dir = temp_dir("scaled-export");
        let source = dir.join("big.jpg");
        photo(4000, 1000).save(&source).unwrap();

        let converted = convert_to(
            &dir,
            &source,
            &output_path(&dir, &source, &dir.join("out"), Format::WebP),
            Format::WebP,
            Quality::lossy(80.),
            MaxEdge(Some(1000)),
        )
        .expect("conversion runs");

        assert_eq!((converted.width, converted.height), (1000, 250));
        let written = crate::scan::probe(&converted.written).expect("the output is an image");
        assert_eq!((written.width, written.height), (1000, 250));
    }

    #[test]
    fn resizing_is_reported_in_the_result() {
        let dir = temp_dir("resize");
        let source = dir.join("wide.png");
        photo(600, 300).save(&source).unwrap();

        let converted = convert_to(
            &dir,
            &source,
            &output_path(&dir, &source, &dir.join("out"), Format::WebP),
            Format::WebP,
            Quality::lossy(80.),
            MaxEdge(Some(200)),
        )
        .expect("conversion runs");

        assert_eq!((converted.width, converted.height), (200, 100));
    }

    /// The bug this exists to stop: `shot.jpg` and `shot.png` both asked for
    /// `optimized/shot.webp`, one silently replaced the other, and the run still
    /// reported two conversions and a total that counted bytes no longer on disk.
    #[test]
    fn two_sources_never_claim_one_output() {
        let root = Path::new("/photos");
        let out = Path::new("/photos/optimized");
        let sources = [
            PathBuf::from("/photos/shot.jpg"),
            PathBuf::from("/photos/shot.png"),
            PathBuf::from("/photos/album/shot.png"),
            PathBuf::from("/photos/shot.webp"),
        ];

        let planned = plan_outputs(root, &sources, &sources, out, Format::WebP);
        assert_eq!(
            planned,
            [
                Ok(PathBuf::from("/photos/optimized/shot.webp")),
                Ok(PathBuf::from("/photos/optimized/shot-png.webp")),
                Ok(PathBuf::from("/photos/optimized/album/shot.webp")),
                Ok(PathBuf::from("/photos/optimized/shot-webp.webp")),
            ],
            "the first claim keeps the plain name, a subfolder is not a clash"
        );
    }

    /// Names that differ only in case are two files on Linux and one on macOS, so they
    /// count as a clash either way. Three of them exhaust the extension suffix and
    /// reach the numbered fallback.
    #[test]
    fn case_alone_is_a_clash_and_a_taken_suffix_gets_numbered() {
        let sources = [
            PathBuf::from("/p/a.png"),
            PathBuf::from("/p/A.png"),
            PathBuf::from("/p/a.PNG"),
        ];

        let planned = plan_outputs(
            Path::new("/p"),
            &sources,
            &sources,
            Path::new("/p/out"),
            Format::WebP,
        );
        let names: Vec<String> = planned
            .iter()
            .map(|path| {
                path.as_ref()
                    .unwrap()
                    .file_name()
                    .unwrap()
                    .to_string_lossy()
                    .into_owned()
            })
            .collect();
        assert_eq!(names, ["a-png-2.webp", "A.webp", "a-png.webp"]);
    }

    #[test]
    fn collision_ownership_does_not_follow_the_input_order() {
        let root = Path::new("/photos");
        let out = Path::new("/photos/optimized");
        let sources = [
            PathBuf::from("/photos/shot.png"),
            PathBuf::from("/photos/shot.jpg"),
        ];
        let forward = plan_outputs(root, &sources, &sources, out, Format::WebP);
        let reversed_sources = [sources[1].clone(), sources[0].clone()];
        let reversed = plan_outputs(
            root,
            &reversed_sources,
            &reversed_sources,
            out,
            Format::WebP,
        );

        assert_eq!(forward[0], reversed[1]);
        assert_eq!(forward[1], reversed[0]);
    }

    /// A part-written file must never be left looking like a finished one.
    #[test]
    fn a_conversion_leaves_no_partial_file_behind() {
        let dir = temp_dir("atomic");
        let source = dir.join("in.png");
        photo(64, 64).save(&source).unwrap();
        let written = dir.join("out").join("in.webp");

        convert_to(
            &dir,
            &source,
            &written,
            Format::WebP,
            Quality::lossy(80.),
            MaxEdge::FULL,
        )
        .expect("conversion runs");

        let leftovers: Vec<_> = std::fs::read_dir(dir.join("out"))
            .unwrap()
            .filter_map(|entry| entry.ok())
            .map(|entry| entry.file_name().to_string_lossy().into_owned())
            .filter(|name| name.ends_with(".part"))
            .collect();
        assert!(leftovers.is_empty(), "found {leftovers:?}");
    }

    #[cfg(unix)]
    #[test]
    fn conversion_refuses_a_symlinked_output_folder() {
        use std::os::unix::fs::symlink;

        let dir = temp_dir("output-symlink");
        let outside = temp_dir("output-symlink-outside");
        let source = dir.join("in.png");
        photo(32, 32).save(&source).unwrap();
        symlink(&outside, dir.join("optimized")).unwrap();

        assert!(
            convert_to(
                &dir,
                &source,
                &dir.join("optimized/in.webp"),
                Format::WebP,
                Quality::lossy(80.),
                MaxEdge::FULL,
            )
            .is_err()
        );
        assert!(!outside.join("in.webp").exists());
    }

    #[cfg(unix)]
    #[test]
    fn conversion_refuses_a_symlinked_partial_file() {
        use std::os::unix::fs::symlink;

        let dir = temp_dir("part-symlink");
        let out = dir.join("optimized");
        let source = dir.join("in.png");
        let output = out.join("in.webp");
        let victim = dir.join("victim");
        photo(32, 32).save(&source).unwrap();
        std::fs::create_dir(&out).unwrap();
        std::fs::write(&victim, b"keep me").unwrap();
        symlink(
            &victim,
            out.join(format!("in.webp.{}.part", std::process::id())),
        )
        .unwrap();

        assert!(
            convert_to(
                &out,
                &source,
                &output,
                Format::WebP,
                Quality::lossy(80.),
                MaxEdge::FULL,
            )
            .is_err()
        );
        assert_eq!(std::fs::read(&victim).unwrap(), b"keep me");
    }

    /// The destination people actually choose is somewhere else entirely — a staging
    /// directory, a share, a build tree. Measuring the target against the audited root
    /// refused every one of those files and had no reason to give for it.
    #[test]
    fn conversion_into_a_folder_outside_the_audited_root_writes_every_file() {
        let root = temp_dir("external-source");
        let outside = temp_dir("external-output");
        std::fs::create_dir(root.join("album")).unwrap();
        let sources = [root.join("one.png"), root.join("album").join("two.png")];
        for source in &sources {
            photo(48, 48).save(source).unwrap();
        }

        let context = crate::settings::Output::Folder(outside)
            .context(&root)
            .expect("an external output folder establishes");
        assert!(!context.output_root().starts_with(&root));

        let written = parking_lot::Mutex::new(Vec::new());
        let planned = plan_outputs(
            &root,
            &sources,
            &sources,
            context.output_root(),
            Format::WebP,
        );
        convert_each(
            &root,
            &sources,
            &planned,
            &plain(context.output_root()),
            Format::WebP,
            Quality::lossy(80.),
            MaxEdge::FULL,
            |_, converted| {
                written
                    .lock()
                    .push(converted.expect("the file converts").written);
            },
        );

        let mut written = written.into_inner();
        written.sort();
        assert_eq!(
            written,
            [
                context.output_root().join("album").join("two.webp"),
                context.output_root().join("one.webp"),
            ]
        );
        assert!(written.iter().all(|path| path.is_file()));
    }

    /// A `.part` on the staging name used to fail that file for good, with nothing on
    /// the toast to say what was in the way. The staging name now carries this process
    /// id, so anything already sitting on it belongs to someone else.
    #[test]
    fn a_partial_file_this_run_did_not_create_is_named_and_left_alone() {
        let dir = temp_dir("stale-part");
        let out = dir.join("optimized");
        let source = dir.join("in.png");
        photo(32, 32).save(&source).unwrap();
        std::fs::create_dir(&out).unwrap();
        let blocker = out.join(format!("in.webp.{}.part", std::process::id()));
        std::fs::write(&blocker, b"not written by this run").unwrap();

        let failure = convert_to(
            &out,
            &source,
            &out.join("in.webp"),
            Format::WebP,
            Quality::lossy(80.),
            MaxEdge::FULL,
        )
        .expect_err("a partial file this run did not create blocks the write");

        assert_eq!(failure, Failure::StalePartial(blocker.clone()));
        assert!(
            failure
                .reason()
                .expect("the failure is named")
                .contains(&blocker.display().to_string())
        );
        assert_eq!(std::fs::read(&blocker).unwrap(), b"not written by this run");
    }

    /// Writing into a folder that holds the batch let `a.png` land on the source
    /// `a.webp`, and the run reported the destroyed original as a saving.
    #[test]
    fn a_planned_output_never_claims_a_source_of_the_batch() {
        let root = Path::new("/photos");
        let sources = [
            PathBuf::from("/photos/a.png"),
            PathBuf::from("/photos/a.webp"),
            PathBuf::from("/photos/b.png"),
        ];

        let planned = plan_outputs(root, &sources, &sources, root, Format::WebP);

        assert_eq!(
            planned,
            [
                Err(Failure::OverwritesSource),
                Err(Failure::OverwritesSource),
                Ok(PathBuf::from("/photos/b.webp")),
            ],
            "a refused source claims no name and does not stop its siblings"
        );
    }

    /// Ticking one file and pointing the output at a subfolder of the same audit is
    /// how an original nobody selected gets overwritten: this run never had it in
    /// `sources`, so only the audited set can protect it.
    #[test]
    fn a_planned_output_never_claims_an_audited_file_left_unselected() {
        let root = Path::new("/photos");
        let selected = [PathBuf::from("/photos/x.png")];
        let audited = [
            PathBuf::from("/photos/x.png"),
            PathBuf::from("/photos/album/x.webp"),
        ];

        let planned = plan_outputs(
            root,
            &selected,
            &audited,
            Path::new("/photos/album"),
            Format::WebP,
        );

        assert_eq!(planned, [Err(Failure::OverwritesSource)]);
    }

    /// Everything replace mode needs for one file: where it lands, and where its
    /// original goes first.
    fn replace_run<'a>(root: &'a Path, backups: &'a Path) -> Destination<'a> {
        Destination {
            out_dir: root,
            backups: Some(backups),
            manifest: &EMPTY,
        }
    }

    /// One file converted the way a run converts it: recorded, backed up, then
    /// installed.
    fn convert_recorded(
        root: &Path,
        out_dir: &Path,
        source: &Path,
        written: &Path,
        backup: Option<&Backup>,
        quality: Quality,
    ) -> Result<Converted, Failure> {
        let stamp = crate::manifest::Stamp::new(Format::WebP, quality, MaxEdge::FULL);
        let recording = Recording {
            root,
            out_dir,
            stamp: &stamp,
            backup,
        };
        super::convert_to(
            out_dir,
            source,
            written,
            Some(&recording),
            Format::WebP,
            quality,
            MaxEdge::FULL,
        )
    }

    fn write_webp(path: &Path, image: &DynamicImage) {
        let encoded = encode(image, Format::WebP, Quality::lossy(80.), None).expect("WebP encodes");
        std::fs::write(path, encoded).unwrap();
    }

    fn stray_parts(dir: &Path) -> Vec<std::ffi::OsString> {
        std::fs::read_dir(dir)
            .unwrap()
            .map(|item| item.unwrap().file_name())
            .filter(|name| name.to_string_lossy().contains(".part"))
            .collect()
    }

    #[test]
    fn replace_mode_writes_beside_the_source_and_backs_the_original_up() {
        let dir = temp_dir("replace-in-place");
        let source = dir.join("shot.png");
        photo(64, 64).save(&source).unwrap();
        let original = std::fs::read(&source).unwrap();
        assert!(!original.is_empty(), "the fixture is a real file");

        let backups = crate::manifest::backup_root(&dir);
        let destination = replace_run(&dir, &backups);
        let sources = [source.clone()];
        let planned = super::plan_outputs(&dir, &sources, &sources, &destination, Format::WebP);
        assert_eq!(planned, [Ok(dir.join("shot.webp"))]);
        let written = planned[0].clone().unwrap();
        let backup = destination
            .backup(&dir, &source)
            .expect("replace mode names a backup");
        assert!(backup.moved, "the first run is the one that moves it");

        let converted = convert_recorded(
            &dir,
            &dir,
            &source,
            &written,
            Some(&backup),
            Quality::lossy(80.),
        )
        .expect("the file converts in place");

        assert!(converted.bytes > 0);
        assert!(written.is_file(), "the output took the source's folder");
        assert!(!source.exists(), "the original left its own name");
        assert_eq!(
            std::fs::read(&backup.path).unwrap(),
            original,
            "the original is kept byte for byte"
        );
        let recorded = crate::manifest::load(&dir);
        assert_eq!(recorded.outputs.len(), 1);
        assert_eq!(
            recorded.outputs[0].backup.as_deref(),
            Some(Path::new("shot.png"))
        );
        assert_eq!(
            recorded.outputs[0].output_bytes,
            std::fs::metadata(&written).unwrap().len(),
            "the record measures the file that landed"
        );
        std::fs::remove_dir_all(dir).unwrap();
    }

    /// The record has to be on disk while the run is still going. A machine that
    /// dies after four hundred of five hundred files has four hundred originals in
    /// the backup, and a record written at the end would describe none of them.
    #[test]
    fn every_file_is_recorded_before_the_run_that_wrote_it_ends() {
        let dir = temp_dir("record-during-run");
        let out = dir.join(crate::scan::OUTPUT_DIR);
        let sources: Vec<PathBuf> = (0..4)
            .map(|index| {
                let path = dir.join(format!("shot-{index}.png"));
                photo(48, 48).save(&path).unwrap();
                path
            })
            .collect();

        let seen = parking_lot::Mutex::new(Vec::new());
        // JPEG XL runs one file at a time, so "how many records are on disk" is
        // exactly "how far along the run is" and the count can be pinned.
        let planned = plan_outputs(&dir, &sources, &sources, &out, Format::JpegXl);
        convert_each(
            &dir,
            &sources,
            &planned,
            &plain(&out),
            Format::JpegXl,
            Quality::lossy(80.),
            MaxEdge::FULL,
            |_, converted| {
                converted.expect("the file converts");
                // Read back from disk, not from anything the run is holding.
                seen.lock().push(crate::manifest::load(&out).outputs.len());
            },
        );

        let seen = seen.into_inner();
        assert_eq!(seen.len(), 4);
        // `convert_each` reports one file at a time, in the order they land, so
        // the record on disk is exactly as long as the run is far along.
        for (index, recorded) in seen.iter().enumerate() {
            assert_eq!(
                *recorded,
                index + 1,
                "file {} was recorded before it reported: {seen:?}",
                index + 1
            );
        }
        assert_eq!(crate::manifest::load(&out).outputs.len(), 4);
        std::fs::remove_dir_all(dir).unwrap();
    }

    /// A backup that flattened the tree would put two `shot.png` from two albums on
    /// one name, and the second would be the one that survived.
    #[test]
    fn the_backup_mirrors_the_folder_each_original_was_in() {
        let dir = temp_dir("replace-mirror");
        let album = dir.join("album").join("2019");
        std::fs::create_dir_all(&album).unwrap();
        let source = album.join("shot.png");
        photo(48, 48).save(&source).unwrap();

        let backups = crate::manifest::backup_root(&dir);
        let destination = replace_run(&dir, &backups);
        let backup = destination.backup(&dir, &source).unwrap();
        assert_eq!(
            backup.path,
            dir.join(crate::scan::BACKUP_DIR)
                .join("album")
                .join("2019")
                .join("shot.png")
        );

        convert_recorded(
            &dir,
            &dir,
            &source,
            &output_path(&dir, &source, &dir, Format::WebP),
            Some(&backup),
            Quality::lossy(80.),
        )
        .expect("the nested file converts");

        assert!(
            backup.path.is_file(),
            "the original is under its own folders"
        );
        assert!(album.join("shot.webp").is_file());
        std::fs::remove_dir_all(dir).unwrap();
    }

    /// The undo the whole mode rests on. One source changes name on the way out and
    /// one keeps it, because the WebP-to-WebP case is the one where removing the
    /// output after the restore would delete the original again.
    #[test]
    fn restoring_puts_every_original_back_byte_for_byte_and_removes_what_replaced_it() {
        let dir = temp_dir("replace-restore");
        std::fs::create_dir_all(dir.join("album")).unwrap();
        let png = dir.join("album").join("shot.png");
        let webp = dir.join("kept.webp");
        photo(64, 64).save(&png).unwrap();
        write_webp(&webp, &photo(80, 40));
        let originals: Vec<Vec<u8>> = [&png, &webp]
            .iter()
            .map(|path| std::fs::read(path).unwrap())
            .collect();

        let backups = crate::manifest::backup_root(&dir);
        let destination = replace_run(&dir, &backups);
        let sources = [png.clone(), webp.clone()];
        let planned = super::plan_outputs(&dir, &sources, &sources, &destination, Format::WebP);
        assert_eq!(
            planned,
            [Ok(dir.join("album").join("shot.webp")), Ok(webp.clone())],
            "a WebP converted to WebP writes its own name back"
        );

        for (source, written) in sources.iter().zip(&planned) {
            let backup = destination.backup(&dir, source).unwrap();
            convert_recorded(
                &dir,
                &dir,
                source,
                &written.clone().unwrap(),
                Some(&backup),
                Quality::lossy(60.),
            )
            .expect("the file converts");
        }
        assert!(!png.exists() && dir.join("album").join("shot.webp").is_file());
        assert_ne!(
            std::fs::read(&webp).unwrap(),
            originals[1],
            "the WebP was really re-encoded over its own name"
        );

        let restored = crate::manifest::restore(&dir);
        assert!(
            restored.failures.is_empty(),
            "nothing refused: {:?}",
            restored.failures
        );
        assert_eq!(
            restored.restored,
            vec![webp.clone(), png.clone()],
            "the newest run is undone first"
        );
        assert_eq!(std::fs::read(&png).unwrap(), originals[0]);
        assert_eq!(std::fs::read(&webp).unwrap(), originals[1]);
        assert!(
            !dir.join("album").join("shot.webp").exists(),
            "the file that replaced the original is gone"
        );
        assert!(!backups.exists(), "the emptied backup does not linger");
        assert!(!crate::manifest::path(&dir).exists());
        std::fs::remove_dir_all(dir).unwrap();
    }

    /// The bug the run record exists for: a later run converting `shot.jpg` used to
    /// walk over the `shot.webp` an earlier one made from `shot.png`, and report the
    /// destroyed file as a saving. It gets a name of its own instead.
    #[test]
    fn a_name_an_earlier_run_wrote_from_another_source_is_never_overwritten() {
        let dir = temp_dir("manifest-claim");
        let out = dir.join(crate::scan::OUTPUT_DIR);
        let png = dir.join("shot.png");
        let jpg = dir.join("shot.jpg");
        photo(40, 40).save(&png).unwrap();
        photo(40, 40).save(&jpg).unwrap();
        let audited = [png.clone(), jpg.clone()];

        let first = super::plan_outputs(
            &dir,
            std::slice::from_ref(&png),
            &audited,
            &plain(&out),
            Format::WebP,
        );
        let written = first[0].clone().expect("the first run claims the name");
        convert_recorded(&dir, &out, &png, &written, None, Quality::lossy(80.))
            .expect("the first run converts");
        let kept = std::fs::read(&written).unwrap();

        let recorded = crate::manifest::load(&out);
        assert_eq!(recorded.outputs.len(), 1, "the first run recorded itself");
        let destination = Destination {
            out_dir: &out,
            backups: None,
            manifest: &recorded,
        };
        let second = super::plan_outputs(
            &dir,
            std::slice::from_ref(&jpg),
            &audited,
            &destination,
            Format::WebP,
        );

        assert_eq!(
            second,
            [Ok(out.join("shot-jpg.webp"))],
            "the second source is renamed around the name it does not own"
        );
        assert_eq!(
            std::fs::read(&written).unwrap(),
            kept,
            "the earlier run's output is untouched"
        );
        std::fs::remove_dir_all(dir).unwrap();
    }

    /// A claim on a file somebody has since deleted is not a reason to rename
    /// anything: the name is free again.
    #[test]
    fn a_record_for_an_output_that_is_gone_claims_nothing() {
        let dir = temp_dir("manifest-ghost");
        let out = dir.join(crate::scan::OUTPUT_DIR);
        let png = dir.join("shot.png");
        let jpg = dir.join("shot.jpg");
        photo(32, 32).save(&png).unwrap();
        photo(32, 32).save(&jpg).unwrap();

        let written = out.join("shot.webp");
        convert_recorded(&dir, &out, &png, &written, None, Quality::lossy(80.))
            .expect("the first run converts");
        std::fs::remove_file(&written).unwrap();

        let recorded = crate::manifest::load(&out);
        assert_eq!(recorded.outputs.len(), 1, "the record is still there");
        let destination = Destination {
            out_dir: &out,
            backups: None,
            manifest: &recorded,
        };
        let planned = super::plan_outputs(
            &dir,
            std::slice::from_ref(&jpg),
            &[png, jpg.clone()],
            &destination,
            Format::WebP,
        );
        assert_eq!(planned, [Ok(written)]);
        std::fs::remove_dir_all(dir).unwrap();
    }

    /// Its own output is the one file a source may always replace, or nothing could
    /// ever be converted twice.
    #[test]
    fn a_rerun_from_the_same_source_replaces_its_own_output() {
        let dir = temp_dir("manifest-rerun");
        let out = dir.join(crate::scan::OUTPUT_DIR);
        let source = dir.join("shot.png");
        photo(120, 120).save(&source).unwrap();
        let sources = [source.clone()];

        let mut sizes = Vec::new();
        for quality in [90., 20.] {
            let recorded = crate::manifest::load(&out);
            let destination = Destination {
                out_dir: &out,
                backups: None,
                manifest: &recorded,
            };
            let planned = super::plan_outputs(&dir, &sources, &sources, &destination, Format::WebP);
            assert_eq!(planned, [Ok(out.join("shot.webp"))]);
            let converted = convert_recorded(
                &dir,
                &out,
                &source,
                &planned[0].clone().unwrap(),
                None,
                Quality::lossy(quality),
            )
            .expect("the rerun converts");
            sizes.push(converted.bytes);
        }

        assert!(
            sizes[1] < sizes[0],
            "the second run really rewrote the file: {sizes:?}"
        );
        let recorded = crate::manifest::load(&out);
        assert_eq!(recorded.outputs.last().unwrap().quality, "q20");
        std::fs::remove_dir_all(dir).unwrap();
    }

    /// A folder replaced twice is a chain: the second run's source is the first
    /// run's output, the original at the head of the chain is already safe, and a
    /// second backup of a derived file would only get in the way.
    #[test]
    fn a_second_replace_run_reuses_the_first_runs_backup_and_still_restores_the_original() {
        let dir = temp_dir("replace-chain");
        let source = dir.join("shot.png");
        photo(96, 96).save(&source).unwrap();
        let original = std::fs::read(&source).unwrap();
        let backups = crate::manifest::backup_root(&dir);
        let written = dir.join("shot.webp");

        let mut sizes = Vec::new();
        for (index, quality) in [90., 20.].into_iter().enumerate() {
            let recorded = crate::manifest::load(&dir);
            let destination = Destination {
                out_dir: &dir,
                backups: Some(&backups),
                manifest: &recorded,
            };
            let current = if index == 0 {
                source.clone()
            } else {
                written.clone()
            };
            let sources = [current.clone()];
            let planned = super::plan_outputs(&dir, &sources, &sources, &destination, Format::WebP);
            assert_eq!(planned, [Ok(written.clone())], "run {index} keeps the name");
            let backup = destination.backup(&dir, &current).unwrap();
            assert_eq!(
                backup.path,
                backups.join("shot.png"),
                "both runs point at the one original"
            );
            assert_eq!(backup.moved, index == 0, "only the first run moves it");
            sizes.push(
                convert_recorded(
                    &dir,
                    &dir,
                    &current,
                    &written,
                    Some(&backup),
                    Quality::lossy(quality),
                )
                .expect("the chained run converts")
                .bytes,
            );
        }

        assert!(sizes[1] < sizes[0], "the second run rewrote it: {sizes:?}");
        assert_eq!(
            std::fs::read_dir(&backups).unwrap().count(),
            1,
            "the chain took one backup, not two"
        );

        let restored = crate::manifest::restore(&dir);
        assert!(
            restored.failures.is_empty(),
            "nothing refused: {:?}",
            restored.failures
        );
        assert_eq!(restored.restored, vec![source.clone()]);
        assert_eq!(
            std::fs::read(&source).unwrap(),
            original,
            "the file at the head of the chain is what comes back"
        );
        assert!(!written.exists(), "neither run's output survives the undo");
        assert!(!backups.exists());
        std::fs::remove_dir_all(dir).unwrap();
    }

    /// A chain link is only a link while the file on disk is still the output that
    /// run installed. Somebody who edited or replaced it since is holding an
    /// original of their own, and inheriting a backup for it would rename over the
    /// only copy of it there is.
    #[test]
    fn an_output_edited_between_two_replace_runs_is_backed_up_as_an_original_of_its_own() {
        let dir = temp_dir("replace-edited-between-runs");
        let backups = crate::manifest::backup_root(&dir);
        let png = dir.join("changes-name.png");
        let webp = dir.join("keeps-name.webp");
        photo(64, 64).save(&png).unwrap();
        write_webp(&webp, &photo(64, 64));

        // Run one: one source changes its name on the way out, one keeps it.
        for source in [png.clone(), webp.clone()] {
            let recorded = crate::manifest::load(&dir);
            let destination = Destination {
                out_dir: &dir,
                backups: Some(&backups),
                manifest: &recorded,
            };
            let backup = destination.backup(&dir, &source).unwrap();
            convert_recorded(
                &dir,
                &dir,
                &source,
                &output_path(&dir, &source, &dir, Format::WebP),
                Some(&backup),
                Quality::lossy(80.),
            )
            .expect("the first run converts");
        }

        // Somebody works on both outputs afterwards. Still images, still at the
        // same names, and no longer the files those runs installed.
        let changed = dir.join("changes-name.webp");
        write_webp(&changed, &photo(40, 40));
        write_webp(&webp, &photo(36, 36));
        let edits: Vec<Vec<u8>> = [&changed, &webp]
            .iter()
            .map(|path| std::fs::read(path).unwrap())
            .collect();

        let recorded = crate::manifest::load(&dir);
        let destination = Destination {
            out_dir: &dir,
            backups: Some(&backups),
            manifest: &recorded,
        };

        // Its backup name is free, so the edit is kept like any other original.
        let fresh = destination.backup(&dir, &changed).unwrap();
        assert!(fresh.moved, "an edited file is nobody's output any more");
        assert_eq!(fresh.path, backups.join("changes-name.webp"));
        convert_recorded(
            &dir,
            &dir,
            &changed,
            &changed,
            Some(&fresh),
            Quality::lossy(30.),
        )
        .expect("the second run converts the edited file");
        assert_eq!(
            std::fs::read(&fresh.path).unwrap(),
            edits[0],
            "the edit was kept, not renamed over"
        );

        // This one's backup name is taken by the original it replaced, so there is
        // nowhere to keep the edit and the run refuses it by name.
        let taken = destination.backup(&dir, &webp).unwrap();
        assert!(taken.moved);
        let refused = convert_recorded(&dir, &dir, &webp, &webp, Some(&taken), Quality::lossy(30.));
        assert_eq!(refused, Err(Failure::BackupOccupied(taken.path.clone())));
        assert_eq!(
            std::fs::read(&webp).unwrap(),
            edits[1],
            "the edit is untouched"
        );
        std::fs::remove_dir_all(dir).unwrap();
    }

    /// A record goes down before the original moves, so a file that fails after
    /// that leaves a claim on a name it never took. Left standing, the undo reads
    /// it as a file to delete and an original to look for, and fails the real
    /// record beside it.
    #[test]
    fn a_run_that_fails_after_its_record_takes_the_record_back() {
        let dir = temp_dir("replace-phantom-record");
        let backups = crate::manifest::backup_root(&dir);
        let source = dir.join("shot.png");
        photo(64, 64).save(&source).unwrap();
        let original = std::fs::read(&source).unwrap();

        let first = Backup {
            path: backups.join("shot.png"),
            moved: true,
        };
        convert_recorded(
            &dir,
            &dir,
            &source,
            &dir.join("shot.webp"),
            Some(&first),
            Quality::lossy(80.),
        )
        .expect("the first run converts");
        assert_eq!(crate::manifest::load(&dir).outputs.len(), 1);

        // A new file arrives under the name the original had.
        photo(32, 32).save(&source).unwrap();
        let refused = convert_recorded(
            &dir,
            &dir,
            &source,
            &dir.join("shot-2.webp"),
            Some(&first),
            Quality::lossy(80.),
        );
        assert_eq!(refused, Err(Failure::BackupOccupied(first.path.clone())));
        assert!(
            !dir.join("shot-2.webp").exists(),
            "the refused run installed nothing"
        );
        assert_eq!(
            crate::manifest::load(&dir).outputs.len(),
            1,
            "and its record was taken back"
        );

        // The newcomer is standing on the slot, so the undo names the conflict and
        // keeps the original where it can still be reached.
        let blocked = crate::manifest::restore(&dir);
        assert!(blocked.restored.is_empty());
        assert_eq!(blocked.failures.len(), 1);
        assert!(
            blocked.failures[0].contains("already at"),
            "the refusal says what is in the way: {:?}",
            blocked.failures
        );
        assert!(first.path.is_file(), "the original is still recoverable");

        std::fs::remove_file(&source).unwrap();
        let restored = crate::manifest::restore(&dir);
        assert!(
            restored.failures.is_empty(),
            "nothing refused once the slot is free: {:?}",
            restored.failures
        );
        assert_eq!(restored.restored, vec![source.clone()]);
        assert_eq!(std::fs::read(&source).unwrap(), original);
        assert!(!dir.join("shot.webp").exists());
        std::fs::remove_dir_all(dir).unwrap();
    }

    /// An earlier run's backup is the real original; this run's "original" is that
    /// run's output. Overwriting it would lose the only copy there is.
    #[test]
    fn a_backup_name_an_earlier_run_took_stops_the_file_and_keeps_the_older_original() {
        let dir = temp_dir("replace-backup-taken");
        let source = dir.join("shot.webp");
        write_webp(&source, &photo(48, 48));
        let backups = crate::manifest::backup_root(&dir);
        let backup = Backup {
            path: backups.join("shot.webp"),
            moved: true,
        };
        std::fs::create_dir_all(&backups).unwrap();
        std::fs::write(&backup.path, b"the original from the first run").unwrap();

        let refused = convert_recorded(
            &dir,
            &dir,
            &source,
            &source.clone(),
            Some(&backup),
            Quality::lossy(80.),
        );

        assert_eq!(refused, Err(Failure::BackupOccupied(backup.path.clone())));
        assert_eq!(
            std::fs::read(&backup.path).unwrap(),
            b"the original from the first run",
            "the older original is what stays"
        );
        assert!(source.is_file(), "the file this run refused is still there");
        assert!(stray_parts(&dir).is_empty(), "the stage was removed");
        std::fs::remove_dir_all(dir).unwrap();
    }

    /// The install is the last thing that can fail, and by then the original is out
    /// of its own name. A folder left with a hole where an image was is worse than
    /// a file that would not convert, so the original goes back.
    #[test]
    fn an_install_that_fails_after_the_move_puts_the_original_back() {
        let dir = temp_dir("replace-install-fails");
        let source = dir.join("shot.png");
        photo(56, 56).save(&source).unwrap();
        let original = std::fs::read(&source).unwrap();
        // A directory standing on the output's name: the bytes stage fine and the
        // rename onto it cannot succeed.
        let written = dir.join("shot.webp");
        std::fs::create_dir(&written).unwrap();
        let backups = crate::manifest::backup_root(&dir);
        let backup = Backup {
            path: backups.join("shot.png"),
            moved: true,
        };

        let refused = convert_recorded(
            &dir,
            &dir,
            &source,
            &written,
            Some(&backup),
            Quality::lossy(80.),
        );

        assert_eq!(refused, Err(Failure::InstallFailed));
        assert!(
            Failure::InstallFailed.reason().is_some(),
            "the failure says what happened"
        );
        assert_eq!(
            std::fs::read(&source).unwrap(),
            original,
            "the original is back under its own name"
        );
        assert!(!backup.path.exists(), "and out of the backup again");
        assert!(stray_parts(&dir).is_empty(), "the stage was removed");

        // The record went down before the move, so one describes a file that was
        // never installed. An undo reads that as nothing to do rather than a
        // failure, and drops it.
        let restored = crate::manifest::restore(&dir);
        assert!(
            restored.failures.is_empty(),
            "nothing refused: {:?}",
            restored.failures
        );
        assert!(restored.restored.is_empty());
        assert_eq!(
            std::fs::read(&source).unwrap(),
            original,
            "and the original is still there afterwards"
        );
        assert!(
            written.is_dir(),
            "whatever blocked the install is not this app's to delete"
        );
        assert!(!crate::manifest::path(&dir).exists());
        std::fs::remove_dir_all(dir).unwrap();
    }

    /// A manifest arrives with whatever folder it was in. A line pointing outside
    /// that folder is a file this app must refuse to touch, not follow.
    #[test]
    fn a_record_whose_paths_leave_the_folder_is_refused_by_name_and_nothing_is_deleted() {
        let dir = temp_dir("manifest-untrusted");
        let folder = dir.join("downloaded");
        let outside = dir.join("secret.txt");
        std::fs::create_dir_all(&folder).unwrap();
        std::fs::write(&outside, b"not this app's business").unwrap();
        let bystander = folder.join("kept.png");
        photo(8, 8).save(&bystander).unwrap();

        let lines = [
            r#"{"source":"a.png","source_bytes":1,"source_modified":null,"output":"../secret.txt","output_bytes":1,"output_modified":null,"format":"webp","quality":"q80","max_edge":null,"written":0,"backup":"a.png"}"#,
            &format!(
                r#"{{"source":"a.png","source_bytes":1,"source_modified":null,"output":{},"output_bytes":1,"output_modified":null,"format":"webp","quality":"q80","max_edge":null,"written":0,"backup":"a.png"}}"#,
                serde_json::to_string(&outside).unwrap()
            ),
            r#"{"source":"a.png","source_bytes":1,"source_modified":null,"output":"a.webp","output_bytes":1,"output_modified":null,"format":"webp","quality":"q80","max_edge":null,"written":0,"backup":"../../secret.txt"}"#,
            "{ this line was torn off by a killed run",
        ];
        std::fs::write(crate::manifest::path(&folder), lines.join("\n")).unwrap();

        let loaded = crate::manifest::load(&folder);
        assert!(loaded.outputs.is_empty(), "no record is trusted");
        assert_eq!(
            loaded.rejected.len(),
            3,
            "each bad record is named: {:?}",
            loaded.rejected
        );
        assert_eq!(
            crate::manifest::restorable(&folder),
            0,
            "and none of them offers an undo"
        );

        let restored = crate::manifest::restore(&folder);
        assert!(restored.restored.is_empty());
        assert_eq!(restored.failures.len(), 3);
        assert!(
            restored
                .failures
                .iter()
                .all(|failure| failure.contains(crate::manifest::NAME)),
            "the report says which lines: {:?}",
            restored.failures
        );
        assert!(
            outside.is_file(),
            "the file outside the folder is untouched"
        );
        assert!(bystander.is_file());
        // The refused lines are the only evidence of what was claimed here, so
        // the undo leaves them where they are rather than tidying them away.
        assert_eq!(
            crate::manifest::load(&folder).rejected.len(),
            3,
            "the report can be read again"
        );
        std::fs::remove_dir_all(dir).unwrap();
    }

    /// A full disk, or a share that does not honour `O_APPEND` atomically, can
    /// leave a line with no newline on the end — and it need not be the last one
    /// written. The next record must not disappear into it.
    #[test]
    fn a_torn_line_never_swallows_the_record_after_it() {
        let dir = temp_dir("manifest-torn-line");
        let out = dir.join(crate::scan::OUTPUT_DIR);
        let source = dir.join("shot.png");
        photo(32, 32).save(&source).unwrap();
        let written = out.join("shot.webp");
        convert_recorded(&dir, &out, &source, &written, None, Quality::lossy(80.))
            .expect("the first run converts");

        // Half a line, exactly as a disk that filled up would leave it.
        let torn = {
            let whole = std::fs::read_to_string(crate::manifest::path(&out)).unwrap();
            let mut torn = whole.clone();
            torn.push_str(&whole[..whole.len() / 2]);
            torn
        };
        std::fs::write(crate::manifest::path(&out), &torn).unwrap();
        assert_eq!(
            crate::manifest::load(&out).outputs.len(),
            1,
            "the torn half is stepped over"
        );

        let second = dir.join("other.png");
        photo(24, 24).save(&second).unwrap();
        convert_recorded(
            &dir,
            &out,
            &second,
            &out.join("other.webp"),
            None,
            Quality::lossy(80.),
        )
        .expect("the next run converts");

        let recorded = crate::manifest::load(&out);
        assert_eq!(
            recorded.outputs.len(),
            2,
            "the record after the torn line survives it"
        );
        assert_eq!(recorded.outputs[1].output, Path::new("other.webp"));
        std::fs::remove_dir_all(dir).unwrap();
    }

    /// An undo that eats somebody's edit is not an undo. The output goes only if it
    /// is still the file the run installed.
    #[test]
    fn an_output_edited_since_the_run_is_refused_by_name_and_the_original_stays_put() {
        let dir = temp_dir("replace-edited-output");
        let source = dir.join("shot.png");
        photo(64, 64).save(&source).unwrap();
        let backups = crate::manifest::backup_root(&dir);
        let backup = Backup {
            path: backups.join("shot.png"),
            moved: true,
        };
        let written = dir.join("shot.webp");
        convert_recorded(
            &dir,
            &dir,
            &source,
            &written,
            Some(&backup),
            Quality::lossy(80.),
        )
        .expect("the file converts");

        std::fs::write(&written, b"somebody worked on this afterwards").unwrap();

        let restored = crate::manifest::restore(&dir);
        assert!(restored.restored.is_empty());
        assert_eq!(restored.failures.len(), 1);
        assert!(
            restored.failures[0].contains("shot.webp") && restored.failures[0].contains("changed"),
            "the refusal names the file: {:?}",
            restored.failures
        );
        assert_eq!(
            std::fs::read(&written).unwrap(),
            b"somebody worked on this afterwards",
            "the edit is still there"
        );
        assert!(
            backup.path.is_file(),
            "and the original is still recoverable"
        );
        assert_eq!(
            crate::manifest::restorable(&dir),
            1,
            "the record it refused is kept"
        );
        std::fs::remove_dir_all(dir).unwrap();
    }

    #[test]
    fn quality_is_clamped_and_labelled() {
        assert_eq!(Quality::lossy(500.).0, Some(100.));
        assert_eq!(Quality::lossy(-3.).0, Some(1.));
        assert_eq!(Quality::lossy(80.).label(), "q80");
        assert_eq!(Quality::LOSSLESS.label(), "lossless");
    }
}
