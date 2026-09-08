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
    orientation_swaps: c_int,
}

/// Facts libavif can read from the container before any AV1 pixels are decoded.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct Header {
    pub width: u32,
    pub height: u32,
    pub depth: u8,
    pub alpha: bool,
    pub profile: bool,
    pub orientation_swaps: bool,
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
}

fn header(raw: ImageGuideAvifHeader) -> Option<Header> {
    Some(Header {
        width: raw.width,
        height: raw.height,
        depth: u8::try_from(raw.depth).ok()?,
        alpha: raw.alpha_present != 0,
        profile: raw.icc_present != 0,
        orientation_swaps: raw.orientation_swaps != 0,
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
