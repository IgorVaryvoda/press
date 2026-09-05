//! Build an original-versus-converted pair for pixel peeping.
//!
//! The conversion happens in memory. Nothing is written, because the point is to
//! decide whether the trade is acceptable *before* committing to it.

use std::path::Path;
use std::sync::Arc;

use gpui_kit::RenderImage;
use image::{DynamicImage, Frame, RgbaImage};

use crate::convert::{self, Format, MaxEdge, Quality};
use crate::thumbs::to_bgra;

/// Everything that changes what a `Pair` contains. Reopening the same image with the
/// same settings should not re-encode it, which at AVIF speeds is a two-second wait.
/// The revision is what the file was when the comparison was asked for — bytes and
/// mtime read once at the request, never per render. A reopened file with new
/// contents misses the cache instead of wearing old pixels, and a result landing
/// re-stats before it trusts the pair. `None` is a file that could not be stated;
/// the comparison it builds still fails on decode rather than on the key.
/// `avif_speed` is the encoder input: another speed writes other bytes.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Key {
    pub path: std::path::PathBuf,
    pub source: std::path::PathBuf,
    pub format: Format,
    /// Quality as raw bits, because f32 is not `Eq`.
    pub quality: Option<u32>,
    pub max_edge: Option<u32>,
    pub revision: Option<SourceRevision>,
    pub source_revision: Option<SourceRevision>,
    pub avif_speed: u8,
}

/// Bytes and mtime of one file at one moment. Best effort on filesystems with
/// coarse timestamps: same size within one tick still reads as unchanged.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct SourceRevision {
    pub bytes: u64,
    pub modified: Option<(u64, u32)>,
}

impl SourceRevision {
    /// One stat call. Callers do this once per compare request — at a job
    /// boundary, never once per row render.
    pub fn of(path: &Path) -> Option<Self> {
        let metadata = std::fs::metadata(path).ok()?;
        let modified = metadata
            .modified()
            .ok()
            .and_then(|time| time.duration_since(std::time::UNIX_EPOCH).ok())
            .map(|age| (age.as_secs(), age.subsec_nanos()));
        Some(Self {
            bytes: metadata.len(),
            modified,
        })
    }
}

impl Key {
    pub fn new(
        path: &Path,
        source: &Path,
        format: Format,
        quality: Quality,
        max_edge: MaxEdge,
    ) -> Self {
        let revision = SourceRevision::of(path);
        let source_revision = if source == path {
            revision
        } else {
            SourceRevision::of(source)
        };
        Self {
            path: path.to_path_buf(),
            source: source.to_path_buf(),
            format,
            quality: quality.0.map(f32::to_bits),
            max_edge: max_edge.0,
            revision,
            source_revision,
            avif_speed: crate::avif::speed(),
        }
    }

    /// Re-stat both files the pair depends on. True when neither changed since
    /// the request. Landing checks call this; renders never do.
    pub fn fresh(&self) -> bool {
        SourceRevision::of(&self.path) == self.revision
            && SourceRevision::of(&self.source) == self.source_revision
    }
}

pub struct Pair {
    pub original: Arc<RenderImage>,
    pub converted: Arc<RenderImage>,
    /// Bytes the selected encoded format would occupy on disk.
    pub converted_bytes: u64,
    pub width: u32,
    pub height: u32,
}

/// One decoded source image for the preview-first view. Opening a file should
/// not run an encoder until the user asks to compare it.
pub struct Preview {
    pub image: Arc<RenderImage>,
    pub width: u32,
    pub height: u32,
    /// The source's ICC profile. Only meaningful while `decoded` is true.
    pub profile: Option<Vec<u8>>,
    /// True when `image` is this file decoded at full size, so `build` may re-encode
    /// it instead of reading the file again. False for the thumbnail that stands in
    /// while the decode runs: that one has been through the cache's lossy WebP and is
    /// a picture to look at, not a source.
    pub decoded: bool,
}

impl Pair {
    /// What the conversion saved, as a percentage. Negative when the file grew.
    pub fn saving_percent(&self, source_bytes: u64) -> f32 {
        if source_bytes == 0 {
            return 0.;
        }
        (source_bytes as f32 - self.converted_bytes as f32) / source_bytes as f32 * 100.
    }
}

pub fn preview(path: &Path) -> Option<Preview> {
    // The native decoders first, as before. Both of them are eight-bit, so what they
    // hand back is the whole source and a comparison can be built from it. The general
    // fallback has already lost anything deeper to `into_rgba8`, and re-encoding those
    // pixels would quietly write an eight-bit file from a sixteen-bit source, so it
    // stays a picture to look at.
    let (image, decoded) = match crate::thumbs::decode_native(path, None) {
        Some(image) => (image, true),
        None => (crate::scan::decode(path)?.into_rgba8(), false),
    };
    let (width, height) = image.dimensions();
    Some(Preview {
        image: Arc::new(RenderImage::new(vec![Frame::new(to_bgra(image))])),
        width,
        height,
        profile: decoded.then(|| crate::scan::icc_profile(path)).flatten(),
        decoded,
    })
}

/// The window's BGRA buffer read back as RGBA. `to_bgra` is its own inverse, so this
/// is the same swap the preview already went through, run over a copy of it.
fn rgba(image: &RenderImage) -> Option<RgbaImage> {
    let size = image.size(0);
    let pixels = image.as_bytes(0)?.to_vec();
    Some(to_bgra(RgbaImage::from_raw(
        u32::from(size.width),
        u32::from(size.height),
        pixels,
    )?))
}

/// Decode `path`, encode it at `quality`, and decode that back, so both sides are
/// real pixels rather than a promise.
///
/// When a size budget is set, the *original* side is downscaled too. Comparing a
/// 6400px source against a 2000px export would measure the resize, not the
/// compression, and the resize is not the part you need to eyeball. Both sides are
/// the delivered resolution; only one of them has been through the encoder.
///
/// `preview` is the preview of this same file, if one is open. Comparing an image you
/// are already looking at used to read and decode it a second time; its pixels are
/// already here, and swapping a copy of them back out of BGRA costs a memcpy against a
/// full decode. Only the original side is reused — the converted side is still
/// encoded and decoded back, because a promise about the output is not the output.
///
/// Reused only at full size. A preview is a full decode, while the writer now decodes
/// a JPEG at a reduced DCT scale before the same Lanczos step, so under a size budget
/// the two would disagree by a fraction of a percent and the bytes reported here would
/// stop being the bytes the file would be. Nothing is lost by reading the file in that
/// case: the scaled decode is what made it cheap.
pub fn build(
    path: &Path,
    format: Format,
    quality: Quality,
    max_edge: MaxEdge,
    preview: Option<&Preview>,
) -> Option<Pair> {
    // The same decode and the same profile the writer uses, so the size shown beside
    // the comparison is the size the file would actually be.
    let format = format.resolve(path).ok()?;
    let (decoded, profile) = preview
        .filter(|preview| preview.decoded && max_edge == MaxEdge::FULL)
        .and_then(|preview| {
            Some((
                DynamicImage::ImageRgba8(rgba(&preview.image)?),
                preview.profile.clone(),
            ))
        })
        // A frame that will not read back is a reason to decode the file, not a reason
        // to fail the comparison.
        .or_else(|| crate::scan::decode_for_conversion(path, max_edge).ok())?;
    let original = max_edge.apply(decoded);
    // The one lossless-depth verdict, shared with the disk writer: a refused
    // request builds no comparison rather than a quiet eight-bit stand-in.
    convert::check_lossless_depth(&original, format, quality).ok()?;
    let encoded = convert::encode(&original, format, quality, profile.as_deref()).ok()?;
    let decoded = crate::scan::decode_bytes(&encoded)?;

    let (width, height) = (original.width(), original.height());

    Some(Pair {
        original: Arc::new(RenderImage::new(vec![Frame::new(to_bgra(
            original.into_rgba8(),
        ))])),
        converted: Arc::new(RenderImage::new(vec![Frame::new(to_bgra(
            decoded.into_rgba8(),
        ))])),
        converted_bytes: encoded.len() as u64,
        width,
        height,
    })
}

/// The same pair, but the converted side is a file that already exists rather
/// than an encode done for the preview. This is what a finished run produced,
/// read back off disk — bytes and pixels both — so nothing here is a promise
/// about what conversion would do.
///
/// The original is brought down to the output's dimensions when a resize was
/// part of the job, for the same reason `build` does it: otherwise the divider
/// measures the resize instead of the compression.
pub fn build_written(source: &Path, written: &Path) -> Option<Pair> {
    let converted = crate::scan::decode(written)?;
    let (width, height) = (converted.width(), converted.height());
    let original = crate::scan::decode(source)?;
    let original = if original.width() == width && original.height() == height {
        original
    } else {
        original.resize_exact(width, height, image::imageops::FilterType::Lanczos3)
    };
    let converted_bytes = std::fs::metadata(written).map(|meta| meta.len()).ok()?;

    Some(Pair {
        original: Arc::new(RenderImage::new(vec![Frame::new(to_bgra(
            original.into_rgba8(),
        ))])),
        converted: Arc::new(RenderImage::new(vec![Frame::new(to_bgra(
            converted.into_rgba8(),
        ))])),
        converted_bytes,
        width,
        height,
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use image::{ImageBuffer, Rgb};

    /// Per-process unique, so parallel threads and repeated runs never share a
    /// fixture dir.
    fn test_dir(tag: &str) -> std::path::PathBuf {
        let dir =
            std::env::temp_dir().join(format!("imageguide-compare-{tag}-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();
        dir
    }

    #[test]
    fn both_sides_decode_at_the_source_dimensions() {
        let dir = test_dir("dimensions");
        let path = dir.join("sample.png");
        ImageBuffer::from_fn(120, 80, |x, y| {
            Rgb([(x * 2 % 256) as u8, (y * 3 % 256) as u8, 90])
        })
        .save(&path)
        .unwrap();

        let pair = build(
            &path,
            Format::WebP,
            Quality::lossy(70.),
            MaxEdge::FULL,
            None,
        )
        .expect("pair builds");

        assert_eq!((pair.width, pair.height), (120, 80));
        // The compare view lines the two up pixel for pixel. If the encoder ever
        // changed the geometry the slider would show a lie.
        assert_eq!(u32::from(pair.original.size(0).width), 120);
        assert_eq!(u32::from(pair.converted.size(0).width), 120);
        assert_eq!(u32::from(pair.converted.size(0).height), 80);
        assert!(pair.converted_bytes > 0);
    }

    #[test]
    fn preview_decodes_only_the_source() {
        let dir = test_dir("preview");
        let path = dir.join("source.png");
        ImageBuffer::from_fn(73, 41, |x, y| Rgb([x as u8, y as u8, 120]))
            .save(&path)
            .unwrap();

        let preview = preview(&path).expect("source preview decodes");

        assert_eq!((preview.width, preview.height), (73, 41));
        assert_eq!(u32::from(preview.image.size(0).width), 73);
        assert_eq!(u32::from(preview.image.size(0).height), 41);
    }

    /// Comparing the image you are already looking at used to read and decode it a
    /// second time. Proved the only way that cannot be faked: the file is gone by the
    /// time the pair is built.
    #[test]
    fn a_comparison_built_from_a_preview_does_not_read_the_file_again() {
        let dir = test_dir("reuse");
        let path = dir.join("shot.jpg");
        crate::convert::tests::photo(200, 120).save(&path).unwrap();

        let preview = preview(&path).expect("the JPEG previews");
        assert!(preview.decoded, "a JPEG preview is the decoded source");
        std::fs::remove_file(&path).unwrap();

        let pair = build(
            &path,
            Format::WebP,
            Quality::lossy(70.),
            MaxEdge::FULL,
            Some(&preview),
        )
        .expect("the pair builds from the preview's pixels");
        assert_eq!((pair.width, pair.height), (200, 120));
        assert_eq!(u32::from(pair.original.size(0).width), 200);
        // The converted side is still an encode of those pixels decoded back, not a
        // promise about one.
        assert_eq!(u32::from(pair.converted.size(0).width), 200);
        assert!(pair.converted_bytes > 0);

        // And with nothing to reuse there is nothing to build from.
        assert!(
            build(
                &path,
                Format::WebP,
                Quality::lossy(70.),
                MaxEdge::FULL,
                None
            )
            .is_none(),
            "the file is gone, so a cold build cannot succeed"
        );

        // Under a size budget the preview is not the decode the writer would make, so
        // it is not reused and the file is read — which, with the file gone, fails.
        assert!(
            build(
                &path,
                Format::WebP,
                Quality::lossy(70.),
                MaxEdge(Some(100)),
                Some(&preview),
            )
            .is_none(),
            "a budgeted comparison reused pixels the writer would not have produced"
        );
    }

    /// The picture on screen while a preview decodes is a thumbnail out of the disk
    /// cache, which has been through lossy WebP. Re-encoding that would report the
    /// size of a comparison nobody asked for.
    #[test]
    fn a_thumbnail_standing_in_for_a_preview_is_never_used_as_a_source() {
        let dir = test_dir("standin");
        let path = dir.join("shot.jpg");
        crate::convert::tests::photo(200, 120).save(&path).unwrap();

        let standin = Preview {
            image: Arc::new(RenderImage::new(vec![Frame::new(
                crate::convert::tests::photo(20, 12).into_rgba8(),
            )])),
            width: 200,
            height: 120,
            profile: None,
            decoded: false,
        };

        let pair = build(
            &path,
            Format::WebP,
            Quality::lossy(70.),
            MaxEdge::FULL,
            Some(&standin),
        )
        .expect("the pair builds from the file");
        assert_eq!(
            (pair.width, pair.height),
            (200, 120),
            "the 20px stand-in was re-encoded as if it were the source"
        );
    }

    /// The native decoders carry no ICC profile, so the preview reads it beside them.
    /// Without it the comparison under-reports every wide gamut file by exactly the
    /// bytes the writer would attach.
    #[test]
    fn a_preview_carries_the_profile_a_comparison_would_write() {
        let dir = test_dir("profile");
        let path = dir.join("wide.webp");
        let profile = crate::convert::tests::rgb_profile();
        let tagged = convert::encode(
            &crate::convert::tests::photo(64, 48),
            Format::WebP,
            Quality::lossy(80.),
            Some(&profile),
        )
        .expect("a tagged WebP encodes");
        std::fs::write(&path, tagged).unwrap();

        let preview = preview(&path).expect("the WebP previews");
        assert!(preview.decoded);
        assert_eq!(
            preview.profile.as_deref(),
            Some(profile.as_slice()),
            "the preview dropped the source profile"
        );
    }

    #[test]
    fn written_comparison_reads_the_existing_output() {
        let dir = test_dir("written");
        let source = dir.join("source.png");
        let written = dir.join("already-written.png");
        ImageBuffer::from_fn(120, 80, |x, y| Rgb([x as u8, y as u8, 40]))
            .save(&source)
            .unwrap();
        ImageBuffer::from_fn(60, 40, |x, y| Rgb([y as u8, x as u8, 90]))
            .save(&written)
            .unwrap();

        let pair = build_written(&source, &written).expect("existing output compares");

        assert_eq!((pair.width, pair.height), (60, 40));
        assert_eq!(
            pair.converted_bytes,
            std::fs::metadata(&written).unwrap().len()
        );
    }

    /// The cache is only correct if the key notices every setting that changes the
    /// output. A missed field would serve a WebP pair for an AVIF request.
    #[test]
    fn cache_keys_separate_every_setting() {
        let path = Path::new("/photos/one.png");
        let base = Key::new(path, path, Format::WebP, Quality::lossy(80.), MaxEdge::FULL);

        assert_eq!(
            base,
            Key::new(path, path, Format::WebP, Quality::lossy(80.), MaxEdge::FULL)
        );
        assert_ne!(
            base,
            Key::new(path, path, Format::Avif, Quality::lossy(80.), MaxEdge::FULL)
        );
        assert_ne!(
            base,
            Key::new(
                path,
                path,
                Format::JpegXl,
                Quality::lossy(80.),
                MaxEdge::FULL
            )
        );
        assert_ne!(
            base,
            Key::new(path, path, Format::WebP, Quality::lossy(60.), MaxEdge::FULL)
        );
        assert_ne!(
            base,
            Key::new(path, path, Format::WebP, Quality::LOSSLESS, MaxEdge::FULL)
        );
        assert_ne!(
            base,
            Key::new(
                path,
                path,
                Format::WebP,
                Quality::lossy(80.),
                MaxEdge(Some(1600))
            )
        );
        assert_ne!(
            base,
            Key::new(
                Path::new("/photos/two.png"),
                Path::new("/photos/two.png"),
                Format::WebP,
                Quality::lossy(80.),
                MaxEdge::FULL
            )
        );
        // A speed change writes other bytes; an edit or replace re-stats.
        // Either must miss the cache instead of wearing the old pair.
        assert_ne!(
            base,
            Key {
                avif_speed: base.avif_speed.wrapping_add(1),
                ..base.clone()
            }
        );
        let edited = SourceRevision {
            bytes: 1,
            modified: None,
        };
        assert_ne!(
            base,
            Key {
                revision: Some(edited),
                source_revision: Some(edited),
                ..base.clone()
            }
        );
    }

    #[test]
    fn a_size_budget_shrinks_both_sides_together() {
        let dir = test_dir("resize");
        let path = dir.join("wide.png");
        ImageBuffer::from_fn(400, 200, |x, y| Rgb([(x % 256) as u8, (y % 256) as u8, 40]))
            .save(&path)
            .unwrap();

        let pair = build(
            &path,
            Format::WebP,
            Quality::lossy(70.),
            MaxEdge(Some(100)),
            None,
        )
        .expect("pair builds");

        assert_eq!((pair.width, pair.height), (100, 50));
        assert_eq!(u32::from(pair.original.size(0).width), 100);
        assert_eq!(
            u32::from(pair.converted.size(0).width),
            100,
            "both sides must be the delivered size or the divider compares nothing"
        );
    }

    #[test]
    fn saving_is_reported_against_the_source_size() {
        let pair = Pair {
            original: Arc::new(RenderImage::new(vec![Frame::new(ImageBuffer::from_pixel(
                1,
                1,
                image::Rgba([0u8, 0, 0, 255]),
            ))])),
            converted: Arc::new(RenderImage::new(vec![Frame::new(ImageBuffer::from_pixel(
                1,
                1,
                image::Rgba([0u8, 0, 0, 255]),
            ))])),
            converted_bytes: 250,
            width: 1,
            height: 1,
        };

        assert_eq!(pair.saving_percent(1000), 75.);
        assert_eq!(pair.saving_percent(0), 0.);
        assert!(pair.saving_percent(100) < 0., "growth reads as negative");
    }

    #[test]
    fn a_lossless_webp_comparison_refuses_the_same_depth_change_as_the_writer() {
        let dir = tempfile::tempdir().unwrap();
        let source = dir.path().join("sixteen-bit.png");
        let out_dir = dir.path().join("optimized");
        let written = out_dir.join("sixteen-bit.webp");
        ImageBuffer::from_pixel(8, 6, Rgb([1025u16, 32001, 65001]))
            .save(&source)
            .unwrap();
        let original = std::fs::read(&source).unwrap();
        let preview = preview(&source).unwrap();
        assert!(
            !preview.decoded,
            "the display buffer must not replace 16-bit samples"
        );

        for max_edge in [MaxEdge::FULL, MaxEdge(Some(4))] {
            for cached in [None, Some(&preview)] {
                assert!(
                    build(&source, Format::WebP, Quality::LOSSLESS, max_edge, cached).is_none(),
                    "comparison accepted an encode the writer refuses"
                );
            }
            assert_eq!(
                convert::convert_to(
                    &out_dir,
                    &source,
                    &written,
                    None,
                    Format::WebP,
                    Quality::LOSSLESS,
                    max_edge,
                ),
                Err(convert::Failure::LosslessNeedsEightBit)
            );
        }
        assert!(
            !out_dir.exists(),
            "a refused comparison or export must not write"
        );
        assert_eq!(std::fs::read(&source).unwrap(), original);
        assert!(
            build(
                &source,
                Format::WebP,
                Quality::lossy(80.),
                MaxEdge::FULL,
                None
            )
            .is_some(),
            "an explicit lossy request remains supported"
        );
    }

    #[test]
    fn the_shared_depth_check_keeps_supported_paths_and_refuses_the_rest() {
        use image::{Luma, Rgba};
        let rgb8 = DynamicImage::ImageRgb8(ImageBuffer::from_pixel(2, 2, Rgb([1u8, 2, 3])));
        let gray8 = DynamicImage::ImageLuma8(ImageBuffer::from_pixel(2, 2, Luma([4u8])));
        let sixteen = DynamicImage::ImageRgb16(ImageBuffer::from_pixel(2, 2, Rgb([1u16, 2, 3])));
        let gray16 = DynamicImage::ImageLuma16(ImageBuffer::from_pixel(2, 2, Luma([5u16])));
        let rgba16 =
            DynamicImage::ImageRgba16(ImageBuffer::from_pixel(2, 2, Rgba([1u16, 2, 3, 4])));
        let float =
            DynamicImage::ImageRgb32F(ImageBuffer::from_pixel(2, 2, Rgb([0.1f32, 0.2, 0.3])));
        // Eight-bit sources survive every lossless request the app makes.
        for image in [&rgb8, &gray8] {
            for format in [Format::WebP, Format::JpegXl, Format::Avif] {
                assert!(
                    convert::check_lossless_depth(image, format, Quality::LOSSLESS).is_ok(),
                    "the shared check must keep an eight-bit source"
                );
            }
        }
        // Sixteen-bit sources refuse lossless WebP, the one depth libwebp drops.
        for image in [&sixteen, &gray16, &rgba16] {
            assert_eq!(
                convert::check_lossless_depth(image, Format::WebP, Quality::LOSSLESS),
                Err(convert::Failure::LosslessNeedsEightBit)
            );
            assert!(
                convert::check_lossless_depth(image, Format::JpegXl, Quality::LOSSLESS).is_ok(),
                "JPEG XL keeps sixteen-bit integer samples"
            );
        }
        assert_eq!(
            convert::check_lossless_depth(&float, Format::JpegXl, Quality::LOSSLESS),
            Err(convert::Failure::LosslessNeedsIntegerSamples)
        );
        assert!(
            convert::check_lossless_depth(&float, Format::JpegXl, Quality::lossy(80.)).is_ok(),
            "a lossy request promises nothing about depth"
        );
    }
}
