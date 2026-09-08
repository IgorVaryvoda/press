//! Minimal safe boundary around the system libavif encoder.

use std::ffi::{CString, c_char, c_int, c_uchar};
#[cfg(unix)]
use std::os::unix::ffi::OsStrExt;
use std::path::Path;
use std::sync::atomic::{AtomicU8, Ordering};

/// libaom's speed dial, 0 (slowest, smallest) to 10 (fastest, largest).
///
/// Six is what Press has always encoded at, and it stays the default so an
/// unconfigured run writes the same bytes it wrote before this knob existed. Eight
/// roughly halves a batch for a fraction of a percent of size, which is the trade a
/// folder of five thousand photos usually wants.
pub const DEFAULT_SPEED: u8 = 6;

/// One value for the whole process, written once at startup from `--avif-speed` or
/// the settings file and read at one call site. Speed changes nothing about the pixels
/// an encoder is handed, so threading it through `convert::encode` would be nine call
/// sites and every test carrying a constant; `encode` below takes it as an argument so
/// nothing has to write this to test it.
static SPEED: AtomicU8 = AtomicU8::new(DEFAULT_SPEED);

/// The one place the range is enforced. The command line rejects a bad value by name
/// before it gets here, but the settings file is a text file a person may have typed
/// into, and an encoder that refuses to run is a worse answer than a clamp.
pub fn set_speed(speed: u8) {
    SPEED.store(speed.min(10), Ordering::Relaxed);
}

pub fn speed() -> u8 {
    SPEED.load(Ordering::Relaxed)
}

/// What the settings file is worth writing. The default writes nothing, the same
/// shape as every other optional key there.
pub fn configured_speed() -> Option<u8> {
    let speed = speed();
    (speed != DEFAULT_SPEED).then_some(speed)
}

#[repr(C)]
struct ImageGuideAvifData {
    _private: [u8; 0],
}

#[repr(C)]
#[derive(Default)]
struct ImageGuideAvifHeader {
    width: u32,
    height: u32,
    depth: u32,
    alpha_present: c_int,
    icc_present: c_int,
    unsupported_transform: c_int,
}

#[repr(C)]
#[derive(Default)]
struct ImageGuideAvifDecoded {
    width: u32,
    height: u32,
    depth: u32,
    icc: *mut c_uchar,
    icc_size: usize,
}

/// Facts libavif can read from the container before any AV1 pixels are decoded.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct Header {
    pub width: u32,
    pub height: u32,
    pub depth: u8,
    pub alpha: bool,
    pub profile: bool,
    /// The image crate does not apply AVIF irot/imir/clap properties. These
    /// outputs are refused until Press can apply them to pixels and dimensions.
    pub unsupported_transform: bool,
}

pub(crate) enum DecodedPixels {
    U8(Vec<u8>),
    U16(Vec<u16>),
}

pub(crate) struct Decoded {
    pub(crate) pixels: DecodedPixels,
    pub(crate) width: u32,
    pub(crate) height: u32,
    pub(crate) depth: u8,
    pub(crate) profile: Option<Vec<u8>>,
}

unsafe extern "C" {
    fn imageguide_avif_encode(
        pixels: *const c_uchar,
        width: u32,
        height: u32,
        has_alpha: c_int,
        quality: c_int,
        speed: c_int,
        threads: c_int,
        profile: *const c_uchar,
        profile_size: usize,
    ) -> *mut ImageGuideAvifData;
    fn imageguide_avif_data(encoded: *const ImageGuideAvifData) -> *const c_uchar;
    fn imageguide_avif_size(encoded: *const ImageGuideAvifData) -> usize;
    fn imageguide_avif_free(encoded: *mut ImageGuideAvifData);
    fn imageguide_avif_probe_memory(
        data: *const c_uchar,
        size: usize,
        header: *mut ImageGuideAvifHeader,
    ) -> c_int;
    fn imageguide_avif_probe_file(path: *const c_char, header: *mut ImageGuideAvifHeader) -> c_int;
    fn imageguide_avif_decode_memory(
        data: *const c_uchar,
        size: usize,
        image_size_limit: u32,
        image_dimension_limit: u32,
        pixels: *mut c_uchar,
        pixels_size: usize,
        decoded: *mut ImageGuideAvifDecoded,
    ) -> c_int;
    fn imageguide_avif_decoded_free(decoded: *mut ImageGuideAvifDecoded);
}

fn header(raw: ImageGuideAvifHeader) -> Option<Header> {
    Some(Header {
        width: raw.width,
        height: raw.height,
        depth: u8::try_from(raw.depth).ok()?,
        alpha: raw.alpha_present != 0,
        profile: raw.icc_present != 0,
        unsupported_transform: raw.unsupported_transform != 0,
    })
}

/// Parse AVIF container metadata without constructing an AV1 image buffer.
pub fn probe_bytes(bytes: &[u8]) -> Option<Header> {
    let mut raw = ImageGuideAvifHeader::default();
    // SAFETY: the bridge reads the byte slice only during this synchronous call
    // and writes one initialized header into the valid output pointer.
    let parsed = unsafe { imageguide_avif_probe_memory(bytes.as_ptr(), bytes.len(), &mut raw) };
    (parsed != 0).then(|| header(raw)).flatten()
}

/// Parse an AVIF file through libavif's file reader, without reading its pixels.
pub fn probe_file(path: &Path) -> Option<Header> {
    let path = {
        #[cfg(unix)]
        {
            CString::new(path.as_os_str().as_bytes()).ok()?
        }
        #[cfg(not(unix))]
        {
            CString::new(path.to_str()?).ok()?
        }
    };
    let mut raw = ImageGuideAvifHeader::default();
    // SAFETY: the NUL-terminated path and output header remain alive for the
    // synchronous bridge call; libavif opens and closes the file itself.
    let parsed = unsafe { imageguide_avif_probe_file(path.as_ptr(), &mut raw) };
    (parsed != 0).then(|| header(raw)).flatten()
}

/// Decode through the same libavif instance that admits the header. Its size and
/// dimension limits are applied before libavif asks the AV1 codec for a frame.
/// The RGB buffer is allocated by Rust and borrowed by the bridge, so native
/// decoding does not keep a second full-size malloc copy or repack 16-bit samples.
pub(crate) fn decode_bytes_with_limits(
    bytes: &[u8],
    image_size_limit: u32,
    image_dimension_limit: u32,
) -> Option<Decoded> {
    let info = probe_bytes(bytes)?;
    if info.unsupported_transform
        || info.width > image_dimension_limit
        || info.height > image_dimension_limit
    {
        return None;
    }
    let sample_bytes = usize::from(info.depth > 8) + 1;
    let pixel_count_u64 = u64::from(info.width) * u64::from(info.height);
    // libavif's imageSizeLimit is a pixel-count limit, while the Rust output
    // buffer also stays inside Press's byte budget below.
    if pixel_count_u64 > u64::from(image_size_limit) {
        return None;
    }
    let pixel_count = usize::try_from(pixel_count_u64).ok()?;
    let pixels_len = pixel_count.checked_mul(4)?.checked_mul(sample_bytes)?;
    if pixels_len as u64 > crate::convert::MAX_DECODE_BYTES {
        return None;
    }
    let mut pixels8 = (sample_bytes == 1).then(|| vec![0; pixels_len]);
    let mut pixels16 = (sample_bytes == 2).then(|| vec![0; pixels_len / 2]);
    let pixels = pixels8
        .as_mut()
        .map_or(std::ptr::null_mut(), |pixels| pixels.as_mut_ptr());
    let pixels = pixels16
        .as_mut()
        .map_or(pixels, |pixels| pixels.as_mut_ptr().cast::<c_uchar>());
    let mut raw = ImageGuideAvifDecoded::default();
    // SAFETY: the bridge borrows `bytes` only for this synchronous call and owns
    // the ICC buffer until the matching free function below. `pixels` points
    // into the live Rust vector selected above, whose capacity is pixels_len.
    let decoded = unsafe {
        imageguide_avif_decode_memory(
            bytes.as_ptr(),
            bytes.len(),
            image_size_limit,
            image_dimension_limit,
            pixels,
            pixels_len,
            &mut raw,
        )
    };
    if decoded == 0 {
        return None;
    }
    if (raw.width, raw.height, raw.depth) != (info.width, info.height, u32::from(info.depth)) {
        // A coded frame that disagrees with the admitted container dimensions is
        // not allowed to use a buffer sized from the container header.
        unsafe { imageguide_avif_decoded_free(&mut raw) };
        return None;
    }
    let profile = if raw.icc.is_null() {
        None
    } else {
        // SAFETY: the bridge returned `icc_size` initialized bytes owned by
        // `raw`, which remains alive until it is freed below.
        Some(unsafe { std::slice::from_raw_parts(raw.icc, raw.icc_size).to_vec() })
    };
    let depth = u8::try_from(raw.depth).ok();
    let pixels = match sample_bytes {
        1 => DecodedPixels::U8(pixels8.take().expect("one-byte AVIF buffer exists")),
        2 => DecodedPixels::U16(pixels16.take().expect("two-byte AVIF buffer exists")),
        _ => unreachable!(),
    };
    let result = depth.map(|depth| Decoded {
        pixels,
        width: raw.width,
        height: raw.height,
        depth,
        profile,
    });
    // SAFETY: `raw` is exactly the object initialized by the bridge call, and the
    // ICC slice above was copied into an independent Rust allocation.
    unsafe { imageguide_avif_decoded_free(&mut raw) };
    result
}

// One argument per parameter of `imageguide_avif_encode`, which is what a boundary
// this thin is for. A struct here would only be the same list with a name on it.
#[allow(clippy::too_many_arguments)]
pub fn encode(
    pixels: &[u8],
    width: u32,
    height: u32,
    has_alpha: bool,
    quality: u8,
    speed: u8,
    threads: usize,
    profile: Option<&[u8]>,
) -> Option<Vec<u8>> {
    let channels = if has_alpha { 4 } else { 3 };
    let expected = usize::try_from(width)
        .ok()?
        .checked_mul(usize::try_from(height).ok()?)?
        .checked_mul(channels)?;
    if pixels.len() != expected {
        return None;
    }

    // SAFETY: the validated pixel slice and the profile remain alive for the
    // synchronous encode, which copies the profile. The bridge owns its output and is
    // always asked to free it after the copy.
    unsafe {
        let encoded = imageguide_avif_encode(
            pixels.as_ptr(),
            width,
            height,
            c_int::from(has_alpha),
            c_int::from(quality),
            c_int::from(speed),
            c_int::try_from(threads).ok()?,
            profile.map_or(std::ptr::null(), <[u8]>::as_ptr),
            profile.map_or(0, <[u8]>::len),
        );
        if encoded.is_null() {
            return None;
        }
        let data = imageguide_avif_data(encoded);
        let size = imageguide_avif_size(encoded);
        let output = if data.is_null() {
            None
        } else {
            Some(std::slice::from_raw_parts(data, size).to_vec())
        };
        imageguide_avif_free(encoded);
        output
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn dav1d_frame_size_limit_is_checked_with_room_for_the_output() {
        let bytes = include_bytes!("../fixtures/avif/coded-mismatch.avif");
        let mut pixels = vec![0u8; 4 * 4 * 4];
        let mut decoded = ImageGuideAvifDecoded::default();
        // The output buffer is large enough for the coded 2x2 frame. Only the
        // one-pixel image limit can refuse this call, proving the cap reaches
        // the native AV1 decoder rather than our container-sized buffer check.
        let result = unsafe {
            imageguide_avif_decode_memory(
                bytes.as_ptr(),
                bytes.len(),
                1,
                100,
                pixels.as_mut_ptr(),
                pixels.len(),
                &mut decoded,
            )
        };
        assert_eq!(result, 0);
        unsafe { imageguide_avif_decoded_free(&mut decoded) };
    }
}
