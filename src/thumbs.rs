//! Turn a file on disk into something the GPU can draw.
//!
//! JPEG and WebP dominate normal photo folders, so their native decoders scale while
//! decoding instead of allocating every source pixel just to throw most of them away.
//! Other formats retain the general decoder. Work stays off the main thread and the
//! scaled result is cached on disk for later opens.
//!
//! Before any decode runs, two cheaper sources get their turn: the desktop's own
//! thumbnail store (another app has often drawn this exact file already) and the
//! sibling cache entry for the other view edge (a 224px entry downscales to 96px
//! for nearly nothing). Both feed the same disk write as a fresh decode, so the
//! saving repeats on every later open.
//!
//! Each platform reuses its own store: the freedesktop cache on Linux, Quick Look
//! on macOS, the shell thumbnail cache on Windows. All three are best-effort and
//! miss on any error, and all three validate before they draw.

use std::io::Cursor;
use std::path::{Path, PathBuf};
use std::sync::Arc;

use gpui_kit::RenderImage;
use image::{
    DynamicImage, Frame, ImageDecoder as _, ImageFormat, ImageReader, RgbaImage,
    metadata::Orientation,
};
use libwebp_sys::{
    VP8StatusCode, WEBP_CSP_MODE, WebPDecode, WebPDecoderConfig, WebPFreeDecBuffer,
    WebPGetFeatures, WebPRGBABuffer,
};
#[cfg(target_os = "windows")]
use windows_sys::Win32::{
    Foundation::{HWND, S_FALSE, S_OK, SIZE},
    Graphics::Gdi::{
        BI_RGB, BITMAP, BITMAPINFO, BITMAPINFOHEADER, DIB_RGB_COLORS, DeleteObject, GetDC,
        GetDIBits, GetObjectW, HBITMAP, HDC, HGDIOBJ, ReleaseDC,
    },
    System::Com::{COINIT_MULTITHREADED, CoInitializeEx, CoUninitialize},
    UI::Shell::{SHCreateItemFromParsingName, SIIGBF_INCACHEONLY, SIIGBF_THUMBNAILONLY},
};
#[cfg(target_os = "windows")]
use windows_sys::core::{GUID, HRESULT, PCWSTR};

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
    if let Some(thumbnail) =
        read_os_thumb(path, edge).or_else(|| read_other_edge(cache, path, edge))
    {
        return FastLoad::Ready(loaded(thumbnail, cached));
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
    if let Some(thumbnail) =
        read_os_thumb(path, edge).or_else(|| read_other_edge(cache, path, edge))
    {
        return Some(loaded(thumbnail, cached));
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
    // A camera JPEG often carries a small preview of itself up front. Decoding
    // that instead of the full frame skips the Huffman pass over tens of
    // megapixels; at thumb size nobody can tell the pixels apart.
    if let Some(edge) = edge
        && let Some(preview) = decode_embedded_preview(bytes, edge)
    {
        return orient(preview, bytes, ImageFormat::Jpeg);
    }
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

/// A downscaled copy of the sibling edge entry. The gallery entry covers the
/// list size, so switching views reuses pixels instead of decoding again.
/// Only downscales: a 96px entry blown up to 224px would look worse than the
/// decode it replaces.
fn read_other_edge(cache: Option<&Path>, path: &Path, edge: u32) -> Option<RgbaImage> {
    let dir = cache?;
    let other = if edge == TABLE_THUMB_EDGE {
        THUMB_EDGE
    } else {
        return None;
    };
    let thumbnail = cache_file(dir, path, other)
        .as_deref()
        .and_then(read_cached)?;
    Some(
        DynamicImage::ImageRgba8(thumbnail)
            .thumbnail(edge, edge)
            .into_rgba8(),
    )
}

#[cfg(target_os = "linux")]
/// The desktop thumbnail store's file URI for `path`. Percent-encoding follows
/// the file URI rule: unreserved bytes pass through, all else escapes as
/// uppercase hex. Only ASCII paths need this to be exact, and those are the
/// ones photo folders hold.
fn os_thumb_uri(path: &Path) -> Option<String> {
    let absolute = std::fs::canonicalize(path)
        .ok()
        .filter(|path| path.is_absolute())
        .or_else(|| {
            let mut base = std::env::current_dir().ok()?;
            base.push(path);
            Some(base)
        })?;
    let mut uri = String::from("file://");
    for byte in absolute.as_os_str().as_encoded_bytes() {
        match byte {
            b'/' | b'-' | b'.' | b'_' | b'~' | b'0'..=b'9' | b'A'..=b'Z' | b'a'..=b'z' => {
                uri.push(*byte as char);
            }
            _ => uri.push_str(&format!("%{byte:02X}")),
        }
    }
    Some(uri)
}

#[cfg(target_os = "linux")]
/// Candidate store entries, largest first. A 256px store entry covers both of
/// this app's edges; a 128px one covers the list but never feeds the gallery.
fn os_thumb_candidates(path: &Path) -> Vec<PathBuf> {
    let Some(uri) = os_thumb_uri(path) else {
        return Vec::new();
    };
    let digest = md5::compute(uri.as_bytes());
    let name = format!("{digest:x}.png");
    let Some(mut base) = std::env::var_os("XDG_CACHE_HOME")
        .map(PathBuf::from)
        .or_else(|| std::env::var_os("HOME").map(|home| PathBuf::from(home).join(".cache")))
    else {
        return Vec::new();
    };
    base.push("thumbnails");
    let sizes: &[&str] = if TABLE_THUMB_EDGE > 128 {
        &["x-large", "large", "normal"]
    } else {
        &["large", "normal", "x-large"]
    };
    // `x-large` exists on newer desktops only; missing folders simply miss.
    sizes
        .iter()
        .map(|size| base.join(size).join(&name))
        .collect()
}

#[cfg(target_os = "linux")]
/// The `Thumb::URI` and `Thumb::MTime` text entries inside a store PNG.
/// Plain pairs, in order: the store writes few, and the caller wants at most two.
fn png_text_entries(bytes: &[u8]) -> Vec<(String, String)> {
    const SIGNATURE: [u8; 8] = [137, 80, 78, 71, 13, 10, 26, 10];
    if bytes.len() < 8 || bytes[..8] != SIGNATURE {
        return Vec::new();
    }
    let mut entries = Vec::new();
    let mut offset = 8;
    while offset + 8 <= bytes.len() {
        let length =
            u32::from_be_bytes(bytes[offset..offset + 4].try_into().unwrap_or([0; 4])) as usize;
        let kind = &bytes[offset + 4..offset + 8];
        offset += 8;
        if offset + length + 4 > bytes.len() {
            return entries;
        }
        if kind == b"tEXt"
            && let Some(split) = bytes[offset..offset + length]
                .iter()
                .position(|byte| *byte == 0)
        {
            let keyword = String::from_utf8_lossy(&bytes[offset..offset + split]).into_owned();
            let value =
                String::from_utf8_lossy(&bytes[offset + split + 1..offset + length]).into_owned();
            entries.push((keyword, value));
        }
        offset += length + 4;
        if kind == b"IEND" {
            break;
        }
    }
    entries
}

/// A thumbnail the desktop already drew for this file, from whichever store the
/// platform keeps. Best-effort: any failure is a miss, and the decode runs.
fn read_os_thumb(path: &Path, edge: u32) -> Option<RgbaImage> {
    #[cfg(target_os = "linux")]
    {
        read_freedesktop_thumb(path, edge)
    }
    #[cfg(target_os = "macos")]
    {
        read_quicklook_thumb(path, edge)
    }
    #[cfg(target_os = "windows")]
    {
        read_shell_thumb(path, edge)
    }
    #[cfg(not(any(target_os = "linux", target_os = "macos", target_os = "windows")))]
    {
        let _ = (path, edge);
        None
    }
}

/// JPEG and WebP own their fast decoders, so the OS stores never see them: a
/// process spawn or COM round trip cannot beat the scaled native decode, and
/// trying would only slow the format that needs no help.
#[cfg(any(target_os = "macos", target_os = "windows"))]
fn os_store_skips(path: &Path) -> bool {
    use std::io::Read;
    let mut head = [0u8; 16];
    let sniffed = std::fs::File::open(path)
        .and_then(|mut file| file.read_exact(&mut head).map(|_| head))
        .ok();
    sniffed.is_some_and(|head| {
        image::guess_format(&head)
            .is_ok_and(|format| matches!(format, ImageFormat::Jpeg | ImageFormat::WebP))
    })
}

/// The freedesktop entry for this file. Validated against the source's
/// modification time, so an edited photo never wears its old face. Accepted
/// without store metadata only when the entry itself is newer than the source.
/// Returns pixels at `edge`, scaled down from the store size.
#[cfg(target_os = "linux")]
fn read_freedesktop_thumb(path: &Path, edge: u32) -> Option<RgbaImage> {
    let metadata = std::fs::metadata(path).ok()?;
    let modified = metadata
        .modified()
        .ok()
        .and_then(|time| time.duration_since(std::time::UNIX_EPOCH).ok())
        .map(|since| since.as_secs())
        .unwrap_or(0);
    let uri = os_thumb_uri(path);
    for candidate in os_thumb_candidates(path) {
        let bytes = std::fs::read(&candidate).ok()?;
        let entries = png_text_entries(&bytes);
        let stored_uri = entries
            .iter()
            .find(|(keyword, _)| keyword == "Thumb::URI")
            .map(|(_, value)| value.as_str());
        let stored_time = entries
            .iter()
            .find(|(keyword, _)| keyword == "Thumb::MTime")
            .and_then(|(_, value)| value.parse::<u64>().ok());
        match (stored_uri, stored_time, uri.as_deref()) {
            (Some(stored), Some(when), _) if Some(stored) != uri.as_deref() || when != modified => {
                continue;
            }
            (Some(_), None, _) | (None, Some(_), _) => continue,
            (None, None, _) => {
                let fresh = std::fs::metadata(&candidate)
                    .ok()
                    .and_then(|meta| meta.modified().ok())
                    .and_then(|time| time.duration_since(std::time::UNIX_EPOCH).ok())
                    .map(|since| since.as_secs())
                    .unwrap_or(0);
                if fresh < modified {
                    continue;
                }
            }
            _ => {}
        }
        let decoded = image::load_from_memory(&bytes).ok()?;
        // A store entry smaller than the box is another app's small idea of
        // this file, not a source: decoding the file itself stays truer.
        if decoded.width().max(decoded.height()) < edge {
            continue;
        }
        return Some(decoded.thumbnail(edge, edge).into_rgba8());
    }
    None
}

/// The `qlmanage` tool, looked up once. It lives in `/usr/bin` on every macOS,
/// but the lookup still goes through `PATH` so a missing tool is a miss rather
/// than a spawn error on every thumb.
#[cfg(target_os = "macos")]
fn quicklook_tool() -> Option<std::path::PathBuf> {
    static TOOL: std::sync::OnceLock<Option<std::path::PathBuf>> = std::sync::OnceLock::new();
    TOOL.get_or_init(|| {
        std::env::var_os("PATH").and_then(|paths| {
            std::env::split_paths(&paths)
                .map(|dir| dir.join("qlmanage"))
                .find(|tool| {
                    std::fs::metadata(tool).is_ok_and(|meta| !meta.is_dir()) && is_executable(tool)
                })
        })
    })
    .clone()
}

/// Executable bit set for the owner, group or world. `qlmanage` is world
/// executable, so any set bit counts.
#[cfg(target_os = "macos")]
fn is_executable(tool: &Path) -> bool {
    use std::os::unix::fs::PermissionsExt;
    std::fs::metadata(tool)
        .map(|meta| meta.permissions().mode() & 0o111 != 0)
        .unwrap_or(false)
}

/// A thumbnail Quick Look draws for this file. Unlike the Linux store this
/// generates rather than reuses: macOS keeps no readable cache, so the tool is
/// the cache. It runs only for formats the native decoders do not own, with a
/// bounded wait on the slow worker, and anything it draws feeds the Press disk
/// cache like any other decode.
#[cfg(target_os = "macos")]
fn read_quicklook_thumb(path: &Path, edge: u32) -> Option<RgbaImage> {
    if os_store_skips(path) {
        return None;
    }
    let tool = quicklook_tool()?;
    static NEXT_ID: std::sync::atomic::AtomicU64 = std::sync::atomic::AtomicU64::new(0);
    let id = NEXT_ID.fetch_add(1, std::sync::atomic::Ordering::Relaxed);
    let out = std::env::temp_dir().join(format!("press-ql-{}-{id}", std::process::id()));
    std::fs::create_dir_all(&out).ok()?;
    let size = edge.max(512).to_string();
    let mut child = std::process::Command::new(&tool)
        .args(["-t", "-s", &size, "-o"])
        .arg(&out)
        .arg(path)
        .stdin(std::process::Stdio::null())
        .stdout(std::process::Stdio::null())
        .stderr(std::process::Stdio::null())
        .spawn()
        .ok()?;
    // Five seconds for one thumb: a stuck generator must never hold the slow
    // worker past the next viewport.
    let mut waited = 0;
    let status = loop {
        if let Some(status) = child.try_wait().ok().flatten() {
            break Some(status);
        }
        if waited >= 100 {
            let _ = child.kill();
            let _ = child.wait();
            break None;
        }
        std::thread::sleep(std::time::Duration::from_millis(50));
        waited += 1;
    };
    let ok = status.is_some_and(|status| status.success());
    // `qlmanage` names its output after the source with `.png` added, but the
    // lookup takes whatever single PNG it left: the name is a detail, the
    // pixels are the contract.
    let drawn = ok.then(|| {
        std::fs::read_dir(&out)
            .ok()?
            .filter_map(|entry| entry.ok())
            .find_map(|entry| {
                let file = entry.path();
                (file.extension() == Some(std::ffi::OsStr::new("png"))
                    && entry.metadata().is_ok_and(|meta| meta.is_file()))
                .then(|| std::fs::read(&file).ok())
                .flatten()
            })
    });
    let _ = std::fs::remove_dir_all(&out);
    let bytes = drawn??;
    let decoded = image::load_from_memory(&bytes).ok()?;
    if decoded.width().max(decoded.height()) == 0 {
        return None;
    }
    Some(decoded.thumbnail(edge, edge).into_rgba8())
}

/// The shell thumbnail cache, read-only. `INCACHEONLY` asks only for what
/// Explorer already drew: a generation would block the slow worker on another
/// app's decoder, while a hit is just a small bitmap copy. Anything the cache
/// does not hold falls through to the general decode.
#[cfg(target_os = "windows")]
fn read_shell_thumb(path: &Path, edge: u32) -> Option<RgbaImage> {
    if os_store_skips(path) {
        return None;
    }
    // SAFETY: every raw pointer below stays inside this call. COM references
    // release before return on all paths, the bitmap deletes after its pixels
    // are copied out, and a miss never draws.
    unsafe { shell_thumb_cached(path, edge) }
}

/// `IShellItemImageFactory`, hand-declared. `windows-sys` ships the shell
/// functions but not this interface, and a tiny local vtable beats a new
/// wrapper crate for one method past `IUnknown`.
#[cfg(target_os = "windows")]
#[repr(C)]
struct FactoryVtbl {
    query: unsafe extern "system" fn(
        *mut core::ffi::c_void,
        *const GUID,
        *mut *mut core::ffi::c_void,
    ) -> HRESULT,
    add: unsafe extern "system" fn(*mut core::ffi::c_void) -> u32,
    release: unsafe extern "system" fn(*mut core::ffi::c_void) -> u32,
    image: unsafe extern "system" fn(
        *mut core::ffi::c_void,
        HWND,
        *const SIZE,
        i32,
        *mut HBITMAP,
    ) -> HRESULT,
}

#[cfg(target_os = "windows")]
const FACTORY_ID: GUID = GUID {
    data1: 0xbcc1_8b79,
    data2: 0xba16,
    data3: 0x442f,
    data4: [0x80, 0xc4, 0x19, 0xa5, 0xea, 0xad, 0x13, 0x2b],
};

#[cfg(target_os = "windows")]
unsafe fn shell_thumb_cached(path: &Path, edge: u32) -> Option<RgbaImage> {
    use std::os::windows::ffi::OsStrExt;
    let mut wide: Vec<u16> = path.as_os_str().encode_wide().collect();
    wide.push(0);
    // Each successful init balances with one uninit, including `S_FALSE`
    // (already initialized on this thread).
    let init = CoInitializeEx(std::ptr::null(), COINIT_MULTITHREADED as u32);
    if init != S_OK && init != S_FALSE {
        return None;
    }
    let thumb = shell_thumb_inner(&wide, edge);
    CoUninitialize();
    thumb
}

#[cfg(target_os = "windows")]
unsafe fn shell_thumb_inner(wide: &[u16], edge: u32) -> Option<RgbaImage> {
    let mut factory: *mut core::ffi::c_void = std::ptr::null_mut();
    let status = SHCreateItemFromParsingName(
        wide.as_ptr() as PCWSTR,
        std::ptr::null_mut(),
        &FACTORY_ID,
        &mut factory,
    );
    if status != S_OK || factory.is_null() {
        return None;
    }
    let vtbl = *(factory as *const *const FactoryVtbl);
    let size = SIZE {
        cx: edge as i32,
        cy: edge as i32,
    };
    let mut bitmap: HBITMAP = std::ptr::null_mut();
    let flags = SIIGBF_THUMBNAILONLY | SIIGBF_INCACHEONLY;
    let status = ((*vtbl).image)(factory, std::ptr::null_mut(), &size, flags, &mut bitmap);
    ((*vtbl).release)(factory);
    if status != S_OK || bitmap.is_null() {
        return None;
    }
    let image = bitmap_pixels(bitmap, edge);
    DeleteObject(bitmap as HGDIOBJ);
    image
}

/// Pixels out of a shell bitmap. GDI hands back BGRA; the same swap `to_bgra`
/// performs turns it into the RGBA the cache round trip expects.
#[cfg(target_os = "windows")]
unsafe fn bitmap_pixels(bitmap: HBITMAP, edge: u32) -> Option<RgbaImage> {
    let mut info: BITMAP = std::mem::zeroed();
    if GetObjectW(
        bitmap as HGDIOBJ,
        std::mem::size_of::<BITMAP>() as i32,
        &mut info as *mut BITMAP as *mut core::ffi::c_void,
    ) == 0
    {
        return None;
    }
    if info.bmWidth <= 0
        || info.bmHeight <= 0
        || info.bmWidth > 4096
        || info.bmHeight > 4096
        || (info.bmBitsPixel != 32 && info.bmBitsPixel != 24)
    {
        return None;
    }
    let (width, height) = (info.bmWidth as u32, info.bmHeight as u32);
    let screen: HDC = GetDC(std::ptr::null_mut());
    if screen.is_null() {
        return None;
    }
    let mut format: BITMAPINFO = std::mem::zeroed();
    format.bmiHeader.biSize = std::mem::size_of::<BITMAPINFOHEADER>() as u32;
    format.bmiHeader.biWidth = info.bmWidth;
    format.bmiHeader.biHeight = -info.bmHeight;
    format.bmiHeader.biPlanes = 1;
    format.bmiHeader.biBitCount = 32;
    format.bmiHeader.biCompression = BI_RGB;
    let mut pixels = vec![0u8; width as usize * height as usize * 4];
    let lines = GetDIBits(
        screen,
        bitmap,
        0,
        height,
        pixels.as_mut_ptr() as *mut core::ffi::c_void,
        &mut format,
        DIB_RGB_COLORS,
    );
    ReleaseDC(std::ptr::null_mut(), screen);
    if lines != height as i32 {
        return None;
    }
    for pixel in pixels.chunks_exact_mut(4) {
        pixel.swap(0, 2);
    }
    let image = RgbaImage::from_raw(width, height, pixels)?;
    Some(
        DynamicImage::ImageRgba8(image)
            .thumbnail(edge, edge)
            .into_rgba8(),
    )
}

/// The small preview JPEG cameras embed up front (EXIF tag 0x0201 in IFD1).
/// Offsets count from the TIFF header inside APP1, so the base travels with
/// the parse. `None` for files without one, which is most of the web.
fn decode_embedded_preview(bytes: &[u8], edge: u32) -> Option<RgbaImage> {
    let segment = exif_segment(bytes)?;
    let thumbnail = exif_thumbnail(&segment)?;
    if thumbnail.len() < 64 {
        return None;
    }
    // The preview is already small; a scaled DCT decode keeps it that way.
    // A corrupt preview must never fail the thumb: the caller falls through
    // to the full decode.
    let preview = decode_jpeg_bytes(&thumbnail, Some(edge)).or_else(|| {
        image::load_from_memory(&thumbnail)
            .ok()
            .map(|image| image.thumbnail(edge, edge).into_rgba8())
    })?;
    (preview.width().max(preview.height()) >= edge / 2).then_some(preview)
}

/// The APP1 segment body holding `Exif\0\0`, if the file carries one.
fn exif_segment(bytes: &[u8]) -> Option<Vec<u8>> {
    if bytes.len() < 4 || bytes[0..2] != [0xFF, 0xD8] {
        return None;
    }
    let mut offset = 2;
    while offset + 4 <= bytes.len() {
        if bytes[offset] != 0xFF {
            return None;
        }
        let marker = bytes[offset + 1];
        // Standalone markers carry no length: SOI, EOI, RSTn, TEM.
        if marker == 0xD8 || marker == 0xD9 || (0xD0..=0xD7).contains(&marker) || marker == 0x01 {
            offset += 2;
            continue;
        }
        let length = u16::from_be_bytes([bytes[offset + 2], bytes[offset + 3]]) as usize;
        if length < 2 || offset + 2 + length > bytes.len() {
            return None;
        }
        if marker == 0xE1 {
            let body = &bytes[offset + 4..offset + 2 + length];
            if body.starts_with(b"Exif\0\0") {
                return Some(body[6..].to_vec());
            }
        }
        if marker == 0xDA {
            return None;
        }
        offset += 2 + length;
    }
    None
}

/// Byte order inside the TIFF header.
#[derive(Clone, Copy)]
enum TiffOrder {
    Little,
    Big,
}

fn tiff_u16(tiff: &[u8], offset: usize, order: TiffOrder) -> Option<u16> {
    let pair: [u8; 2] = tiff.get(offset..offset + 2)?.try_into().ok()?;
    Some(match order {
        TiffOrder::Little => u16::from_le_bytes(pair),
        TiffOrder::Big => u16::from_be_bytes(pair),
    })
}

fn tiff_u32(tiff: &[u8], offset: usize, order: TiffOrder) -> Option<u32> {
    let quad: [u8; 4] = tiff.get(offset..offset + 4)?.try_into().ok()?;
    Some(match order {
        TiffOrder::Little => u32::from_le_bytes(quad),
        TiffOrder::Big => u32::from_be_bytes(quad),
    })
}

/// The value of one LONG entry: inline when the count is one, an offset into
/// the TIFF body otherwise.
fn tiff_long(tiff: &[u8], entry: usize, order: TiffOrder) -> Option<u32> {
    let kind = tiff_u16(tiff, entry + 2, order)?;
    let count = tiff_u32(tiff, entry + 4, order)?;
    if kind != 4 || count == 0 {
        return None;
    }
    if count == 1 {
        return tiff_u32(tiff, entry + 8, order);
    }
    let at = tiff_u32(tiff, entry + 8, order)? as usize;
    tiff_u32(tiff, at, order)
}

/// Offset and length of the IFD1 thumbnail, read off the directory chain.
fn exif_thumbnail(tiff: &[u8]) -> Option<Vec<u8>> {
    if tiff.len() < 8 {
        return None;
    }
    let order = match &tiff[0..4] {
        [0x49, 0x49, 0x2A, 0x00] => TiffOrder::Little,
        [0x4D, 0x4D, 0x00, 0x2A] => TiffOrder::Big,
        _ => return None,
    };
    let mut directory = tiff_u32(tiff, 4, order)? as usize;
    for _ in 0..2 {
        let count = tiff_u16(tiff, directory, order)? as usize;
        if directory + 2 + count * 12 + 4 > tiff.len() {
            return None;
        }
        if directory == tiff_u32(tiff, 4, order)? as usize {
            // IFD0's own offset lives here; IFD1 follows its entries.
            directory = tiff_u32(tiff, directory + 2 + count * 12, order)? as usize;
            continue;
        }
        let mut offset = None;
        let mut length = None;
        for index in 0..count {
            let entry = directory + 2 + index * 12;
            match tiff_u16(tiff, entry, order)? {
                0x0201 => offset = tiff_long(tiff, entry, order),
                0x0202 => length = tiff_long(tiff, entry, order),
                _ => {}
            }
        }
        let (offset, length) = (offset? as usize, length? as usize);
        if length == 0 || offset + length > tiff.len() {
            return None;
        }
        return Some(tiff[offset..offset + length].to_vec());
    }
    None
}

/// A JPEG decode shared by the full frame and the embedded preview, without
/// the preview shortcut itself.
fn decode_jpeg_bytes(bytes: &[u8], edge: Option<u32>) -> Option<RgbaImage> {
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
    Some(match edge {
        Some(edge) => DynamicImage::ImageRgba8(image)
            .thumbnail(edge, edge)
            .into_rgba8(),
        None => image,
    })
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

    fn with_thumb_home(tag: &str) -> (PathBuf, Option<std::ffi::OsString>) {
        let dir = scratch(tag);
        let home = dir.join("home");
        std::fs::create_dir_all(&home).unwrap();
        let previous = std::env::var_os("XDG_CACHE_HOME");
        // `set_var` is `unsafe` on this toolchain: the test harness owns the
        // process, and no other thread reads this variable without holding the
        // same fixture path.
        unsafe { std::env::set_var("XDG_CACHE_HOME", home.join("cache")) };
        (dir, previous)
    }

    fn restore_thumb_home(previous: Option<std::ffi::OsString>) {
        unsafe {
            match previous {
                Some(value) => std::env::set_var("XDG_CACHE_HOME", value),
                None => std::env::remove_var("XDG_CACHE_HOME"),
            }
        }
    }

    /// Another app often draws the same file first. Dolphin writes hundreds of
    /// store entries while Press requests only its viewport; reusing them skips
    /// the decode entirely.
    #[test]
    fn an_os_thumbnail_feeds_a_file_another_app_already_drew() {
        if !cfg!(target_os = "linux") {
            return;
        }
        let (dir, previous) = with_thumb_home("os-thumb");
        let path = dir.join("photo.jpg");
        crate::convert::tests::photo(800, 600).save(&path).unwrap();
        // Age the source so the fresh store entry below counts as newer.
        let old = std::time::SystemTime::now() - std::time::Duration::from_secs(3600);
        std::fs::File::options()
            .write(true)
            .open(&path)
            .expect("the fixture opens")
            .set_modified(old)
            .expect("the mtime is set");

        let candidates = os_thumb_candidates(&path);
        assert!(!candidates.is_empty(), "the store names this file");
        let first = &candidates[0];
        std::fs::create_dir_all(first.parent().unwrap()).unwrap();
        crate::convert::tests::photo(256, 192).save(first).unwrap();

        let found = read_os_thumb(&path, TABLE_THUMB_EDGE).expect("the store hits");
        assert_eq!(found.dimensions(), (96, 72));
        restore_thumb_home(previous);
    }

    /// A store entry older than the source is the photo as it was, not as it
    /// is. It must miss so the file decodes again.
    #[test]
    fn a_stale_os_entry_never_wears_an_old_face() {
        if !cfg!(target_os = "linux") {
            return;
        }
        let (dir, previous) = with_thumb_home("os-thumb-stale");
        let path = dir.join("photo.jpg");
        crate::convert::tests::photo(400, 300).save(&path).unwrap();
        let candidates = os_thumb_candidates(&path);
        let first = &candidates[0];
        std::fs::create_dir_all(first.parent().unwrap()).unwrap();
        crate::convert::tests::photo(256, 192).save(first).unwrap();
        // Backdate the entry below the source: the source is newer.
        let old = std::time::SystemTime::now() - std::time::Duration::from_secs(3600);
        std::fs::File::options()
            .write(true)
            .open(first)
            .expect("the fixture opens")
            .set_modified(old)
            .expect("the mtime is set");

        assert!(
            read_os_thumb(&path, TABLE_THUMB_EDGE).is_none(),
            "the stale entry must miss"
        );
        restore_thumb_home(previous);
    }

    /// Switching views must not decode again: the 224px entry downscales to
    /// the 96px list size for nearly nothing.
    #[test]
    fn the_list_edge_reuses_the_gallery_entry() {
        let dir = scratch("cross-edge");
        let cache = dir.join("cache");
        let path = dir.join("photo.png");
        crate::convert::tests::photo(800, 600).save(&path).unwrap();
        load_using(Some(&cache), &path, THUMB_EDGE).expect("the gallery decodes");

        let reused = read_other_edge(Some(&cache), &path, TABLE_THUMB_EDGE)
            .expect("the gallery entry feeds the list");
        assert_eq!(reused.dimensions(), (96, 72));
    }

    /// Most web images carry no preview. The lookup must say so fast and let
    /// the normal decode run.
    #[test]
    fn a_jpeg_without_a_preview_falls_through() {
        let dir = scratch("no-preview");
        let path = dir.join("plain.jpg");
        crate::convert::tests::photo(400, 300).save(&path).unwrap();
        let bytes = std::fs::read(&path).unwrap();
        assert!(
            decode_embedded_preview(&bytes, TABLE_THUMB_EDGE).is_none(),
            "an encoder-written JPEG holds no EXIF preview"
        );
    }

    /// Camera files hold a small JPEG up front. Decoding it skips the Huffman
    /// pass over the full frame.
    #[test]
    fn an_embedded_preview_decodes_at_thumb_size() {
        let mut thumb_jpeg = Vec::new();
        crate::convert::tests::photo(160, 120)
            .write_to(
                &mut std::io::Cursor::new(&mut thumb_jpeg),
                image::ImageFormat::Jpeg,
            )
            .unwrap();
        let length = thumb_jpeg.len() as u32;
        // Minimal TIFF: empty IFD0 that points at an IFD1 with the two tags a
        // preview needs, then the preview bytes themselves.
        let mut tiff = Vec::new();
        tiff.extend_from_slice(b"II\x2a\x00\x08\x00\x00\x00");
        tiff.extend_from_slice(&[0x00, 0x00]);
        tiff.extend_from_slice(&14u32.to_le_bytes());
        tiff.extend_from_slice(&[0x02, 0x00]);
        tiff.extend_from_slice(&[0x01, 0x02, 0x04, 0x00, 0x01, 0x00, 0x00, 0x00]);
        tiff.extend_from_slice(&44u32.to_le_bytes());
        tiff.extend_from_slice(&[0x02, 0x02, 0x04, 0x00, 0x01, 0x00, 0x00, 0x00]);
        tiff.extend_from_slice(&length.to_le_bytes());
        tiff.extend_from_slice(&[0x00, 0x00, 0x00, 0x00]);
        tiff.extend_from_slice(&thumb_jpeg);
        let mut bytes = vec![0xFF, 0xD8, 0xFF, 0xE1];
        let segment = (tiff.len() + 8) as u16;
        bytes.extend_from_slice(&segment.to_be_bytes());
        bytes.extend_from_slice(b"Exif\0\0");
        bytes.extend_from_slice(&tiff);
        bytes.extend_from_slice(&[0xFF, 0xD9]);

        let preview =
            decode_embedded_preview(&bytes, TABLE_THUMB_EDGE).expect("the preview decodes");
        assert_eq!(preview.dimensions(), (96, 72));
    }

    /// Times each thumb source against a real folder. Ignored by default: it
    /// needs a folder of real photos and prints timings instead of asserting
    /// them. Run it in release mode, or debug overhead drowns every stage:
    ///
    /// `PRESS_BENCH_FOLDER=~/Pictures cargo test --release --locked -- --ignored --nocapture bench_thumb_sources`
    #[test]
    #[ignore]
    fn bench_thumb_sources() {
        let root = std::env::var_os("PRESS_BENCH_FOLDER")
            .map(PathBuf::from)
            .expect("set PRESS_BENCH_FOLDER to a folder of real photos");
        let mut photos: Vec<PathBuf> = walkdir::WalkDir::new(&root)
            .follow_links(false)
            .into_iter()
            .filter_map(|entry| entry.ok())
            .map(|entry| entry.into_path())
            .filter(|path| {
                path.extension()
                    .and_then(|extension| extension.to_str())
                    .is_some_and(|extension| {
                        matches!(
                            extension.to_ascii_lowercase().as_str(),
                            "jpg" | "jpeg" | "png" | "webp"
                        )
                    })
            })
            .collect();
        photos.sort();
        photos.truncate(160);
        assert!(
            photos.len() >= 20,
            "the bench needs at least 20 photos, found {}",
            photos.len()
        );

        let dir = scratch("bench");
        let cold_cache = dir.join("cold");
        // Stage 1: cold. Empty Press cache, empty OS store: every file decodes.
        // `with_thumb_home` points the store at a scratch home; the empty home
        // below keeps stage 1 honest.
        let (_home_dir, previous) = with_thumb_home("bench");
        let empty_home = dir.join("empty-home");
        std::fs::create_dir_all(&empty_home).unwrap();
        // SAFETY: same rule as the helper above — the harness owns the process.
        unsafe { std::env::set_var("XDG_CACHE_HOME", &empty_home) };
        let timed = time_stage(&photos, &cold_cache, "cold full decode");
        let decodable = timed;
        assert!(
            !decodable.is_empty(),
            "no photo in {root:?} decoded, the bench measures nothing"
        );

        // Stage 2: Press cache warm. Same folder, same cache: every file hits.
        let (total, mean) = stage(&decodable, |path| {
            matches!(
                load_fast_using(Some(&cold_cache), path, TABLE_THUMB_EDGE, true),
                FastLoad::Ready(_)
            )
        });
        report("press cache hit", decodable.len(), total, mean);

        // Stage 3: OS store. Fresh Press cache, planted store entries at 256px:
        // this is the folder after Dolphin looked at it. The planted home is
        // the scratch home `with_thumb_home` already installed.
        let planted_home = dir.join("home").join("cache");
        // SAFETY: same rule as the helper above — the harness owns the process.
        unsafe { std::env::set_var("XDG_CACHE_HOME", &planted_home) };
        let mut planted = 0;
        for path in &decodable {
            if load_fallback_using(None, path, 256).is_none() {
                continue;
            }
            for candidate in os_thumb_candidates(path).into_iter().take(1) {
                if std::fs::create_dir_all(candidate.parent().unwrap()).is_ok()
                    && image::open(path)
                        .map(|image| image.thumbnail(256, 256))
                        .and_then(|small| small.save(&candidate))
                        .is_ok()
                {
                    planted += 1;
                    break;
                }
            }
        }
        let store_cache = dir.join("store");
        let (total, mean) = stage(&decodable, |path| {
            match load_fast_using(Some(&store_cache), path, TABLE_THUMB_EDGE, true) {
                FastLoad::Ready(loaded) => {
                    if let Some(cache) = loaded.cache {
                        persist(cache);
                    }
                    true
                }
                FastLoad::Fallback => false,
            }
        });
        println!("planted {planted} of {} store entries", decodable.len());
        report("os store hit", decodable.len(), total, mean);

        // Stage 4: cross-edge. Only 224px entries primed, the list asks for 96.
        let cross_cache = dir.join("cross");
        for path in &decodable {
            if let Some(loaded) = load_fallback_using(Some(&cross_cache), path, THUMB_EDGE)
                && let Some(cache) = loaded.cache
            {
                persist(cache);
            }
        }
        let (total, mean) = stage(&decodable, |path| {
            matches!(
                load_fast_using(Some(&cross_cache), path, TABLE_THUMB_EDGE, true),
                FastLoad::Ready(_)
            )
        });
        report("cross-edge 224 to 96", decodable.len(), total, mean);

        // Stage 5: embedded preview against the full decode, on files that
        // actually carry one.
        let previewed: Vec<Vec<u8>> = decodable
            .iter()
            .filter_map(|path| std::fs::read(path).ok())
            .filter(|bytes| exif_thumbnail(&exif_segment(bytes).unwrap_or_default()).is_some())
            .collect();
        if previewed.is_empty() {
            println!("no embedded previews in this folder, stage skipped");
        } else {
            let start = std::time::Instant::now();
            for bytes in &previewed {
                assert!(decode_embedded_preview(bytes, TABLE_THUMB_EDGE).is_some());
            }
            let preview_total = start.elapsed();
            let start = std::time::Instant::now();
            for bytes in &previewed {
                assert!(decode_jpeg_bytes(bytes, Some(TABLE_THUMB_EDGE)).is_some());
            }
            let full_total = start.elapsed();
            println!(
                "embedded preview over {} files: {:?} total ({:.1}ms mean)",
                previewed.len(),
                preview_total,
                preview_total.as_secs_f64() * 1000. / previewed.len() as f64
            );
            println!(
                "full decode over {} files: {:?} total ({:.1}ms mean)",
                previewed.len(),
                full_total,
                full_total.as_secs_f64() * 1000. / previewed.len() as f64
            );
        }
        restore_thumb_home(previous);
    }

    /// Stage 1 doubles as the decodable set: files that miss everywhere still
    /// decode, so the later stages compare against the same files.
    fn time_stage(photos: &[PathBuf], cache: &Path, label: &str) -> Vec<PathBuf> {
        let mut decodable = Vec::new();
        let start = std::time::Instant::now();
        for path in photos {
            match load_fast_using(Some(cache), path, TABLE_THUMB_EDGE, true) {
                FastLoad::Ready(loaded) => {
                    if let Some(cache) = loaded.cache {
                        persist(cache);
                    }
                    decodable.push(path.clone());
                }
                FastLoad::Fallback => {
                    if let Some(loaded) = load_fallback_using(Some(cache), path, TABLE_THUMB_EDGE) {
                        if let Some(cache) = loaded.cache {
                            persist(cache);
                        }
                        decodable.push(path.clone());
                    }
                }
            }
        }
        let total = start.elapsed();
        report(
            label,
            decodable.len(),
            total,
            mean_of(total, decodable.len()),
        );
        decodable
    }

    fn stage(files: &[PathBuf], mut load: impl FnMut(&Path) -> bool) -> (std::time::Duration, f64) {
        let start = std::time::Instant::now();
        let mut hits = 0;
        for path in files {
            hits += usize::from(load(path));
        }
        assert_eq!(
            hits,
            files.len(),
            "a reuse stage must hit every decodable file"
        );
        let total = start.elapsed();
        (total, mean_of(total, files.len()))
    }

    fn mean_of(total: std::time::Duration, count: usize) -> f64 {
        total.as_secs_f64() * 1000. / count.max(1) as f64
    }

    fn report(label: &str, files: usize, total: std::time::Duration, mean: f64) {
        println!("{label} over {files} files: {total:?} total ({mean:.1}ms mean)");
    }
}
