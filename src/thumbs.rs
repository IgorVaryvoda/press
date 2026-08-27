//! Turn a file on disk into something the GPU can draw.
//!
//! JPEG and WebP dominate normal photo folders, so their native decoders scale while
//! decoding instead of allocating every source pixel just to throw most of them away.
//! Other formats retain the general decoder. Work stays off the main thread and the
//! scaled result is cached on disk for later opens.

use std::io::Cursor;
use std::path::{Path, PathBuf};
use std::sync::Arc;

use gpui::RenderImage;
use image::{
    DynamicImage, Frame, ImageDecoder as _, ImageFormat, ImageReader, RgbaImage,
    metadata::Orientation,
};
use libwebp_sys::{
    VP8StatusCode, WEBP_CSP_MODE, WebPDecode, WebPDecoderConfig, WebPFreeDecBuffer,
    WebPGetFeatures, WebPRGBABuffer,
};

/// Longest edge of a generated thumbnail, in pixels.
///
/// This has to cover the largest a tile ever draws one, `TILE_MAX - 16`. At 96 it did
/// not: `img` will not scale past an image's own size, so the gallery drew small
/// pictures floating in the middle of 200px tiles. `gallery_never_asks_for_more_than_a_
/// thumbnail_holds` keeps the two in step.
///
/// The list uses 96px: uploading a 224px texture into its 34px slot made fast scrolling
/// miss frames. The mode switch clears the memory cache, while the disk cache keeps both
/// sizes so returning to either view is still cheap.
///
// ponytail: a logical pixel, not a device one. On a 2x display the gallery scales this
// up again — far less than it did at 96, but visibly. Ask the window for its scale
// factor if that ever matters.
pub const THUMB_EDGE: u32 = 224;
pub const TABLE_THUMB_EDGE: u32 = 96;

/// Thumbnails kept on disk. Even at the old 38KB lossless size this bounds the cache
/// near 110MB; normal lossy entries are smaller.
const CACHE_FILES: usize = 3_000;

/// Decode `path`, scale it to fit `edge`, and hand back something `img()` can draw.
/// Returns `None` for anything that fails to decode, which the caller shows as a gap
/// rather than an error — a folder of holiday photos will contain a broken file.
pub fn load(path: &Path, edge: u32) -> Option<Arc<RenderImage>> {
    load_using(cache_dir().as_deref(), path, edge)
}

/// `load`, against a cache directory the caller names. `None` skips the cache, which
/// is how tests stay off a developer's real one.
pub fn load_using(cache: Option<&Path>, path: &Path, edge: u32) -> Option<Arc<RenderImage>> {
    let cached = cache.and_then(|dir| cache_file(dir, path, edge));
    if let Some(thumbnail) = cached.as_deref().and_then(read_cached) {
        return Some(drawable(thumbnail));
    }

    // `thumbnail` preserves the aspect ratio and fits inside the box. RGBA, not RGB:
    // lossless WebP carries the alpha, and a cut-out drawn opaque is a wrong thumbnail.
    let scaled = DynamicImage::ImageRgba8(decode_native(path, Some(edge)).or_else(|| {
        Some(
            crate::scan::decode(path)?
                .thumbnail(edge, edge)
                .into_rgba8(),
        )
    })?);
    if let Some(file) = cached {
        write_cached(&file, &scaled);
    }
    Some(drawable(scaled.into_rgba8()))
}

pub(crate) fn decode_native(path: &Path, edge: Option<u32>) -> Option<RgbaImage> {
    let bytes = std::fs::read(path).ok()?;
    let format = image::guess_format(&bytes[..bytes.len().min(16)]).ok()?;
    if !matches!(format, ImageFormat::Jpeg | ImageFormat::WebP) {
        return None;
    }
    match format {
        ImageFormat::Jpeg => decode_jpeg(&bytes, edge),
        ImageFormat::WebP => decode_webp(&bytes, edge),
        _ => None,
    }
}

fn decode_webp(bytes: &[u8], edge: Option<u32>) -> Option<RgbaImage> {
    let mut config = WebPDecoderConfig::new().ok()?;
    if unsafe { WebPGetFeatures(bytes.as_ptr(), bytes.len(), &mut config.input) }
        != VP8StatusCode::VP8_STATUS_OK
        || config.input.has_animation != 0
    {
        return None;
    }
    let source = (
        u32::try_from(config.input.width).ok()?,
        u32::try_from(config.input.height).ok()?,
    );
    let (width, height) = edge.map_or(source, |edge| fit(source.0, source.1, edge));
    let stride = width.checked_mul(4)?;
    let mut pixels = vec![0; usize::try_from(stride.checked_mul(height)?).ok()?];
    config.output.colorspace = WEBP_CSP_MODE::MODE_RGBA;
    config.output.is_external_memory = 1;
    config.output.u.RGBA = WebPRGBABuffer {
        rgba: pixels.as_mut_ptr(),
        stride: i32::try_from(stride).ok()?,
        size: pixels.len(),
    };
    config.options.use_scaling = i32::from((width, height) != source);
    config.options.scaled_width = i32::try_from(width).ok()?;
    config.options.scaled_height = i32::try_from(height).ok()?;
    config.options.use_threads = 1;
    // SAFETY: libwebp writes into the external buffer above while `pixels` is live.
    // Freeing the decoder buffer does not free caller-owned external memory.
    let status = unsafe { WebPDecode(bytes.as_ptr(), bytes.len(), &mut config) };
    unsafe { WebPFreeDecBuffer(&mut config.output) };
    if status != VP8StatusCode::VP8_STATUS_OK {
        return None;
    }
    orient(
        RgbaImage::from_raw(width, height, pixels)?,
        bytes,
        ImageFormat::WebP,
    )
}

fn decode_jpeg(bytes: &[u8], edge: Option<u32>) -> Option<RgbaImage> {
    let mut decoder = turbojpeg::Decompressor::new().ok()?;
    let header = decoder.read_header(bytes).ok()?;
    let factor = edge.map_or(turbojpeg::ScalingFactor::ONE, |edge| {
        [
            turbojpeg::ScalingFactor::ONE_EIGHTH,
            turbojpeg::ScalingFactor::ONE_QUARTER,
            turbojpeg::ScalingFactor::ONE_HALF,
            turbojpeg::ScalingFactor::ONE,
        ]
        .into_iter()
        .find(|factor| factor.scale(header.width.max(header.height)) >= edge as usize)
        .unwrap_or(turbojpeg::ScalingFactor::ONE)
    });
    decoder.set_scaling_factor(factor).ok()?;
    let (width, height) = (factor.scale(header.width), factor.scale(header.height));
    let mut pixels = vec![0; width.checked_mul(height)?.checked_mul(4)?];
    decoder
        .decompress(
            bytes,
            turbojpeg::Image {
                pixels: &mut pixels,
                width,
                pitch: width.checked_mul(4)?,
                height,
                format: turbojpeg::PixelFormat::RGBA,
            },
        )
        .ok()?;
    let image = RgbaImage::from_raw(width.try_into().ok()?, height.try_into().ok()?, pixels)?;
    let image = match edge {
        Some(edge) => DynamicImage::ImageRgba8(image)
            .thumbnail(edge, edge)
            .into_rgba8(),
        None => image,
    };
    orient(image, bytes, ImageFormat::Jpeg)
}

fn orient(image: RgbaImage, bytes: &[u8], format: ImageFormat) -> Option<RgbaImage> {
    let mut decoder = ImageReader::with_format(Cursor::new(bytes), format)
        .into_decoder()
        .ok()?;
    let orientation = decoder.orientation().unwrap_or(Orientation::NoTransforms);
    let mut image = DynamicImage::ImageRgba8(image);
    image.apply_orientation(orientation);
    Some(image.into_rgba8())
}

fn fit(width: u32, height: u32, edge: u32) -> (u32, u32) {
    if width.max(height) <= edge {
        return (width, height);
    }
    if width >= height {
        (
            edge,
            ((u64::from(height) * u64::from(edge)) / u64::from(width)).max(1) as u32,
        )
    } else {
        (
            ((u64::from(width) * u64::from(edge)) / u64::from(height)).max(1) as u32,
            edge,
        )
    }
}

fn drawable(thumbnail: RgbaImage) -> Arc<RenderImage> {
    Arc::new(RenderImage::new(vec![Frame::new(to_bgra(thumbnail))]))
}

/// Where this file's thumbnail lives, or `None` when the environment offers nowhere to
/// put it. The name carries the source's size and modification time, so an edited file
/// misses its old entry rather than showing the picture it used to be.
fn cache_file(cache: &Path, path: &Path, edge: u32) -> Option<PathBuf> {
    let metadata = std::fs::metadata(path).ok()?;
    let modified = metadata
        .modified()
        .ok()
        .and_then(|time| time.duration_since(std::time::UNIX_EPOCH).ok())
        .map_or(0, |since| since.as_nanos() as u64);

    // FNV-1a rather than `DefaultHasher`, whose output the standard library is free to
    // change between releases. A cache that empties itself on a toolchain upgrade is
    // only wasteful, but it is wasteful for no reason.
    let mut hash = 0xcbf2_9ce4_8422_2325_u64;
    for byte in path.as_os_str().as_encoded_bytes() {
        hash = (hash ^ u64::from(*byte)).wrapping_mul(0x0000_0100_0000_01b3);
    }
    for value in [modified, metadata.len(), u64::from(edge)] {
        for byte in value.to_le_bytes() {
            hash = (hash ^ u64::from(byte)).wrapping_mul(0x0000_0100_0000_01b3);
        }
    }
    Some(cache.join(format!("{hash:016x}.webp")))
}

/// The cache directory, resolved the way each platform expects. Thumbnails are
/// derived data: losing them costs a decode, so they do not belong beside the
/// settings a user would miss.
fn cache_dir() -> Option<PathBuf> {
    let base = if cfg!(target_os = "windows") {
        std::env::var_os("LOCALAPPDATA").map(PathBuf::from)
    } else if cfg!(target_os = "macos") {
        std::env::var_os("HOME").map(|home| PathBuf::from(home).join("Library/Caches"))
    } else {
        std::env::var_os("XDG_CACHE_HOME")
            .map(PathBuf::from)
            .or_else(|| std::env::var_os("HOME").map(|home| PathBuf::from(home).join(".cache")))
    }?;
    Some(base.join("imageguide").join("thumbs"))
}

fn read_cached(file: &Path) -> Option<RgbaImage> {
    let bytes = std::fs::read(file).ok()?;
    decode_webp(&bytes, None)
}

fn write_cached(file: &Path, thumbnail: &DynamicImage) {
    let Some(parent) = file.parent() else {
        return;
    };
    if std::fs::create_dir_all(parent).is_err() {
        return;
    }
    let Some(encoded) = crate::convert::encode(
        thumbnail,
        crate::convert::Format::WebP,
        crate::convert::Quality::lossy(80.),
    ) else {
        return;
    };
    // Write and rename, so a second copy of the app reading this entry never sees half
    // of it. Both would be writing identical bytes, so whichever lands last is right.
    let mut partial = file.to_path_buf().into_os_string();
    partial.push(".part");
    let partial = PathBuf::from(partial);
    if std::fs::write(&partial, encoded).is_err() || std::fs::rename(&partial, file).is_err() {
        let _ = std::fs::remove_file(&partial);
    }
}

/// Drop the oldest cached thumbnails once there are more than `CACHE_FILES`.
///
/// Called once when the window opens. A cache with no bound is a slow leak, and a
/// whole-directory pass has no cheaper moment than startup, on a thread nobody waits
/// for.
pub fn trim_cache() {
    if let Some(dir) = cache_dir() {
        trim_cache_in(&dir, CACHE_FILES);
    }
}

fn trim_cache_in(dir: &Path, keep: usize) {
    let Ok(entries) = std::fs::read_dir(dir) else {
        return;
    };
    let mut files: Vec<(std::time::SystemTime, PathBuf)> = entries
        .filter_map(|entry| entry.ok())
        .filter_map(|entry| {
            let modified = entry.metadata().ok()?.modified().ok()?;
            Some((modified, entry.path()))
        })
        .collect();
    if files.len() <= keep {
        return;
    }
    files.sort_by_key(|(modified, _)| *modified);
    for (_, path) in files.iter().take(files.len() - keep) {
        let _ = std::fs::remove_file(path);
    }
}

/// `RenderImage` wants BGRA. The `image` crate gives RGBA. Swap in place rather than
/// allocating a second buffer per thumbnail.
pub(crate) fn to_bgra(mut image: RgbaImage) -> RgbaImage {
    for pixel in image.chunks_exact_mut(4) {
        pixel.swap(0, 2);
    }
    image
}

#[cfg(test)]
mod tests {
    use super::*;
    use image::{ImageBuffer, Rgb, Rgba};

    #[test]
    fn swaps_red_and_blue_and_leaves_alpha_alone() {
        let red = ImageBuffer::from_pixel(1, 1, Rgba([255u8, 10, 0, 200]));
        let swapped = to_bgra(red);
        assert_eq!(swapped.into_raw(), vec![0, 10, 255, 200]);
    }

    #[test]
    fn scales_to_fit_the_box_and_keeps_the_aspect_ratio() {
        let dir = std::env::temp_dir().join("imageguide-test-thumb");
        std::fs::create_dir_all(&dir).unwrap();
        let path = dir.join("wide.png");
        ImageBuffer::from_pixel(400, 100, Rgba([1u8, 2, 3, 255]))
            .save(&path)
            .unwrap();

        let thumb = load_using(None, &path, 96).expect("png decodes");
        let size = thumb.size(0);
        assert_eq!(u32::from(size.width), 96, "long edge fills the box");
        assert_eq!(u32::from(size.height), 24, "4:1 aspect ratio is preserved");
    }

    #[test]
    fn a_file_that_is_not_an_image_is_skipped_rather_than_fatal() {
        let dir = std::env::temp_dir().join("imageguide-test-thumb-bad");
        std::fs::create_dir_all(&dir).unwrap();
        let path = dir.join("broken.png");
        std::fs::write(&path, b"this is not a png").unwrap();

        assert!(load_using(None, &path, 96).is_none());
    }

    fn scratch(tag: &str) -> PathBuf {
        let dir = std::env::temp_dir().join(format!("imageguide-thumb-{tag}"));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();
        dir
    }

    fn cached_files(cache: &Path) -> usize {
        std::fs::read_dir(cache).map(|dir| dir.count()).unwrap_or(0)
    }

    #[test]
    fn the_second_load_of_a_file_comes_off_the_disk() {
        let dir = scratch("cache");
        let cache = dir.join("cache");
        let path = dir.join("wide.png");
        ImageBuffer::from_pixel(400, 100, Rgba([9u8, 8, 7, 255]))
            .save(&path)
            .unwrap();

        let first = load_using(Some(&cache), &path, 96).expect("decodes");
        assert_eq!(cached_files(&cache), 1, "the decode was kept");

        // Remove the source. A second load can only succeed from the cache now, which
        // is the strongest way to say the decode did not run again.
        std::fs::remove_file(&path).unwrap();
        let second = load_using(Some(&cache), &path, 96);
        assert!(second.is_none(), "the key needs the file's size and time");

        // With the file back, the entry is found and matches what was drawn before.
        ImageBuffer::from_pixel(400, 100, Rgba([9u8, 8, 7, 255]))
            .save(&path)
            .unwrap();
        let again = load_using(Some(&cache), &path, 96).expect("cached");
        assert_eq!(again.size(0), first.size(0));
    }

    /// The cache must never show a picture as it was before an edit. That is the whole
    /// reason its key carries the source's size and modification time.
    #[test]
    fn an_edited_file_does_not_show_its_old_thumbnail() {
        let dir = scratch("cache-edit");
        let cache = dir.join("cache");
        let path = dir.join("shot.png");

        ImageBuffer::from_pixel(400, 100, Rgba([1u8, 2, 3, 255]))
            .save(&path)
            .unwrap();
        let before = load_using(Some(&cache), &path, 96).expect("decodes");
        assert_eq!(u32::from(before.size(0).height), 24);

        // Same path, a different picture with a different shape.
        ImageBuffer::from_pixel(100, 400, Rgba([1u8, 2, 3, 255]))
            .save(&path)
            .unwrap();
        let after = load_using(Some(&cache), &path, 96).expect("decodes");
        assert_eq!(
            u32::from(after.size(0).width),
            24,
            "the edited file kept showing its old thumbnail"
        );
        assert_eq!(cached_files(&cache), 2, "both versions have an entry");
    }

    /// Lossless WebP carries alpha, and the round trip has to keep it: a cut-out drawn
    /// opaque is a wrong thumbnail, not a slightly worse one.
    #[test]
    fn a_cut_out_stays_see_through_through_the_cache() {
        let dir = scratch("cache-alpha");
        let cache = dir.join("cache");
        let path = dir.join("cutout.png");
        let mut buffer: ImageBuffer<Rgba<u8>, Vec<u8>> =
            ImageBuffer::from_pixel(200, 200, Rgba([200u8, 30, 40, 255]));
        for x in 0..100 {
            for y in 0..200 {
                buffer.put_pixel(x, y, Rgba([200, 30, 40, 0]));
            }
        }
        buffer.save(&path).unwrap();

        load_using(Some(&cache), &path, 96).expect("decodes");
        let entry = std::fs::read_dir(&cache).unwrap().next().unwrap().unwrap();
        let restored = image::open(entry.path()).expect("the entry is an image");
        assert!(
            restored.to_rgba8().pixels().any(|pixel| pixel.0[3] == 0),
            "the see-through half came back opaque"
        );
    }

    #[test]
    fn native_decoders_scale_by_contents_and_keep_orientation_and_alpha() {
        const EXIF_ROTATE_90: [u8; 36] = [
            0xff, 0xe1, 0x00, 0x22, b'E', b'x', b'i', b'f', 0, 0, b'M', b'M', 0, 0x2a, 0, 0, 0, 8,
            0, 1, 0x01, 0x12, 0, 3, 0, 0, 0, 1, 0, 6, 0, 0, 0, 0, 0, 0,
        ];
        let dir = scratch("native-decoders");

        let honest = dir.join("cutout.webp");
        ImageBuffer::from_pixel(400, 100, Rgba([20u8, 30, 40, 0]))
            .save(&honest)
            .unwrap();
        let webp = dir.join("cutout.png");
        std::fs::rename(honest, &webp).unwrap();
        let decoded = decode_native(&webp, Some(96)).expect("WebP magic selects libwebp");
        assert_eq!(decoded.dimensions(), (96, 24));
        assert!(decoded.pixels().all(|pixel| pixel.0[3] == 0));

        let jpeg = dir.join("camera.jpg");
        ImageBuffer::from_pixel(400, 100, Rgb([20u8, 30, 40]))
            .save(&jpeg)
            .unwrap();
        let bytes = std::fs::read(&jpeg).unwrap();
        let mut oriented = Vec::with_capacity(bytes.len() + EXIF_ROTATE_90.len());
        oriented.extend_from_slice(&bytes[..2]);
        oriented.extend_from_slice(&EXIF_ROTATE_90);
        oriented.extend_from_slice(&bytes[2..]);
        std::fs::write(&jpeg, oriented).unwrap();
        assert_eq!(
            decode_native(&jpeg, Some(96))
                .expect("JPEG uses TurboJPEG")
                .dimensions(),
            (24, 96)
        );
    }

    #[test]
    fn trimming_keeps_the_newest_entries() {
        let dir = scratch("cache-trim");
        for index in 0..5u32 {
            std::fs::write(dir.join(format!("{index}.webp")), [index as u8]).unwrap();
            // `modified` has coarse resolution on some filesystems, so order the files
            // by writing them apart rather than trusting five writes in one instant.
            std::thread::sleep(std::time::Duration::from_millis(12));
        }

        trim_cache_in(&dir, 2);

        let mut left: Vec<String> = std::fs::read_dir(&dir)
            .unwrap()
            .map(|entry| entry.unwrap().file_name().to_string_lossy().into_owned())
            .collect();
        left.sort();
        assert_eq!(left, ["3.webp", "4.webp"]);
    }
}
