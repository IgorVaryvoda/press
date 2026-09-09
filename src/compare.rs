//! Build an original-versus-converted pair for pixel peeping.
//!
//! The conversion happens in memory. Nothing is written, because the point is to
//! decide whether the trade is acceptable *before* committing to it.

use std::path::Path;
use std::sync::Arc;

use gpui_kit::RenderImage;
use image::Frame;

use crate::convert::{self, Failure, Format, MaxEdge, Quality};
use crate::manifest::SourceIdentity;
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
    /// The exact source bytes these pixels were prepared from. The cache key holds
    /// a size and an mtime, which a same-length edit written back inside one
    /// filesystem tick does not move; this is what the decoder actually consumed.
    pub source: SourceIdentity,
    /// The installed output this pair read back, when it is a finished result
    /// rather than an encode done for the view.
    pub written: Option<SourceIdentity>,
}

/// One decoded source image for the preview-first view. Opening a file should
/// not run an encoder until the user asks to compare it.
///
/// This is a picture to look at and nothing else. It carries no profile and no
/// claim about depth, because a comparison built from a window buffer would report
/// the size of an eight-bit BGRA round trip rather than the size of the file the
/// writer would produce. A comparison always prepares the source itself.
pub struct Preview {
    pub image: Arc<RenderImage>,
    pub width: u32,
    pub height: u32,
}

impl Pair {
    /// The files this pair was made from, re-read and confirmed still to hold the
    /// exact bytes it consumed.
    ///
    /// The cache key is a size and an mtime taken at the request. That is a cheap
    /// miss, not an authority: a file rewritten to the same length inside one
    /// filesystem tick still stats as the file it was. These are the bytes the
    /// decoder actually read, hashed, so a replacement is a replacement.
    ///
    /// Reads files, so this belongs on a background executor, between the build and
    /// the moment the result reaches the window.
    pub fn confirm(self, source: &Path, written: Option<&Path>) -> Result<Self, Failure> {
        if !self.source.matches_path(source) {
            return Err(Failure::SourceChanged);
        }
        if let (Some(installed), Some(written)) = (self.written.as_ref(), written)
            && !installed.matches_path(written)
        {
            return Err(Failure::SourceChanged);
        }
        Ok(self)
    }

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
    // hand back is the whole source and a comparison can be built from it. A
    // lying header refuses before any decoder allocates on its word; the
    // preview is only a picture, so an over-budget file simply has none.
    if let Some(header) = crate::scan::probe(path) {
        convert::check_budget_bytes(convert::decode_budget_estimate(header.width, header.height))
            .ok()?;
    }
    let image = match crate::thumbs::decode_native(path, None) {
        Some(image) => image,
        None => crate::scan::decode(path)?.into_rgba8(),
    };
    let (width, height) = image.dimensions();
    Some(Preview {
        image: Arc::new(RenderImage::new(vec![Frame::new(to_bgra(image))])),
        width,
        height,
    })
}

/// Decode `path`, encode it at `quality`, and decode that back, so both sides are
/// real pixels rather than a promise.
///
/// The source goes through the same shared preparation the disk writer uses — one
/// bounded snapshot, one decode, one resize, the source's own profile and depth —
/// so the size printed beside the divider is the size the file would actually be.
/// Nothing on screen is a source: the window's BGRA buffer has lost the profile and
/// any depth above eight bits, and re-encoding it would quote a number for a file
/// nobody would ever write.
///
/// When a size budget is set, the *original* side is downscaled too. Comparing a
/// 6400px source against a 2000px export would measure the resize, not the
/// compression, and the resize is not the part you need to eyeball. Both sides are
/// the delivered resolution; only one of them has been through the encoder.
///
/// `avif_speed` is frozen by the request that asked for this comparison, so the
/// bytes reported are the bytes that speed writes even if the setting moves while
/// the encoder runs.
///
/// The pair comes back carrying the identity of the bytes it consumed; an AVIF
/// encode is seconds long, so callers pass it through `Pair::confirm` before it
/// reaches the window.
///
/// Reads files, so this belongs on a background executor.
pub fn build(
    path: &Path,
    format: Format,
    quality: Quality,
    max_edge: MaxEdge,
    avif_speed: u8,
) -> Result<Pair, Failure> {
    let prepared = convert::prepare(path, max_edge)?;
    let (_, encoded) = convert::encode_prepared(&prepared, path, format, quality, avif_speed)?;
    let decoded = crate::scan::decode_bytes(&encoded).ok_or(Failure::Failed)?;
    let (width, height) = (prepared.image.width(), prepared.image.height());
    Ok(Pair {
        // Converted to the window's byte order only now: after the encode, so no
        // display buffer was ever anybody's source.
        original: Arc::new(RenderImage::new(vec![Frame::new(to_bgra(
            prepared.image.into_rgba8(),
        ))])),
        converted: Arc::new(RenderImage::new(vec![Frame::new(to_bgra(
            decoded.into_rgba8(),
        ))])),
        converted_bytes: encoded.len() as u64,
        width,
        height,
        source: prepared.identity,
        written: None,
    })
}

/// The same pair, but the converted side is a file that already exists rather
/// than an encode done for the preview. This is what a finished run produced,
/// read back off disk — bytes and pixels both — so nothing here is a promise
/// about what conversion would do. Both identities come back with it: the source
/// it was made from and the output actually installed.
///
/// The original is brought down to the output's dimensions when a resize was
/// part of the job, for the same reason `build` does it: otherwise the divider
/// measures the resize instead of the compression.
///
/// Reads files, so this belongs on a background executor.
pub fn build_written(source: &Path, written: &Path) -> Result<Pair, Failure> {
    let installed = convert::prepare(written, MaxEdge::FULL)?;
    let (width, height) = (installed.image.width(), installed.image.height());
    let prepared = convert::prepare(source, MaxEdge::FULL)?;
    let original = if prepared.image.width() == width && prepared.image.height() == height {
        prepared.image
    } else {
        prepared
            .image
            .resize_exact(width, height, image::imageops::FilterType::Lanczos3)
    };
    Ok(Pair {
        original: Arc::new(RenderImage::new(vec![Frame::new(to_bgra(
            original.into_rgba8(),
        ))])),
        converted: Arc::new(RenderImage::new(vec![Frame::new(to_bgra(
            installed.image.into_rgba8(),
        ))])),
        // The length of the snapshot that was decoded, not a separate stat that
        // could answer for a different file.
        converted_bytes: installed.identity.bytes,
        width,
        height,
        source: prepared.identity,
        written: Some(installed.identity),
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use image::{DynamicImage, ImageBuffer, Rgb};

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
            crate::avif::DEFAULT_SPEED,
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

    /// A comparison is a claim about the file a run would write. The picture on
    /// screen cannot answer for it: it has been through the window's eight-bit BGRA
    /// buffer and carries no profile. Proved the only way that cannot be faked — the
    /// file is gone by the time the pair is asked for, and no preview can stand in
    /// for it.
    #[test]
    fn a_comparison_is_never_built_from_the_picture_on_screen() {
        let dir = test_dir("reuse");
        let path = dir.join("shot.jpg");
        crate::convert::tests::photo(200, 120).save(&path).unwrap();

        let preview = preview(&path).expect("the JPEG previews");
        assert_eq!((preview.width, preview.height), (200, 120));
        std::fs::remove_file(&path).unwrap();
        drop(preview);

        assert_eq!(
            build(
                &path,
                Format::WebP,
                Quality::lossy(70.),
                MaxEdge::FULL,
                crate::avif::DEFAULT_SPEED,
            )
            .err(),
            Some(convert::Failure::Failed),
            "a comparison answered without reading the source"
        );
    }

    /// The profile the writer attaches comes back with the pixels the comparison
    /// encodes, out of one shared preparation. Without it every wide gamut file is
    /// under-reported by exactly the bytes the writer would add.
    #[test]
    fn a_comparison_carries_the_profile_the_writer_would_attach() {
        let dir = test_dir("profile-parity");
        let source = dir.join("wide.png");
        let out_dir = dir.join("optimized");
        let written = out_dir.join("wide.webp");
        let profile = crate::convert::tests::rgb_profile();
        crate::convert::tests::write_tagged_png(
            &source,
            &crate::convert::tests::photo(64, 48),
            &profile,
        );

        let pair = build(
            &source,
            Format::WebP,
            Quality::lossy(80.),
            MaxEdge::FULL,
            crate::avif::DEFAULT_SPEED,
        )
        .expect("the tagged source compares");
        let run = convert::convert_to(
            &out_dir,
            &source,
            &written,
            None,
            Format::WebP,
            Quality::lossy(80.),
            MaxEdge::FULL,
        )
        .expect("the tagged source converts");

        assert_eq!(
            pair.converted_bytes, run.bytes,
            "the comparison quoted a size the writer does not produce"
        );
        assert_eq!(
            pair.source,
            crate::manifest::SourceIdentity::from_bytes(&std::fs::read(&source).unwrap()),
            "the pair does not name the bytes it consumed"
        );
        assert_eq!(pair.written, None, "nothing was installed for this pair");
    }

    /// One run of the writer, one build of the comparison, over the same file at the
    /// same recipe. `Keep` is the case that used to drift: the picture on screen is
    /// four-channel eight-bit BGRA with no profile, and a comparison built from it
    /// quoted the size of a file the writer would never produce.
    fn writer_and_comparison_agree(
        tag: &str,
        name: &str,
        source_bytes: &[u8],
        format: Format,
        quality: Quality,
    ) -> (Pair, Vec<u8>) {
        let dir = test_dir(tag);
        let source = dir.join(name);
        let out_dir = dir.join("optimized");
        std::fs::write(&source, source_bytes).unwrap();
        let written = out_dir.join(name);

        // The view is already open on this file when the comparison is asked for.
        let preview = preview(&source).expect("the source previews");
        assert!(preview.width > 0);

        let pair = build(
            &source,
            format,
            quality,
            MaxEdge::FULL,
            crate::avif::DEFAULT_SPEED,
        )
        .expect("the comparison builds");
        let run = convert::convert_to(
            &out_dir,
            &source,
            &written,
            None,
            format,
            quality,
            MaxEdge::FULL,
        )
        .expect("the writer converts");
        let installed = std::fs::read(&written).unwrap();

        assert_eq!(
            pair.converted_bytes, run.bytes,
            "the comparison quoted a size the writer does not produce"
        );
        assert_eq!(pair.converted_bytes, installed.len() as u64);
        assert_eq!((pair.width, pair.height), (run.width, run.height));
        (pair, installed)
    }

    /// A grayscale JPEG kept as a JPEG stays one plane. Promoted to RGB through a
    /// display buffer it would be bigger, and the number beside the divider would be
    /// a number for another file.
    #[test]
    fn a_grayscale_jpeg_keeps_its_single_plane_through_comparison_and_writer() {
        let mut source = Vec::new();
        DynamicImage::ImageLuma8(crate::convert::tests::photo(96, 96).to_luma8())
            .write_to(
                &mut std::io::Cursor::new(&mut source),
                image::ImageFormat::Jpeg,
            )
            .unwrap();

        let (_, installed) = writer_and_comparison_agree(
            "gray-keep",
            "gray.jpg",
            &source,
            Format::Same,
            Quality::lossy(80.),
        );
        assert_eq!(
            image::load_from_memory(&installed).unwrap().color(),
            image::ColorType::L8,
            "the kept JPEG was promoted to colour"
        );
    }

    /// Sixteen bits and an ICC profile, kept as a PNG. Both are lost the moment
    /// anything reaches for the window's buffer instead of the file.
    #[test]
    fn a_deep_tagged_png_keeps_its_depth_and_profile_through_comparison_and_writer() {
        let profile = crate::convert::tests::rgb_profile();
        let deep = DynamicImage::ImageRgb16(ImageBuffer::from_fn(48, 32, |x, y| {
            image::Rgb([
                (x * 1367 % 65536) as u16,
                (y * 2039 % 65536) as u16,
                ((x + y) * 811 % 65536) as u16,
            ])
        }));
        let source = convert::encode(&deep, Format::Png, Quality::LOSSLESS, Some(&profile))
            .expect("the tagged sixteen-bit source encodes");

        let (_, installed) = writer_and_comparison_agree(
            "deep-keep",
            "deep.png",
            &source,
            Format::Same,
            Quality::lossy(80.),
        );
        let decoded = image::load_from_memory(&installed).expect("the output decodes");
        assert_eq!(
            decoded.color(),
            image::ColorType::Rgb16,
            "the kept PNG lost its depth"
        );
        assert_eq!(
            crate::scan::decode_for_conversion_from_bytes(&installed, MaxEdge::FULL)
                .expect("the output prepares")
                .profile
                .as_deref(),
            Some(profile.as_slice()),
            "the kept PNG lost its profile"
        );
    }

    /// Real transparency, through the comparison and through the writer. The pixels
    /// under the divider are the output decoded back, so the see-through pixel has
    /// to survive to the screen as well as to disk.
    #[test]
    fn transparency_survives_the_comparison_and_the_written_file_alike() {
        let mut buffer = image::RgbaImage::from_pixel(24, 24, image::Rgba([9, 40, 200, 255]));
        buffer.put_pixel(0, 0, image::Rgba([9, 40, 200, 0]));
        let mut source = Vec::new();
        DynamicImage::ImageRgba8(buffer)
            .write_to(
                &mut std::io::Cursor::new(&mut source),
                image::ImageFormat::Png,
            )
            .unwrap();

        let (pair, installed) = writer_and_comparison_agree(
            "alpha-parity",
            "cutout.png",
            &source,
            Format::WebP,
            Quality::lossy(70.),
        );
        assert_eq!(
            image::load_from_memory(&installed)
                .unwrap()
                .to_rgba8()
                .get_pixel(0, 0)[3],
            0,
            "the written file filled in the see-through pixel"
        );
        let shown = pair
            .converted
            .as_bytes(0)
            .expect("the converted side is drawable");
        assert_eq!(
            shown[3], 0,
            "the pixels under the divider filled in what the file leaves clear"
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
        // Both ends are named by the bytes actually read, and the converted side is
        // the installed file rather than an encode done to describe it.
        assert_eq!(
            pair.source,
            crate::manifest::SourceIdentity::from_bytes(&std::fs::read(&source).unwrap())
        );
        assert_eq!(
            pair.written,
            Some(crate::manifest::SourceIdentity::from_bytes(
                &std::fs::read(&written).unwrap()
            ))
        );

        // Change either end and the comparison is a different comparison. Same
        // length, restored timestamp: the key cannot see this, the identity can.
        let stamp = std::fs::File::open(&written)
            .unwrap()
            .metadata()
            .unwrap()
            .modified()
            .unwrap();
        let before = pair.written.clone().unwrap();
        ImageBuffer::from_fn(60, 40, |x, y| Rgb([x as u8, 200, y as u8]))
            .save(&written)
            .unwrap();
        std::fs::File::options()
            .write(true)
            .open(&written)
            .unwrap()
            .set_modified(stamp)
            .unwrap();
        let replaced = build_written(&source, &written).expect("the new output compares");
        assert_ne!(
            replaced.written.as_ref(),
            Some(&before),
            "the installed output changed and the pair claimed the old one"
        );
        assert_eq!(
            pair.confirm(&source, Some(&written)).err(),
            Some(convert::Failure::SourceChanged),
            "a pair made from bytes that are gone must not land"
        );
    }

    /// The exact failure a size-and-timestamp key cannot see. Both files are the
    /// same length and wear the same mtime, so the key matches; the bytes do not,
    /// and the comparison is about pixels nobody is looking at.
    #[test]
    fn a_same_length_edit_under_the_old_timestamp_is_a_different_source() {
        let dir = test_dir("same-length");
        let path = dir.join("shot.png");
        // Two real PNGs of the same length. Nothing reads past IEND, so the shorter
        // one is padded rather than hand-tuned into an accidental coincidence.
        let png = |shift: u8| {
            let mut bytes = Vec::new();
            DynamicImage::ImageRgb8(ImageBuffer::from_fn(24, 24, |x, y| {
                Rgb([(x as u8).wrapping_add(shift), y as u8, 7])
            }))
            .write_to(
                &mut std::io::Cursor::new(&mut bytes),
                image::ImageFormat::Png,
            )
            .unwrap();
            bytes
        };
        let (mut before, mut after) = (png(0), png(97));
        let length = before.len().max(after.len());
        before.resize(length, 0);
        after.resize(length, 0);
        assert_ne!(before, after);

        std::fs::write(&path, &before).unwrap();
        let stamp = std::fs::metadata(&path).unwrap().modified().unwrap();
        let key = Key::new(
            &path,
            &path,
            Format::WebP,
            Quality::lossy(70.),
            MaxEdge::FULL,
        );
        let pair = build(
            &path,
            Format::WebP,
            Quality::lossy(70.),
            MaxEdge::FULL,
            crate::avif::DEFAULT_SPEED,
        )
        .expect("the first pair builds");
        assert_eq!(
            pair.source,
            crate::manifest::SourceIdentity::from_bytes(&before)
        );

        // Same length, and the timestamp is put back by hand.
        std::fs::write(&path, &after).unwrap();
        std::fs::File::options()
            .write(true)
            .open(&path)
            .unwrap()
            .set_modified(stamp)
            .unwrap();
        assert_eq!(std::fs::metadata(&path).unwrap().len() as usize, length);

        assert!(
            key.fresh(),
            "the stat key is a cheap miss, and this edit is exactly what it misses"
        );
        assert_eq!(
            pair.confirm(&path, None).err(),
            Some(convert::Failure::SourceChanged),
            "the pair was landed over bytes it never saw"
        );
        let rebuilt = build(
            &path,
            Format::WebP,
            Quality::lossy(70.),
            MaxEdge::FULL,
            crate::avif::DEFAULT_SPEED,
        )
        .expect("the current bytes build their own pair");
        assert_eq!(
            rebuilt.source,
            crate::manifest::SourceIdentity::from_bytes(&after)
        );
        rebuilt
            .confirm(&path, None)
            .expect("a pair made from the bytes on disk lands");
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
            crate::avif::DEFAULT_SPEED,
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
            source: crate::manifest::SourceIdentity::from_bytes(b"source"),
            written: None,
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

        for max_edge in [MaxEdge::FULL, MaxEdge(Some(4))] {
            assert_eq!(
                build(
                    &source,
                    Format::WebP,
                    Quality::LOSSLESS,
                    max_edge,
                    crate::avif::DEFAULT_SPEED,
                )
                .err(),
                Some(convert::Failure::LosslessNeedsEightBit),
                "comparison accepted an encode the writer refuses"
            );
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
                crate::avif::DEFAULT_SPEED,
            )
            .is_ok(),
            "an explicit lossy request remains supported"
        );
    }

    /// The writer refuses a gigapixel header without decoding; the comparison
    /// must give the same answer rather than allocating eighty gigabytes to
    /// look at it.
    #[test]
    fn a_gigapixel_header_builds_no_comparison() {
        let dir = tempfile::tempdir().unwrap();
        let source = dir.path().join("huge.png");
        crate::convert::tests::lying_dimensions(&source, 100_000, 100_000);
        assert_eq!(
            build(
                &source,
                Format::WebP,
                Quality::lossy(80.),
                MaxEdge::FULL,
                crate::avif::DEFAULT_SPEED,
            )
            .err(),
            Some(convert::Failure::TooLarge),
            "a 100000x100000 claim never reaches a decoder"
        );
        assert!(
            preview(&source).is_none(),
            "an over-budget file has no preview either"
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
