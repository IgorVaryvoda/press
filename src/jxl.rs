//! JPEG XL header reads and decoding in safe Rust, plus a small libjxl encoder boundary.

use std::ffi::{c_float, c_int, c_uchar};
use std::io::Read as _;
use std::path::Path;

use image::DynamicImage;
use jxl_oxide::integration::JxlDecoder;
use jxl_oxide::{InitializeResult, JxlImage};

const HEADER_LIMIT: usize = 16 * 1024 * 1024;

pub struct Info {
    pub width: u32,
    pub height: u32,
    pub animated: bool,
}

/// Read only enough of a JPEG XL file to initialize its image header.
pub fn probe(path: &Path) -> Option<Info> {
    let mut reader = std::fs::File::open(path).ok()?;
    let mut uninit = JxlImage::builder().build_uninit();
    let mut buffer = vec![0u8; 4096];
    let mut valid = 0usize;

    loop {
        if valid == buffer.len() {
            if buffer.len() >= HEADER_LIMIT {
                return None;
            }
            buffer.resize((buffer.len() * 2).min(HEADER_LIMIT), 0);
        }
        let read = reader.read(&mut buffer[valid..]).ok()?;
        if read == 0 {
            return None;
        }
        valid += read;
        let consumed = uninit.feed_bytes(&buffer[..valid]).ok()?;
        buffer.copy_within(consumed..valid, 0);
        valid -= consumed;

        match uninit.try_init().ok()? {
            InitializeResult::NeedMoreData(next) => uninit = next,
            InitializeResult::Initialized(image) => {
                return Some(Info {
                    width: image.width(),
                    height: image.height(),
                    animated: image.image_header().metadata.animation.is_some(),
                });
            }
        }
    }
}

pub fn decode_path(path: &Path) -> Option<DynamicImage> {
    decode(std::fs::File::open(path).ok()?)
}

pub fn decode_bytes(bytes: &[u8]) -> Option<DynamicImage> {
    decode(std::io::Cursor::new(bytes))
}

fn decode(reader: impl std::io::Read) -> Option<DynamicImage> {
    DynamicImage::from_decoder(JxlDecoder::new(reader).ok()?).ok()
}

#[repr(C)]
struct PressJxlData {
    _private: [u8; 0],
}

unsafe extern "C" {
    fn press_jxl_encode(
        pixels: *const c_uchar,
        size: usize,
        width: u32,
        height: u32,
        has_alpha: c_int,
        lossless: c_int,
        distance: c_float,
    ) -> *mut PressJxlData;
    fn press_jxl_data(encoded: *const PressJxlData) -> *const c_uchar;
    fn press_jxl_size(encoded: *const PressJxlData) -> usize;
    fn press_jxl_free(encoded: *mut PressJxlData);
}

pub fn encode(
    pixels: &[u8],
    width: u32,
    height: u32,
    has_alpha: bool,
    lossless: bool,
    distance: f32,
) -> Option<Vec<u8>> {
    let channels = if has_alpha { 4 } else { 3 };
    let expected = usize::try_from(width)
        .ok()?
        .checked_mul(usize::try_from(height).ok()?)?
        .checked_mul(channels)?;
    if pixels.len() != expected || !distance.is_finite() || distance < 0. {
        return None;
    }

    // SAFETY: the validated pixel slice stays alive for the synchronous encode. The
    // bridge owns its output, which is copied before the matching free call.
    unsafe {
        let encoded = press_jxl_encode(
            pixels.as_ptr(),
            pixels.len(),
            width,
            height,
            c_int::from(has_alpha),
            c_int::from(lossless),
            distance,
        );
        if encoded.is_null() {
            return None;
        }
        let data = press_jxl_data(encoded);
        let size = press_jxl_size(encoded);
        let output = if data.is_null() || size == 0 {
            None
        } else {
            Some(std::slice::from_raw_parts(data, size).to_vec())
        };
        press_jxl_free(encoded);
        output
    }
}
