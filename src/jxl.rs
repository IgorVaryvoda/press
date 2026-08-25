//! JPEG XL header reads, decoding, and encoding in Rust.

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

pub fn encode(
    pixels: &[u8],
    width: u32,
    height: u32,
    has_alpha: bool,
    quality: Option<f32>,
) -> Option<Vec<u8>> {
    let config = match quality {
        Some(quality) if quality.is_finite() => {
            jixel::EncodeConfig::default().with_quality(quality)
        }
        Some(_) => return None,
        None => jixel::EncodeConfig::default().with_lossless(true),
    };
    let (width, height) = (usize::try_from(width).ok()?, usize::try_from(height).ok()?);
    if has_alpha {
        jixel::encode_image_with_alpha(pixels, width, height, &config).ok()
    } else {
        jixel::encode_image(pixels, width, height, &config).ok()
    }
}
