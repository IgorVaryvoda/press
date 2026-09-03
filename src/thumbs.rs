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

/// Thumbnails kept on disk, bounded by what they occupy rather than by how many
/// there are.
///
/// Every image can hold two entries, 96px for the list and 224px for the gallery, so
/// a 5,000-image folder wants 10,000 of them and the old 3,000-file bound threw the
/// tail of one folder away on every launch — those files were then decoded again the
/// next time it was opened. A count is also the wrong unit: an entry is a lossy WebP
/// whose size follows its picture, and the thing worth bounding is the disk it uses.
///
/// A noise photograph — the worst case, since noise does not compress — costs about
/// 11KB across both edges, so this holds roughly 24,000 images where the old bound
/// held 1,500. `the_byte_budget_holds_both_edges_of_a_five_thousand_image_folder`
/// measures a real pair of entries against this number, so the thumbnail encoder
/// cannot quietly outgrow it.
const CACHE_BYTES: u64 = 256 * 1024 * 1024;

pub(crate) struct PendingCache {
    file: PathBuf,
    thumbnail: RgbaImage,
}

pub(crate) struct LoadedThumb {
    pub(crate) image: Arc<RenderImage>,
    pub(crate) cache: Option<PendingCache>,
}

pub(crate) enum FastLoad {
    Ready(LoadedThumb),
    Fallback,
}

/// The synchronous form used by focused cache tests. Production publishes the fast
/// and fallback stages separately so cache writing cannot hold up the window.
#[cfg(test)]
fn load_using(cache: Option<&Path>, path: &Path, edge: u32) -> Option<Arc<RenderImage>> {
    let loaded = match load_fast_using(cache, path, edge, true) {
        FastLoad::Ready(loaded) => loaded,
        FastLoad::Fallback => load_fallback_using(cache, path, edge)?,
    };
    if let Some(cache) = loaded.cache {
        persist(cache);
    }
    Some(loaded.image)
}

/// Read a persistent thumbnail, or use a decoder that can scale without allocating
/// the full source. A cache hit is fast regardless of the source format.
pub(crate) fn load_fast(path: &Path, edge: u32, native_scaled: bool) -> FastLoad {
    load_fast_using(cache_dir().as_deref(), path, edge, native_scaled)
}

fn load_fast_using(cache: Option<&Path>, path: &Path, edge: u32, native_scaled: bool) -> FastLoad {
    let cached = cache.and_then(|dir| cache_file(dir, path, edge));
    if let Some(thumbnail) = cached.as_deref().and_then(read_cached) {
        return FastLoad::Ready(LoadedThumb {
            image: drawable(thumbnail),
            cache: None,
        });
    }

    if !native_scaled {
        return FastLoad::Fallback;
    }

    let Some(thumbnail) = decode_native(path, Some(edge)) else {
        return FastLoad::Fallback;
    };
    FastLoad::Ready(loaded(thumbnail, cached))
}

/// The general decoder is the expensive fallback. It runs only after the viewport
/// settles and after the persistent cache has already missed.
pub(crate) fn load_fallback(path: &Path, edge: u32) -> Option<LoadedThumb> {
    load_fallback_using(cache_dir().as_deref(), path, edge)
}

fn load_fallback_using(cache: Option<&Path>, path: &Path, edge: u32) -> Option<LoadedThumb> {
    let cached = cache.and_then(|dir| cache_file(dir, path, edge));
    if let Some(thumbnail) = cached.as_deref().and_then(read_cached) {
        return Some(LoadedThumb {
            image: drawable(thumbnail),
            cache: None,
        });
    }

    // `thumbnail` preserves the aspect ratio and fits inside the box. RGBA, not RGB:
    // lossless WebP carries the alpha, and a cut-out drawn opaque is a wrong thumbnail.
    let thumbnail = crate::scan::decode(path)?
        .thumbnail(edge, edge)
        .into_rgba8();
    Some(loaded(thumbnail, cached))
}

fn loaded(thumbnail: RgbaImage, file: Option<PathBuf>) -> LoadedThumb {
    let cache = file.map(|file| PendingCache {
        file,
        thumbnail: thumbnail.clone(),
    });
    LoadedThumb {
        image: drawable(thumbnail),
        cache,
    }
}

/// Persist after the drawable image has been handed to the UI. Cache encoding and
/// writing are useful for the next visit, but must not delay this one.
pub(crate) fn persist(cache: PendingCache) {
    write_cached(&cache.file, &DynamicImage::ImageRgba8(cache.thumbnail));
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

/// The smallest DCT scale libjpeg-turbo can finish at whose result still covers
/// `edge`, given the source's longest side. `ONE` when nothing smaller does, which is
/// also the answer for a source already inside the box.
///
/// Shared with conversion: the export path wants the same choice for the same reason,
/// and two copies of this list would drift.
pub(crate) fn jpeg_scaling_factor(longest: usize, edge: u32) -> turbojpeg::ScalingFactor {
    [
        turbojpeg::ScalingFactor::ONE_EIGHTH,
        turbojpeg::ScalingFactor::ONE_QUARTER,
        turbojpeg::ScalingFactor::ONE_HALF,
        turbojpeg::ScalingFactor::ONE,
    ]
    .into_iter()
    .find(|factor| factor.scale(longest) >= edge as usize)
    .unwrap_or(turbojpeg::ScalingFactor::ONE)
}

fn decode_jpeg(bytes: &[u8], edge: Option<u32>) -> Option<RgbaImage> {
    let mut decoder = turbojpeg::Decompressor::new().ok()?;
    let header = decoder.read_header(bytes).ok()?;
    let factor = edge.map_or(turbojpeg::ScalingFactor::ONE, |edge| {
        jpeg_scaling_factor(header.width.max(header.height), edge)
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

pub(crate) fn fit(width: u32, height: u32, edge: u32) -> (u32, u32) {
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
    let Ok(encoded) = crate::convert::encode(
        thumbnail,
        crate::convert::Format::WebP,
        crate::convert::Quality::lossy(80.),
        None,
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

/// Drop the oldest cached thumbnails until the rest fit `CACHE_BYTES`.
///
/// Called once when the window opens, from `cx.background_executor()`. A cache with
/// no bound is a slow leak, and a whole-directory pass has no cheaper moment than
/// startup, on a thread nobody waits for. The size comes off the same directory read
/// that already reports each entry's modification time, so bounding by bytes costs
/// nothing over bounding by count.
pub fn trim_cache() {
    if let Some(dir) = cache_dir() {
        trim_cache_in(&dir, CACHE_BYTES);
    }
}

fn trim_cache_in(dir: &Path, keep_bytes: u64) {
    let Ok(entries) = std::fs::read_dir(dir) else {
        return;
    };
    let mut files: Vec<(std::time::SystemTime, u64, PathBuf)> = entries
        .filter_map(|entry| entry.ok())
        .filter_map(|entry| {
            let metadata = entry.metadata().ok()?;
            Some((metadata.modified().ok()?, metadata.len(), entry.path()))
        })
        .collect();
    let mut held: u64 = files.iter().map(|(_, bytes, _)| bytes).sum();
    if held <= keep_bytes {
        return;
    }
    files.sort_by_key(|(modified, _, _)| *modified);
    for (_, bytes, path) in &files {
        if held <= keep_bytes {
            return;
        }
        if std::fs::remove_file(path).is_ok() {
            held -= bytes;
        }
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
        let dir = scratch("thumb");
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
        let dir = scratch("thumb-bad");
        let path = dir.join("broken.png");
        std::fs::write(&path, b"this is not a png").unwrap();

        assert!(load_using(None, &path, 96).is_none());
    }

    fn scratch(tag: &str) -> PathBuf {
        let dir =
            std::env::temp_dir().join(format!("imageguide-thumb-{tag}-{}", std::process::id()));
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

    #[test]
    fn a_cached_png_uses_the_fast_stage() {
        let dir = scratch("cache-fast-png");
        let cache = dir.join("cache");
        let path = dir.join("wide.png");
        ImageBuffer::from_pixel(400, 100, Rgba([9u8, 8, 7, 255]))
            .save(&path)
            .unwrap();
        load_using(Some(&cache), &path, 96).expect("initial decode");

        let FastLoad::Ready(loaded) = load_fast_using(Some(&cache), &path, 96, false) else {
            panic!("a persistent WebP hit must not inherit the PNG source delay");
        };
        assert_eq!(u32::from(loaded.image.size(0).width), 96);
        assert!(loaded.cache.is_none(), "a cache hit has nothing to rewrite");
    }

    #[test]
    fn a_new_thumbnail_is_drawable_before_its_cache_write() {
        let dir = scratch("cache-deferred-write");
        let cache = dir.join("cache");
        let path = dir.join("wide.png");
        ImageBuffer::from_pixel(400, 100, Rgba([9u8, 8, 7, 255]))
            .save(&path)
            .unwrap();

        let loaded = load_fallback_using(Some(&cache), &path, 96).expect("decodes");
        assert_eq!(u32::from(loaded.image.size(0).width), 96);
        assert_eq!(cached_files(&cache), 0, "display does not wait for disk");
        persist(loaded.cache.expect("cache write remains pending"));
        assert_eq!(cached_files(&cache), 1);
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
    fn trimming_keeps_the_newest_entries_that_fit_the_byte_budget() {
        let dir = scratch("cache-trim");
        let base = std::time::SystemTime::now();
        for index in 0..5u32 {
            let path = dir.join(format!("{index}.webp"));
            std::fs::write(&path, vec![index as u8; 100]).unwrap();
            // `modified` has coarse resolution on some filesystems, so stamp
            // each file with its own second rather than trusting five writes
            // in one instant.
            std::fs::File::options()
                .write(true)
                .open(&path)
                .expect("the fixture opens")
                .set_modified(base + std::time::Duration::from_secs(u64::from(index)))
                .expect("the mtime is set");
        }

        // 250 bytes holds two of these and not three, which a file count could not
        // have told apart.
        trim_cache_in(&dir, 250);

        let mut left: Vec<String> = std::fs::read_dir(&dir)
            .unwrap()
            .map(|entry| entry.unwrap().file_name().to_string_lossy().into_owned())
            .collect();
        left.sort();
        assert_eq!(left, ["3.webp", "4.webp"]);

        // A cache already inside the budget is left alone.
        trim_cache_in(&dir, 250);
        assert_eq!(cached_files(&dir), 2);
    }

    /// The bound has to cover the folder this app is for. A 5,000-image folder holds
    /// two entries per image, and the old 3,000-file bound dropped most of them on
    /// every launch, so the tail was decoded again on the next open. Measured against
    /// real entries rather than an assumed size, because the number that matters is
    /// what the thumbnail encoder actually writes.
    #[test]
    fn the_byte_budget_holds_both_edges_of_a_five_thousand_image_folder() {
        let dir = scratch("cache-budget");
        let cache = dir.join("cache");
        let path = dir.join("photo.png");
        // Noise, not a flat colour: a thumbnail of a real photograph does not
        // compress to nothing, and a budget proved against nothing proves nothing.
        crate::convert::tests::photo(1200, 900).save(&path).unwrap();
        for edge in [TABLE_THUMB_EDGE, THUMB_EDGE] {
            load_using(Some(&cache), &path, edge).expect("the photo decodes");
        }

        let per_image: u64 = std::fs::read_dir(&cache)
            .unwrap()
            .map(|entry| entry.unwrap().metadata().unwrap().len())
            .sum();
        assert!(per_image > 0, "no entries were written");
        assert!(
            CACHE_BYTES >= per_image * 5_000,
            "the budget holds both edges of only {} images",
            CACHE_BYTES / per_image
        );
    }
}
