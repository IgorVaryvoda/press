//! Camera raw through FlashRaw: header reads, a developed decode, and the embedded
//! preview for thumbnails.
//!
//! Every raw path runs before the `image` crate sees the bytes. Most raw formats are
//! TIFF containers, and a TIFF decoder reads a NEF as its 160x120 preview, which
//! makes every number derived from it a lie.

use std::{
    io::{Read as _, Seek as _, SeekFrom},
    path::Path,
};

use flashraw::{DecodeOptions, FlashRaw, Limits, Orientation, OutputFormat, PreviewEncoding};
use image::{DynamicImage, ImageBuffer, ImageFormat, RgbaImage};

pub struct Info {
    pub width: u32,
    pub height: u32,
}

/// The decode stays inside the budget every other decoder answers to. Sixteen-bit
/// samples, so this caps any one buffer FlashRaw allocates at `MAX_DECODE_BYTES`.
fn limits() -> Limits {
    Limits::default().with_max_samples(crate::convert::MAX_DECODE_BYTES / 2)
}

fn open(bytes: &[u8]) -> Option<FlashRaw<'_>> {
    FlashRaw::open_with(bytes, limits()).ok()
}

/// Dimensions as the developed image has them: default crop applied, then the
/// camera's orientation, which `OutputFormat::Rgb16` also applies.
fn info(raw: &FlashRaw<'_>) -> Info {
    let meta = raw.metadata();
    let (width, height) = (meta.width, meta.height);
    match meta.orientation {
        Orientation::Transpose
        | Orientation::Rotate90
        | Orientation::Transverse
        | Orientation::Rotate270 => Info {
            width: height,
            height: width,
        },
        _ => Info { width, height },
    }
}

/// Read the raw header from bytes already owned by the caller. `None` for anything
/// FlashRaw does not open, which includes every ordinary image.
pub fn probe_bytes(bytes: &[u8]) -> Option<Info> {
    open(bytes).map(|raw| info(&raw))
}

/// Header-only, like every other probe in the walk.
pub fn probe(path: &Path) -> Option<Info> {
    with_header(path, info)
}

/// Leading bytes read before FlashRaw is asked to parse, smallest first. Measured
/// on real files: CR3 parses from 64 KiB, NEF and ARW from 256 KiB, RAF needs
/// 4 MiB. A container that still refuses gets the whole file.
const HEADER_READS: [u64; 3] = [64 << 10, 256 << 10, 4 << 20];

/// Parse a raw file's container from as few leading bytes as it needs. Read, not
/// memory-mapped: a mapped file that another process truncates kills Press with
/// SIGBUS, and a walk over a synced folder meets files mid-write.
///
/// Files whose first bytes name an ordinary format stop at the first read. So do
/// files FlashRaw does not recognise, unless the name says raw: a big TIFF from a
/// print workflow is not read whole to learn it is a TIFF.
fn with_header<T>(path: &Path, read: impl FnOnce(&FlashRaw<'_>) -> T) -> Option<T> {
    let mut file = std::fs::File::open(path).ok()?;
    let length = file.metadata().ok()?.len();
    if length > crate::convert::MAX_SOURCE_BYTES {
        return None;
    }
    let mut bytes = Vec::new();
    (&mut file)
        .take(HEADER_READS[0])
        .read_to_end(&mut bytes)
        .ok()?;
    let ordinary = matches!(image::guess_format(&bytes), Ok(format) if format != ImageFormat::Tiff);
    if ordinary || (flashraw::supports(&bytes).is_none() && !crate::scan::is_raw(path)) {
        return None;
    }
    for size in HEADER_READS[1..].iter().copied().chain([length]) {
        if let Some(raw) = open(&bytes) {
            return Some(read(&raw));
        }
        if bytes.len() as u64 >= length {
            return None;
        }
        (&mut file)
            .take(size.saturating_sub(bytes.len() as u64))
            .read_to_end(&mut bytes)
            .ok()?;
    }
    open(&bytes).map(|raw| read(&raw))
}

/// Develop the sensor data to sixteen-bit sRGB with the default crop, the camera's
/// orientation and the camera's tone (`camera_curve`). Truncated sensor data fails
/// rather than coming back zero-filled: a converted file with a black band is a
/// wrong output, and this one would be reported as a saving.
pub fn decode_bytes(bytes: &[u8]) -> Option<DynamicImage> {
    let raw = open(bytes)?;
    let options = DecodeOptions {
        output: OutputFormat::Rgb16,
        limits: limits(),
        corruption_policy: flashraw::CorruptionPolicy::Strict,
        ..Default::default()
    };
    match raw.decode(options.clone()) {
        Ok(frame) => {
            let mut pixels = frame.as_u16()?.to_vec();
            if let Some(curve) = camera_curve(&raw, &pixels) {
                for sample in &mut pixels {
                    *sample = curve[usize::from(*sample)];
                }
            }
            ImageBuffer::from_raw(frame.width(), frame.height(), pixels)
                .map(DynamicImage::ImageRgb16)
        }
        // Nikon High Efficiency NEF, and any other sensor data FlashRaw cannot
        // read, still carries the full-size JPEG the camera rendered from it.
        // That is the one image these files can give, so it is the source.
        // FlashRaw takes it only when it covers 90% of the image each way.
        Err(flashraw::Error::UnsupportedCompression { .. }) => {
            let frame = raw
                .decode(DecodeOptions {
                    output: OutputFormat::Rgba8,
                    preview_fallback: true,
                    ..options
                })
                .ok()?;
            let rgba = RgbaImage::from_raw(frame.width(), frame.height(), frame.as_u8()?.to_vec())?;
            Some(DynamicImage::ImageRgb8(
                DynamicImage::ImageRgba8(rgba).into_rgb8(),
            ))
        }
        Err(_) => None,
    }
}

/// Smallest embedded JPEG worth matching. Below this the camera's thumbnail is
/// often letterboxed (Nikon's 160x120 for a 3:2 frame), and the black bars
/// would read as shadows.
const MIN_REFERENCE_EDGE: u32 = 640;

/// The edge the reference is decoded at. A scaled DCT decode, so a full-size
/// preview costs a fraction of its pixels.
const REFERENCE_EDGE: u32 = 1024;

/// The camera's tone curve, taken from the camera. FlashRaw develops with a plain
/// sRGB transfer and no tone curve, which reads flat next to the camera's JPEG.
/// The largest embedded JPEG is that same frame as the camera rendered it, so
/// matching the develop's histogram to it recovers the camera's contrast and
/// brightness while the output keeps the full sensor size and sixteen bits
/// (RawTherapee's "auto-matched tone curve" does the same). One curve for all
/// three channels, so the colour stays the colour matrix's. `None` when the file
/// has no usable preview: the plain develop is then the honest result.
fn camera_curve(raw: &FlashRaw<'_>, developed: &[u16]) -> Option<Vec<u16>> {
    let preview = raw
        .previews()
        .iter()
        .filter(|preview| preview.encoding == PreviewEncoding::Jpeg)
        .max_by_key(|preview| preview.length)?;
    let reference =
        crate::thumbs::decode_jpeg_bytes(raw.preview_bytes(preview).ok()?, Some(REFERENCE_EDGE))?;
    if reference.width().max(reference.height()) < MIN_REFERENCE_EDGE {
        return None;
    }
    let rgb: Vec<u8> = reference
        .pixels()
        .flat_map(|pixel| [pixel[0], pixel[1], pixel[2]])
        .collect();
    Some(matched_curve(developed, &rgb))
}

/// Source bins: sixteen-bit samples in 64-level steps.
const BINS: usize = 1024;

/// A monotone lookup table that gives `developed` the histogram of `reference`.
/// Every source bin maps to the reference level at the same rank, with
/// sub-level interpolation so eight-bit targets do not band a sixteen-bit image.
fn matched_curve(developed: &[u16], reference: &[u8]) -> Vec<u16> {
    let mut source = [0u64; BINS];
    for &sample in developed {
        source[usize::from(sample >> 6)] += 1;
    }
    let mut target = [0u64; 256];
    for &sample in reference {
        target[usize::from(sample)] += 1;
    }
    let cumulative = |counts: &[u64]| -> Vec<f64> {
        let total = counts.iter().sum::<u64>().max(1) as f64;
        counts
            .iter()
            .scan(0u64, |sum, count| {
                *sum += count;
                Some(*sum as f64 / total)
            })
            .collect()
    };
    let (source, target) = (cumulative(&source), cumulative(&target));

    let mut curve = [0f64; BINS];
    for (bin, level) in curve.iter_mut().enumerate() {
        let below = if bin == 0 { 0. } else { source[bin - 1] };
        let rank = (below + source[bin]) / 2.;
        let at = target.partition_point(|&reached| reached < rank).min(255);
        let start = if at == 0 { 0. } else { target[at - 1] };
        let within = if target[at] > start {
            (rank - start) / (target[at] - start)
        } else {
            0.5
        };
        *level = (at as f64 - 0.5 + within).clamp(0., 255.);
    }
    // A short box filter takes the steps out of sparse bins. An average of a
    // monotone sequence stays monotone.
    const RADIUS: usize = 7;
    let smooth: Vec<f64> = (0..BINS)
        .map(|bin| {
            let window = bin.saturating_sub(RADIUS)..(bin + RADIUS + 1).min(BINS);
            let width = window.len() as f64;
            curve[window].iter().sum::<f64>() / width
        })
        .collect();

    (0..=u16::MAX)
        .map(|value| {
            let position = ((f64::from(value) - 32.) / 64.).clamp(0., (BINS - 1) as f64);
            let low = position.floor() as usize;
            let high = (low + 1).min(BINS - 1);
            let level = smooth[low] + (smooth[high] - smooth[low]) * (position - low as f64);
            (level * 257.).round() as u16
        })
        .collect()
}

/// The camera's own embedded JPEG, for a thumbnail: a full develop costs a second
/// or more, and a gallery row needs 96 pixels. The smallest preview that still
/// fills `edge` wins, and only its bytes are read from the file.
pub struct Preview {
    pub jpeg: Vec<u8>,
    pub orientation: Option<image::metadata::Orientation>,
}

pub fn preview(path: &Path, edge: u32) -> Option<Preview> {
    let chosen = with_header(path, |raw| {
        let jpegs = || {
            raw.previews()
                .iter()
                .filter(|preview| preview.encoding == PreviewEncoding::Jpeg)
        };
        jpegs()
            .filter(|preview| preview.width.max(preview.height) >= edge)
            .min_by_key(|preview| u64::from(preview.width) * u64::from(preview.height))
            .or_else(|| jpegs().max_by_key(|preview| preview.length))
            .cloned()
    })??;
    let mut file = std::fs::File::open(path).ok()?;
    file.seek(SeekFrom::Start(chosen.offset)).ok()?;
    let mut jpeg = Vec::new();
    file.take(chosen.length).read_to_end(&mut jpeg).ok()?;
    (jpeg.len() as u64 == chosen.length).then_some(Preview {
        jpeg,
        orientation: image::metadata::Orientation::from_exif(chosen.orientation as u8),
    })
}

#[cfg(test)]
mod tests {
    use crate::scan::{FileFormat, probe};

    /// Every sixteen-bit level once, so the source histogram is flat.
    fn ramp() -> Vec<u16> {
        (0..=u16::MAX).collect()
    }

    #[test]
    fn a_develop_that_already_matches_the_camera_keeps_its_levels() {
        let reference: Vec<u8> = (0..=255u8).flat_map(|level| [level; 256]).collect();
        let curve = super::matched_curve(&ramp(), &reference);
        for value in [4_000u16, 16_384, 32_768, 50_000, 62_000] {
            let moved = (i32::from(curve[usize::from(value)]) - i32::from(value)).abs();
            assert!(moved < 700, "{value} moved by {moved}");
        }
    }

    #[test]
    fn a_brighter_camera_render_lifts_the_develop_by_its_own_curve() {
        // The camera rendered this frame with a square-root curve.
        let reference: Vec<u8> = (0..=u16::MAX)
            .map(|value| ((f64::from(value) / 65_535.).sqrt() * 255.).round() as u8)
            .collect();
        let curve = super::matched_curve(&ramp(), &reference);
        let quarter = f64::from(curve[16_384]) / 65_535.;
        assert!(
            (quarter - 0.5).abs() < 0.03,
            "a quarter-level sample takes the camera's half-level, got {quarter}"
        );
        assert!(
            curve.windows(2).all(|pair| pair[0] <= pair[1]),
            "the curve never inverts tones"
        );
    }

    /// Real raw files are tens of megabytes, so none ship with the repo. Point
    /// `IMAGEGUIDE_RAW_DIR` at a folder of them (raw.pixls.us has every camera):
    /// `IMAGEGUIDE_RAW_DIR=~/raws cargo test --bin press -- --ignored raw::`
    #[test]
    #[ignore]
    fn every_raw_sample_measures_develops_and_thumbnails_at_one_size() {
        let dir = std::env::var("IMAGEGUIDE_RAW_DIR").expect("IMAGEGUIDE_RAW_DIR names a folder");
        let mut seen = 0;
        for file in std::fs::read_dir(dir).unwrap() {
            let path = file.unwrap().path();
            let entry = probe(&path).unwrap_or_else(|| panic!("{} probes", path.display()));
            assert_eq!(entry.format, FileFormat::Raw, "{}", path.display());
            let (image, _) =
                crate::scan::decode_for_conversion(&path, crate::convert::MaxEdge::FULL).unwrap();
            assert_eq!(
                (image.width(), image.height()),
                (entry.width, entry.height),
                "{}: the audit states the size conversion writes",
                path.display()
            );
            let thumb = crate::thumbs::decode_native(&path, Some(96))
                .expect("the embedded preview decodes");
            assert_eq!(thumb.width().max(thumb.height()), 96);
            assert_eq!(
                thumb.width() > thumb.height(),
                entry.width > entry.height,
                "{}: the preview turns the way the develop does",
                path.display()
            );
            seen += 1;
        }
        assert!(seen > 0, "the folder holds raw files");
    }
}
