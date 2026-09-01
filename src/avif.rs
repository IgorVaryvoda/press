//! Minimal safe boundary around the system libavif encoder.

use std::ffi::{c_int, c_uchar};

#[repr(C)]
struct ImageGuideAvifData {
    _private: [u8; 0],
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
}

pub fn encode(
    pixels: &[u8],
    width: u32,
    height: u32,
    has_alpha: bool,
    quality: u8,
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
            6,
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
