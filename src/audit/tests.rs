use super::local_ai_actions::local_ai_landing_applies;
use super::media::comparison_landing_applies;
use super::sirv_actions::{
    browser_landing_applies, remember_failure, transfer_failure, walk_landing_applies,
};
use super::studio_actions::studio_landing_applies;
use super::*;
use crate::{
    Launch, WINDOW_DEFAULT_HEIGHT, WINDOW_DEFAULT_WIDTH, WINDOW_MIN_HEIGHT, WINDOW_MIN_WIDTH,
    init_theme, restored_window_size,
};
use gpui_kit::component::Root;
use gpui_kit::{HeadlessAppContext, TestAppContext, size};
use image::ImageFormat;
use std::path::{Path, PathBuf};
use std::sync::{Arc, atomic::Ordering};
use std::time::Duration;

fn retained_scan(audit: &mut Audit) -> Arc<std::sync::atomic::AtomicBool> {
    let cancellation = ScanCancellation::new();
    let token = cancellation.token.clone();
    audit.scan_cancellation = Some(cancellation);
    audit.scanning = Some("old folder".into());
    token
}

fn scan_fixture(name: &str) -> PathBuf {
    scan_fixture_in(&std::env::temp_dir(), name)
}

fn scan_fixture_in(base: &Path, name: &str) -> PathBuf {
    let nonce = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .expect("the system clock is after the Unix epoch")
        .as_nanos();
    let root = base.join(format!("press-audit-{name}-{nonce}"));
    std::fs::create_dir_all(&root).expect("the scan fixture folder is created");
    std::fs::canonicalize(root).expect("the scan fixture has one filesystem identity")
}

fn write_png(root: &Path, name: &str) -> PathBuf {
    let path = root.join(name);
    image::RgbImage::from_pixel(8, 8, image::Rgb([20, 40, 60]))
        .save(&path)
        .expect("the scan fixture image is written");
    path
}

/// A folder of real photographs on disk. Noise, because a flat colour compresses
/// to nothing and would let a projection of zero pass for a saving.
fn photo_fixture(name: &str, count: usize) -> PathBuf {
    let root = scan_fixture(name);
    for index in 0..count {
        crate::convert::tests::photo(64, 64)
            .save(root.join(format!("shot-{index}.png")))
            .expect("the fixture photo is written");
    }
    root
}

fn test_pairing() -> SirvPairing {
    SirvPairing {
        dir: "/paired".into(),
        files: Listing::Ready(HashMap::new()),
        cdn_host: CdnHost::Ready("test.sirv.com".into()),
        client: Arc::new(parking_lot::Mutex::new(sirv::Client::new(
            sirv::Credentials {
                client_id: String::new(),
                client_secret: String::new(),
            },
        ))),
    }
}

#[test]
fn filesystem_root_needs_a_custom_output() {
    let root = Path::new(std::path::MAIN_SEPARATOR_STR);
    assert!(root_needs_custom_output(root, &Output::Optimized));
    assert!(!root_needs_custom_output(
        root,
        &Output::Folder(PathBuf::from("output"))
    ));
}

#[test]
fn home_shortcut_uses_the_navigation_path_identity() {
    let variable = if cfg!(windows) { "USERPROFILE" } else { "HOME" };
    if let Some(home) = std::env::var_os(variable) {
        assert_eq!(browser::home_dir(), Some(navigation_path(home.into())));
    }
}

#[gpui_kit::test]
fn direct_navigation_refuses_the_filesystem_root_with_default_output(cx: &mut TestAppContext) {
    let root = std::env::current_dir()
        .unwrap()
        .ancestors()
        .last()
        .unwrap()
        .to_path_buf();
    let (audit, cx) = finding_audit(cx);

    audit.update(cx, |audit, cx| audit.request_path(root, cx));
    cx.run_until_parked();

    audit.read_with(cx, |audit, _| {
        assert!(audit.root.as_os_str().is_empty());
        assert!(audit.scanning.is_none());
    });
    assert_eq!(notification_count(cx), 1);
}

#[gpui_kit::test]
fn filesystem_root_output_cannot_be_reset_to_optimized(cx: &mut TestAppContext) {
    let root = std::env::current_dir()
        .unwrap()
        .ancestors()
        .last()
        .unwrap()
        .to_path_buf();
    let output = Output::Folder(PathBuf::from("custom-output"));
    let (audit, cx) = finding_audit(cx);

    audit.update(cx, |audit, cx| {
        audit.root = root;
        audit.output = output.clone();
        audit.reset_output(cx);
    });
    cx.run_until_parked();

    audit.read_with(cx, |audit, _| assert_eq!(audit.output, output));
    assert_eq!(notification_count(cx), 1);
}

#[gpui_kit::test]
fn opening_one_file_through_request_paths_cancels_the_active_folder_scan(cx: &mut TestAppContext) {
    let root = scan_fixture("one-file");
    let child = root.join("child");
    std::fs::create_dir_all(&child).unwrap();
    let file = write_png(&root, "one.png");
    let (audit, cx) = finding_audit(cx);
    let old = audit.update_in(cx, |audit, window, cx| {
        let old = retained_scan(audit);
        audit.request_paths(vec![file], window, cx);
        old
    });
    assert!(old.load(Ordering::Acquire));
    cx.run_until_parked();
    audit.read_with(cx, |audit, _| {
        assert_eq!(audit.root, root);
        assert_eq!(audit.folders, vec![child.clone()]);
        assert!(audit.tree_paths.values().any(|path| path == &child));
    });
    std::fs::remove_dir_all(root).expect("the scan fixture is removed");
}
#[gpui_kit::test]
fn opening_one_folder_through_request_paths_cancels_the_active_folder_scan(
    cx: &mut TestAppContext,
) {
    let root = scan_fixture("one-folder");
    write_png(&root, "one.png");
    let (audit, cx) = finding_audit(cx);
    let old = audit.update_in(cx, |audit, window, cx| {
        let old = retained_scan(audit);
        audit.request_paths(vec![root.clone()], window, cx);
        old
    });
    assert!(old.load(Ordering::Acquire));
    cx.run_until_parked();
    std::fs::remove_dir_all(root).expect("the scan fixture is removed");
}
#[gpui_kit::test]
fn opening_many_files_cancels_the_active_folder_scan(cx: &mut TestAppContext) {
    let root = scan_fixture("many-files");
    let first = write_png(&root, "first.png");
    let second = write_png(&root, "second.png");
    let (audit, cx) = finding_audit(cx);
    let old = audit.update_in(cx, |audit, window, cx| {
        let old = retained_scan(audit);
        audit.request_paths(vec![first, second], window, cx);
        old
    });
    assert!(old.load(Ordering::Acquire));
    cx.run_until_parked();
    std::fs::remove_dir_all(root).expect("the scan fixture is removed");
}
#[gpui_kit::test]
fn an_invalid_path_keeps_the_active_folder_scan(cx: &mut TestAppContext) {
    let root = scan_fixture("missing");
    let (audit, cx) = finding_audit(cx);
    let (old, generation) = audit.update_in(cx, |audit, window, cx| {
        let old = retained_scan(audit);
        let generation = audit.scan_generation;
        audit.request_paths(vec![root.join("missing.png")], window, cx);
        (old, generation)
    });
    audit.read_with(cx, |audit, _| {
        assert_eq!(audit.scan_generation, generation);
        assert!(!old.load(Ordering::Acquire));
        assert!(
            audit
                .scan_cancellation
                .as_ref()
                .is_some_and(|current| Arc::ptr_eq(&current.token, &old))
        )
    });
    std::fs::remove_dir_all(root).expect("the scan fixture is removed");
}
#[gpui_kit::test]
fn a_rejected_mixed_root_selection_keeps_the_active_folder_scan(cx: &mut TestAppContext) {
    let first_root = scan_fixture("mixed-first");
    let second_root = scan_fixture("mixed-second");
    let first = write_png(&first_root, "first.png");
    let second = write_png(&second_root, "second.png");
    let (audit, cx) = finding_audit(cx);
    let (old, generation) = audit.update_in(cx, |audit, window, cx| {
        let old = retained_scan(audit);
        let generation = audit.scan_generation;
        audit.request_paths(vec![first, second], window, cx);
        (old, generation)
    });
    audit.read_with(cx, |audit, _| {
        assert_eq!(audit.scan_generation, generation);
        assert!(!old.load(Ordering::Acquire));
        assert!(
            audit
                .scan_cancellation
                .as_ref()
                .is_some_and(|current| Arc::ptr_eq(&current.token, &old))
        )
    });
    std::fs::remove_dir_all(first_root).expect("the first scan fixture is removed");
    std::fs::remove_dir_all(second_root).expect("the second scan fixture is removed");
}
#[gpui_kit::test]
fn an_old_completion_cannot_clear_the_new_scan_handle(cx: &mut TestAppContext) {
    let (audit, cx) = finding_audit(cx);
    audit.update(cx, |audit, _| {
        let old = retained_scan(audit);
        let request = audit.scan_generation;
        let new = ScanCancellation::new();
        audit.scan_cancellation = Some(new);
        assert!(!audit.owns_scan_request(request, Some(&old)));
    });
}
#[gpui_kit::test]
fn same_root_rescan_remains_cancellable(cx: &mut TestAppContext) {
    let root = scan_fixture("same-root");
    write_png(&root, "one.png");
    let (audit, cx) = finding_audit(cx);
    audit.update_in(cx, |audit, window, cx| {
        audit.root = root.clone();
        audit.sirv_pairing = Some(test_pairing());
        audit.request_paths(vec![root.clone()], window, cx);
        assert!(audit.scan_cancellation.is_some());
        audit.cancel_retained_scan();
    });
    cx.run_until_parked();
    std::fs::remove_dir_all(root).expect("the scan fixture is removed");
}
#[gpui_kit::test]
fn a_failed_file_replacement_keeps_the_last_dataset(cx: &mut TestAppContext) {
    let root = scan_fixture("corrupt");
    let corrupt = root.join("corrupt.png");
    std::fs::write(&corrupt, b"not a png").expect("the corrupt fixture is written");
    let (audit, cx) = finding_audit(cx);
    let old = audit.update_in(cx, |audit, window, cx| {
        let old = retained_scan(audit);
        audit.request_paths(vec![corrupt], window, cx);
        old
    });
    cx.run_until_parked();
    audit.read_with(cx, |audit, _| {
        assert!(old.load(Ordering::Acquire));
        assert!(audit.scanning.is_none());
        assert_eq!(audit.dataset_generation, 0);
        assert_eq!(audit.entries.len(), 3);
    });
    std::fs::remove_dir_all(root).expect("the scan fixture is removed");
}
#[gpui_kit::test]
fn an_active_scan_blocks_delivery_actions(cx: &mut TestAppContext) {
    let (audit, cx) = finding_audit(cx);
    audit.update(cx, |audit, _| audit.scanning = Some("partial".into()));
    audit.read_with(cx, |audit, _| assert!(audit.scan_blocks_delivery()));
}
#[gpui_kit::test]
fn a_successful_retry_replaces_the_dataset(cx: &mut TestAppContext) {
    let root = scan_fixture("retry");
    write_png(&root, "retry.png");
    let (audit, cx) = finding_audit(cx);
    let (old, estimates) = audit.update_in(cx, |audit, window, cx| {
        let old = retained_scan(audit);
        let estimates = audit.estimate_generation;
        audit.request_paths(vec![root.clone()], window, cx);
        (old, estimates)
    });
    cx.run_until_parked();
    audit.read_with(cx, |audit, _| {
        assert!(old.load(Ordering::Acquire));
        assert_eq!(audit.root, root);
        assert_eq!(audit.entries.len(), 1);
        assert_eq!(audit.dataset_generation, 1);
        assert!(audit.scanning.is_none());
        assert!(audit.scan_cancellation.is_none());
        assert!(audit.estimate_generation > estimates);
    });
    std::fs::remove_dir_all(root).expect("the scan fixture is removed");
}
#[gpui_kit::test]
fn closing_the_window_cancels_only_its_retained_scan(cx: &mut TestAppContext) {
    let (audit, cx) = finding_audit(cx);
    let token = audit.update(cx, |audit, _| retained_scan(audit));
    let other = cx.add_empty_window();
    other.update(|window, _| window.remove_window());
    other.run_until_parked();
    assert!(!token.load(Ordering::Acquire));
    let _ = other;
    cx.update(|window, _| window.remove_window());
    cx.run_until_parked();
    assert!(token.load(Ordering::Acquire));
}
#[gpui_kit::test]
fn releasing_the_audit_cancels_a_silent_scan(cx: &mut TestAppContext) {
    cx.update(init_theme);
    let mut weak_audit = None;
    let mut token = None;
    let (harness, cx) = cx.add_window_view(|window, cx| {
        let audit = build_audit(finding_launch(), window, cx);
        token = Some(audit.update(cx, |audit, _| retained_scan(audit)));
        weak_audit = Some(audit.downgrade());
        ReleasingAuditHarness { audit: Some(audit) }
    });
    let token = token.expect("the retained scan token is captured");
    let weak_audit = weak_audit.expect("the audit is weakly held for the release check");
    harness.update(cx, |harness, cx| {
        harness.audit = None;
        cx.notify();
    });
    cx.update(|window, cx| window.draw(cx).clear(cx));
    cx.run_until_parked();
    assert!(weak_audit.upgrade().is_none());
    assert!(token.load(Ordering::Acquire));
}
#[gpui_kit::test]
fn a_normal_folder_scan_lands_once(cx: &mut TestAppContext) {
    let root = scan_fixture("normal");
    write_png(&root, "normal.png");
    let (audit, cx) = finding_audit(cx);
    audit.update_in(cx, |audit, window, cx| {
        audit.request_paths(vec![root.clone()], window, cx)
    });
    cx.run_until_parked();
    audit.read_with(cx, |audit, _| {
        assert_eq!(audit.root, root);
        assert_eq!(audit.entries.len(), 1);
        assert_eq!(audit.dataset_generation, 1);
        assert!(audit.scanning.is_none());
        assert!(audit.scan_cancellation.is_none());
    });
    std::fs::remove_dir_all(root).expect("the scan fixture is removed");
}

/// Render the audit window to a PNG, so a change to it can actually be looked at.
///
/// gpui draws the frame to a texture and hands back the pixels, which needs no
/// screen and no screen-recording permission — the alternative was describing the
/// window to someone else and asking them what they saw.
///
///     cargo test --bin press -- --ignored --nocapture screenshot
///
/// Set `IMAGEGUIDE_SHOT_DIR` to choose the folder to audit and `IMAGEGUIDE_SHOT_OUT`
/// to choose where the picture lands.
#[test]
#[ignore = "renders a window; run it deliberately"]
fn screenshot() {
    let folder = std::env::var("IMAGEGUIDE_SHOT_DIR")
        .map(PathBuf::from)
        .unwrap_or_else(|_| std::env::temp_dir().join("imageguide-demo"));
    let out = std::env::var("IMAGEGUIDE_SHOT_OUT")
        .unwrap_or_else(|_| "/tmp/imageguide-shot.png".to_string());
    // Which of the shapes the window can take: list, grid, compare or empty.
    let mode = std::env::var("IMAGEGUIDE_SHOT_MODE").unwrap_or_else(|_| "list".to_string());

    let mut scanned = scan::scan(&folder, &folder.join(scan::OUTPUT_DIR));
    assert!(
        !scanned.entries.is_empty(),
        "{} holds no images to draw",
        folder.display()
    );
    // The empty state only appears for a root that is not a folder at all.
    let root = if mode == "empty" {
        scanned.entries.clear();
        PathBuf::new()
    } else {
        folder
    };

    // A real platform, only for its text system: glyph metrics decide every
    // width in the window, so a fake one would measure a different layout.
    let text_system = gpui_kit::platform::current_platform(true).text_system();
    let mut cx = HeadlessAppContext::with_platform(
        text_system,
        std::sync::Arc::new(gpui_kit::assets::Assets),
        gpui_kit::platform::current_headless_renderer,
    );

    cx.update(init_theme);

    let window = cx
        .open_window(size(px(1100.), px(720.)), |window, cx| {
            let audit = build_audit(
                Launch {
                    root: root.clone(),
                    entries: scanned.entries,
                    skipped_raw: scanned.skipped_raw,
                    skipped_heic: scanned.skipped_heic,
                    skipped_packages: scanned.skipped_packages,
                    unreadable: scanned.unreadable,
                    walk_errors: scanned.walk_errors,
                    existing_output: scanned.existing_output,
                    open_single: mode == "compare",
                    format: Format::WebP,
                    quality: Quality::lossy(80.),
                    max_edge: MaxEdge::FULL,
                    grid: mode == "grid",
                    recent_folders: Vec::new(),
                    columns: ColumnPrefs::default(),
                    output: crate::settings::Output::default(),
                    include_subfolders: false,
                    sidebar_open: true,
                },
                window,
                cx,
            );
            // The rails are part of the window's shape, so they have to be
            // reachable from here too — otherwise the only way to look at one
            // is to run the app and point Sirv at a real account.
            if let Some(rail) = match mode.as_str() {
                "studio" => Some(Rail::Studio),
                "convert" => Some(Rail::Convert),
                _ => None,
            } {
                audit.update(cx, |audit, cx| {
                    audit.rail = rail;
                    cx.notify();
                });
            }
            cx.new(|cx| Root::new(audit, window, cx).bg(cx.theme().background))
        })
        .expect("window opens");

    // Let the thumbnail decodes and the estimate land before drawing. The
    // estimate waits out a settling timer first, so the clock has to move.
    cx.allow_parking();
    cx.run_until_parked();
    cx.advance_clock(ESTIMATE_DELAY + Duration::from_millis(200));
    cx.run_until_parked();
    cx.advance_clock(Duration::from_millis(1200));
    cx.run_until_parked();

    let image = cx
        .capture_screenshot(window.into())
        .expect("frame renders to an image");
    image.save(&out).expect("png writes");
    println!("wrote {out} ({}x{})", image.width(), image.height());
}
struct AuditHarness {
    audit: gpui_kit::Entity<Audit>,
}

impl gpui_kit::Render for AuditHarness {
    fn render(&mut self, _window: &mut Window, _cx: &mut Context<Self>) -> impl IntoElement {
        self.audit.clone()
    }
}

struct ReleasingAuditHarness {
    audit: Option<gpui_kit::Entity<Audit>>,
}

impl gpui_kit::Render for ReleasingAuditHarness {
    fn render(&mut self, _window: &mut Window, _cx: &mut Context<Self>) -> impl IntoElement {
        self.audit
            .clone()
            .map(IntoElement::into_any_element)
            .unwrap_or_else(|| div().into_any_element())
    }
}

fn entry(name: &str, width: u32, height: u32, bytes: u64, format: ImageFormat) -> Entry {
    Entry {
        path: PathBuf::from(name),
        format: format.into(),
        width,
        height,
        bytes,
    }
}

fn names(entries: &[Entry]) -> Vec<String> {
    entries.iter().map(|entry| entry.name()).collect()
}

/// The list is sorted heaviest first, so its outlier is always sample one. Whatever
/// that file does must stop at the slice it was taken from.
#[test]
fn each_slice_is_projected_by_its_own_sample() {
    // A gigabyte of images that compress 100:1, then a gigabyte that does not
    // compress at all.
    let (projected, counted) = project_total(&[
        (1_000_000_000, Some((10_000_000, 100_000))),
        (1_000_000_000, Some((10_000_000, 10_000_000))),
    ])
    .expect("two samples encoded");

    assert_eq!(counted, 2);
    assert_eq!(
        projected, 1_010_000_000,
        "10 MB from the first slice and the whole gigabyte from the second"
    );
    // The summed-bytes ratio this replaced: 10.1 MB of sample from 20 MB of source
    // called the entire 2 GB half its size.
}

#[test]
fn a_slice_whose_sample_would_not_decode_borrows_the_average() {
    let (projected, counted) = project_total(&[
        (100, Some((1000, 100))),
        (100, Some((1000, 300))),
        (100, None),
    ])
    .expect("two of three encoded");

    assert_eq!(counted, 2, "the broken file is not counted as evidence");
    assert_eq!(projected, 10 + 30 + 20, "its slice takes the 0.2 average");
}

#[test]
fn nothing_encoded_is_no_estimate() {
    assert!(project_total(&[(1000, None), (2000, None)]).is_none());
    assert!(project_total(&[]).is_none());
}

#[test]
fn sampling_metadata_only_names_partial_estimates() {
    assert_eq!(panel::sampling_note(2, 2), "");
    assert_eq!(
        panel::sampling_note(2, 20),
        " · 2\u{a0}of\u{a0}20\u{a0}sampled"
    );
}

#[test]
fn fine_tuning_has_one_named_owner() {
    assert_eq!(
        panel::active_preset(Format::WebP, Quality::lossy(80.), MaxEdge::FULL),
        Some(0)
    );
    assert_eq!(
        panel::active_preset(Format::WebP, Quality::lossy(57.), MaxEdge::FULL),
        None,
        "a manual change is a custom configuration, not Recommended"
    );
}

#[test]
fn compact_results_keep_both_byte_values() {
    assert_eq!(
        table::result_size_text(159_100, 128_500, true),
        "155.4 KB → 125.5 KB"
    );
    assert_eq!(table::result_size_text(159_100, 128_500, false), "125.5 KB");
}

#[test]
fn conversion_targets_follow_visible_order() {
    let visible = [2, 0, 1];
    assert!(conversion_targets(&visible, &HashSet::new()).is_empty());

    let selected = HashSet::from([0, 3]);
    assert_eq!(conversion_targets(&visible, &selected), vec![0]);

    let hidden = HashSet::from([3]);
    assert!(conversion_targets(&visible, &hidden).is_empty());
}

#[gpui_kit::test]
fn comparison_navigation_stops_at_visible_edges(cx: &mut TestAppContext) {
    let (audit, cx) = finding_audit(cx);
    audit.read_with(cx, |audit, _| {
        let first = audit.visible[0];
        let middle = audit.visible[1];
        let last = audit.visible[2];

        assert_eq!(audit.compare_target_from(first, -1), None);
        assert_eq!(audit.compare_target_from(first, 1), Some((1, middle)));
        assert_eq!(audit.compare_target_from(middle, -1), Some((0, first)));
        assert_eq!(audit.compare_target_from(middle, 1), Some((2, last)));
        assert_eq!(audit.compare_target_from(last, 1), None);
    });
}

#[gpui_kit::test]
fn the_next_pair_is_built_before_navigation_asks_for_it(cx: &mut TestAppContext) {
    let folder =
        std::env::temp_dir().join(format!("press-compare-prefetch-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&folder);
    std::fs::create_dir_all(&folder).expect("the fixture folder is created");
    for (name, edge) in [("a.png", 40), ("b.png", 30)] {
        image::ImageBuffer::from_fn(edge, edge, |x, y| image::Rgb([x as u8, y as u8, 90]))
            .save(folder.join(name))
            .expect("the fixture image is written");
    }

    cx.update(init_theme);
    let scanned = scan::scan(&folder, &folder.join(scan::OUTPUT_DIR));
    let launch = Launch {
        root: folder.clone(),
        entries: scanned.entries,
        skipped_raw: 0,
        skipped_heic: 0,
        skipped_packages: 0,
        unreadable: Vec::new(),
        walk_errors: Vec::new(),
        existing_output: 0,
        open_single: false,
        format: Format::WebP,
        quality: Quality::lossy(80.),
        max_edge: MaxEdge::FULL,
        grid: false,
        recent_folders: Vec::new(),
        columns: ColumnPrefs::default(),
        output: crate::settings::Output::default(),
        include_subfolders: false,
        sidebar_open: true,
    };
    let (harness, cx) = cx.add_window_view(move |window, cx| AuditHarness {
        audit: build_audit(launch, window, cx),
    });
    let audit = harness.read_with(cx, |harness, _| harness.audit.clone());
    let (first, second, next_path) = audit.read_with(cx, |audit, _| {
        (
            audit.visible[0],
            audit.visible[1],
            audit.entries[audit.visible[1]].path.clone(),
        )
    });

    audit.update(cx, |audit, cx| audit.open_compare(first, cx));
    for _ in 0..2 {
        cx.run_until_parked();
        cx.executor()
            .advance_clock(COMPARE_DELAY + Duration::from_millis(50));
        cx.run_until_parked();
    }
    audit.read_with(cx, |audit, _| {
        assert!(
            audit
                .compare
                .as_ref()
                .is_some_and(|comparison| comparison.pair.is_some())
        );
        assert_eq!(
            audit.ahead.as_ref().map(|(key, _)| key.path.clone()),
            Some(next_path.clone())
        );
    });

    audit.update(cx, |audit, cx| audit.step_compare(1, cx));
    audit.read_with(cx, |audit, _| {
        let comparison = audit.compare.as_ref().expect("the comparison stays open");
        assert_eq!(comparison.index, second);
        assert!(comparison.pair.is_some());
        assert_eq!(
            audit.cached.as_ref().map(|(key, _)| key.path.clone()),
            Some(next_path.clone())
        );
    });

    let optimized = folder.join(scan::OUTPUT_DIR);
    std::fs::create_dir_all(&optimized).expect("the output folder is created");
    let next_output = optimized.join(next_path.file_name().expect("the fixture is a file"));
    audit.update(cx, |audit, _| {
        for index in [first, second] {
            let source = audit.entries[index].path.clone();
            let written = optimized.join(source.file_name().expect("the fixture is a file"));
            std::fs::copy(&source, &written).expect("the output is written");
            audit.result_paths.insert(index, written);
        }
    });
    audit.update(cx, |audit, cx| audit.open_result(first, cx));
    for _ in 0..2 {
        cx.run_until_parked();
        cx.executor()
            .advance_clock(COMPARE_DELAY + Duration::from_millis(50));
        cx.run_until_parked();
    }
    audit.read_with(cx, |audit, _| {
        assert_eq!(
            audit.ahead.as_ref().map(|(key, _)| key.path.clone()),
            Some(next_output)
        );
    });
    audit.update(cx, |audit, cx| audit.step_compare(1, cx));
    audit.read_with(cx, |audit, _| {
        let comparison = audit.compare.as_ref().expect("the results view stays open");
        assert_eq!(comparison.index, second);
        assert!(comparison.pair.is_some());
    });

    let _ = std::fs::remove_dir_all(&folder);
}

#[gpui_kit::test]
fn preview_navigation_adopts_and_promotes_lookahead(cx: &mut TestAppContext) {
    let folder =
        std::env::temp_dir().join(format!("press-preview-prefetch-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&folder);
    std::fs::create_dir_all(&folder).expect("the fixture folder is created");
    for (name, edge) in [("a.png", 40), ("b.png", 30), ("c.png", 20)] {
        image::ImageBuffer::from_fn(edge, edge, |x, y| image::Rgb([x as u8, y as u8, 90]))
            .save(folder.join(name))
            .expect("the fixture image is written");
    }

    cx.update(init_theme);
    let scanned = scan::scan(&folder, &folder.join(scan::OUTPUT_DIR));
    let launch = Launch {
        root: folder.clone(),
        entries: scanned.entries,
        skipped_raw: 0,
        skipped_heic: 0,
        skipped_packages: 0,
        unreadable: Vec::new(),
        walk_errors: Vec::new(),
        existing_output: 0,
        open_single: false,
        format: Format::WebP,
        quality: Quality::lossy(80.),
        max_edge: MaxEdge::FULL,
        grid: false,
        recent_folders: Vec::new(),
        columns: ColumnPrefs::default(),
        output: crate::settings::Output::default(),
        include_subfolders: false,
        sidebar_open: true,
    };
    let (harness, cx) = cx.add_window_view(move |window, cx| AuditHarness {
        audit: build_audit(launch, window, cx),
    });
    let audit = harness.read_with(cx, |harness, _| harness.audit.clone());
    let (first, second, third, second_path, third_path) = audit.read_with(cx, |audit, _| {
        let [first, second, third, ..] = audit.visible.as_slice() else {
            panic!("the fixture has three visible images");
        };
        (
            *first,
            *second,
            *third,
            audit.entries[*second].path.clone(),
            audit.entries[*third].path.clone(),
        )
    });

    audit.update(cx, |audit, cx| audit.open_preview(first, cx));
    cx.run_until_parked();
    audit.update(cx, |audit, cx| audit.step_compare(1, cx));
    audit.read_with(cx, |audit, _| {
        let comparison = audit.compare.as_ref().expect("the preview stays open");
        assert_eq!(comparison.index, second);
        assert!(
            audit.prefetch_key.as_ref().is_some_and(|(key, mode)| {
                key.path == second_path && *mode == MediaMode::Preview
            })
        );
    });

    cx.executor()
        .advance_clock(PREVIEW_DELAY + Duration::from_millis(50));
    cx.run_until_parked();
    audit.read_with(cx, |audit, _| {
        let comparison = audit.compare.as_ref().expect("the preview stays open");
        assert!(comparison.preview.is_some());
        assert!(audit.cached.as_ref().is_some_and(|(key, media)| {
            key.path == second_path && matches!(media, CachedMedia::Preview(_))
        }));
    });

    cx.executor()
        .advance_clock(PREVIEW_DELAY + Duration::from_millis(50));
    cx.run_until_parked();
    let prefetched = audit.read_with(cx, |audit, _| match audit.ahead.as_ref() {
        Some((key, CachedMedia::Preview(preview))) if key.path == third_path => preview.clone(),
        _ => panic!("the next full-resolution preview is ready"),
    });

    audit.update(cx, |audit, cx| audit.step_compare(1, cx));
    audit.read_with(cx, |audit, _| {
        let comparison = audit.compare.as_ref().expect("the preview stays open");
        assert_eq!(comparison.index, third);
        let shown = comparison
            .preview
            .as_ref()
            .expect("the preview is immediate");
        assert!(Arc::ptr_eq(shown, &prefetched));
    });

    let _ = std::fs::remove_dir_all(&folder);
}

#[test]
fn push_plan_lists_only_files_sirv_lacks() {
    let entries = vec![
        entry("photos/local.jpg", 1, 1, 10, ImageFormat::Jpeg),
        entry("photos/same.jpg", 1, 1, 20, ImageFormat::Jpeg),
        entry("photos/changed.jpg", 1, 1, 30, ImageFormat::Jpeg),
    ];
    let files = HashMap::from([
        (
            "same.jpg".into(),
            sirv::Node {
                filename: "/d/same.jpg".into(),
                is_directory: false,
                kind: None,
                size: 20,
            },
        ),
        (
            "changed.jpg".into(),
            sirv::Node {
                filename: "/d/changed.jpg".into(),
                is_directory: false,
                kind: None,
                size: 31,
            },
        ),
    ]);

    assert_eq!(
        sirv_push_plan(
            Path::new("photos"),
            &entries,
            &files,
            sirv::SyncState::OnlyLocal,
        ),
        [("local.jpg".into(), PathBuf::from("photos/local.jpg"))]
    );
}

#[test]
fn the_forced_push_plan_takes_changed_files_and_leaves_synced_ones() {
    let entries = vec![
        entry("photos/same.jpg", 1, 1, 20, ImageFormat::Jpeg),
        entry("photos/changed.jpg", 1, 1, 30, ImageFormat::Jpeg),
    ];
    let files = HashMap::from([
        (
            "same.jpg".into(),
            sirv::Node {
                filename: "/d/same.jpg".into(),
                is_directory: false,
                kind: None,
                size: 20,
            },
        ),
        (
            "changed.jpg".into(),
            sirv::Node {
                filename: "/d/changed.jpg".into(),
                is_directory: false,
                kind: None,
                size: 31,
            },
        ),
    ]);

    assert_eq!(
        sirv_push_plan(
            Path::new("photos"),
            &entries,
            &files,
            sirv::SyncState::Changed,
        ),
        [("changed.jpg".into(), PathBuf::from("photos/changed.jpg"))]
    );
}

#[test]
fn conversion_progress_publishes_by_worker_window_and_flushes_the_tail() {
    assert!(!progress_batch_ready(7, 8, true));
    assert!(progress_batch_ready(8, 8, true));
    assert!(progress_batch_ready(3, 8, false));
}

#[test]
fn a_comparison_result_only_belongs_to_its_exact_request() {
    let key = compare::Key::new(
        Path::new("photo.jpg"),
        Format::WebP,
        Quality::lossy(80.),
        MaxEdge::FULL,
    );
    let comparison = Comparison {
        index: 2,
        dataset_generation: 7,
        mode: MediaMode::Compare,
        focused: false,
        key: key.clone(),
        preview: None,
        pair: None,
        failed: false,
        split: 0.5,
        pan: (0., 0.),
        zoom: None,
        drag: None,
        written: None,
        produced_by: None,
    };

    assert!(comparison_landing_applies(
        Some(&comparison),
        2,
        7,
        MediaMode::Compare,
        &key
    ));
    assert!(!comparison_landing_applies(
        Some(&comparison),
        2,
        8,
        MediaMode::Compare,
        &key
    ));
    assert!(!comparison_landing_applies(
        Some(&comparison),
        2,
        7,
        MediaMode::Preview,
        &key
    ));
}

#[test]
fn marquee_bounds_work_in_every_drag_direction() {
    let marquee = Marquee {
        start: (90., 80.),
        current: (20., 30.),
        base: HashSet::new(),
        toggle: false,
    };

    let bounds = marquee.bounds();
    assert_eq!(bounds.origin, gpui_kit::point(px(20.), px(30.)));
    assert_eq!(bounds.size, gpui_kit::size(px(70.), px(50.)));
}

#[test]
fn a_local_ai_result_belongs_to_its_exact_file_and_dataset() {
    let cancelled = Arc::new(std::sync::atomic::AtomicBool::new(false));
    let job = LocalAiJob {
        tool: local_ai::Tool::RemoveBackground,
        index: 2,
        dataset_generation: 7,
        source_name: "photo.jpg".into(),
        first_setup: false,
        state: LocalAiJobState::Running,
        cancelled: cancelled.clone(),
    };

    assert!(local_ai_landing_applies(
        Some(&job),
        2,
        7,
        local_ai::Tool::RemoveBackground
    ));
    assert!(!local_ai_landing_applies(
        Some(&job),
        2,
        8,
        local_ai::Tool::RemoveBackground
    ));
    assert!(!local_ai_landing_applies(
        Some(&job),
        2,
        7,
        local_ai::Tool::Upscale
    ));
    cancelled.store(true, std::sync::atomic::Ordering::Relaxed);
    assert!(!local_ai_landing_applies(
        Some(&job),
        2,
        7,
        local_ai::Tool::RemoveBackground
    ));
}

#[test]
fn a_studio_result_belongs_to_its_exact_file_and_dataset() {
    let cancelled = Arc::new(std::sync::atomic::AtomicBool::new(false));
    let job = StudioJob {
        tool: studio::Tool::ReplaceBackground,
        index: 2,
        dataset_generation: 7,
        source_name: "photo.jpg".into(),
        output_source: PathBuf::from("photo.jpg"),
        prompt: "white background".into(),
        state: StudioJobState::Running,
        cancelled: cancelled.clone(),
    };

    assert!(studio_landing_applies(
        Some(&job),
        2,
        7,
        studio::Tool::ReplaceBackground
    ));
    assert!(!studio_landing_applies(
        Some(&job),
        2,
        8,
        studio::Tool::ReplaceBackground
    ));
    assert!(!studio_landing_applies(
        Some(&job),
        2,
        7,
        studio::Tool::Upscale
    ));
    cancelled.store(true, std::sync::atomic::Ordering::Relaxed);
    assert!(!studio_landing_applies(
        Some(&job),
        2,
        7,
        studio::Tool::ReplaceBackground
    ));
}

#[test]
fn table_layout_keeps_decision_columns_at_compact_width() {
    let prefs = ColumnPrefs::default();
    // The narrowest the list ever gets: the minimum window with a rail open.
    let minimum_left_pane = WINDOW_MIN_WIDTH - panel::RAIL_WIDTH - 44.;
    let (_, narrow_name, narrow_columns) =
        AuditTable::layout(minimum_left_pane, prefs, true, false);
    assert!(narrow_name >= W_NAME_MIN);
    // What survives is the file and what happened to it.
    assert!(narrow_columns.contains(&TableColumn::Name));
    assert!(narrow_columns.contains(&TableColumn::Result));
    assert!(!narrow_columns.contains(&TableColumn::Pixels));
    assert!(!narrow_columns.contains(&TableColumn::Weight));

    let (_, _, before_columns) = AuditTable::layout(minimum_left_pane, prefs, false, false);
    assert!(before_columns.contains(&TableColumn::Weight));
    assert!(!before_columns.contains(&TableColumn::Result));

    let (wide, wide_name, wide_columns) = AuditTable::layout(1100., prefs, true, false);
    assert!(!wide);
    assert!(wide_name > narrow_name);
    assert!(wide_columns.contains(&TableColumn::Format));
    assert!(wide_columns.contains(&TableColumn::Weight));
    assert!(wide_columns.contains(&TableColumn::Result));
    assert!(!wide_columns.contains(&TableColumn::Sync));

    let (_, _, synced_columns) = AuditTable::layout(1100., prefs, false, true);
    assert!(synced_columns.contains(&TableColumn::Sync));
}

/// The picker is the only thing that decides an optional column, and B/px is
/// the one that starts off. Every layout ends with the gutter that opens it.
#[test]
fn column_preferences_decide_the_optional_columns() {
    let prefs = ColumnPrefs::default();
    let (_, _, default_columns) = AuditTable::layout(1100., prefs, false, false);
    assert!(!default_columns.contains(&TableColumn::Density));
    assert_eq!(default_columns.last(), Some(&TableColumn::Options));

    let with_density = ColumnPrefs {
        density: true,
        ..prefs
    };
    let (_, _, dense_columns) = AuditTable::layout(1100., with_density, false, false);
    assert!(dense_columns.contains(&TableColumn::Density));

    // Every optional column off leaves the tick, the name and the gutter, and
    // the name takes the room the others gave up.
    let bare = ColumnPrefs {
        thumb: false,
        format: false,
        pixels: false,
        density: false,
        weight: false,
    };
    let (_, bare_name, bare_columns) = AuditTable::layout(1100., bare, false, false);
    assert_eq!(
        bare_columns,
        vec![TableColumn::Tick, TableColumn::Name, TableColumn::Options]
    );
    let (_, default_name, _) = AuditTable::layout(1100., prefs, false, false);
    assert!(bare_name > default_name);
}

#[gpui_kit::test]
fn table_select_all_follows_the_visible_rows(cx: &mut TestAppContext) {
    let (audit, cx) = finding_audit(cx);
    audit.update(cx, |audit, cx| {
        audit.visible = vec![2, 0];
        // A folder now opens fully ticked, and this test is about the control
        // itself, so it starts from nothing ticked.
        audit.selected.clear();
        assert!(matches!(
            audit.selection_state(),
            table::SelectionState::None
        ));

        audit.toggle_select_all(cx);
        assert_eq!(audit.selected, HashSet::from([0, 2]));
        assert!(matches!(
            audit.selection_state(),
            table::SelectionState::All
        ));

        audit.selected.remove(&2);
        assert!(matches!(
            audit.selection_state(),
            table::SelectionState::Some
        ));
        audit.selected.insert(2);
        audit.toggle_select_all(cx);
        assert!(audit.selected.is_empty());
    });
}

/// The app sorts indices into an unmoved `entries`; these tests sort the data
/// directly, which is the same comparator either way.
fn sort_entries(entries: &mut [Entry], sort: Sort) {
    entries.sort_by(|a, b| {
        compare_entries(
            a,
            b,
            sort,
            &a.name_lossy(),
            &b.name_lossy(),
            &a.name_lossy().to_lowercase(),
            &b.name_lossy().to_lowercase(),
        )
    });
}

#[test]
fn batch_name_sorting_uses_the_displayed_relative_path() {
    let first = entry("a.png", 1, 1, 1, ImageFormat::Png);
    let second = entry("z.png", 1, 1, 1, ImageFormat::Png);
    let sort = Sort {
        column: Column::Name,
        descending: false,
    };

    assert_eq!(
        compare_entries(
            &first, &second, sort, "z/a.png", "a/z.png", "z/a.png", "a/z.png"
        ),
        std::cmp::Ordering::Greater
    );
}

/// `img` will not scale an image past its own size, so a thumbnail smaller than the
/// slot it is drawn in does not fill it — it sits in the middle of the empty space.
/// The gallery looked like that at 96px in a 224px tile. The two constants live in
/// different modules, so this is what stops them drifting apart again.
#[test]
fn the_gallery_never_asks_for_more_than_a_thumbnail_holds() {
    // `tile` draws the image inside the tile's own padding.
    let widest = TILE_MAX - 16.;
    assert!(
        widest <= thumbs::THUMB_EDGE as f32,
        "a {TILE_MAX}px tile draws an image {widest}px wide, \
         and thumbnails are only {}px",
        thumbs::THUMB_EDGE
    );
}

/// Names before counts, wherever the window reports a set of files it could not
/// handle. "3 would not decode" gives you nowhere to look.
#[test]
fn a_report_names_a_few_files_and_then_counts_the_rest() {
    let of = |names: &[&str]| named(names.iter().map(|name| name.to_string()));
    assert_eq!(of(&[]), "");
    assert_eq!(of(&["a.png"]), "a.png");
    assert_eq!(of(&["a.png", "b.png", "c.png"]), "a.png, b.png, c.png");
    assert_eq!(
        of(&["a.png", "b.png", "c.png", "d.png", "e.png"]),
        "a.png, b.png, c.png and 2 more"
    );
}

/// The audit's findings have to be reachable. Narrowing to one shows those rows and
/// nothing else, and asking for the same one again widens the list back out.
#[gpui_kit::test]
fn a_finding_narrows_the_list_and_a_second_click_widens_it(cx: &mut TestAppContext) {
    let (audit, cx) = finding_audit(cx);
    let shown = |audit: &Audit| -> Vec<String> {
        audit
            .visible
            .iter()
            .filter_map(|index| audit.entries.get(*index))
            .map(|entry| entry.name())
            .collect()
    };

    audit.update(cx, |audit, cx| {
        assert_eq!(audit.visible.len(), 3, "everything, to begin with");

        audit.set_finding(Finding::Mislabelled, cx);
        assert_eq!(
            shown(audit),
            ["liar.webp"],
            "only the file whose extension disagrees with its bytes"
        );

        audit.set_finding(Finding::Heavy, cx);
        assert_eq!(
            shown(audit),
            ["screenshot.png"],
            "one finding at a time, and heavy means bytes per pixel"
        );

        audit.set_finding(Finding::Heavy, cx);
        assert_eq!(audit.visible.len(), 3, "asking again puts the list back");
    });
}

/// Unpairing retires the loop before dropping its status, so the next loop
/// check cannot keep uploading into a folder this window no longer owns.
#[gpui_kit::test]
fn unpairing_discards_the_job_and_stops_the_loop(cx: &mut TestAppContext) {
    let (audit, cx) = finding_audit(cx);

    audit.update(cx, |audit, cx| {
        audit.sirv_job = Some(SirvJob {
            kind: SirvJobKind::Push,
            done: 3,
            total: 100,
            failed: 0,
            failures: Vec::new(),
            current: None,
            finished: false,
            stopping: false,
            generation: audit.sirv_generation,
        });
        let running = audit.sirv_generation;

        audit.unpair_sirv(cx);

        assert_ne!(
            audit.sirv_generation, running,
            "the loop's next check has to fail"
        );
        assert!(audit.sirv_job.is_none());
        assert!(audit.sirv_pairing.is_none());
    });
}

#[gpui_kit::test]
fn repairing_stops_a_running_transfer(cx: &mut TestAppContext) {
    let (audit, cx) = finding_audit(cx);

    audit.update(cx, |audit, cx| {
        audit.sirv_job = Some(SirvJob {
            kind: SirvJobKind::Push,
            done: 3,
            total: 100,
            failed: 0,
            failures: Vec::new(),
            current: None,
            finished: false,
            stopping: false,
            generation: audit.sirv_generation,
        });
        audit.sirv_browser = Some(SirvBrowser {
            client: Arc::new(parking_lot::Mutex::new(sirv::Client::new(
                sirv::Credentials {
                    client_id: String::new(),
                    client_secret: String::new(),
                },
            ))),
            path: "/photos".into(),
            needs_credentials: false,
            nodes: None,
            generation: 0,
            session: 1,
            focused: false,
            focus: cx.focus_handle(),
        });

        audit.pair_sirv(cx);

        let job = audit
            .sirv_job
            .as_ref()
            .expect("the retiring job stays busy");
        assert!(
            job.stopping,
            "the loop acknowledges after its in-flight file"
        );
        assert_eq!(audit.sirv_pairing.as_ref().unwrap().dir, "/photos");
    });
}

#[test]
fn a_listing_from_a_closed_browser_cannot_land_in_its_replacement() {
    assert!(!browser_landing_applies(4, 5, 1, 1, "/photos", "/photos"));
    assert!(browser_landing_applies(5, 5, 2, 2, "/photos", "/photos"));
}

#[test]
fn sirv_jobs_count_every_failure_but_keep_three_examples() {
    let mut count = 0;
    let mut examples = Vec::new();
    for index in 0..5 {
        remember_failure(&mut count, &mut examples, format!("file-{index}"));
    }

    assert_eq!(count, 5);
    assert_eq!(examples, ["file-0", "file-1", "file-2"]);

    let job = SirvJob {
        kind: SirvJobKind::Push,
        done: 5,
        total: 5,
        failed: count,
        failures: examples,
        current: None,
        finished: true,
        stopping: false,
        generation: 1,
    };
    assert_eq!(
        transfer_failure(&job),
        Some((
            "Sirv upload incomplete",
            "5 of 5 failed: file-0, file-1, file-2 and 2 more".into()
        ))
    );
}

fn notification_audit(
    cx: &mut TestAppContext,
    entries: Vec<Entry>,
) -> (gpui_kit::Entity<Audit>, &mut gpui_kit::VisualTestContext) {
    cx.update(init_theme);
    let audit_entity = Rc::new(RefCell::new(None));
    let capture = audit_entity.clone();
    let (_, cx) = cx.add_window_view(move |window, cx| {
        let audit = build_audit(
            Launch {
                root: PathBuf::new(),
                entries,
                skipped_raw: 0,
                skipped_heic: 0,
                skipped_packages: 0,
                unreadable: Vec::new(),
                walk_errors: Vec::new(),
                existing_output: 0,
                open_single: false,
                format: Format::WebP,
                quality: Quality::lossy(80.),
                max_edge: MaxEdge::FULL,
                grid: false,
                recent_folders: Vec::new(),
                columns: ColumnPrefs::default(),
                output: crate::settings::Output::default(),
                include_subfolders: false,
                sidebar_open: true,
            },
            window,
            cx,
        );
        *capture.borrow_mut() = Some(audit.clone());
        let content = cx.new(|_| crate::WindowContent { audit });
        Root::new(content, window, cx).bg(cx.theme().background)
    });
    let audit = audit_entity
        .borrow_mut()
        .take()
        .expect("audit is built for the production Root");
    (audit, cx)
}

fn notification_count(cx: &mut gpui_kit::VisualTestContext) -> usize {
    let mut count = 0;
    cx.update(|window, cx| count = window.notifications(cx).len());
    count
}

fn finish_notification_exit(cx: &mut gpui_kit::VisualTestContext) {
    cx.executor().advance_clock(Duration::from_millis(250));
    cx.run_until_parked();
}

#[gpui_kit::test]
fn a_newer_error_replaces_the_old_content_in_its_scope(cx: &mut TestAppContext) {
    let (audit, cx) = notification_audit(cx, Vec::new());

    audit.update(cx, |audit, cx| {
        audit.notify_error("conversion", "First error", "first detail", cx);
    });
    cx.run_until_parked();
    assert_eq!(notification_count(cx), 1);

    audit.update(cx, |audit, cx| {
        audit.clear_error("conversion", cx);
        audit.notify_error("conversion", "Latest error", "latest detail", cx);
    });
    cx.run_until_parked();

    assert_eq!(notification_count(cx), 1);
    assert!(
        cx.debug_bounds("error-toast-message:latest detail")
            .is_some()
    );
    assert!(
        cx.debug_bounds("error-toast-message:first detail")
            .is_none()
    );
}

#[gpui_kit::test]
fn a_successful_retry_removes_its_old_error(cx: &mut TestAppContext) {
    let (audit, cx) = notification_audit(cx, Vec::new());
    audit.update(cx, |audit, cx| {
        audit.notify_error("conversion", "Conversion incomplete", "disk was full", cx);
    });
    cx.run_until_parked();
    assert_eq!(notification_count(cx), 1);

    audit.update(cx, |audit, cx| audit.clear_error("conversion", cx));
    cx.run_until_parked();
    finish_notification_exit(cx);

    assert_eq!(notification_count(cx), 0);
    assert!(
        cx.debug_bounds("error-toast-message:disk was full")
            .is_none()
    );
}

#[gpui_kit::test]
fn errors_from_different_scopes_coexist(cx: &mut TestAppContext) {
    let (audit, cx) = notification_audit(cx, Vec::new());
    audit.update(cx, |audit, cx| {
        audit.notify_error("conversion", "Conversion incomplete", "decode failed", cx);
        audit.notify_error("settings", "Couldn’t save settings", "read-only folder", cx);
    });
    cx.run_until_parked();

    assert_eq!(notification_count(cx), 2);
    assert!(
        cx.debug_bounds("error-toast-message:decode failed")
            .is_some()
    );
    assert!(
        cx.debug_bounds("error-toast-message:read-only folder")
            .is_some()
    );
}

#[gpui_kit::test]
fn installing_a_dataset_announces_unreadable_files_once_in_a_toast(cx: &mut TestAppContext) {
    let (audit, cx) = notification_audit(cx, Vec::new());
    let scan = || scan::Scan {
        entries: Vec::new(),
        skipped_raw: 0,
        skipped_heic: 0,
        skipped_packages: 0,
        unreadable: vec![PathBuf::from("a.jpg"), PathBuf::from("b.jpg")],
        walk_errors: vec![PathBuf::from("locked")],
        existing_output: 0,
    };
    audit.update_in(cx, |audit, window, cx| {
        audit.install_dataset(scan(), PathBuf::from("/photos"), false, None, window, cx);
    });
    cx.run_until_parked();
    assert_eq!(notification_count(cx), 1);
    assert!(
        cx.debug_bounds(
            "error-toast-message:would not decode: a.jpg, b.jpg  ·  could not enter: locked"
        )
        .is_some()
    );
    // A second landing replaces the toast rather than stacking another.
    audit.update_in(cx, |audit, window, cx| {
        audit.install_dataset(scan(), PathBuf::from("/photos"), false, None, window, cx);
    });
    cx.run_until_parked();
    assert_eq!(notification_count(cx), 1);
}

#[gpui_kit::test]
fn a_clean_dataset_replaces_the_scan_error_with_its_totals(cx: &mut TestAppContext) {
    let (audit, cx) = notification_audit(cx, Vec::new());
    audit.update_in(cx, |audit, window, cx| {
        audit.install_dataset(
            scan::Scan {
                entries: Vec::new(),
                skipped_raw: 0,
                skipped_heic: 0,
                skipped_packages: 0,
                unreadable: vec![PathBuf::from("a.jpg")],
                walk_errors: Vec::new(),
                existing_output: 0,
            },
            PathBuf::from("/photos"),
            false,
            None,
            window,
            cx,
        );
    });
    cx.run_until_parked();
    assert_eq!(notification_count(cx), 1);
    audit.update_in(cx, |audit, window, cx| {
        audit.install_dataset(
            scan::Scan {
                entries: Vec::new(),
                skipped_raw: 0,
                skipped_heic: 0,
                skipped_packages: 0,
                unreadable: Vec::new(),
                walk_errors: Vec::new(),
                existing_output: 0,
            },
            PathBuf::from("/photos"),
            false,
            None,
            window,
            cx,
        );
    });
    cx.run_until_parked();
    // The error is gone; the clean folder announces its (empty) totals in the
    // same scope instead.
    assert_eq!(notification_count(cx), 1);
    assert!(
        cx.debug_bounds("error-toast-message:0 images").is_some(),
        "a clean folder toasts its counts, not the old damage"
    );
    assert!(
        cx.debug_bounds("error-toast-message:would not decode: a.jpg")
            .is_none()
    );
}

#[gpui_kit::test]
fn superseded_media_cannot_publish_an_error_into_the_new_dataset(cx: &mut TestAppContext) {
    let (audit, cx) = notification_audit(
        cx,
        vec![entry("missing.png", 10, 10, 100, ImageFormat::Png)],
    );
    audit.update(cx, |audit, cx| audit.open_preview(0, cx));
    cx.update(|window, cx| {
        audit.update(cx, |audit, cx| {
            audit.install_dataset(
                scan::Scan {
                    entries: Vec::new(),
                    skipped_raw: 0,
                    skipped_heic: 0,
                    skipped_packages: 0,
                    unreadable: Vec::new(),
                    walk_errors: Vec::new(),
                    existing_output: 0,
                },
                PathBuf::from("replacement"),
                false,
                None,
                window,
                cx,
            );
        });
    });
    cx.executor()
        .advance_clock(PREVIEW_DELAY + Duration::from_millis(50));
    cx.run_until_parked();

    // The only toast is the new folder's totals: the superseded preview's
    // failure never reaches the new dataset.
    assert_eq!(notification_count(cx), 1);
    assert!(
        cx.debug_bounds("error-toast-message:0 images").is_some(),
        "a clean folder toasts its counts"
    );
}

#[gpui_kit::test]
fn unpairing_clears_the_finished_job(cx: &mut TestAppContext) {
    let (audit, cx) = finding_audit(cx);

    audit.update(cx, |audit, cx| {
        audit.sirv_job = Some(SirvJob {
            kind: SirvJobKind::Pull,
            done: 1,
            total: 1,
            failed: 0,
            failures: Vec::new(),
            current: None,
            finished: true,
            stopping: false,
            generation: audit.sirv_generation,
        });

        audit.unpair_sirv(cx);

        assert!(audit.sirv_job.is_none());
    });
}

#[gpui_kit::test]
fn sirv_difference_filters_match_their_category_counts(cx: &mut TestAppContext) {
    let (audit, cx) = finding_audit(cx);
    audit.update(cx, |audit, cx| {
        let files = HashMap::from([
            (
                "photo.jpg".to_string(),
                sirv::Node {
                    filename: "/photos/photo.jpg".into(),
                    size: 100_000,
                    is_directory: false,
                    kind: None,
                },
            ),
            (
                "screenshot.png".to_string(),
                sirv::Node {
                    filename: "/photos/screenshot.png".into(),
                    size: 200_000,
                    is_directory: false,
                    kind: None,
                },
            ),
            (
                "remote.jpg".to_string(),
                sirv::Node {
                    filename: "/photos/remote.jpg".into(),
                    size: 50_000,
                    is_directory: false,
                    kind: None,
                },
            ),
        ]);
        audit.sirv_pairing = Some(SirvPairing {
            dir: "/photos".into(),
            files: Listing::Ready(files),
            cdn_host: CdnHost::Ready("test.sirv.com".into()),
            client: Arc::new(parking_lot::Mutex::new(sirv::Client::new(
                sirv::Credentials {
                    client_id: String::new(),
                    client_secret: String::new(),
                },
            ))),
        });
        audit.sirv_local_presence =
            HashSet::from(["photo.jpg".to_string(), "screenshot.png".to_string()]);
        audit.refresh_sirv_counts();

        assert_eq!(audit.sirv_counts, Some((1, 1, 1)));
        assert_eq!(audit.sirv_remote_only, ["remote.jpg"]);

        audit.set_sirv_scope(SirvScope::OnlyLocal, cx);
        assert_eq!(audit.entries[audit.visible[0]].name_lossy(), "liar.webp");
        audit.set_sirv_scope(SirvScope::Changed, cx);
        assert_eq!(
            audit.entries[audit.visible[0]].name_lossy(),
            "screenshot.png"
        );
        audit.set_sirv_scope(SirvScope::OnlyRemote, cx);
        assert!(audit.visible.is_empty());
        assert_eq!(audit.sirv_remote_only, ["remote.jpg"]);
    });
}

#[gpui_kit::test]
fn new_credentials_retire_the_old_listing(cx: &mut TestAppContext) {
    let (audit, cx) = finding_audit(cx);
    audit.update(cx, |audit, cx| {
        let old_client = Arc::new(parking_lot::Mutex::new(sirv::Client::new(
            sirv::Credentials {
                client_id: "old-id".into(),
                client_secret: "old-secret".into(),
            },
        )));
        audit.sirv_pairing = Some(SirvPairing {
            dir: "/photos".into(),
            files: Listing::Ready(HashMap::new()),
            cdn_host: CdnHost::Ready("old.sirv.com".into()),
            client: old_client.clone(),
        });
        audit.sirv_local_presence.insert("old.jpg".into());
        audit.sirv_counts = Some((1, 1, 1));
        audit.sirv_job = Some(SirvJob {
            kind: SirvJobKind::Push,
            done: 3,
            total: 100,
            failed: 0,
            failures: Vec::new(),
            current: None,
            finished: false,
            stopping: false,
            generation: audit.sirv_generation,
        });
        let generation_before = audit.sirv_pairing_generation;

        audit.adopt_new_credentials(
            sirv::Credentials {
                client_id: "new-id".into(),
                client_secret: "new-secret".into(),
            },
            cx,
        );

        let pairing = audit.sirv_pairing.as_ref().unwrap();
        assert!(matches!(pairing.files, Listing::Walking));
        assert!(matches!(pairing.cdn_host, CdnHost::Loading));
        assert!(audit.sirv_local_presence.is_empty());
        assert!(audit.sirv_counts.is_none());
        assert!(!Arc::ptr_eq(&pairing.client, &old_client));
        assert!(audit.sirv_job.as_ref().unwrap().stopping);
        // This rejects stale walks through walk_landing_applies.
        assert_ne!(audit.sirv_pairing_generation, generation_before);
    });
}

#[test]
fn a_walk_from_a_previous_pairing_lands_nowhere() {
    assert!(!walk_landing_applies(1, 2, 3, 3));
    assert!(!walk_landing_applies(2, 1, 3, 3));
    assert!(!walk_landing_applies(1, 1, 3, 4));
    assert!(!walk_landing_applies(1, 1, 4, 3));
}

#[test]
fn a_current_walk_lands() {
    assert!(walk_landing_applies(1, 1, 2, 2));
}

/// A finding belongs to the folder it was found in.
#[gpui_kit::test]
fn opening_another_folder_clears_the_finding(cx: &mut TestAppContext) {
    let (audit, cx) = finding_audit(cx);
    audit.update(cx, |audit, cx| audit.set_finding(Finding::Heavy, cx));

    cx.update(|window, cx| {
        audit.update(cx, |audit, cx| {
            audit.install_dataset(
                scan::Scan {
                    entries: vec![entry("new.png", 10, 10, 100, ImageFormat::Png)],
                    skipped_raw: 0,
                    skipped_heic: 0,
                    skipped_packages: 0,
                    unreadable: Vec::new(),
                    walk_errors: Vec::new(),
                    existing_output: 0,
                },
                PathBuf::from("/elsewhere"),
                false,
                None,
                window,
                cx,
            );
        });
    });

    audit.read_with(cx, |audit, _| {
        assert_eq!(audit.finding, None);
        assert_eq!(audit.visible.len(), 1);
    });
}

#[gpui_kit::test]
fn opening_another_folder_retires_the_pairing(cx: &mut TestAppContext) {
    let (audit, cx) = finding_audit(cx);
    audit.update(cx, |audit, _| {
        audit.sirv_pairing = Some(SirvPairing {
            dir: "/photos".into(),
            files: Listing::Ready(HashMap::new()),
            cdn_host: CdnHost::Ready("demo.sirv.com".into()),
            client: Arc::new(parking_lot::Mutex::new(sirv::Client::new(
                sirv::Credentials {
                    client_id: String::new(),
                    client_secret: String::new(),
                },
            ))),
        });
        audit.sirv_local_presence.insert("a.jpg".into());
    });

    cx.update(|window, cx| {
        audit.update(cx, |audit, cx| {
            audit.install_dataset(
                scan::Scan {
                    entries: vec![entry("new.png", 10, 10, 100, ImageFormat::Png)],
                    skipped_raw: 0,
                    skipped_heic: 0,
                    unreadable: Vec::new(),
                    walk_errors: Vec::new(),
                    existing_output: 0,
                    skipped_packages: 0,
                },
                PathBuf::from("/elsewhere"),
                false,
                None,
                window,
                cx,
            );
        });
    });

    audit.read_with(cx, |audit, _| {
        assert!(audit.sirv_local_presence.is_empty());
        assert!(audit.sirv_pairing.is_none());
    });
}

fn finding_launch() -> Launch {
    // A PNG named `.webp` is the mislabelled one. The screenshot is 30 bytes per
    // pixel; the photo is a tenth of one.
    Launch {
        root: PathBuf::new(),
        entries: vec![
            entry("photo.jpg", 1000, 1000, 100_000, ImageFormat::Jpeg),
            entry("screenshot.png", 100, 100, 300_000, ImageFormat::Png),
            entry("liar.webp", 100, 100, 1_000, ImageFormat::Png),
        ],
        skipped_raw: 0,
        skipped_heic: 0,
        skipped_packages: 0,
        unreadable: Vec::new(),
        walk_errors: Vec::new(),
        existing_output: 0,
        open_single: false,
        format: Format::WebP,
        quality: Quality::lossy(80.),
        max_edge: MaxEdge::FULL,
        grid: false,
        recent_folders: Vec::new(),
        columns: ColumnPrefs::default(),
        output: crate::settings::Output::default(),
        include_subfolders: false,
        sidebar_open: true,
    }
}

fn finding_audit(
    cx: &mut TestAppContext,
) -> (gpui_kit::Entity<Audit>, &mut gpui_kit::VisualTestContext) {
    cx.update(init_theme);
    let launch = finding_launch();
    let mut audit = None;
    let (_, cx) = cx.add_window_view(|window, cx| {
        let built = build_audit(launch, window, cx);
        audit = Some(built.clone());
        Root::new(built, window, cx).bg(cx.theme().background)
    });
    (
        audit.expect("the audit is built for the production Root"),
        cx,
    )
}

fn tree_row_bounds(
    audit: &gpui_kit::Entity<Audit>,
    path: &Path,
    selector_prefix: &str,
    cx: &mut gpui_kit::VisualTestContext,
) -> gpui_kit::Bounds<gpui_kit::Pixels> {
    let index = audit.read_with(cx, |audit, cx| {
        let id = audit
            .tree_paths
            .iter()
            .find_map(|(id, candidate)| (candidate == path).then(|| id.clone().into()))
            .expect("the requested path is in the tree");
        audit
            .tree_state
            .read(cx)
            .index_of(&id)
            .expect("the requested path is visible")
    });
    audit.update(cx, |audit, cx| {
        audit.tree_state.update(cx, |tree, cx| {
            tree.scroll_to_item(index, ScrollStrategy::Top);
            cx.notify();
        });
    });
    cx.run_until_parked();
    let selector = Box::leak(format!("{selector_prefix}-{index}").into_boxed_str());
    cx.debug_bounds(selector)
        .expect("the requested tree row is rendered")
}

#[gpui_kit::test]
fn acquisition_extras_stay_off_the_primary_surface(cx: &mut TestAppContext) {
    let (audit, cx) = finding_audit(cx);
    audit.update(cx, |audit, cx| {
        audit.spins = vec![acquisition::SpinSet {
            name: "shoe".into(),
            indices: (0..8).collect(),
            remote_folder: "press-spins/shoe".into(),
            issue: None,
        }];
        cx.notify();
    });
    cx.update(|window, cx| window.draw(cx).clear(cx));

    assert!(cx.debug_bounds("copy-audit-report").is_none());
    assert!(cx.debug_bounds("spin-preflight").is_none());
}

#[gpui_kit::test]
fn a_desktop_drop_opens_every_dropped_file_and_no_neighbours(cx: &mut TestAppContext) {
    let nonce = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .unwrap()
        .as_nanos();
    let root = std::env::temp_dir().join(format!("press-multi-drop-{nonce}"));
    std::fs::create_dir_all(&root).unwrap();
    let root = std::fs::canonicalize(root).unwrap();
    let write = |name: &str, colour: [u8; 3]| {
        let path = root.join(name);
        image::RgbImage::from_pixel(8, 8, image::Rgb(colour))
            .save(&path)
            .unwrap();
        path
    };
    let first = write("first.png", [255, 0, 0]);
    let second = write("second.png", [0, 255, 0]);
    write("not-dropped.png", [0, 0, 255]);

    let (audit, cx) = finding_audit(cx);
    cx.update(|window, cx| window.draw(cx).clear(cx));
    let position = cx.debug_bounds("audit-header").unwrap().center();
    cx.simulate_event(gpui_kit::FileDropEvent::Entered {
        position,
        paths: gpui_kit::ExternalPaths([first, second].into_iter().collect()),
    });
    cx.simulate_event(gpui_kit::FileDropEvent::Submit { position });
    cx.run_until_parked();

    audit.read_with(cx, |audit, _| {
        assert_eq!(audit.root, root);
        assert_eq!(audit.batch_size, Some(2));
        assert_eq!(names(&audit.entries), ["first.png", "second.png"]);
    });
}

#[gpui_kit::test]
fn a_desktop_drop_opens_direct_images_from_every_dropped_folder(cx: &mut TestAppContext) {
    let root = scan_fixture("multi-folder-drop");
    let first_folder = root.join("first");
    let second_folder = root.join("second");
    std::fs::create_dir_all(first_folder.join("nested")).unwrap();
    std::fs::create_dir_all(&second_folder).unwrap();
    write_png(&first_folder, "z.png");
    write_png(&second_folder, "a.png");
    write_png(&first_folder.join("nested"), "not-direct.png");

    let (audit, cx) = finding_audit(cx);
    audit.update(cx, |audit, _| {
        audit.sort = Sort {
            column: Column::Name,
            descending: false,
        };
    });
    cx.update(|window, cx| window.draw(cx).clear(cx));
    let position = cx.debug_bounds("audit-header").unwrap().center();
    cx.simulate_event(gpui_kit::FileDropEvent::Entered {
        position,
        paths: gpui_kit::ExternalPaths([first_folder, second_folder].into_iter().collect()),
    });
    cx.simulate_event(gpui_kit::FileDropEvent::Submit { position });
    cx.run_until_parked();

    audit.read_with(cx, |audit, _| {
        assert_eq!(audit.root, root);
        assert_eq!(audit.batch_size, Some(2));
        assert_eq!(audit.batch_folders, Some(2));
        let labels = audit
            .visible
            .iter()
            .map(|index| entry_label(&audit.root, true, &audit.entries[*index]))
            .collect::<Vec<_>>();
        assert_eq!(
            labels,
            [
                PathBuf::from("first").join("z.png").display().to_string(),
                PathBuf::from("second").join("a.png").display().to_string(),
            ]
        );
    });
    std::fs::remove_dir_all(root).unwrap();
}

#[cfg(unix)]
#[gpui_kit::test]
fn a_symlinked_multi_folder_drop_keeps_one_root_identity(cx: &mut TestAppContext) {
    use std::os::unix::fs::symlink;

    let fixture = scan_fixture("multi-folder-alias");
    let root = fixture.join("real");
    let first = root.join("first");
    let second = root.join("second");
    let alias = fixture.join("alias");
    std::fs::create_dir_all(&first).unwrap();
    std::fs::create_dir_all(&second).unwrap();
    write_png(&first, "one.png");
    write_png(&second, "two.png");
    symlink(&root, &alias).unwrap();
    let (audit, cx) = finding_audit(cx);

    audit.update_in(cx, |audit, window, cx| {
        audit.request_paths(vec![alias.join("first"), alias.join("second")], window, cx);
    });
    cx.run_until_parked();

    audit.read_with(cx, |audit, _| {
        assert_eq!(audit.root, root);
        assert_eq!(audit.batch_folders, Some(2));
        assert!(
            audit
                .entries
                .iter()
                .all(|entry| entry.path.starts_with(&audit.root))
        );
    });
    std::fs::remove_dir_all(fixture).unwrap();
}

/// Escape in the list puts the selection down. Escape in the size box is the
/// box's business, and the ticked rows must not pay for it.
#[gpui_kit::test]
fn typing_in_the_max_edge_box_does_not_clear_the_selection(cx: &mut TestAppContext) {
    let (audit, cx) = finding_audit(cx);
    audit.update(cx, |audit, cx| {
        // Opening ticks every row; narrow to one so the assertion is specific.
        audit.selected = HashSet::from([0]);
        audit.rail = Rail::Convert;
        audit.selection_changed(cx);
    });
    cx.update(|window, cx| window.draw(cx).clear(cx));

    let input = cx
        .debug_bounds("max-edge-input")
        .expect("the size box is in the convert rail");
    cx.simulate_click(input.center(), gpui_kit::Modifiers::none());
    cx.update(|window, cx| assert!(audit.read(cx).text_input_focused(window, cx)));
    cx.simulate_keystrokes("escape");

    audit.read_with(cx, |audit, _| {
        assert_eq!(audit.selected, HashSet::from([0]));
    });
}

/// With the format kept, the output name is the source name. The rail says so
/// before the button can be pressed, instead of the failure list saying it once
/// per file afterwards.
#[gpui_kit::test]
fn keeping_the_format_into_the_audited_folder_disables_convert_with_a_reason(
    cx: &mut TestAppContext,
) {
    let (audit, cx) = finding_audit(cx);
    audit.update(cx, |audit, cx| {
        // Opening ticks every row; narrow to one so the label reads "1 selected".
        audit.selected = HashSet::from([0]);
        audit.selection_changed(cx);
        audit.apply_format(Format::Same, cx);
        assert!(!audit.keep_format_overwrites_sources());
        assert!(
            audit
                .conversion_action_label()
                .starts_with("Convert 1 selected")
        );

        audit.output = Output::Folder(audit.root.clone());
        assert!(audit.keep_format_overwrites_sources());
        assert_eq!(
            audit.conversion_action_label(),
            "Keep format needs a different output folder"
        );

        audit.apply_format(Format::WebP, cx);
        assert!(!audit.keep_format_overwrites_sources());
    });
}

/// The box beside the presets follows `--max-edge`: a positive whole number of
/// pixels applies, junk changes nothing, and an emptied box is the source size.
#[gpui_kit::test]
fn a_typed_max_edge_applies_and_junk_leaves_the_last_size(cx: &mut TestAppContext) {
    let (audit, cx) = finding_audit(cx);
    audit.update(cx, |audit, cx| {
        assert_eq!(audit.max_edge, MaxEdge::FULL);
        audit.apply_custom_max_edge("1200", cx);
        assert_eq!(audit.max_edge, MaxEdge(Some(1200)));
        assert!(
            panel::active_preset(audit.format, audit.quality, audit.max_edge).is_none(),
            "a typed size is a custom configuration"
        );

        for junk in ["0", "abc", "-4"] {
            audit.apply_custom_max_edge(junk, cx);
            assert_eq!(
                audit.max_edge,
                MaxEdge(Some(1200)),
                "{junk:?} changed the size"
            );
        }

        audit.apply_custom_max_edge("", cx);
        assert_eq!(audit.max_edge, MaxEdge::FULL);
    });
}

#[gpui_kit::test]
fn choosing_avif_leaves_lossless_for_the_last_slider_quality(cx: &mut TestAppContext) {
    let (audit, cx) = finding_audit(cx);
    audit.update(cx, |audit, cx| {
        audit.quality = Quality::LOSSLESS;
        audit.slider_quality = 73.;
        audit.apply_format(Format::Avif, cx);

        assert_eq!(audit.format, Format::Avif);
        assert_eq!(audit.quality, Quality::lossy(73.));
    });
}

/// `conversion_action_label` only offers Replace where `write_output` can swap a
/// finished file atomically. std's Windows rename cannot, so that build keeps
/// saying Convert and the expectation has to follow the same `cfg`.
fn replace_label() -> &'static str {
    if cfg!(windows) {
        "Convert 2 selected to WEBP"
    } else {
        "Replace 2 selected WEBP outputs"
    }
}

#[gpui_kit::test]
fn render_totals_change_with_selection_and_results(cx: &mut TestAppContext) {
    let (audit, cx) = finding_audit(cx);
    audit.update(cx, |audit, _| {
        assert_eq!(audit.heavy, 1);
        assert_eq!(audit.mislabelled, 1);
        // A folder now opens fully ticked, so reaching the empty-selection label
        // means unticking everything.
        audit.selected.clear();
        audit.refresh_target_summary();
        assert_eq!(audit.target_count(), 0);
        assert_eq!(audit.conversion_action_label(), "Select images to convert");
        assert_eq!(audit.target_bytes(), 0);

        audit.selected.extend([0, 2, 99]);
        audit.refresh_target_summary();
        assert_eq!(
            audit.target_count(),
            2,
            "a hidden or stale index is not a target"
        );
        assert_eq!(
            audit.conversion_action_label(),
            "Convert 2 selected to WEBP"
        );
        audit.converting = true;
        assert_eq!(audit.conversion_action_label(), "Converting…");
        audit.converting = false;
        assert_eq!(audit.target_bytes(), 101_000);

        audit.record_result(0, Format::WebP, 50_000, PathBuf::from("/tmp/out.webp"));
        assert_eq!(
            audit.conversion_action_label(),
            "Convert 2 selected to WEBP"
        );
        audit.record_result(2, Format::WebP, 500, PathBuf::from("/tmp/out.webp"));
        assert_eq!(audit.converted_totals(), (101_000, 50_500));
        assert_eq!(audit.conversion_action_label(), replace_label());
        audit.record_result(0, Format::WebP, 40_000, PathBuf::from("/tmp/out.webp"));
        assert_eq!(audit.converted_totals(), (101_000, 40_500));
        audit.clear_results();
        assert_eq!(audit.converted_totals(), (0, 0));
        assert_eq!(
            audit.conversion_action_label(),
            replace_label(),
            "changing settings clears old measurements, not known output files"
        );
    });
}

#[gpui_kit::test]
fn an_automatic_update_never_restarts_during_file_writes(cx: &mut TestAppContext) {
    let (audit, cx) = finding_audit(cx);
    audit.update(cx, |audit, _| {
        assert!(audit.automatic_update_can_restart());

        audit.converting = true;
        assert!(!audit.automatic_update_can_restart());
        audit.converting = false;

        audit.sirv_job = Some(SirvJob {
            kind: SirvJobKind::Push,
            done: 0,
            total: 1,
            failed: 0,
            failures: Vec::new(),
            current: None,
            finished: false,
            stopping: false,
            generation: audit.sirv_generation,
        });
        assert!(!audit.automatic_update_can_restart());
        audit.sirv_job.as_mut().unwrap().finished = true;
        assert!(audit.automatic_update_can_restart());

        audit.local_ai_job = Some(LocalAiJob {
            tool: local_ai::Tool::Upscale,
            index: 0,
            dataset_generation: audit.dataset_generation,
            source_name: "photo.jpg".to_string(),
            first_setup: false,
            state: LocalAiJobState::Running,
            cancelled: Arc::new(std::sync::atomic::AtomicBool::new(false)),
        });
        assert!(!audit.automatic_update_can_restart());
        audit.local_ai_job.as_mut().unwrap().state =
            LocalAiJobState::Done(PathBuf::from("optimized/photo-4x.png"));
        assert!(audit.automatic_update_can_restart());

        audit.studio_job = Some(StudioJob {
            tool: studio::Tool::Upscale,
            index: 0,
            dataset_generation: audit.dataset_generation,
            source_name: "photo.jpg".to_string(),
            output_source: PathBuf::from("photo.jpg"),
            prompt: String::new(),
            state: StudioJobState::Running,
            cancelled: Arc::new(std::sync::atomic::AtomicBool::new(false)),
        });
        assert!(!audit.automatic_update_can_restart());
        audit.studio_job.as_mut().unwrap().state =
            StudioJobState::Done(PathBuf::from("optimized/photo-studio-2x.png"));
        assert!(audit.automatic_update_can_restart());
    });
}

#[gpui_kit::test]
fn erasing_the_filter_text_restores_the_audit(cx: &mut TestAppContext) {
    let (audit, cx) = finding_audit(cx);
    audit.update_in(cx, |audit, window, cx| {
        audit
            .filter_input
            .update(cx, |input, cx| input.set_value("no-match", window, cx));
        audit.set_filter("no-match".into(), cx);
    });
    cx.update(|window, cx| window.draw(cx).clear(cx));

    assert!(
        cx.debug_bounds("filter-empty-result").is_some(),
        "the empty filter result is rendered as a named recovery state"
    );
    assert!(
        cx.debug_bounds("clear-filter").is_none(),
        "there is no Clear button duplicating backspace"
    );

    // Erasing the box widens the list back out, the way backspace does.
    audit.update_in(cx, |audit, window, cx| {
        audit
            .filter_input
            .update(cx, |input, cx| input.set_value("", window, cx));
        audit.set_filter(String::new(), cx);
    });

    audit.read_with(cx, |audit, cx| {
        assert!(audit.filter.is_empty());
        assert!(audit.filter_input.read(cx).value().is_empty());
        assert_eq!(audit.visible.len(), 3);
    });
}

#[test]
fn sirv_credentials_require_both_nonblank_fields() {
    assert!(!credentials_complete("", "secret"));
    assert!(!credentials_complete("client", "  "));
    assert!(credentials_complete(" client ", " secret "));
}

#[gpui_kit::test]
fn key_repeats_share_one_next_frame_redraw(cx: &mut TestAppContext) {
    let (audit, cx) = finding_audit(cx);
    audit.update_in(cx, |audit, window, cx| {
        audit.step_cursor(1, false, window, cx);
        audit.step_cursor(1, false, window, cx);
        assert_eq!(audit.cursor, 2);
        assert!(audit.cursor_redraw_pending);
    });

    cx.update(|window, cx| {
        window.simulate_next_frame(cx);
    });
    audit.read_with(cx, |audit, _| {
        assert!(!audit.cursor_redraw_pending);
        assert_eq!(audit.cursor, 2);
    });
}

#[gpui_kit::test]
fn grid_arrows_move_by_tile_and_band_and_shift_range_shrinks(cx: &mut TestAppContext) {
    let (audit, cx) = finding_audit(cx);
    audit.update_in(cx, |audit, window, cx| {
        audit.entries = (0..7)
            .map(|index| entry(&format!("{index}.jpg"), 10, 10, 100, ImageFormat::Jpeg))
            .collect();
        audit.visible = (0..7).collect();
        audit.grid = true;
        audit.gallery_columns = Some(3);
        audit.cursor = 0;
        audit.anchor = 0;
        audit.selected.clear();

        audit.step_cursor_lateral(1, false, window, cx);
        assert_eq!((audit.cursor, audit.anchor), (1, 1));

        audit.step_cursor_vertical(1, true, window, cx);
        assert_eq!(audit.cursor, 4);
        assert_eq!(audit.selected, HashSet::from([1, 2, 3, 4]));

        audit.step_cursor_lateral(-1, true, window, cx);
        assert_eq!(audit.cursor, 3);
        assert_eq!(audit.selected, HashSet::from([1, 2, 3]));

        audit.step_cursor_vertical(-1, true, window, cx);
        assert_eq!(audit.cursor, 0);
        assert_eq!(audit.selected, HashSet::from([0, 1]));
    });
}

#[gpui_kit::test]
fn gallery_thumbs_follow_the_virtual_range(cx: &mut TestAppContext) {
    let (audit, cx) = finding_audit(cx);
    audit.update(cx, |audit, cx| {
        let first = audit.entry_at(0).unwrap();
        let third = audit.entry_at(2).unwrap();
        audit.grid = true;
        audit.gallery_columns = Some(2);
        audit.gallery_visible = 1..2;

        assert!(!audit.thumb_is_visible(first, cx));
        assert!(audit.thumb_is_visible(third, cx));
    });
}

#[test]
fn thumbnail_overscan_covers_four_neighbor_viewports() {
    assert_eq!(thumb_overscan_rows(20..30, 100, 100), 0..70);
    assert_eq!(thumb_overscan_rows(0..10, 100, 100), 0..50);
    assert_eq!(thumb_overscan_rows(90..100, 100, 100), 50..100);
}

#[test]
fn thumbnail_overscan_never_outgrows_the_cache() {
    let limit = thumb_cache_limit(thumbs::THUMB_EDGE);
    let wanted = thumb_overscan_rows(400..448, 1_000, limit);

    assert_eq!(wanted.len(), limit);
    assert!(wanted.contains(&400));
    assert!(wanted.contains(&447));
}

#[gpui_kit::test]
fn thumbnail_decodes_share_four_slots_and_cap_fallbacks_at_two(cx: &mut TestAppContext) {
    let (audit, cx) = finding_audit(cx);
    audit.update(cx, |audit, cx| {
        audit.grid = true;
        audit.gallery_columns = Some(1);
        audit.gallery_visible = 0..1;
        let index = audit.entry_at(0).unwrap();
        let next = audit.entry_at(1).unwrap();
        for queued in [index, next] {
            audit.thumb_queue.push_back(ThumbRequest {
                index: queued,
                dataset_generation: audit.dataset_generation,
                edge: thumbs::THUMB_EDGE,
                path: PathBuf::from("missing-thumbnail.png"),
                native_scaled: true,
                fallback: false,
            });
        }
        assert!(audit.promote_thumb(next));
        assert_eq!(audit.thumb_queue.front().unwrap().index, next);
        audit.thumb_queue.clear();

        for _ in 0..THUMB_WORKERS + 2 {
            audit.thumb_queue.push_back(ThumbRequest {
                index,
                dataset_generation: audit.dataset_generation,
                edge: thumbs::THUMB_EDGE,
                path: PathBuf::from("missing-thumbnail.png"),
                native_scaled: true,
                fallback: false,
            });
        }

        audit.start_thumb_jobs(cx);

        assert_eq!(audit.thumb_inflight, THUMB_WORKERS);
        assert_eq!(audit.thumb_queue.len(), 2);
    });
    cx.run_until_parked();
    audit.read_with(cx, |audit, _| {
        assert_eq!(audit.thumb_inflight, 0);
        assert_eq!(audit.thumb_slow_inflight, 0);
        assert!(audit.thumb_queue.is_empty());
    });

    audit.update(cx, |audit, cx| {
        let index = audit.entry_at(0).unwrap();
        for _ in 0..THUMB_SLOW_WORKERS + 2 {
            audit.thumb_queue.push_back(ThumbRequest {
                index,
                dataset_generation: audit.dataset_generation,
                edge: thumbs::THUMB_EDGE,
                path: PathBuf::from("missing-thumbnail.png"),
                native_scaled: false,
                fallback: true,
            });
        }
        audit.start_thumb_jobs(cx);
        assert_eq!(audit.thumb_inflight, THUMB_SLOW_WORKERS);
        assert_eq!(audit.thumb_slow_inflight, THUMB_SLOW_WORKERS);
        assert_eq!(audit.thumb_queue.len(), 2);
    });
    cx.run_until_parked();

    audit.update(cx, |audit, cx| {
        let index = audit.entry_at(0).unwrap();
        for native_scaled in std::iter::repeat_n(false, THUMB_SLOW_WORKERS)
            .chain(std::iter::repeat_n(true, THUMB_WORKERS))
        {
            audit.thumb_queue.push_back(ThumbRequest {
                index,
                dataset_generation: audit.dataset_generation,
                edge: thumbs::THUMB_EDGE,
                path: PathBuf::from("missing-thumbnail.png"),
                native_scaled,
                fallback: !native_scaled,
            });
        }
        audit.start_thumb_jobs(cx);
        assert_eq!(audit.thumb_inflight, THUMB_WORKERS);
        assert_eq!(audit.thumb_slow_inflight, THUMB_SLOW_WORKERS);
        assert_eq!(audit.thumb_queue.len(), THUMB_SLOW_WORKERS);
    });
    cx.run_until_parked();
}

#[gpui_kit::test]
fn closing_settings_and_sirv_restores_the_audit_focus(cx: &mut TestAppContext) {
    let (audit, cx) = finding_audit(cx);

    audit.update_in(cx, |audit, window, cx| {
        audit.open_settings(window, cx);
        assert!(
            audit
                .settings_panel
                .as_ref()
                .unwrap()
                .client_secret
                .read(cx)
                .presentation()
                .is_masked(),
            "a client secret is masked before any text is entered"
        );
        let field = audit
            .settings_panel
            .as_ref()
            .unwrap()
            .client_id
            .read(cx)
            .focus_handle(cx);
        window.focus(&field, cx);
        audit.close_settings(window, cx);
    });
    cx.run_until_parked();
    cx.update(|window, cx| {
        assert!(audit.read(cx).focus.is_focused(window));
    });

    audit.update_in(cx, |audit, window, cx| {
        let browser_focus = cx.focus_handle();
        audit.sirv_browser = Some(SirvBrowser {
            client: Arc::new(parking_lot::Mutex::new(sirv::Client::new(
                sirv::Credentials {
                    client_id: String::new(),
                    client_secret: String::new(),
                },
            ))),
            path: "/".into(),
            needs_credentials: false,
            nodes: None,
            generation: 0,
            session: 1,
            focused: true,
            focus: browser_focus.clone(),
        });
        window.focus(&browser_focus, cx);
        audit.close_sirv_browser(window, cx);
    });
    cx.run_until_parked();
    cx.update(|window, cx| {
        assert!(audit.read(cx).focus.is_focused(window));
    });
}

#[gpui_kit::test]
fn flushing_settings_clears_the_pending_debounce(cx: &mut TestAppContext) {
    let (audit, cx) = finding_audit(cx);
    let dir = std::env::temp_dir().join(format!(
        "press-audit-flush-{}",
        std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .unwrap()
            .as_nanos()
    ));
    std::fs::create_dir_all(&dir).unwrap();
    let path = dir.join("settings");
    audit.update(cx, |audit, cx| {
        audit.settings_writer = crate::settings::SettingsWriter::new(Some(path.clone()));
        let mut settings = audit.settings.clone();
        settings.width = Some(1280.);
        audit.remember_settings(settings, cx);
        assert!(audit.pending_settings.is_some());

        match audit.flush_settings() {
            crate::settings::WriteOutcome::Written { .. } => {}
            outcome => panic!("the flush writes: {outcome:?}"),
        }
        assert!(audit.pending_settings.is_none());
    });
    assert!(
        std::fs::read_to_string(&path)
            .unwrap()
            .contains("width=1280"),
        "the flush stores the debounce it cancelled"
    );
    std::fs::remove_dir_all(dir).unwrap();
}

#[gpui_kit::test]
fn a_failed_debounced_save_reports_until_a_later_revision_clears_it(cx: &mut TestAppContext) {
    let (audit, cx) = finding_audit(cx);
    // Drain any debounce a setup render scheduled, so only this test's
    // revisions are ever pending.
    cx.executor()
        .advance_clock(std::time::Duration::from_millis(600));
    cx.run_until_parked();
    let before = notification_count(cx);

    // Renders may schedule their own debounces alongside this one; every
    // revision fails the same way while no config directory exists, so the
    // toast count still moves by exactly one either way.
    audit.update(cx, |audit, cx| {
        audit.settings_writer = crate::settings::SettingsWriter::new(None);
        audit.remember_settings(audit.settings.clone(), cx);
    });
    cx.executor()
        .advance_clock(std::time::Duration::from_millis(600));
    cx.run_until_parked();
    cx.executor()
        .advance_clock(std::time::Duration::from_millis(200));
    cx.run_until_parked();
    assert_eq!(
        notification_count(cx),
        before + 1,
        "the failed save stays reported"
    );

    let dir = std::env::temp_dir().join(format!(
        "press-audit-settings-notice-{}",
        std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .unwrap()
            .as_nanos()
    ));
    std::fs::create_dir_all(&dir).unwrap();
    let path = dir.join("settings");
    audit.update(cx, |audit, cx| {
        audit.settings_writer = crate::settings::SettingsWriter::new(Some(path.clone()));
        let mut settings = audit.settings.clone();
        settings.width = Some(1280.);
        audit.remember_settings(settings, cx);
    });
    cx.executor()
        .advance_clock(std::time::Duration::from_millis(600));
    cx.run_until_parked();
    cx.executor()
        .advance_clock(std::time::Duration::from_millis(200));
    cx.run_until_parked();
    assert_eq!(
        notification_count(cx),
        before,
        "the later successful revision clears the report"
    );
    assert!(
        path.exists(),
        "a successful revision landed while clearing the report"
    );
}

#[gpui_kit::test]
fn rapid_remembers_keep_only_the_newest_pending_snapshot(cx: &mut TestAppContext) {
    let (audit, cx) = finding_audit(cx);
    let dir = std::env::temp_dir().join(format!(
        "press-audit-pending-{}",
        std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .unwrap()
            .as_nanos()
    ));
    let path = dir.join("settings");
    audit.update(cx, |audit, cx| {
        audit.settings_writer = crate::settings::SettingsWriter::new(Some(path.clone()));
        let mut first = audit.settings.clone();
        first.width = Some(800.);
        audit.remember_settings(first, cx);
        let (first_revision, _) = audit.pending_settings.clone().expect("first pends");
        let mut second = audit.settings.clone();
        second.width = Some(810.);
        audit.remember_settings(second, cx);
        let (second_revision, snapshot) = audit.pending_settings.clone().expect("second pends");
        assert!(second_revision > first_revision, "revisions order the drag");
        assert_eq!(snapshot.width, Some(810.));
    });
    // No clock moves: nothing may have written yet.
    assert!(!path.exists(), "the debounce holds the drag");
    std::fs::remove_dir_all(dir).unwrap_or(());
}

fn pointer_checkbox_audit(
    grid: bool,
    cx: &mut TestAppContext,
) -> (gpui_kit::Entity<Audit>, &mut gpui_kit::VisualTestContext) {
    cx.update(init_theme);
    let launch = Launch {
        root: PathBuf::new(),
        entries: vec![
            entry("first.png", 10, 10, 100, ImageFormat::Png),
            entry("second.png", 10, 10, 200, ImageFormat::Png),
        ],
        skipped_raw: 0,
        skipped_heic: 0,
        skipped_packages: 0,
        unreadable: Vec::new(),
        walk_errors: Vec::new(),
        existing_output: 0,
        open_single: false,
        format: Format::WebP,
        quality: Quality::lossy(80.),
        max_edge: MaxEdge::FULL,
        grid,
        recent_folders: Vec::new(),
        columns: ColumnPrefs::default(),
        output: crate::settings::Output::default(),
        include_subfolders: false,
        sidebar_open: true,
    };
    let (harness, cx) = cx.add_window_view(move |window, cx| {
        let built = build_audit(launch, window, cx);
        AuditHarness { audit: built }
    });
    let audit = harness.read_with(cx, |harness, _| harness.audit.clone());
    // Both rows are already ticked: opening the folder did that.
    audit.update(cx, |audit, _| audit.estimate = Some((123, 2)));
    cx.update(|window, cx| window.draw(cx).clear(cx));
    (audit, cx)
}

#[gpui_kit::test]
fn gallery_exposes_sorting_and_a_separate_compare_action(cx: &mut TestAppContext) {
    let (audit, cx) = pointer_checkbox_audit(true, cx);
    assert!(cx.debug_bounds("gallery-sort").is_some());

    let compare = cx
        .debug_bounds("grid-compare-0")
        .expect("each gallery image has a named comparison action");
    cx.simulate_click(compare.center(), gpui_kit::Modifiers::none());

    audit.read_with(cx, |audit, _| {
        assert_eq!(
            audit.compare.as_ref().map(|comparison| comparison.index),
            Some(0)
        );
        assert_eq!(audit.selected, [0, 1].into_iter().collect());
    });
}

#[gpui_kit::test]
fn an_empty_gallery_selection_still_offers_select_all(cx: &mut TestAppContext) {
    let (audit, cx) = pointer_checkbox_audit(true, cx);
    audit.update(cx, |audit, cx| {
        audit.selected.clear();
        audit.selection_changed(cx);
    });
    cx.update(|window, cx| window.draw(cx).clear(cx));

    let select_all = cx
        .debug_bounds("bar-select-all")
        .expect("the gallery action bar keeps its bulk-selection action");
    cx.simulate_click(select_all.center(), gpui_kit::Modifiers::none());

    audit.read_with(cx, |audit, _| {
        assert_eq!(audit.selected, HashSet::from([0, 1]));
    });
}

#[gpui_kit::test]
fn double_click_opens_a_source_preview_before_comparison(cx: &mut TestAppContext) {
    let (audit, cx) = finding_audit(cx);
    let click = gpui_kit::ClickEvent::Mouse(gpui_kit::MouseClickEvent {
        down: gpui_kit::MouseDownEvent {
            button: gpui_kit::MouseButton::Left,
            click_count: 2,
            ..Default::default()
        },
        up: gpui_kit::MouseUpEvent {
            button: gpui_kit::MouseButton::Left,
            click_count: 2,
            ..Default::default()
        },
    });

    audit.update(cx, |audit, cx| audit.click_row(0, &click, cx));

    audit.read_with(cx, |audit, _| {
        let opened = audit.compare.as_ref().expect("preview opens");
        assert_eq!(opened.mode, MediaMode::Preview);
        assert!(
            opened.pair.is_none(),
            "preview opening does not run an encoder"
        );
    });
}

#[gpui_kit::test]
fn an_open_preview_draws_the_loaded_thumbnail_immediately(cx: &mut TestAppContext) {
    let (audit, cx) = finding_audit(cx);
    cx.run_until_parked();
    audit.update(cx, |audit, cx| {
        let thumbnail = Arc::new(RenderImage::new(vec![image::Frame::new(
            image::RgbaImage::new(1, 1),
        )]));
        audit.thumbs.insert(0, thumbnail.clone());

        audit.open_preview(0, cx);

        let preview = audit
            .compare
            .as_ref()
            .and_then(|comparison| comparison.preview.as_ref())
            .expect("the thumbnail is already visible");
        assert!(Arc::ptr_eq(&preview.image, &thumbnail));
        assert_eq!((preview.width, preview.height), (1000, 1000));
    });
}

#[gpui_kit::test]
fn a_running_ai_job_overlays_only_its_own_preview(cx: &mut TestAppContext) {
    let (audit, cx) = finding_audit(cx);
    audit.update(cx, |audit, cx| {
        let image = Arc::new(RenderImage::new(vec![image::Frame::new(
            image::RgbaImage::new(1, 1),
        )]));
        audit.compare = Some(Comparison {
            index: 0,
            dataset_generation: audit.dataset_generation,
            mode: MediaMode::Preview,
            key: compare::Key::new(
                Path::new("photo.jpg"),
                Format::WebP,
                Quality::lossy(80.),
                MaxEdge::FULL,
            ),
            preview: Some(Arc::new(Preview {
                image,
                width: 1000,
                height: 1000,
                profile: None,
                decoded: false,
            })),
            pair: None,
            failed: false,
            split: 0.5,
            pan: (0., 0.),
            zoom: None,
            drag: None,
            written: None,
            produced_by: None,
            focused: false,
        });
        audit.local_ai_job = Some(LocalAiJob {
            tool: local_ai::Tool::RemoveBackground,
            index: 1,
            dataset_generation: audit.dataset_generation,
            source_name: "screenshot.png".into(),
            first_setup: false,
            state: LocalAiJobState::Running,
            cancelled: Arc::new(std::sync::atomic::AtomicBool::new(false)),
        });
        cx.notify();
    });
    cx.update(|window, cx| window.draw(cx).clear(cx));
    assert!(cx.debug_bounds("preview-processing-overlay").is_none());

    audit.update(cx, |audit, cx| {
        audit.local_ai_job.as_mut().unwrap().index = 0;
        cx.notify();
    });
    cx.update(|window, cx| window.draw(cx).clear(cx));
    assert!(cx.debug_bounds("preview-processing-overlay").is_some());
}

#[gpui_kit::test]
fn marquee_selects_intersecting_visible_items(cx: &mut TestAppContext) {
    let (audit, cx) = finding_audit(cx);
    audit.update(cx, |audit, cx| {
        audit.selection_surface.set(gpui_kit::Bounds::from_corners(
            gpui_kit::point(px(0.), px(0.)),
            gpui_kit::point(px(300.), px(300.)),
        ));
        audit.selection_bounds.borrow_mut().extend([
            (
                0,
                gpui_kit::Bounds::from_corners(
                    gpui_kit::point(px(70.), px(70.)),
                    gpui_kit::point(px(100.), px(100.)),
                ),
            ),
            (
                1,
                gpui_kit::Bounds::from_corners(
                    gpui_kit::point(px(200.), px(200.)),
                    gpui_kit::point(px(230.), px(230.)),
                ),
            ),
        ]);
        audit.start_marquee(
            &gpui_kit::MouseDownEvent {
                button: gpui_kit::MouseButton::Left,
                position: gpui_kit::point(px(50.), px(50.)),
                ..Default::default()
            },
            cx,
        );
        assert!(audit.marquee.is_none(), "the table header owns its drags");
        audit.start_marquee(
            &gpui_kit::MouseDownEvent {
                button: gpui_kit::MouseButton::Left,
                position: gpui_kit::point(px(50.), px(120.)),
                ..Default::default()
            },
            cx,
        );
        audit.move_marquee(
            &gpui_kit::MouseMoveEvent {
                position: gpui_kit::point(px(120.), px(50.)),
                pressed_button: Some(gpui_kit::MouseButton::Left),
                ..Default::default()
            },
            cx,
        );

        assert_eq!(audit.selected, HashSet::from([0]));
        audit.finish_marquee(cx);
        assert!(audit.marquee.is_none());

        audit.start_marquee(
            &gpui_kit::MouseDownEvent {
                button: gpui_kit::MouseButton::Left,
                position: gpui_kit::point(px(50.), px(120.)),
                modifiers: gpui_kit::Modifiers {
                    control: true,
                    ..Default::default()
                },
                ..Default::default()
            },
            cx,
        );
        audit.move_marquee(
            &gpui_kit::MouseMoveEvent {
                position: gpui_kit::point(px(120.), px(50.)),
                pressed_button: Some(gpui_kit::MouseButton::Left),
                ..Default::default()
            },
            cx,
        );
        assert!(audit.selected.is_empty(), "control-drag toggles a hit off");
        audit.finish_marquee(cx);
    });
}

#[gpui_kit::test]
fn action_bar_clicks_do_not_replace_the_marquee_selection(cx: &mut TestAppContext) {
    let (audit, cx) = pointer_checkbox_audit(false, cx);
    let selected = audit.read_with(cx, |audit, _| audit.selected.clone());
    let convert = cx
        .debug_bounds("action-bar")
        .expect("the audit action bar is visible");

    cx.simulate_click(
        gpui_kit::point(convert.left() + px(2.), convert.center().y),
        gpui_kit::Modifiers::none(),
    );

    audit.read_with(cx, |audit, _| {
        assert_eq!(audit.selected, selected);
        assert!(audit.marquee.is_none());
    });
}

#[gpui_kit::test]
fn ai_operations_target_the_context_image(cx: &mut TestAppContext) {
    let (audit, cx) = finding_audit(cx);
    audit.update(cx, |audit, cx| {
        audit.selected.extend([0, 1]);
        audit.rail = Rail::Studio;
        audit.selection_changed(cx);

        audit.open_ai_operations(2, None, cx);

        assert_eq!(audit.selected, HashSet::from([2]));
        assert_eq!(audit.cursor, audit.row_of(2).unwrap());
        assert_eq!(audit.rail, Rail::Studio);
    });
}

#[gpui_kit::test]
fn scan_blocked_studio_confirmation_is_disabled(cx: &mut TestAppContext) {
    let (audit, cx) = finding_audit(cx);
    audit.update(cx, |audit, cx| {
        audit.selected.insert(0);
        audit.studio_key = Some("sk_live_test".into());
        audit.studio_job = Some(StudioJob {
            tool: audit.studio_tool,
            index: 0,
            dataset_generation: audit.dataset_generation,
            source_name: "photo.jpg".into(),
            output_source: PathBuf::from("photo.jpg"),
            prompt: String::new(),
            state: StudioJobState::AwaitingConfirmation(studio::PreparedUpload::for_test()),
            cancelled: Arc::new(std::sync::atomic::AtomicBool::new(false)),
        });
        audit.rail = Rail::Studio;
        retained_scan(audit);
        assert!(audit.studio_commit_disabled(Some(0), true, false, true));
        audit.scanning = None;
        assert!(!audit.studio_commit_disabled(Some(0), true, false, true));
        retained_scan(audit);
        cx.notify();
    });
    cx.update(|window, cx| window.draw(cx).clear(cx));
    assert!(cx.debug_bounds("studio-commit").is_none());
    audit.update(cx, |audit, cx| audit.confirm_studio_for_test(cx));
    audit.read_with(cx, |audit, _| assert!(audit.studio_job.is_none()));
}

#[gpui_kit::test]
fn studio_confirm_card_shows_upload_and_source_sizes(cx: &mut TestAppContext) {
    let (audit, cx) = finding_audit(cx);
    audit.update(cx, |audit, cx| {
        audit.selected.clear();
        audit.selected.insert(0);
        audit.studio_key = Some("sk_live_test".into());
        audit.studio_job = Some(StudioJob {
            tool: audit.studio_tool,
            index: 0,
            dataset_generation: audit.dataset_generation,
            source_name: "photo.jpg".into(),
            output_source: PathBuf::from("photo.jpg"),
            prompt: String::new(),
            state: StudioJobState::AwaitingConfirmation(studio::PreparedUpload::for_test()),
            cancelled: Arc::new(std::sync::atomic::AtomicBool::new(false)),
        });
        audit.rail = Rail::Studio;
        audit.selection_changed(cx);
        cx.notify();
    });
    cx.update(|window, cx| window.draw(cx).clear(cx));

    assert!(
        cx.debug_bounds("studio-confirm-card").is_some(),
        "the awaiting Studio job renders its confirm card"
    );
    audit.read_with(cx, |audit, _| {
        let job = audit.studio_job.as_ref().expect("the Studio job is kept");
        assert!(
            job.message(&audit.root).contains("1×1"),
            "the confirm message names the known upload dimensions"
        );
        assert_eq!(
            (audit.entries[0].width, audit.entries[0].height),
            (1000, 1000),
            "the source dimensions come from the scan entry without decoding"
        );
    });
}

#[gpui_kit::test]
fn studio_confirm_card_is_absent_while_running(cx: &mut TestAppContext) {
    let (audit, cx) = finding_audit(cx);
    audit.update(cx, |audit, cx| {
        audit.selected.clear();
        audit.selected.insert(0);
        audit.studio_key = Some("sk_live_test".into());
        audit.studio_job = Some(StudioJob {
            tool: audit.studio_tool,
            index: 0,
            dataset_generation: audit.dataset_generation,
            source_name: "photo.jpg".into(),
            output_source: PathBuf::from("photo.jpg"),
            prompt: String::new(),
            state: StudioJobState::Running,
            cancelled: Arc::new(std::sync::atomic::AtomicBool::new(false)),
        });
        audit.rail = Rail::Studio;
        audit.selection_changed(cx);
        cx.notify();
    });
    cx.update(|window, cx| window.draw(cx).clear(cx));

    assert!(
        cx.debug_bounds("studio-confirm-card").is_none(),
        "the confirm card shows only while awaiting confirmation"
    );
}
#[gpui_kit::test]
fn studio_key_status_renders_checking_saved_and_failure_inline(cx: &mut TestAppContext) {
    let (audit, cx) = finding_audit(cx);
    audit.update(cx, |audit, cx| {
        audit.selected.clear();
        audit.selected.insert(0);
        audit.rail = Rail::Studio;
        audit.selection_changed(cx);
    });

    // While the key verifies, the rail names the checking state.
    audit.update(cx, |audit, cx| {
        audit.studio_key = None;
        audit.studio_key_checking = true;
        audit.studio_status = None;
        cx.notify();
    });
    cx.update(|window, cx| window.draw(cx).clear(cx));
    assert!(cx.debug_bounds("studio-key-status").is_some());

    // Once stored, the rail says the key is saved on this computer.
    audit.update(cx, |audit, cx| {
        audit.studio_key_checking = false;
        audit.studio_key = Some("sk_live_sentinel".into());
        audit.studio_status = None;
        cx.notify();
    });
    cx.update(|window, cx| window.draw(cx).clear(cx));
    assert!(cx.debug_bounds("studio-key-status").is_some());

    // A failure reuses the single status channel, and neither line leaks key
    // material.
    audit.update(cx, |audit, cx| {
        audit.studio_key = None;
        audit.studio_status = Some((false, "Could not verify the key".into()));
        cx.notify();
    });
    cx.update(|window, cx| window.draw(cx).clear(cx));
    assert!(cx.debug_bounds("studio-key-feedback").is_some());
    audit.read_with(cx, |audit, _| {
        let (_, message) = audit
            .studio_status
            .clone()
            .expect("the failure status is set");
        assert!(!message.contains("sk_live_sentinel"), "{message:?}");
        assert!(!message.contains("sk_live_"), "{message:?}");
    });
}

#[gpui_kit::test]
fn studio_run_status_names_the_key_blocker(cx: &mut TestAppContext) {
    let (audit, cx) = finding_audit(cx);
    audit.update(cx, |audit, cx| {
        audit.selected.clear();
        audit.selected.insert(0);
        audit.rail = Rail::Studio;
        audit.studio_key = None;
        audit.studio_job = None;
        audit.studio_status = None;
        audit.selection_changed(cx);
    });
    audit.read_with(cx, |audit, _| {
        let index = audit.single_target();
        assert_eq!(index, Some(0));
        assert!(audit.studio_commit_disabled(index, false, false, false));
        assert_eq!(
            audit.studio_run_status(index, false, false),
            Some("Add an API key to run".to_string())
        );
    });
    cx.update(|window, cx| window.draw(cx).clear(cx));
    assert!(cx.debug_bounds("studio-run-status").is_some());
}

#[gpui_kit::test]
fn scan_blocked_sirv_pair_is_disabled(cx: &mut TestAppContext) {
    let (audit, cx) = finding_audit(cx);
    audit.update(cx, |audit, cx| {
        audit.sirv_browser = Some(SirvBrowser {
            client: test_pairing().client,
            path: "/photos".into(),
            needs_credentials: false,
            nodes: Some(Ok(Vec::new())),
            generation: 0,
            session: 1,
            focused: false,
            focus: cx.focus_handle(),
        });
        retained_scan(audit);
        assert!(audit.sirv_pair_disabled(false, true));
        audit.scanning = None;
        assert!(!audit.sirv_pair_disabled(false, true));
        audit.batch_folders = Some(2);
        assert!(audit.sirv_pair_disabled(false, true));
        audit.pair_sirv(cx);
        assert!(audit.sirv_pairing.is_none());
        audit.batch_folders = None;
        retained_scan(audit);
        cx.notify();
    });
    cx.update(|window, cx| window.draw(cx).clear(cx));
    assert!(cx.debug_bounds("sirv-pair").is_none());
    audit.update(cx, |audit, cx| audit.pair_sirv(cx));
    audit.read_with(cx, |audit, _| assert!(audit.sirv_pairing.is_none()));
}

#[gpui_kit::test]
fn scan_blocked_gallery_context_actions_are_disabled(cx: &mut TestAppContext) {
    scan_blocked_context_actions_leave_state_unchanged(true, cx);
}

#[gpui_kit::test]
fn scan_blocked_table_context_actions_are_disabled(cx: &mut TestAppContext) {
    scan_blocked_context_actions_leave_state_unchanged(false, cx);
}

fn scan_blocked_context_actions_leave_state_unchanged(grid: bool, cx: &mut TestAppContext) {
    let (audit, cx) = pointer_checkbox_audit(grid, cx);
    audit.update(cx, |audit, cx| {
        audit.selected = HashSet::from([1]);
        audit.studio_source = Some((1, PathBuf::from("optimized/second.webp")));
        audit.rail = Rail::Convert;
        retained_scan(audit);
        assert!(audit.media_commit_actions_disabled());
        audit.scanning = None;
        assert!(!audit.media_commit_actions_disabled());
        retained_scan(audit);
        cx.notify();
    });

    cx.update(|window, cx| window.draw(cx).clear(cx));
    assert!(
        cx.debug_bounds(if grid {
            "grid-checkbox-0"
        } else {
            "table-checkbox-0"
        })
        .is_none()
    );

    audit.update(cx, |audit, cx| {
        assert_eq!(audit.selected, HashSet::from([1]));
        assert_eq!(
            audit.studio_source,
            Some((1, PathBuf::from("optimized/second.webp")))
        );
        assert_eq!(audit.rail, Rail::Convert);
        assert!(!audit.converting);

        audit.convert_one(0, cx);
        audit.open_ai_operations(0, None, cx);

        assert_eq!(audit.selected, HashSet::from([1]));
        assert_eq!(
            audit.studio_source,
            Some((1, PathBuf::from("optimized/second.webp")))
        );
        assert_eq!(audit.rail, Rail::Convert);
        assert!(!audit.converting);
    });
}

#[gpui_kit::test]
fn an_incomplete_scan_cannot_copy_a_report(cx: &mut TestAppContext) {
    let (audit, cx) = finding_audit(cx);
    cx.write_to_clipboard(gpui_kit::ClipboardItem::new_string("sentinel".into()));
    audit.update(cx, |audit, cx| {
        audit.report_copied = true;
        retained_scan(audit);
        audit.copy_audit_report(cx);
        assert!(audit.report_copied);
    });
    assert_eq!(
        cx.read_from_clipboard()
            .and_then(|clipboard| clipboard.text()),
        Some("sentinel".into())
    );
}

#[gpui_kit::test]
fn studio_prompt_typing_does_not_toggle_the_audit_selection(cx: &mut TestAppContext) {
    let (audit, cx) = finding_audit(cx);
    audit.update(cx, |audit, cx| {
        // A folder opens fully ticked; Studio acts on one file, so narrow to one.
        audit.selected.clear();
        audit.selected.insert(0);
        audit.rail = Rail::Studio;
        audit.selection_changed(cx);
    });
    cx.update(|window, cx| window.draw(cx).clear(cx));

    let input = cx
        .debug_bounds("studio-prompt-input")
        .expect("the Studio prompt is visible");
    cx.simulate_click(input.center(), gpui_kit::Modifiers::none());
    cx.update(|window, cx| assert!(audit.read(cx).text_input_focused(window, cx)));
    cx.simulate_keystrokes("space");

    audit.read_with(cx, |audit, _| {
        assert_eq!(audit.selected, HashSet::from([0]));
    });
}

#[gpui_kit::test]
fn source_preview_has_ai_actions_but_compare_mode_does_not(cx: &mut TestAppContext) {
    let (audit, cx) = finding_audit(cx);
    audit.update(cx, |audit, cx| {
        audit.compare = Some(Comparison {
            index: 0,
            dataset_generation: audit.dataset_generation,
            mode: MediaMode::Preview,
            focused: false,
            key: compare::Key::new(
                Path::new("photo.jpg"),
                Format::WebP,
                Quality::lossy(80.),
                MaxEdge::FULL,
            ),
            preview: None,
            pair: None,
            failed: false,
            split: 0.5,
            pan: (0., 0.),
            zoom: None,
            drag: None,
            written: None,
            produced_by: None,
        });
        cx.notify();
    });
    cx.update(|window, cx| window.draw(cx).clear(cx));
    cx.update(|window, cx| window.draw(cx).clear(cx));
    assert!(cx.debug_bounds("compare-bar").is_some());
    assert!(cx.debug_bounds("preview-ai-actions").is_some());
    audit.update(cx, |audit, cx| {
        audit.compare.as_mut().unwrap().mode = MediaMode::Compare;
        cx.notify();
    });
    cx.update(|window, cx| window.draw(cx).clear(cx));

    assert!(cx.debug_bounds("preview-ai-actions").is_none());
}

/// Bytes per pixel is a ratio, and a ratio on a 44-byte sliver is arithmetic
/// rather than a finding. The claim is that converting would win something.
#[test]
fn heavy_needs_a_file_worth_converting() {
    // A 300 KB screenshot at 30 bytes per pixel: the finding it was built for.
    let bloated = entry("screenshot.png", 100, 100, 300_000, ImageFormat::Png);
    assert!(Finding::Heavy.holds(&bloated));

    // The same ratio on something too small to give anything back.
    let sliver = entry("sliver.png", 1, 2, 44, ImageFormat::Png);
    assert!(sliver.bytes_per_pixel() > 1.5);
    assert!(!Finding::Heavy.holds(&sliver));

    // Big enough on disk, but a handful of pixels: still nothing to win.
    let tiny = entry("icon.png", 8, 8, 100_000, ImageFormat::Png);
    assert!(!Finding::Heavy.holds(&tiny));

    // A photograph is never heavy however large the file is.
    let photo = entry("photo.jpg", 4000, 3000, 2_000_000, ImageFormat::Jpeg);
    assert!(!Finding::Heavy.holds(&photo));
}

/// The picker's toggle has to reach the table, not only the state: the delegate
/// caches its column list against a signature, and a preference left out of that
/// signature changes nothing on screen.
#[gpui_kit::test]
fn toggling_a_column_reaches_the_table(cx: &mut TestAppContext) {
    let (audit, cx) = finding_audit(cx);
    cx.update(|window, cx| window.draw(cx).clear(cx));

    let columns = |cx: &mut gpui_kit::VisualTestContext| {
        audit.read_with(cx, |audit, cx| {
            audit
                .table
                .as_ref()
                .map(|table| table.read(cx).delegate().columns_for_test().to_vec())
                .unwrap_or_default()
        })
    };
    assert!(!columns(cx).contains(&TableColumn::Density));

    // Index 3 is B/px, the one column that starts off.
    audit.update(cx, |audit, cx| audit.toggle_column(3, cx));
    cx.update(|window, cx| window.draw(cx).clear(cx));
    cx.update(|window, cx| window.draw(cx).clear(cx));
    assert!(columns(cx).contains(&TableColumn::Density));

    audit.update(cx, |audit, cx| audit.reset_columns(cx));
    cx.update(|window, cx| window.draw(cx).clear(cx));
    cx.update(|window, cx| window.draw(cx).clear(cx));
    assert!(!columns(cx).contains(&TableColumn::Density));
}

#[gpui_kit::test]
fn dragging_a_column_changes_and_keeps_its_display_order(cx: &mut TestAppContext) {
    let (audit, cx) = finding_audit(cx);
    let table = audit
        .read_with(cx, |audit, _| audit.table.clone())
        .expect("audit owns its table");

    table.update_in(cx, |table, window, cx| {
        let before = table.delegate().columns_for_test().to_vec();
        let from = before
            .iter()
            .position(|column| *column == TableColumn::Name)
            .unwrap();
        let to = before
            .iter()
            .position(|column| *column == TableColumn::Weight)
            .unwrap();
        TableDelegate::move_column(table.delegate_mut(), from, to, window, cx);

        let moved = table.delegate().columns_for_test().to_vec();
        assert_eq!(moved[to], TableColumn::Name);

        table
            .delegate_mut()
            .set_viewport_width(1100., ColumnPrefs::default(), false, false);
        assert_eq!(table.delegate().columns_for_test()[to], TableColumn::Name);
    });
}

/// The two local models and the Studio API act on one file, so they only
/// light up when the ticked set names exactly one.
#[gpui_kit::test]
fn one_ticked_file_is_what_single_image_tools_act_on(cx: &mut TestAppContext) {
    let (audit, cx) = finding_audit(cx);
    // A folder opens fully ticked, which is already more than one file; this
    // test walks up from nothing ticked.
    audit.update(cx, |audit, cx| {
        audit.selected.clear();
        audit.selection_changed(cx);
    });
    audit.read_with(cx, |audit, _| assert_eq!(audit.single_target(), None));

    let first = audit.read_with(cx, |audit, _| audit.visible[0]);
    audit.update(cx, |audit, cx| {
        audit.selected.insert(first);
        audit.selection_changed(cx);
    });
    audit.read_with(cx, |audit, _| {
        assert_eq!(audit.single_target(), Some(first));
    });

    let second = audit.read_with(cx, |audit, _| audit.visible[1]);
    audit.update(cx, |audit, cx| {
        audit.selected.insert(second);
        audit.selection_changed(cx);
    });
    audit.read_with(cx, |audit, _| assert_eq!(audit.single_target(), None));
}

#[gpui_kit::test]
fn custom_output_and_destination_are_named_in_the_panel(cx: &mut TestAppContext) {
    let (audit, cx) = finding_audit(cx);
    audit.update(cx, |audit, _| {
        audit.quality = Quality::lossy(57.);
        audit.rail = Rail::Convert;
    });
    cx.update(|window, cx| window.draw(cx).clear(cx));

    assert!(cx.debug_bounds("output-destination").is_some());
    assert!(cx.debug_bounds("custom-settings-active").is_some());
}

#[gpui_kit::test]
fn settings_overlay_keeps_the_audit_visible_under_its_scrim(cx: &mut TestAppContext) {
    let (audit, cx) = finding_audit(cx);
    audit.update_in(cx, |audit, window, cx| audit.open_settings(window, cx));
    cx.update(|window, cx| window.draw(cx).clear(cx));

    assert!(cx.debug_bounds("settings-scrim").is_some());
    assert!(
        cx.debug_bounds("audit-header").is_some(),
        "the overlay does not replace the user's audit with a blank canvas"
    );
}

fn assert_pointer_checkbox_toggle(
    audit: &gpui_kit::Entity<Audit>,
    selector: &'static str,
    cx: &mut gpui_kit::VisualTestContext,
) {
    let checkbox = cx
        .debug_bounds(selector)
        .expect("the checkbox must be rendered in its parent event tree");
    let before = audit.read_with(cx, |audit, _| audit.estimate_generation);

    cx.simulate_click(checkbox.center(), gpui_kit::Modifiers::none());
    audit.read_with(cx, |audit, _| {
        assert_eq!(audit.selected, [1].into_iter().collect());
        assert!(audit.compare.is_none());
        assert_eq!(audit.estimate_generation, before + 1);
        assert_eq!(audit.estimate, None);
    });

    cx.update(|window, cx| window.draw(cx).clear(cx));
    let checkbox = cx
        .debug_bounds(selector)
        .expect("the checkbox must remain rendered after its controlled state changes");
    cx.simulate_click(checkbox.center(), gpui_kit::Modifiers::none());
    audit.read_with(cx, |audit, _| {
        assert_eq!(audit.selected, [0, 1].into_iter().collect());
        assert!(audit.compare.is_none());
        assert_eq!(audit.estimate_generation, before + 2);
        assert_eq!(audit.estimate, None);
    });
}

#[gpui_kit::test]
fn grid_checkbox_pointer_click_stays_inside_checkbox(cx: &mut TestAppContext) {
    let (audit, cx) = pointer_checkbox_audit(true, cx);
    assert_pointer_checkbox_toggle(&audit, "grid-checkbox-0", cx);
}

#[gpui_kit::test]
fn table_checkbox_pointer_click_stays_inside_checkbox(cx: &mut TestAppContext) {
    let (audit, cx) = pointer_checkbox_audit(false, cx);
    assert_pointer_checkbox_toggle(&audit, "table-checkbox-0", cx);
}

#[gpui_kit::test]
fn keyboard_selection_refreshes_estimate(cx: &mut TestAppContext) {
    let (audit, cx) = pointer_checkbox_audit(false, cx);
    let before = audit.read_with(cx, |audit, _| audit.estimate_generation);

    audit.update(cx, |audit, cx| audit.toggle_cursor_selection(cx));
    audit.read_with(cx, |audit, _| {
        assert_eq!(audit.selected, [0].into_iter().collect());
        assert_eq!(audit.estimate_generation, before + 1);
        assert_eq!(audit.estimate, None);
    });
}

#[test]
fn checkbox_activation_owns_only_unmodified_space_and_enter() {
    for key in ["space", "enter"] {
        let event = gpui_kit::KeyDownEvent {
            keystroke: gpui_kit::Keystroke {
                key: key.into(),
                ..Default::default()
            },
            is_held: false,
            prefer_character_input: false,
        };
        assert!(is_checkbox_activation_key(&event));

        let mut modified = event.clone();
        modified.keystroke.modifiers.control = true;
        assert!(!is_checkbox_activation_key(&modified));
    }

    let other = gpui_kit::KeyDownEvent {
        keystroke: gpui_kit::Keystroke {
            key: "down".into(),
            ..Default::default()
        },
        is_held: false,
        prefer_character_input: false,
    };
    assert!(!is_checkbox_activation_key(&other));
}

#[test]
fn weight_sorts_heaviest_first_when_descending() {
    let mut entries = vec![
        entry("small.png", 10, 10, 100, ImageFormat::Png),
        entry("big.png", 10, 10, 900, ImageFormat::Png),
        entry("mid.png", 10, 10, 500, ImageFormat::Png),
    ];
    sort_entries(
        &mut entries,
        Sort {
            column: Column::Weight,
            descending: true,
        },
    );
    assert_eq!(names(&entries), ["big.png", "mid.png", "small.png"]);
}

#[test]
fn name_sorting_ignores_case() {
    let mut entries = vec![
        entry("Zebra.png", 1, 1, 1, ImageFormat::Png),
        entry("apple.png", 1, 1, 1, ImageFormat::Png),
    ];
    sort_entries(
        &mut entries,
        Sort {
            column: Column::Name,
            descending: false,
        },
    );
    assert_eq!(names(&entries), ["apple.png", "Zebra.png"]);
}

/// Equal values must not reshuffle between sorts. A list that reorders itself for
/// no visible reason is worse than one sorted badly.
#[test]
fn ties_fall_back_to_the_filename() {
    let mut entries = vec![
        entry("c.png", 4, 4, 200, ImageFormat::Png),
        entry("a.png", 4, 4, 200, ImageFormat::Png),
        entry("b.png", 4, 4, 200, ImageFormat::Png),
    ];
    let sort = Sort {
        column: Column::Density,
        descending: false,
    };
    sort_entries(&mut entries, sort);
    assert_eq!(names(&entries), ["a.png", "b.png", "c.png"]);
}

#[test]
fn pixels_sorts_on_area_not_width() {
    let mut entries = vec![
        entry("wide.png", 1000, 10, 1, ImageFormat::Png),
        entry("square.png", 200, 200, 1, ImageFormat::Png),
    ];
    sort_entries(
        &mut entries,
        Sort {
            column: Column::Pixels,
            descending: true,
        },
    );
    assert_eq!(names(&entries), ["square.png", "wide.png"]);
}

#[test]
fn restored_window_size_defaults_invalid_values_and_clamps_finite_values() {
    for invalid in [
        None,
        Some(f32::NAN),
        Some(f32::INFINITY),
        Some(f32::NEG_INFINITY),
    ] {
        assert_eq!(
            restored_window_size(invalid, invalid),
            (WINDOW_DEFAULT_WIDTH, WINDOW_DEFAULT_HEIGHT)
        );
    }
    assert_eq!(
        restored_window_size(Some(600.), Some(400.)),
        (WINDOW_MIN_WIDTH, WINDOW_MIN_HEIGHT)
    );
    assert_eq!(restored_window_size(Some(1100.), Some(720.)), (1100., 720.));
}

#[test]
fn gallery_geometry_accounts_for_root_chrome_and_supported_widths() {
    assert_eq!(gallery_layout(760., 0., 0., 100).columns, 4);
    assert_eq!(gallery_layout(760., 21., 21., 100).columns, 3);
    assert_eq!(gallery_layout(760., 0., 21., 100).columns, 3);

    assert_eq!(gallery_layout(760., 22., 22., 100).columns, 3);
    assert_eq!(gallery_layout(873., 22., 22., 100).columns, 4);
    assert_eq!(gallery_layout(900., 22., 22., 100).columns, 4);
    assert_eq!(gallery_layout(1100., 22., 22., 100).columns, 5);
    // A wide window keeps filling: the tile size is the only constraint, so a
    // 1920px display shows eight rather than five and a third of empty desk.
    assert_eq!(gallery_layout(1920., 22., 22., 100).columns, 10);
    assert_eq!(gallery_layout(3440., 22., 22., 100).columns, 19);
}

#[gpui_kit::test]
fn folder_browser_is_persistent_only_when_the_workspace_has_room(cx: &mut TestAppContext) {
    let (audit, cx) = finding_audit(cx);

    cx.simulate_resize(size(px(1100.), px(720.)));
    cx.run_until_parked();
    assert!(cx.debug_bounds("folder-sidebar").is_some());
    assert!(cx.debug_bounds("folder-tree-toggle").is_none());

    cx.simulate_resize(size(px(900.), px(720.)));
    cx.run_until_parked();
    assert!(cx.debug_bounds("folder-sidebar").is_none());
    assert!(cx.debug_bounds("folder-tree-toggle").is_some());

    audit.update_in(cx, |audit, window, cx| audit.toggle_browser(window, cx));
    audit.read_with(cx, |audit, _| assert!(audit.browser_overlay));
    cx.simulate_resize(size(px(1100.), px(720.)));
    cx.run_until_parked();
    audit.read_with(cx, |audit, _| assert!(!audit.browser_overlay));
    cx.simulate_resize(size(px(900.), px(720.)));
    cx.run_until_parked();
    assert!(cx.debug_bounds("folder-sidebar").is_none());

    audit.update(cx, |audit, cx| audit.open_rail(Rail::Convert, cx));
    cx.simulate_resize(size(px(1100.), px(720.)));
    cx.run_until_parked();
    // The operations sidebar no longer evicts the folder sidebar: both share
    // the window and the list takes what is left.
    assert!(cx.debug_bounds("folder-sidebar").is_some());
    assert!(cx.debug_bounds("folder-tree-toggle").is_none());
}

#[gpui_kit::test]
fn escape_closes_the_folder_overlay_without_clearing_selection(cx: &mut TestAppContext) {
    let (audit, cx) = finding_audit(cx);
    audit.update(cx, |audit, _| {
        // A folder opens fully ticked; narrow it so the assertion below proves the
        // overlay left the user's own selection alone.
        audit.selected.clear();
        audit.selected.insert(0);
    });
    cx.simulate_resize(size(px(900.), px(720.)));
    cx.run_until_parked();
    audit.update_in(cx, |audit, window, cx| audit.toggle_browser(window, cx));
    cx.run_until_parked();

    audit.read_with(cx, |audit, _| assert!(audit.browser_overlay));
    cx.update(|window, cx| {
        assert!(
            audit
                .read(cx)
                .folder_filter_input
                .read(cx)
                .focus_handle(cx)
                .is_focused(window)
        );
    });
    cx.simulate_keystrokes("escape");
    cx.run_until_parked();

    audit.read_with(cx, |audit, _| {
        assert!(!audit.browser_overlay);
        assert_eq!(audit.selected, HashSet::from([0]));
    });
    cx.update(|window, cx| assert!(audit.read(cx).focus.is_focused(window)));
}

#[gpui_kit::test]
fn backdrop_closes_the_folder_overlay_and_restores_list_focus(cx: &mut TestAppContext) {
    let (audit, cx) = finding_audit(cx);
    cx.simulate_resize(size(px(900.), px(720.)));
    audit.update_in(cx, |audit, window, cx| audit.toggle_browser(window, cx));
    cx.run_until_parked();

    let backdrop = cx
        .debug_bounds("folder-overlay-backdrop")
        .expect("the narrow folder browser has a backdrop");
    cx.simulate_click(backdrop.center(), gpui_kit::Modifiers::none());
    cx.run_until_parked();

    audit.read_with(cx, |audit, _| assert!(!audit.browser_overlay));
    cx.update(|window, cx| assert!(audit.read(cx).focus.is_focused(window)));
}

#[gpui_kit::test]
fn a_stale_recent_removes_itself_without_closing_the_browser(cx: &mut TestAppContext) {
    let stale = std::env::temp_dir().join("press-stale-recent");
    let _ = std::fs::remove_dir_all(&stale);
    let (audit, cx) = finding_audit(cx);
    audit.update(cx, |audit, _| audit.recent_folders = vec![stale.clone()]);
    let original_root = audit.read_with(cx, |audit, _| audit.root.clone());
    cx.simulate_resize(size(px(900.), px(720.)));
    audit.update_in(cx, |audit, window, cx| audit.toggle_browser(window, cx));
    cx.run_until_parked();

    let recent = cx
        .debug_bounds("recent-0")
        .expect("the saved recent folder is visible");
    cx.simulate_click(recent.center(), gpui_kit::Modifiers::none());
    cx.run_until_parked();

    audit.read_with(cx, |audit, _| {
        assert!(audit.recent_folders.is_empty());
        assert_eq!(audit.root, original_root);
        assert!(audit.browser_overlay);
    });
}

#[gpui_kit::test]
fn folder_search_filters_the_loaded_tree_case_insensitively(cx: &mut TestAppContext) {
    let root = scan_fixture("folder-search");
    let alpha = root.join("Alpha");
    let beta = root.join("Beta");
    std::fs::create_dir_all(&alpha).unwrap();
    std::fs::create_dir_all(&beta).unwrap();
    let (audit, cx) = finding_audit(cx);

    audit.update(cx, |audit, cx| audit.request_path(root.clone(), cx));
    cx.run_until_parked();
    cx.simulate_resize(size(px(1100.), px(720.)));
    cx.run_until_parked();
    let search = cx
        .debug_bounds("folder-search")
        .expect("the folder browser has a search field");
    cx.simulate_click(search.center(), gpui_kit::Modifiers::none());
    cx.simulate_input("ALP");
    cx.run_until_parked();

    audit.read_with(cx, |audit, cx| {
        assert_eq!(audit.folder_filter_input.read(cx).value(), "ALP");
        assert!(audit.tree_paths.values().any(|path| path == &alpha));
        assert!(!audit.tree_paths.values().any(|path| path == &beta));
    });
    std::fs::remove_dir_all(root).unwrap();
}

#[gpui_kit::test]
fn a_failed_tree_listing_can_be_retried(cx: &mut TestAppContext) {
    let root = scan_fixture("folder-tree-retry");
    let missing = root.join("later");
    let child = missing.join("child");
    let (audit, cx) = finding_audit(cx);

    audit.update(cx, |audit, cx| {
        audit.tree_anchor = root.clone();
        audit.tree_expanded.insert(missing.clone());
        audit.load_tree_children(missing.clone(), cx);
    });
    cx.run_until_parked();
    audit.read_with(cx, |audit, _| {
        assert!(!audit.tree_loaded.contains(&missing));
        assert!(!audit.tree_loading.contains(&missing));
        assert!(!audit.tree_expanded.contains(&missing));
    });

    std::fs::create_dir_all(&child).unwrap();
    audit.update(cx, |audit, cx| {
        audit.load_tree_children(missing.clone(), cx)
    });
    cx.run_until_parked();
    audit.read_with(cx, |audit, _| {
        assert!(audit.tree_loaded.contains(&missing));
        assert_eq!(audit.tree_children.get(&missing), Some(&vec![child]));
    });
    std::fs::remove_dir_all(root).unwrap();
}

#[gpui_kit::test]
fn changing_output_rebuilds_the_folder_tree(cx: &mut TestAppContext) {
    let root = scan_fixture("folder-output-tree");
    let child = root.join("generated");
    std::fs::create_dir_all(&child).unwrap();
    let (audit, cx) = finding_audit(cx);

    audit.update(cx, |audit, cx| {
        audit.output = Output::Folder(child.clone());
        audit.request_path(root.clone(), cx);
    });
    cx.run_until_parked();
    audit.read_with(cx, |audit, _| {
        assert!(!audit.tree_paths.values().any(|path| path == &child));
    });

    audit.update(cx, |audit, cx| audit.reset_output(cx));
    audit.read_with(cx, |audit, _| {
        assert!(audit.tree_paths.values().any(|path| path == &child));
    });
    std::fs::remove_dir_all(root).unwrap();
}

#[gpui_kit::test]
fn folder_disclosure_collapses_without_reopening_the_folder(cx: &mut TestAppContext) {
    let root = scan_fixture_in(
        &browser::home_dir().unwrap_or_else(std::env::temp_dir),
        "folder-collapse",
    );
    std::fs::create_dir_all(root.join("child")).unwrap();
    let (audit, cx) = finding_audit(cx);

    audit.update(cx, |audit, cx| audit.request_path(root.clone(), cx));
    cx.run_until_parked();
    cx.simulate_resize(size(px(1100.), px(720.)));
    cx.run_until_parked();
    let disclosure = tree_row_bounds(&audit, &root, "folder-disclosure", cx);
    let generation = audit.read_with(cx, |audit, _| audit.dataset_generation);
    cx.simulate_click(disclosure.center(), gpui_kit::Modifiers::none());
    cx.run_until_parked();

    audit.read_with(cx, |audit, _| {
        assert_eq!(audit.root, root);
        assert!(!audit.tree_expanded.contains(&audit.root));
        assert_eq!(audit.dataset_generation, generation);
    });
    std::fs::remove_dir_all(root).unwrap();
}

#[gpui_kit::test]
fn clicking_a_tree_folder_label_opens_the_folder(cx: &mut TestAppContext) {
    let root = scan_fixture_in(
        &browser::home_dir().unwrap_or_else(std::env::temp_dir),
        "folder-pointer-navigation",
    );
    let child = root.join("child");
    std::fs::create_dir_all(&child).unwrap();
    let (audit, cx) = finding_audit(cx);

    audit.update(cx, |audit, cx| audit.request_path(root.clone(), cx));
    cx.run_until_parked();
    cx.simulate_resize(size(px(1100.), px(720.)));
    cx.run_until_parked();
    let folder = tree_row_bounds(&audit, &child, "folder-open", cx);
    cx.simulate_click(folder.center(), gpui_kit::Modifiers::none());
    cx.run_until_parked();

    audit.read_with(cx, |audit, _| assert_eq!(audit.root, child));
    std::fs::remove_dir_all(root).unwrap();
}

#[gpui_kit::test]
fn expanding_a_tree_folder_keeps_the_keyboard_selection(cx: &mut TestAppContext) {
    let root = scan_fixture("folder-expand-selection");
    let child = root.join("child");
    std::fs::create_dir_all(child.join("grandchild")).unwrap();
    let (audit, cx) = finding_audit(cx);

    audit.update(cx, |audit, cx| audit.request_path(root.clone(), cx));
    cx.run_until_parked();
    let child_id: gpui_kit::SharedString = audit.read_with(cx, |audit, _| {
        audit
            .tree_paths
            .iter()
            .find_map(|(id, path)| (path == &child).then(|| id.clone().into()))
            .expect("the child is in the tree")
    });
    cx.simulate_resize(size(px(1100.), px(720.)));
    cx.run_until_parked();
    cx.update(|window, cx| {
        audit.update(cx, |audit, cx| {
            audit.tree_state.update(cx, |tree, cx| {
                tree.set_selected_index(tree.index_of(&child_id), cx);
                tree.focus(window, cx);
            });
        });
    });
    cx.simulate_keystrokes("right");
    cx.run_until_parked();

    audit.read_with(cx, |audit, cx| {
        assert!(audit.tree_loaded.contains(&child));
        assert_eq!(
            audit
                .tree_state
                .read(cx)
                .selected_item()
                .map(|item| &item.id),
            Some(&child_id)
        );
    });
    std::fs::remove_dir_all(root).unwrap();
}

#[gpui_kit::test]
fn enter_opens_the_keyboard_selected_tree_folder(cx: &mut TestAppContext) {
    let root = scan_fixture("folder-keyboard");
    let child = root.join("child");
    std::fs::create_dir_all(&child).unwrap();
    let (audit, cx) = finding_audit(cx);

    audit.update(cx, |audit, cx| audit.request_path(root.clone(), cx));
    cx.run_until_parked();
    cx.simulate_resize(size(px(1100.), px(720.)));
    cx.run_until_parked();
    cx.update(|window, cx| {
        audit.update(cx, |audit, cx| {
            audit.selected.insert(0);
            audit
                .tree_state
                .update(cx, |tree, cx| tree.focus(window, cx));
        });
    });
    cx.simulate_keystrokes("space");
    audit.read_with(cx, |audit, _| {
        assert_eq!(audit.selected, HashSet::from([0]))
    });
    cx.simulate_keystrokes("down enter");
    cx.run_until_parked();

    audit.read_with(cx, |audit, _| assert_eq!(audit.root, child));
    std::fs::remove_dir_all(root).unwrap();
}

#[gpui_kit::test]
fn arrow_down_moves_keyboard_focus_from_search_to_the_tree(cx: &mut TestAppContext) {
    let root = scan_fixture("folder-search-keyboard");
    let child = root.join("child");
    std::fs::create_dir_all(&child).unwrap();
    let (audit, cx) = finding_audit(cx);

    audit.update(cx, |audit, cx| audit.request_path(root.clone(), cx));
    cx.run_until_parked();
    cx.simulate_resize(size(px(900.), px(720.)));
    audit.update_in(cx, |audit, window, cx| audit.toggle_browser(window, cx));
    cx.run_until_parked();

    cx.simulate_keystrokes("down down enter");
    cx.run_until_parked();

    audit.read_with(cx, |audit, _| assert_eq!(audit.root, child));
    std::fs::remove_dir_all(root).unwrap();
}

#[cfg(unix)]
#[gpui_kit::test]
fn output_aliases_stay_out_of_the_folder_tree(cx: &mut TestAppContext) {
    use std::os::unix::fs::symlink;

    let root = scan_fixture("folder-output-alias");
    let output = root.join("generated");
    let alias = root.join("output-link");
    std::fs::create_dir_all(&output).unwrap();
    symlink(&output, &alias).unwrap();
    let (audit, cx) = finding_audit(cx);

    audit.update(cx, |audit, cx| {
        audit.output = Output::Folder(alias);
        audit.request_path(root.clone(), cx);
    });
    cx.run_until_parked();

    audit.read_with(cx, |audit, _| {
        assert_eq!(audit.browser_output_root, output);
        assert!(!audit.tree_paths.values().any(|path| path == &output));
    });
    std::fs::remove_dir_all(root).unwrap();
}

#[cfg(unix)]
#[gpui_kit::test]
fn a_symlinked_source_uses_one_identity_for_its_output(cx: &mut TestAppContext) {
    use std::os::unix::fs::symlink;

    let fixture = scan_fixture("folder-source-alias");
    let root = fixture.join("photos");
    let output = root.join(scan::OUTPUT_DIR);
    let alias = fixture.join("photos-link");
    std::fs::create_dir_all(&output).unwrap();
    symlink(&root, &alias).unwrap();
    let (audit, cx) = finding_audit(cx);

    audit.update(cx, |audit, cx| audit.request_path(alias, cx));
    cx.run_until_parked();

    audit.read_with(cx, |audit, _| {
        assert_eq!(audit.root, root);
        assert_eq!(audit.browser_output_root, output);
        assert!(audit.folders.iter().any(|path| path == &output));
        assert!(!audit.tree_paths.values().any(|path| path == &output));
    });
    std::fs::remove_dir_all(fixture).unwrap();
}

#[gpui_kit::test]
fn opening_a_folder_centers_the_sidebar_tree_on_it(cx: &mut TestAppContext) {
    let root = scan_fixture("tree-centering");
    // Enough siblings that the last child sits far below the fold: selected
    // but nowhere on screen, unless the navigation reveals it.
    for index in 0..40 {
        std::fs::create_dir_all(root.join(format!("child-{index:02}"))).unwrap();
    }
    let (audit, cx) = finding_audit(cx);
    cx.simulate_resize(size(px(1100.), px(720.)));
    audit.update(cx, |audit, cx| audit.request_path(root.clone(), cx));
    cx.run_until_parked();

    let deep = root.join("child-39");
    audit.update(cx, |audit, cx| audit.request_path(deep.clone(), cx));
    cx.run_until_parked();

    let index = audit.read_with(cx, |audit, cx| {
        let tree = audit.tree_state.read(cx);
        let id = audit
            .tree_paths
            .iter()
            .find_map(|(id, path)| (path == &deep).then(|| id.clone()))
            .expect("the open folder is in the tree");
        // The reveal centers what the selection names.
        assert_eq!(
            tree.selected_item().map(|item| item.id.to_string()),
            Some(id.clone())
        );
        tree.index_of(&id.into())
            .expect("the open folder has a row")
    });
    // Drawn after the navigation: the reveal already asked for this row, so a
    // frame either honours it or leaves the row where the scroll was.
    cx.update(|window, cx| window.draw(cx).clear(cx));
    // Tree rows carry their debug selector on the inner navigation div, not
    // the row item, which is how the disclosure tests find them too.
    let selector = Box::leak(format!("folder-open-{index}").into_boxed_str());
    assert!(
        cx.debug_bounds(selector).is_some(),
        "the open folder's tree row is on screen"
    );
    std::fs::remove_dir_all(root).unwrap();
}

#[gpui_kit::test]
fn folder_navigation_is_shallow_and_replaces_the_file_selection(cx: &mut TestAppContext) {
    let root = scan_fixture("shallow-navigation");
    let child = root.join("child");
    let empty = child.join("empty");
    std::fs::create_dir_all(&empty).unwrap();
    write_png(&root, "direct.png");
    write_png(&child, "nested.png");
    let (audit, cx) = finding_audit(cx);

    audit.update(cx, |audit, cx| audit.request_path(root.clone(), cx));
    cx.run_until_parked();
    audit.update(cx, |audit, cx| {
        assert_eq!(audit.folders, vec![child.clone()]);
        assert_eq!(audit.entries.len(), 1);
        audit.selected.insert(0);
        audit.selection_changed(cx);
        audit.request_path(child.clone(), cx);
    });
    cx.run_until_parked();

    audit.read_with(cx, |audit, _| {
        assert_eq!(audit.root, child.clone());
        assert_eq!(audit.entries.len(), 1);
        // The new folder's own row is ticked; the tick from the old folder is gone
        // rather than carried over.
        assert_eq!(audit.selected, HashSet::from([0]));
        assert_eq!(audit.recent_folders.first(), Some(&audit.root));
    });
    audit.update(cx, |audit, cx| audit.request_path(empty.clone(), cx));
    cx.run_until_parked();
    assert!(cx.debug_bounds("audit-header").is_some());
    assert!(cx.debug_bounds("empty-folder-message").is_some());
    assert!(cx.debug_bounds("action-bar").is_none());
    audit.read_with(cx, |audit, _| {
        assert_eq!(audit.root, empty);
        assert!(audit.entries.is_empty() && audit.folders.is_empty());
    });
    std::fs::remove_dir_all(root).unwrap();
}

#[gpui_kit::test]
fn the_subfolders_toggle_lists_nested_images_with_relative_labels(cx: &mut TestAppContext) {
    let root = scan_fixture("subfolders-toggle");
    let child = root.join("child");
    let grandchild = child.join("grandchild");
    std::fs::create_dir_all(&grandchild).unwrap();
    write_png(&root, "direct.png");
    write_png(&child, "nested.png");
    write_png(&grandchild, "deep.png");
    let whole = scan::scan(&root, &root.join(scan::OUTPUT_DIR))
        .entries
        .len();
    assert_eq!(whole, 3);
    let (audit, cx) = finding_audit(cx);

    audit.update(cx, |audit, cx| audit.request_path(root.clone(), cx));
    cx.run_until_parked();
    audit.read_with(cx, |audit, _| {
        assert!(!audit.include_subfolders);
        assert_eq!(audit.entries.len(), 1);
        assert_eq!(audit.folders, vec![child.clone()]);
        assert!(
            !audit.status_line(1).contains("including subfolders"),
            "one level reads as one level"
        );
    });

    // Scope moved from the header chip to the Open menu; the toggle itself is
    // unchanged, so the test drives it directly and reads the status line.
    audit.update(cx, |audit, cx| audit.toggle_subfolders(cx));
    cx.run_until_parked();
    cx.update(|window, cx| window.draw(cx).clear(cx));
    audit.read_with(cx, |audit, _| {
        assert!(audit.include_subfolders);
        assert!(
            audit.settings.include_subfolders,
            "the choice is remembered"
        );
        assert_eq!(audit.root, root);
        assert_eq!(audit.entries.len(), whole);
        assert!(
            audit.status_line(whole).contains("including subfolders"),
            "the status bar says what the numbers cover"
        );
        assert_eq!(
            audit.folders,
            vec![child.clone()],
            "the tree still navigates"
        );
        assert!(audit.tree_paths.values().any(|path| path == &child));
        let mut labels: Vec<String> = audit
            .entries
            .iter()
            .map(|entry| entry_label(&audit.root, audit.show_parent(), entry))
            .collect();
        labels.sort();
        assert_eq!(
            labels,
            vec![
                Path::new("child")
                    .join("grandchild")
                    .join("deep.png")
                    .to_string_lossy()
                    .into_owned(),
                Path::new("child")
                    .join("nested.png")
                    .to_string_lossy()
                    .into_owned(),
                "direct.png".to_string(),
            ]
        );
        assert!(audit.scanning.is_none());
        assert!(audit.scan_cancellation.is_none());
    });

    audit.update(cx, |audit, cx| audit.toggle_subfolders(cx));
    cx.run_until_parked();
    audit.read_with(cx, |audit, _| {
        assert!(!audit.include_subfolders);
        assert_eq!(audit.entries.len(), 1);
        assert_eq!(
            entry_label(&audit.root, audit.show_parent(), &audit.entries[0]),
            "direct.png"
        );
    });
    std::fs::remove_dir_all(root).unwrap();
}

#[gpui_kit::test]
fn the_scanning_screen_shows_the_count_and_cancel_keeps_the_last_folder(cx: &mut TestAppContext) {
    assert_eq!(view::scan_progress_line(1), "Found 1 image…");
    assert_eq!(view::scan_progress_line(999), "Found 999 images…");
    assert_eq!(view::scan_progress_line(1240), "Found 1240 images…");

    let (audit, cx) = finding_audit(cx);
    let (token, request) = audit.update(cx, |audit, _| {
        let token = retained_scan(audit);
        audit.scan_found = Some(1240);
        (token, audit.scan_generation)
    });
    cx.update(|window, cx| window.draw(cx).clear(cx));
    let cancel = cx
        .debug_bounds("cancel-scan")
        .expect("a scan with a token offers Cancel");
    assert!(cx.debug_bounds("audit-header").is_none());

    cx.simulate_click(cancel.center(), gpui_kit::Modifiers::default());
    cx.run_until_parked();
    assert!(
        token.load(Ordering::Acquire),
        "Cancel raises the scan token"
    );
    audit.read_with(cx, |audit, _| {
        assert_eq!(audit.scan_generation, request.wrapping_add(1));
        assert!(audit.scanning.is_none());
        assert!(audit.scan_found.is_none());
        assert!(audit.scan_cancellation.is_none());
        assert_eq!(audit.entries.len(), 3);
        assert_eq!(audit.dataset_generation, 0);
    });
    assert!(cx.debug_bounds("audit-header").is_some());
}

#[gpui_kit::test]
fn cancelling_a_subfolder_scan_mid_walk_keeps_the_previous_dataset(cx: &mut TestAppContext) {
    let root = scan_fixture("subfolders-cancel");
    let child = root.join("child");
    std::fs::create_dir_all(&child).unwrap();
    write_png(&root, "direct.png");
    write_png(&child, "nested.png");
    let (audit, cx) = finding_audit(cx);
    let retained = audit.read_with(cx, |audit, _| {
        audit
            .visible
            .iter()
            .map(|index| audit.entries[*index].name())
            .collect::<Vec<_>>()
    });
    assert_eq!(retained, vec!["screenshot.png", "photo.jpg", "liar.webp"]);

    // Cancel the way the button does, from inside the walk: the token goes up as
    // the second path is handed to a worker, so one file was already in flight.
    let token_slot: Arc<parking_lot::Mutex<Option<Arc<std::sync::atomic::AtomicBool>>>> =
        Arc::new(parking_lot::Mutex::new(None));
    let sends = Arc::new(std::sync::atomic::AtomicUsize::new(0));
    scan::TEST_HOOKS.with(|hooks| {
        let token_slot = token_slot.clone();
        let sends = sends.clone();
        let mut seams = scan::ScanHooks::default();
        seams.before_path_send = Arc::new(move || {
            if sends.fetch_add(1, Ordering::Relaxed) == 1
                && let Some(token) = token_slot.lock().as_ref()
            {
                token.store(true, Ordering::Release);
            }
        });
        *hooks.borrow_mut() = Some(seams);
    });
    let request = audit.update(cx, |audit, cx| {
        audit.include_subfolders = true;
        let request = audit.scan_generation;
        audit.request_path(root.clone(), cx);
        assert!(audit.scanning.is_some());
        *token_slot.lock() = Some(
            audit
                .scan_cancellation
                .as_ref()
                .map(|cancellation| cancellation.token.clone())
                .expect("a tree walk is cancellable"),
        );
        request
    });
    cx.run_until_parked();
    scan::TEST_HOOKS.with(|hooks| *hooks.borrow_mut() = None);
    assert_eq!(
        sends.load(Ordering::Relaxed),
        2,
        "the walk was stopped after one send"
    );
    audit.read_with(cx, |audit, _| {
        assert_eq!(audit.scan_generation, request.wrapping_add(1));
        assert_eq!(audit.dataset_generation, 0, "no half dataset is committed");
        let names: Vec<String> = audit
            .visible
            .iter()
            .map(|index| audit.entries[*index].name())
            .collect();
        assert_eq!(names, retained);
        assert_eq!(audit.root, PathBuf::new());
        assert!(audit.folders.is_empty());
        assert!(audit.scanning.is_none());
        assert!(audit.scan_cancellation.is_none());
        assert!(
            !audit.include_subfolders,
            "the chip follows the list it shows"
        );
        assert!(!audit.dataset_subfolders);
    });

    // The same request completes when nobody stops it, with the count shown on
    // the way: the cancel above stopped a walk that would have landed.
    audit.update(cx, |audit, cx| {
        audit.include_subfolders = true;
        audit.request_path(root.clone(), cx)
    });
    cx.run_until_parked();
    audit.read_with(cx, |audit, _| {
        assert_eq!(audit.dataset_generation, 1);
        assert_eq!(audit.entries.len(), 2);
        assert_eq!(audit.root, root);
        assert!(audit.dataset_subfolders);
        assert_eq!(
            audit.scan_found,
            Some(2),
            "the live count reached the window"
        );
    });
    std::fs::remove_dir_all(root).unwrap();
}

#[gpui_kit::test]
fn turning_subfolders_off_then_cancelling_keeps_the_nested_list_and_the_chip(
    cx: &mut TestAppContext,
) {
    let root = scan_fixture("subfolders-off-cancel");
    let child = root.join("child");
    std::fs::create_dir_all(&child).unwrap();
    write_png(&root, "direct.png");
    write_png(&child, "nested.png");
    let (audit, cx) = finding_audit(cx);
    audit.update(cx, |audit, cx| {
        audit.include_subfolders = true;
        audit.request_path(root.clone(), cx);
    });
    cx.run_until_parked();
    let nested = Path::new("child")
        .join("nested.png")
        .to_string_lossy()
        .into_owned();
    let labels_of = |audit: &Audit| -> Vec<String> {
        audit
            .visible
            .iter()
            .map(|index| entry_label(&audit.root, audit.show_parent(), &audit.entries[*index]))
            .collect()
    };
    let (before, order) = audit.read_with(cx, |audit, _| {
        assert!(audit.dataset_subfolders);
        assert_eq!(audit.entries.len(), 2);
        (labels_of(audit), audit.visible.clone())
    });
    assert!(
        before.contains(&nested),
        "the list is the nested one: {before:?}"
    );

    audit.update(cx, |audit, cx| {
        audit.toggle_subfolders(cx);
        assert!(!audit.include_subfolders);
        assert!(audit.scanning.is_some());
        audit.cancel_scan(cx);
    });
    cx.run_until_parked();
    cx.update(|window, cx| window.draw(cx).clear(cx));
    audit.read_with(cx, |audit, _| {
        assert_eq!(audit.entries.len(), 2);
        assert_eq!(
            audit.visible, order,
            "the order is the one the labels sorted"
        );
        assert_eq!(labels_of(audit), before, "no label lost its folder");
        assert!(
            audit.include_subfolders,
            "the chip shows the scope the list has"
        );
        assert!(
            audit.settings.include_subfolders,
            "and the settings remember that"
        );
    });
    std::fs::remove_dir_all(root).unwrap();
}

#[gpui_kit::test]
fn a_heic_only_folder_says_how_many_it_skipped(cx: &mut TestAppContext) {
    let root = scan_fixture("heic-only");
    std::fs::write(root.join("IMG_0001.heic"), b"not really a heic")
        .expect("the fixture HEIC is written");
    let (audit, cx) = finding_audit(cx);

    audit.update(cx, |audit, cx| audit.request_path(root.clone(), cx));
    cx.run_until_parked();

    audit.read_with(cx, |audit, _| {
        assert!(audit.entries.is_empty(), "no HEIC is decoded");
        assert_eq!(audit.skipped_heic, 1);
        let stats = audit.status_line(0);
        assert!(
            stats.contains("1 HEIC skipped (not supported yet)"),
            "the status bar owes the user the count: {stats}"
        );
    });
    assert!(cx.debug_bounds("empty-folder-message").is_some());
    // The empty state draws this sentence for this folder. gpui's test context can look
    // up bounds by selector but not read drawn text, so the sentence is asserted from
    // the audit's own state rather than off the screen.
    let folder = root
        .file_name()
        .expect("the fixture folder is named")
        .to_string_lossy()
        .into_owned();
    let detail = audit.read_with(cx, |audit, _| {
        view::empty_folder_detail(&folder, audit.skipped_heic, audit.skipped_raw, 0)
    });
    assert_eq!(
        detail,
        format!("The “{folder}” folder has 1 HEIC file, not supported yet.")
    );
    assert_eq!(
        view::empty_folder_detail("shoot", 12, 0, 0),
        "The “shoot” folder has 12 HEIC files, not supported yet."
    );
    std::fs::remove_dir_all(root).unwrap();
}

#[test]
fn empty_folder_detail_names_raw_and_package_counts() {
    assert_eq!(
        view::empty_folder_detail("shoot", 0, 0, 0),
        "The “shoot” folder has no direct supported images."
    );
    assert_eq!(
        view::empty_folder_detail("shoot", 0, 1, 0),
        "The “shoot” folder has no direct supported images. \
         Plus 1 camera raw file (counted, not listed)."
    );
    assert_eq!(
        view::empty_folder_detail("shoot", 0, 3, 2),
        "The “shoot” folder has no direct supported images. \
         Plus 3 camera raw files (counted, not listed). \
         Plus 2 macOS packages (counted, not listed)."
    );
    assert_eq!(
        view::empty_folder_detail("shoot", 2, 0, 1),
        "The “shoot” folder has 2 HEIC files, not supported yet. \
         Plus 1 macOS package (counted, not listed)."
    );
}

#[gpui_kit::test]
fn a_folder_containing_only_output_uses_the_empty_state(cx: &mut TestAppContext) {
    let root = scan_fixture("output-only-empty-state");
    let output = root.join(scan::OUTPUT_DIR);
    std::fs::create_dir_all(&output).unwrap();
    let (audit, cx) = finding_audit(cx);

    audit.update(cx, |audit, cx| audit.request_path(root.clone(), cx));
    cx.run_until_parked();

    audit.read_with(cx, |audit, _| {
        assert!(audit.entries.is_empty());
        assert_eq!(audit.folders, vec![output]);
        assert!(!audit.has_visible_folders());
    });
    assert!(cx.debug_bounds("empty-folder-message").is_some());
    assert!(cx.debug_bounds("child-folders").is_none());
    std::fs::remove_dir_all(root).unwrap();
}
#[gpui_kit::test]
fn empty_state_offers_next_moves_and_conditional_subfolders(cx: &mut TestAppContext) {
    // A truly empty folder names no counts and has no child folders to walk
    // into, so the way out is opening something else — never subfolders.
    let bare = scan_fixture("empty-actions-bare");
    let (audit, cx) = finding_audit(cx);
    audit.update(cx, |audit, cx| audit.request_path(bare.clone(), cx));
    cx.run_until_parked();
    audit.read_with(cx, |audit, _| {
        assert!(audit.entries.is_empty());
        assert!(audit.folders.is_empty());
        assert!(!audit.include_subfolders);
    });
    assert!(cx.debug_bounds("empty-folder-message").is_some());
    assert!(cx.debug_bounds("empty-open-other").is_some());
    assert!(cx.debug_bounds("empty-open-images").is_some());
    assert!(cx.debug_bounds("empty-include-subfolders").is_none());
    std::fs::remove_dir_all(bare).unwrap();

    // Child folders on disk with subfolders off: the walk-into-them way out
    // shows. Flipping scope on removes it, since it can no longer change the
    // list.
    let root = scan_fixture("empty-actions-nested");
    let output = root.join(scan::OUTPUT_DIR);
    std::fs::create_dir_all(&output).unwrap();
    let (audit, cx) = finding_audit(cx);
    audit.update(cx, |audit, cx| audit.request_path(root.clone(), cx));
    cx.run_until_parked();
    audit.read_with(cx, |audit, _| {
        assert!(audit.entries.is_empty());
        assert!(!audit.folders.is_empty());
        assert!(!audit.include_subfolders);
    });
    assert!(cx.debug_bounds("empty-folder-message").is_some());
    assert!(cx.debug_bounds("empty-open-other").is_some());
    assert!(cx.debug_bounds("empty-open-images").is_some());
    assert!(cx.debug_bounds("empty-include-subfolders").is_some());
    audit.update(cx, |audit, cx| audit.toggle_subfolders(cx));
    cx.run_until_parked();
    cx.update(|window, cx| {
        window.draw(cx).clear(cx);
    });
    audit.read_with(cx, |audit, _| assert!(audit.include_subfolders));
    assert!(cx.debug_bounds("empty-include-subfolders").is_none());
    std::fs::remove_dir_all(root).unwrap();
}

#[gpui_kit::test]
fn relative_navigation_stores_an_absolute_root(cx: &mut TestAppContext) {
    let nonce = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .unwrap()
        .as_nanos();
    let relative = PathBuf::from("target").join(format!("press-relative-root-{nonce}"));
    let absolute = std::env::current_dir().unwrap().join(&relative);
    std::fs::create_dir_all(&absolute).unwrap();
    let absolute = std::fs::canonicalize(absolute).unwrap();
    let (audit, cx) = finding_audit(cx);

    audit.update(cx, |audit, cx| audit.request_path(relative, cx));
    cx.run_until_parked();

    audit.read_with(cx, |audit, _| {
        assert_eq!(audit.root, absolute);
        assert_eq!(audit.recent_folders.first(), Some(&absolute));
        assert!(
            audit
                .breadcrumb_parts()
                .iter()
                .all(|(_, path)| !path.as_os_str().is_empty())
        );
    });
    std::fs::remove_dir_all(absolute).unwrap();
}

#[test]
fn gallery_changes_column_only_at_each_reachable_threshold() {
    let root = 22.;
    for columns in 2..=12 {
        let threshold = 2. * root
            + 2. * (ROOT_PADDING + ROOT_BORDER + GALLERY_PADDING + GALLERY_BORDER)
            + columns as f32 * TILE_MIN
            + (columns - 1) as f32 * TILE_GAP;
        assert_eq!(
            gallery_layout(threshold - 1., root, root, 100).columns,
            columns - 1
        );
        assert_eq!(gallery_layout(threshold, root, root, 100).columns, columns);
    }
}

#[test]
fn gallery_bands_cover_each_entry_once_for_one_three_and_five_columns() {
    for columns in [1, 3, 5] {
        let chrome = 2. * (ROOT_PADDING + ROOT_BORDER + GALLERY_PADDING + GALLERY_BORDER);
        let width = chrome + columns as f32 * TILE_MIN + (columns - 1) as f32 * TILE_GAP;
        let layout = gallery_layout(width, 0., 0., 13);
        assert_eq!(layout.columns, columns);
        assert_eq!(layout.rows, 13_usize.div_ceil(columns));
        assert_eq!(
            layout.bands().flatten().collect::<Vec<_>>(),
            (0..13).collect::<Vec<_>>()
        );
    }
}

#[gpui_kit::test]
fn gallery_scroll_resets_only_when_the_production_column_count_changes(
    cx: &mut gpui_kit::TestAppContext,
) {
    cx.update(init_theme);
    let entries = (0..120)
        .map(|index| entry(&format!("image-{index}.png"), 1, 1, 1, ImageFormat::Png))
        .collect();
    let mut audit_entity = None;
    let (_, cx) = cx.add_window_view(|window, cx| {
        let audit = build_audit(
            Launch {
                root: PathBuf::new(),
                entries,
                skipped_raw: 0,
                skipped_heic: 0,
                skipped_packages: 0,
                unreadable: Vec::new(),
                walk_errors: Vec::new(),
                existing_output: 0,
                open_single: false,
                format: Format::WebP,
                quality: Quality::lossy(80.),
                max_edge: MaxEdge::FULL,
                grid: true,
                recent_folders: Vec::new(),
                columns: ColumnPrefs::default(),
                output: crate::settings::Output::default(),
                include_subfolders: false,
                sidebar_open: true,
            },
            window,
            cx,
        );
        audit_entity = Some(audit.clone());
        Root::new(audit, window, cx).bg(cx.theme().background)
    });
    let audit = audit_entity.expect("audit is built for the production Root");

    cx.simulate_resize(size(px(873.), px(720.)));
    cx.run_until_parked();
    assert!(
        cx.debug_bounds("gallery-scrollbar").is_some(),
        "the production gallery exposes a draggable scrollbar"
    );
    // Root installs its client inset during its first draw. Settle that frame
    // before establishing the deliberately deep scroll position.
    cx.simulate_resize(size(px(873.), px(720.)));
    cx.run_until_parked();
    audit.update_in(cx, |audit, window, _| {
        audit
            .gallery_scroll
            .scroll_to_item_strict(12, ScrollStrategy::Top);
        window.refresh();
    });
    cx.simulate_resize(size(px(873.), px(720.)));
    cx.run_until_parked();
    assert!(audit.read_with(cx, |audit, _| audit.gallery_scroll.is_scrollable()));
    assert!(audit.read_with(cx, |audit, _| {
        audit.gallery_scroll.0.borrow().base_handle.offset().y < px(0.)
    }));

    cx.simulate_resize(size(px(600.), px(720.)));
    cx.run_until_parked();
    assert_eq!(
        audit.read_with(cx, |audit, _| audit
            .gallery_scroll
            .0
            .borrow()
            .base_handle
            .offset()
            .y),
        px(0.)
    );

    audit.update_in(cx, |audit, window, _| {
        audit
            .gallery_scroll
            .scroll_to_item_strict(12, ScrollStrategy::Top);
        window.refresh();
    });
    cx.simulate_resize(size(px(600.), px(720.)));
    cx.run_until_parked();
    cx.simulate_resize(size(px(700.), px(720.)));
    cx.run_until_parked();
    assert!(audit.read_with(cx, |audit, _| {
        audit.gallery_scroll.0.borrow().base_handle.offset().y < px(0.)
    }));
}

#[gpui_kit::test]
fn opening_another_large_folder_resets_gallery_scroll_at_the_same_column_count(
    cx: &mut gpui_kit::TestAppContext,
) {
    const PNG: &[u8] = &[
        0x89, 0x50, 0x4e, 0x47, 0x0d, 0x0a, 0x1a, 0x0a, 0x00, 0x00, 0x00, 0x0d, 0x49, 0x48, 0x44,
        0x52, 0x00, 0x00, 0x00, 0x01, 0x00, 0x00, 0x00, 0x01, 0x08, 0x04, 0x00, 0x00, 0x00, 0xb5,
        0x1c, 0x0c, 0x02, 0x00, 0x00, 0x00, 0x0b, 0x49, 0x44, 0x41, 0x54, 0x78, 0x9c, 0x63, 0xfc,
        0xff, 0x9f, 0x01, 0x00, 0x03, 0x03, 0x02, 0x00, 0xee, 0xfe, 0x3d, 0x68, 0x00, 0x00, 0x00,
        0x00, 0x49, 0x45, 0x4e, 0x44, 0xae, 0x42, 0x60, 0x82,
    ];

    let test_root = std::env::temp_dir().join(format!(
        "imageguide-open-path-scroll-{}-{}",
        std::process::id(),
        std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .expect("the system clock is after the Unix epoch")
            .as_nanos()
    ));
    let first_folder = test_root.join("first");
    let second_folder = test_root.join("second");
    for folder in [&first_folder, &second_folder] {
        std::fs::create_dir_all(folder).expect("the test gallery folder is created");
        for index in 0..120 {
            std::fs::write(folder.join(format!("image-{index}.png")), PNG)
                .expect("the test gallery image is written");
        }
    }

    cx.update(init_theme);
    let mut audit_entity = None;
    let (_, cx) = cx.add_window_view(|window, cx| {
        let audit = build_audit(
            Launch {
                root: PathBuf::new(),
                entries: Vec::new(),
                skipped_raw: 0,
                skipped_heic: 0,
                skipped_packages: 0,
                unreadable: Vec::new(),
                walk_errors: Vec::new(),
                existing_output: 0,
                open_single: false,
                format: Format::WebP,
                quality: Quality::lossy(80.),
                max_edge: MaxEdge::FULL,
                grid: true,
                recent_folders: Vec::new(),
                columns: ColumnPrefs::default(),
                output: crate::settings::Output::default(),
                include_subfolders: false,
                sidebar_open: true,
            },
            window,
            cx,
        );
        audit_entity = Some(audit.clone());
        Root::new(audit, window, cx).bg(cx.theme().background)
    });
    let audit = audit_entity.expect("audit is built for the production Root");

    cx.simulate_resize(size(px(873.), px(720.)));
    cx.run_until_parked();
    cx.simulate_resize(size(px(873.), px(720.)));
    cx.run_until_parked();
    let first_scan = scan::scan(&first_folder, &first_folder.join(scan::OUTPUT_DIR));
    audit.update_in(cx, |audit, window, cx| {
        audit.install_dataset(first_scan, first_folder.clone(), false, None, window, cx);
        window.refresh();
    });
    cx.simulate_resize(size(px(873.), px(720.)));
    cx.run_until_parked();
    audit.update_in(cx, |audit, window, _| {
        audit
            .gallery_scroll
            .scroll_to_item_strict(12, ScrollStrategy::Top);
        window.refresh();
    });
    cx.simulate_resize(size(px(873.), px(720.)));
    cx.run_until_parked();
    assert!(audit.read_with(cx, |audit, _| {
        audit.gallery_scroll.0.borrow().base_handle.offset().y < px(0.)
    }));

    let second_scan = scan::scan(&second_folder, &second_folder.join(scan::OUTPUT_DIR));
    audit.update_in(cx, |audit, window, cx| {
        audit.install_dataset(second_scan, second_folder.clone(), false, None, window, cx);
        window.refresh();
    });
    cx.simulate_resize(size(px(873.), px(720.)));
    cx.run_until_parked();
    assert_eq!(
        audit.read_with(cx, |audit, _| audit
            .gallery_scroll
            .0
            .borrow()
            .base_handle
            .offset()
            .y),
        px(0.)
    );

    std::fs::remove_dir_all(test_root).expect("the test gallery folders are removed");
}

#[gpui_kit::test]
fn opening_another_large_folder_resets_table_scroll(cx: &mut gpui_kit::TestAppContext) {
    cx.update(init_theme);
    let entries = (0..120)
        .map(|index| entry(&format!("old-{index}.png"), 1, 1, 1, ImageFormat::Png))
        .collect();
    let mut audit_entity = None;
    let (_, cx) = cx.add_window_view(|window, cx| {
        let audit = build_audit(
            Launch {
                root: PathBuf::from("old"),
                entries,
                skipped_raw: 0,
                skipped_heic: 0,
                skipped_packages: 0,
                unreadable: Vec::new(),
                walk_errors: Vec::new(),
                existing_output: 0,
                open_single: false,
                format: Format::WebP,
                quality: Quality::lossy(80.),
                max_edge: MaxEdge::FULL,
                grid: false,
                recent_folders: Vec::new(),
                columns: ColumnPrefs::default(),
                output: crate::settings::Output::default(),
                include_subfolders: false,
                sidebar_open: true,
            },
            window,
            cx,
        );
        audit_entity = Some(audit.clone());
        Root::new(audit, window, cx).bg(cx.theme().background)
    });
    let audit = audit_entity.unwrap();
    cx.simulate_resize(size(px(900.), px(640.)));
    cx.run_until_parked();
    let table = audit.read_with(cx, |audit, _| audit.table.clone().unwrap());
    table.update(cx, |table, cx| table.scroll_to_row(90, cx));
    cx.simulate_resize(size(px(900.), px(640.)));
    cx.run_until_parked();
    assert!(table.read_with(cx, |table, _| table.visible_range().rows().start > 0));

    let scanned = scan::Scan {
        entries: (0..120)
            .map(|index| entry(&format!("new-{index}.png"), 1, 1, 1, ImageFormat::Png))
            .collect(),
        skipped_raw: 0,
        skipped_heic: 0,
        skipped_packages: 0,
        unreadable: Vec::new(),
        walk_errors: Vec::new(),
        existing_output: 0,
    };
    audit.update_in(cx, |audit, window, cx| {
        audit.install_dataset(scanned, PathBuf::from("new"), false, None, window, cx);
    });
    cx.run_until_parked();
    cx.simulate_resize(size(px(900.), px(640.)));
    cx.run_until_parked();

    assert_eq!(
        table.read_with(cx, |table, _| table.visible_range().rows().start),
        0
    );
}

/// An audit over a folder of real files, so a conversion has something to decode,
/// encode and write. Opening the folder is what ticks every row.
fn convertible_audit(
    count: usize,
    cx: &mut TestAppContext,
) -> (gpui_kit::Entity<Audit>, &mut gpui_kit::VisualTestContext) {
    cx.update(init_theme);
    let root = scan_fixture("convert");
    let entries: Vec<Entry> = (0..count)
        .map(|index| {
            let path = root.join(format!("shot-{index}.png"));
            crate::convert::tests::photo(8, 8)
                .save(&path)
                .expect("the fixture photo is written");
            let bytes = std::fs::metadata(&path)
                .expect("the fixture image is on disk")
                .len();
            Entry {
                path,
                format: ImageFormat::Png.into(),
                width: 8,
                height: 8,
                bytes,
            }
        })
        .collect();
    let launch = Launch {
        root,
        entries,
        skipped_raw: 0,
        skipped_heic: 0,
        skipped_packages: 0,
        unreadable: Vec::new(),
        walk_errors: Vec::new(),
        existing_output: 0,
        open_single: false,
        format: Format::WebP,
        quality: Quality::lossy(80.),
        max_edge: MaxEdge::FULL,
        grid: false,
        recent_folders: Vec::new(),
        columns: ColumnPrefs::default(),
        output: crate::settings::Output::default(),
        include_subfolders: false,
        sidebar_open: true,
    };
    let mut built = None;
    let (_, cx) = cx.add_window_view(|window, cx| {
        let audit = build_audit(launch, window, cx);
        built = Some(audit.clone());
        Root::new(audit, window, cx).bg(cx.theme().background)
    });
    let audit = built.expect("the audit is built for the production Root");
    audit.update(cx, |audit, _| {
        assert_eq!(audit.selected.len(), count);
        audit.rail = Rail::Convert;
    });
    (audit, cx)
}

/// The whole of replace mode through the window: it converts in place, keeps
/// every original, says so in the rail, and hands them all back on request.
#[gpui_kit::test]
fn replacing_converts_in_place_and_the_rail_offers_the_originals_back(cx: &mut TestAppContext) {
    let (audit, cx) = convertible_audit(3, cx);
    let originals: Vec<(PathBuf, Vec<u8>)> = audit.read_with(cx, |audit, _| {
        audit
            .entries
            .iter()
            .map(|entry| {
                (
                    entry.path.clone(),
                    std::fs::read(&entry.path).expect("the fixture is on disk"),
                )
            })
            .collect()
    });
    assert_eq!(originals.len(), 3);

    audit.update(cx, |audit, cx| audit.use_replace_output(cx));
    cx.update(|window, cx| window.draw(cx).clear(cx));
    audit.read_with(cx, |audit, _| assert_eq!(audit.output, Output::Replace));
    assert!(
        cx.debug_bounds("output-promise").is_some(),
        "the rail says what the destination does before the run"
    );

    audit.update(cx, |audit, cx| audit.start_conversion(cx));
    cx.run_until_parked();

    let (root, backups) = audit.read_with(cx, |audit, _| {
        assert!(audit.failures.is_empty(), "{:?}", audit.failures);
        assert_eq!(audit.results.len(), 3);
        assert_eq!(audit.restorable, 3, "every original can be put back");
        (
            audit.root.clone(),
            crate::manifest::backup_root(&audit.root),
        )
    });
    for (path, bytes) in &originals {
        let name = path.file_name().expect("the fixture is a file");
        assert!(!path.exists(), "{} left its own name", path.display());
        assert!(path.with_extension("webp").is_file());
        assert_eq!(
            &std::fs::read(backups.join(name)).expect("the original moved into the backup"),
            bytes,
            "the original is kept byte for byte"
        );
    }

    // A finished run takes you to the first result, so the rail is behind the
    // comparison until it is closed.
    audit.update(cx, |audit, cx| {
        audit.compare = None;
        cx.notify();
    });
    cx.update(|window, cx| window.draw(cx).clear(cx));
    assert!(
        cx.debug_bounds("restore-originals").is_some(),
        "a finished replace run offers the way back"
    );

    audit.update(cx, |audit, cx| audit.restore_originals(cx));
    cx.run_until_parked();

    for (path, bytes) in &originals {
        assert_eq!(&std::fs::read(path).expect("the original came back"), bytes);
        assert!(
            !path.with_extension("webp").exists(),
            "what replaced it is gone"
        );
    }
    audit.read_with(cx, |audit, _| assert_eq!(audit.restorable, 0));
    assert!(!backups.exists(), "the emptied backup does not linger");
    let _ = std::fs::remove_dir_all(root);
}

#[gpui_kit::test]
fn a_clean_run_toasts_the_same_sentence_as_the_results_bar(cx: &mut TestAppContext) {
    let (audit, cx) = convertible_audit(3, cx);
    audit.update(cx, |audit, cx| audit.start_conversion(cx));
    cx.run_until_parked();
    let summary = audit.read_with(cx, |audit, _| {
        assert!(audit.failures.is_empty(), "{:?}", audit.failures);
        assert_eq!(audit.results.len(), 3);
        audit.conversion_summary()
    });
    cx.update(|window, cx| window.draw(cx).clear(cx));
    assert_eq!(notification_count(cx), 1);
    // The toast carries this exact sentence (one shared helper builds both),
    // so pinning the sentence pins the toast.
    assert!(
        summary.starts_with("Converted 3 images to "),
        "a clean run announces its outcome once: {summary}"
    );
    let root = audit.read_with(cx, |audit, _| audit.root.clone());
    let _ = std::fs::remove_dir_all(root);
}

/// A run that finishes after its folder is gone belongs to the old dataset:
/// its trailing recount must not become the new folder's undo count.
#[gpui_kit::test]
fn a_stale_conversion_does_not_overwrite_restorable(cx: &mut TestAppContext) {
    let (audit, cx) = convertible_audit(3, cx);
    let root = audit.read_with(cx, |audit, _| audit.root.clone());
    audit.update(cx, |audit, cx| audit.use_replace_output(cx));
    audit.update(cx, |audit, cx| audit.start_conversion(cx));
    // A new folder opens while the run is in flight. Its own undo count is
    // already on screen; the in-flight run now belongs to the old dataset.
    audit.update(cx, |audit, _| {
        audit.dataset_generation = audit.dataset_generation.wrapping_add(1);
        audit.restorable = 7;
    });
    cx.run_until_parked();
    audit.read_with(cx, |audit, _| {
        assert_eq!(
            audit.restorable, 7,
            "the stale run moved three originals, but its recount must not land here"
        );
    });
    let _ = std::fs::remove_dir_all(root);
}

#[gpui_kit::test]
fn stopping_a_conversion_keeps_every_file_it_already_wrote(cx: &mut TestAppContext) {
    // Three windows of files, so a stop taken at the first batch of results still
    // leaves a window in flight and a queue that never starts.
    let total = convert::workers(Format::WebP) * 3;
    let (audit, cx) = convertible_audit(total, cx);

    audit.update(cx, |audit, cx| audit.start_conversion(cx));
    // One task at a time, so the stop lands in the middle of a real run rather
    // than before it starts or after it is over.
    while audit.read_with(cx, |audit, _| audit.results.is_empty()) {
        assert!(
            cx.executor().tick(),
            "the conversion reaches its first batch of results"
        );
    }

    cx.update(|window, cx| window.draw(cx).clear(cx));
    assert!(
        cx.debug_bounds("convert-stop").is_some(),
        "a running conversion offers the way out of it"
    );

    audit.update(cx, |audit, cx| {
        assert!(!audit.convert_stopping());
        audit.cancel_conversion(cx);
        assert!(audit.convert_stopping(), "the stop is acknowledged at once");
    });
    cx.run_until_parked();

    audit.read_with(cx, |audit, _| {
        assert!(!audit.converting, "the stop ends the run");
        assert!(audit.convert_cancel.is_none());
        assert!(
            audit.automatic_update_can_restart(),
            "the controls a run owns are handed back"
        );
        assert_eq!(audit.stopped_run, Some(total));
        assert!(
            audit.failures.is_empty(),
            "a file that was never started is not a failure"
        );
        assert!(
            !audit.results.is_empty(),
            "the files the run finished are kept"
        );
        assert!(
            audit.results.len() < total,
            "the stop left the rest of the queue unconverted"
        );
        assert_eq!(audit.result_paths.len(), audit.results.len());
        for written in audit.result_paths.values() {
            assert!(
                written.exists(),
                "{} was written and stays written",
                written.display()
            );
        }
        assert!(
            audit.compare.is_none(),
            "a stop is a request to stop, not to be taken somewhere"
        );
    });
    assert_eq!(
        notification_count(cx),
        0,
        "a stopped run raises no failure notice"
    );
}

#[test]
fn a_stopped_run_says_how_far_it_got_rather_than_how_many_failed() {
    let stopped = panel::conversion_result_state(Some(36), 12);
    assert_eq!(stopped, "STOPPED · 12 OF 36 CONVERTED");
    assert!(!stopped.contains("FAILED"));
    assert_eq!(
        panel::conversion_result_state(None, 36),
        "COMPLETED · ACTUAL RESULT"
    );
}

/// The one decoded sample the estimate is holding on to, with its key. Anything
/// else in there would mean the cache outgrew the sample it was taken for.
fn sampled_decode(audit: &Audit) -> ((u64, PathBuf, MaxEdge), SampledDecode) {
    let cache = audit.estimate_decodes.lock();
    assert_eq!(
        cache.len(),
        1,
        "the estimate holds exactly the sample it just took"
    );
    let (key, image) = cache.iter().next().expect("the sample decoded its image");
    (key.clone(), image.clone())
}

fn settle_estimate(cx: &mut gpui_kit::VisualTestContext) {
    cx.run_until_parked();
    cx.executor()
        .advance_clock(ESTIMATE_DELAY + Duration::from_millis(50));
    cx.run_until_parked();
}

#[gpui_kit::test]
fn a_quality_change_reuses_the_sampled_decodes_and_a_max_edge_change_replaces_them(
    cx: &mut TestAppContext,
) {
    let (audit, cx) = convertible_audit(1, cx);

    audit.update(cx, |audit, cx| audit.schedule_estimate(cx));
    settle_estimate(cx);
    let (key, decoded) = audit.read_with(cx, |audit, _| sampled_decode(audit));
    assert_eq!(decoded.0.width(), 8);
    audit.read_with(cx, |audit, _| {
        assert!(
            audit
                .estimate
                .is_some_and(|(projected, counted)| projected > 0 && counted == 1),
            "the sample projected a real total"
        );
    });

    audit.update(cx, |audit, cx| {
        audit.quality = Quality::lossy(40.);
        audit.schedule_estimate(cx);
    });
    settle_estimate(cx);
    let (unchanged, reused) = audit.read_with(cx, |audit, _| sampled_decode(audit));
    assert_eq!(unchanged, key);
    assert!(
        Arc::ptr_eq(&decoded, &reused),
        "a quality change re-encodes the pixels the last estimate decoded"
    );

    audit.update(cx, |audit, cx| {
        audit.max_edge = MaxEdge(Some(4));
        audit.schedule_estimate(cx);
    });
    settle_estimate(cx);
    let (resized, redecoded) = audit.read_with(cx, |audit, _| sampled_decode(audit));
    assert_eq!(resized.2, MaxEdge(Some(4)));
    assert!(
        !Arc::ptr_eq(&decoded, &redecoded),
        "a max edge change is a different image and has to be decoded again"
    );
    assert_eq!(redecoded.0.width(), 4);
    audit.read_with(cx, |audit, _| {
        assert!(
            audit.estimate.is_some_and(|(projected, _)| projected > 0),
            "the resized sample projected a real total"
        );
    });
}

#[gpui_kit::test]
fn opening_a_folder_ticks_every_row_and_projects_a_saving(cx: &mut TestAppContext) {
    let root = photo_fixture("select-all-open", 3);
    let (audit, cx) = finding_audit(cx);

    audit.update(cx, |audit, cx| audit.request_path(root.clone(), cx));
    settle_estimate(cx);

    audit.read_with(cx, |audit, _| {
        assert_eq!(audit.entries.len(), 3);
        assert_eq!(audit.selected.len(), audit.entries.len());
        assert_eq!(audit.target_count(), 3);
        assert!(
            audit
                .estimate
                .is_some_and(|(projected, counted)| projected > 0 && counted > 0),
            "an untouched folder already projects what a run would write"
        );
        assert_eq!(
            audit.conversion_action_label(),
            "Convert 3 selected to WEBP"
        );
    });
    std::fs::remove_dir_all(root).expect("the fixture folder is removed");
}

#[gpui_kit::test]
fn a_sirv_scope_still_leaves_the_next_folder_ticked(cx: &mut TestAppContext) {
    let root = photo_fixture("select-all-scope", 3);
    let (audit, cx) = finding_audit(cx);
    audit.update(cx, |audit, _| {
        // A remote-only scope hides every local row, and opening a folder retires the
        // pairing that scope belongs to. The ticks have to follow the rows that reset
        // brings back, not the empty list the scope showed on the way through.
        audit.sirv_scope = Some(SirvScope::OnlyRemote);
        audit.refresh_visible();
        assert!(audit.visible.is_empty());
    });

    audit.update(cx, |audit, cx| audit.request_path(root.clone(), cx));
    cx.run_until_parked();

    audit.read_with(cx, |audit, _| {
        assert_eq!(audit.sirv_scope, None);
        assert_eq!(audit.entries.len(), 3);
        assert_eq!(audit.selected.len(), audit.entries.len());
        assert_eq!(audit.target_count(), 3);
    });
    std::fs::remove_dir_all(root).expect("the fixture folder is removed");
}

#[gpui_kit::test]
fn unticking_a_row_narrows_the_conversion_targets(cx: &mut TestAppContext) {
    let root = photo_fixture("select-all-untick", 3);
    let (audit, cx) = finding_audit(cx);

    audit.update(cx, |audit, cx| audit.request_path(root.clone(), cx));
    cx.run_until_parked();
    audit.update(cx, |audit, cx| {
        let dropped = audit.visible[0];
        audit.selected.remove(&dropped);
        audit.selection_changed(cx);
        assert_eq!(audit.target_count(), 2);
        assert_eq!(audit.targets().len(), 2);
        assert!(!audit.targets().contains(&dropped));
    });
    std::fs::remove_dir_all(root).expect("the fixture folder is removed");
}

#[gpui_kit::test]
fn an_empty_folder_still_asks_for_a_selection(cx: &mut TestAppContext) {
    let root = photo_fixture("select-all-empty", 0);
    let (audit, cx) = finding_audit(cx);

    audit.update(cx, |audit, cx| audit.request_path(root.clone(), cx));
    cx.run_until_parked();
    audit.read_with(cx, |audit, _| {
        assert!(audit.entries.is_empty());
        assert!(audit.selected.is_empty());
        assert_eq!(audit.target_count(), 0);
        assert_eq!(audit.conversion_action_label(), "Select images to convert");
    });
    std::fs::remove_dir_all(root).expect("the fixture folder is removed");
}

#[gpui_kit::test]
fn opening_another_folder_replaces_the_selection(cx: &mut TestAppContext) {
    let first = photo_fixture("select-all-first", 3);
    let second = photo_fixture("select-all-second", 2);
    let (audit, cx) = finding_audit(cx);

    audit.update(cx, |audit, cx| audit.request_path(first.clone(), cx));
    cx.run_until_parked();
    audit.read_with(cx, |audit, _| assert_eq!(audit.selected.len(), 3));

    audit.update(cx, |audit, cx| audit.request_path(second.clone(), cx));
    cx.run_until_parked();
    audit.read_with(cx, |audit, _| {
        assert_eq!(audit.entries.len(), 2);
        assert_eq!(
            audit.selected,
            HashSet::from([0, 1]),
            "the new folder's rows are ticked and the old folder's are gone"
        );
        assert_eq!(audit.target_count(), 2);
    });
    std::fs::remove_dir_all(first).expect("the fixture folder is removed");
    std::fs::remove_dir_all(second).expect("the fixture folder is removed");
}

/// Opening a folder starts an estimate, and the next click supersedes it. Without
/// an in-flight check that burst runs to the end, decoding a whole folder nobody is
/// looking at any more, one full-size image per worker.
#[gpui_kit::test]
fn a_superseded_estimate_stops_before_it_decodes_the_rest(cx: &mut TestAppContext) {
    let count = convert::workers(Format::WebP) * 2;
    let (audit, cx) = convertible_audit(count, cx);

    // The test executor runs the whole burst inside one clock advance, and a timer
    // that ties with the estimate's own can wake on either side of it, so the
    // supersession is raised from inside the loop: once the first sample is in.
    super::state::ESTIMATE_HOOK.with(|hook| {
        *hook.borrow_mut() = Some(std::rc::Rc::new(|audit: &mut Audit, completed| {
            if completed >= 1 {
                audit.estimate_generation += 1;
            }
        }));
    });
    cx.executor()
        .advance_clock(ESTIMATE_DELAY + Duration::from_millis(50));
    cx.run_until_parked();
    super::state::ESTIMATE_HOOK.with(|hook| *hook.borrow_mut() = None);

    audit.read_with(cx, |audit, _| {
        assert!(
            audit.estimate.is_none(),
            "a superseded estimate never lands"
        );
        let decoded = audit.estimate_decodes.lock().len();
        assert!(
            decoded > 0 && decoded < count,
            "the stale run stopped after {decoded} of {count} samples"
        );
    });
}

#[gpui_kit::test]
fn a_running_conversion_cannot_have_its_stop_closed_away(cx: &mut TestAppContext) {
    // A real run would end inside the click that simulates the close, so the state
    // a run installs is set directly here.
    let (audit, cx) = pointer_checkbox_audit(false, cx);
    audit.update(cx, |audit, cx| {
        audit.rail = Rail::Convert;
        audit.converting = true;
        audit.active_target_count = Some(2);
        cx.notify();
    });
    cx.update(|window, cx| window.draw(cx).clear(cx));

    assert!(cx.debug_bounds("convert-stop").is_some());
    let close = cx
        .debug_bounds("close-rail")
        .expect("the open rail has its close control");
    cx.simulate_click(close.center(), gpui_kit::Modifiers::none());
    cx.update(|window, cx| window.draw(cx).clear(cx));

    audit.read_with(cx, |audit, _| assert_eq!(audit.rail, Rail::Convert));
    assert!(
        cx.debug_bounds("convert-stop").is_some(),
        "the way out of a run cannot be collapsed away: the collapse control \
         is disabled while it runs"
    );
    audit.update(cx, |audit, _| audit.converting = false);
}

/// A chosen destination is routinely somewhere else entirely — a staging directory, a
/// share, a build tree. Measuring each target against the audited root failed every
/// one of those files and put nothing but their names on the toast.
#[gpui_kit::test]
fn converting_into_an_output_folder_outside_the_root_writes_every_file(cx: &mut TestAppContext) {
    let root = scan_fixture("gui-external-source");
    let outside = scan_fixture("gui-external-output");
    write_png(&root, "one.png");
    write_png(&root, "two.png");
    let (audit, cx) = finding_audit(cx);

    audit.update(cx, |audit, cx| {
        audit.root = root.clone();
        audit.entries = vec![
            entry(
                root.join("one.png").to_str().unwrap(),
                8,
                8,
                256,
                ImageFormat::Png,
            ),
            entry(
                root.join("two.png").to_str().unwrap(),
                8,
                8,
                256,
                ImageFormat::Png,
            ),
        ];
        audit.visible = vec![0, 1];
        audit.selected = HashSet::from([0, 1]);
        audit.set_output(Output::Folder(outside.clone()), cx);
        audit.start_conversion(cx);
    });
    cx.run_until_parked();

    audit.read_with(cx, |audit, _| {
        assert!(audit.failures.is_empty(), "{:?}", audit.failures);
        assert_eq!(audit.results.len(), 2);
    });
    assert!(outside.join("one.webp").is_file());
    assert!(outside.join("two.webp").is_file());
    std::fs::remove_dir_all(root).unwrap();
    std::fs::remove_dir_all(outside).unwrap();
}

/// The folder being audited cannot also be the destination: `a.png` would land on the
/// source `a.webp`, and the run would report the destroyed original as a saving.
#[gpui_kit::test]
fn choosing_the_audited_folder_as_the_output_is_refused(cx: &mut TestAppContext) {
    let root = scan_fixture("output-is-the-source");
    let (audit, cx) = finding_audit(cx);

    audit.update(cx, |audit, cx| {
        audit.root = root.clone();
        audit.set_output(Output::Folder(root.clone()), cx);
    });
    cx.run_until_parked();

    audit.read_with(cx, |audit, _| assert_eq!(audit.output, Output::Optimized));
    assert_eq!(notification_count(cx), 1);
    std::fs::remove_dir_all(root).unwrap();
}

/// Only one file is ticked, and the destination is a subfolder of the same audit. The
/// original sitting there was never in this run's source list, so nothing but the
/// audited set can stop the conversion landing on it.
#[gpui_kit::test]
fn converting_into_a_subfolder_refuses_to_overwrite_an_unselected_original(
    cx: &mut TestAppContext,
) {
    let root = scan_fixture("unselected-original");
    let album = root.join("album");
    std::fs::create_dir(&album).expect("the album fixture folder is created");
    write_png(&root, "x.png");
    let original = album.join("x.webp");
    std::fs::write(&original, b"an audited original").unwrap();
    let (audit, cx) = finding_audit(cx);

    audit.update(cx, |audit, cx| {
        audit.root = root.clone();
        audit.entries = vec![
            entry(
                root.join("x.png").to_str().unwrap(),
                8,
                8,
                256,
                ImageFormat::Png,
            ),
            entry(original.to_str().unwrap(), 8, 8, 19, ImageFormat::WebP),
        ];
        audit.visible = vec![0, 1];
        audit.selected = HashSet::from([0]);
        audit.set_output(Output::Folder(album.clone()), cx);
        audit.start_conversion(cx);
    });
    cx.run_until_parked();

    audit.read_with(cx, |audit, _| {
        assert_eq!(audit.failures.len(), 1);
        let named = audit.failure_names();
        assert!(named[0].contains("x.png"), "{named:?}");
        assert!(audit.results.is_empty());
    });
    assert_eq!(std::fs::read(&original).unwrap(), b"an audited original");
    std::fs::remove_dir_all(root).unwrap();
}

/// Forty failures used to be three names on a toast you could dismiss, and rows that
/// looked exactly like the ones nobody converted. The run keeps its reason per row:
/// the row wears it, the chip collects them, the report names them, and converting
/// the failed rows again is what clears them.
#[gpui_kit::test]
fn a_failed_row_wears_its_reason_and_the_failed_chip_collects_it(cx: &mut TestAppContext) {
    let root = scan_fixture("failure-badge");
    let keeper = root.join("keeper.png");
    crate::convert::tests::photo(8, 8)
        .save(&keeper)
        .expect("the fixture photo is written");
    // A PNG under a `.jpg` name. Keeping its format is refused by name, so one row
    // of this run fails while the row beside it converts.
    let liar = root.join("liar.jpg");
    crate::convert::tests::photo(8, 8)
        .save_with_format(&liar, ImageFormat::Png)
        .expect("the lying fixture is written");
    let (audit, cx) = finding_audit(cx);

    audit.update(cx, |audit, cx| {
        audit.root = root.clone();
        audit.entries = vec![
            entry(keeper.to_str().unwrap(), 8, 8, 256, ImageFormat::Png),
            entry(liar.to_str().unwrap(), 8, 8, 256, ImageFormat::Png),
        ];
        audit.visible = vec![0, 1];
        audit.selected = HashSet::from([0, 1]);
        audit.format = Format::Same;
        audit.start_conversion(cx);
    });
    cx.run_until_parked();

    audit.update(cx, |audit, cx| {
        assert_eq!(audit.results.len(), 1, "the readable file still converted");
        assert_eq!(
            audit.failures.get(&1).map(String::as_str),
            Some("named .jpg but the bytes are PNG; convert it explicitly"),
            "{:?}",
            audit.failures
        );
        assert_eq!(
            audit.failure_names(),
            vec!["liar.jpg (named .jpg but the bytes are PNG; convert it explicitly)".to_string()]
        );
        // A finished run opens its first result; the list is what this asserts on.
        audit.compare = None;
        cx.notify();
    });
    cx.update(|window, cx| window.draw(cx).clear(cx));

    assert!(
        cx.debug_bounds("failed-1").is_some(),
        "the failed row is marked where its result would have been"
    );
    assert!(
        cx.debug_bounds("failed-0").is_none(),
        "the row that converted is not marked"
    );
    assert!(
        cx.debug_bounds("finding-failed").is_some(),
        "the failures have a chip to reach them by"
    );

    audit.update(cx, |audit, cx| {
        audit.copy_audit_report(cx);
        assert!(audit.report_copied);
    });
    let report = cx
        .read_from_clipboard()
        .and_then(|clipboard| clipboard.text())
        .expect("the report reaches the clipboard");
    assert!(
        report.contains("Could not convert (1):")
            && report
                .contains("- liar.jpg (named .jpg but the bytes are PNG; convert it explicitly)"),
        "{report}"
    );

    audit.update(cx, |audit, cx| {
        audit.set_finding(Finding::Failed, cx);
        assert_eq!(
            audit.visible,
            vec![1],
            "the chip narrows the list to the failed row"
        );
    });

    // Fix the cause, then convert exactly what the chip is showing.
    crate::convert::tests::photo(8, 8)
        .save_with_format(&liar, ImageFormat::Jpeg)
        .expect("the file becomes the JPEG its name claims");
    audit.update(cx, |audit, cx| {
        audit.selected = HashSet::from([1]);
        audit.start_conversion(cx);
    });
    cx.run_until_parked();

    audit.update(cx, |audit, cx| {
        assert!(audit.failures.is_empty(), "{:?}", audit.failures);
        assert!(
            audit.results.contains_key(&1),
            "the retry converted the row that had failed"
        );
        assert_eq!(audit.finding, None, "the filter goes with its chip");
        assert_eq!(audit.visible.len(), 2, "and the list widens back out");
        audit.compare = None;
        cx.notify();
    });
    cx.update(|window, cx| window.draw(cx).clear(cx));
    assert!(
        cx.debug_bounds("failed-1").is_none(),
        "a row that converted carries no failure"
    );
    assert!(cx.debug_bounds("finding-failed").is_none());
    std::fs::remove_dir_all(root).unwrap();
}

/// The same run seen from the gallery: a tile has no result column, so the badge has
/// to be in the tile itself or the grid is the view that hides the failures.
#[gpui_kit::test]
fn a_failed_tile_carries_the_badge_the_list_row_does(cx: &mut TestAppContext) {
    let root = scan_fixture("failure-badge-grid");
    let liar = root.join("liar.jpg");
    crate::convert::tests::photo(8, 8)
        .save_with_format(&liar, ImageFormat::Png)
        .expect("the lying fixture is written");
    let (audit, cx) = finding_audit(cx);

    audit.update(cx, |audit, cx| {
        audit.root = root.clone();
        audit.entries = vec![entry(liar.to_str().unwrap(), 8, 8, 256, ImageFormat::Png)];
        audit.visible = vec![0];
        audit.selected = HashSet::from([0]);
        audit.format = Format::Same;
        audit.grid = true;
        audit.start_conversion(cx);
    });
    cx.run_until_parked();

    audit.read_with(cx, |audit, _| {
        assert_eq!(
            audit.failures.len(),
            1,
            "the run failed the only file it had"
        );
        assert!(audit.results.is_empty());
    });
    cx.update(|window, cx| window.draw(cx).clear(cx));
    assert!(
        cx.debug_bounds("failed-0").is_some(),
        "the tile says the file failed"
    );

    // The reason belongs to the settings that produced it. Aimed somewhere else,
    // the badge would be claiming a refusal this folder has not been given.
    audit.update(cx, |audit, cx| {
        audit.set_finding(Finding::Failed, cx);
        audit.apply_format(Format::WebP, cx);
        assert!(audit.failures.is_empty(), "{:?}", audit.failures);
        assert_eq!(audit.finding, None, "the filter goes with its chip");
        assert_eq!(audit.visible, vec![0]);
    });
    cx.update(|window, cx| window.draw(cx).clear(cx));
    assert!(cx.debug_bounds("failed-0").is_none());
    std::fs::remove_dir_all(root).unwrap();
}

/// The result column is only laid out when there is something to put in it, and a
/// run where every file failed has exactly one thing: the badge saying so.
#[gpui_kit::test]
fn a_run_that_converted_nothing_still_lays_out_the_column_holding_its_failures(
    cx: &mut TestAppContext,
) {
    let root = scan_fixture("failure-badge-only");
    let liar = root.join("liar.jpg");
    crate::convert::tests::photo(8, 8)
        .save_with_format(&liar, ImageFormat::Png)
        .expect("the lying fixture is written");
    let (audit, cx) = finding_audit(cx);

    audit.update(cx, |audit, cx| {
        audit.root = root.clone();
        audit.entries = vec![entry(liar.to_str().unwrap(), 8, 8, 256, ImageFormat::Png)];
        audit.visible = vec![0];
        audit.selected = HashSet::from([0]);
        audit.format = Format::Same;
        audit.start_conversion(cx);
    });
    cx.run_until_parked();

    audit.read_with(cx, |audit, _| {
        assert!(audit.results.is_empty(), "nothing landed");
        assert_eq!(audit.failures.len(), 1);
        assert!(
            audit.failure_summary.contains("liar.jpg"),
            "{}",
            audit.failure_summary
        );
    });
    cx.update(|window, cx| window.draw(cx).clear(cx));
    cx.run_until_parked();
    cx.update(|window, cx| window.draw(cx).clear(cx));
    assert!(
        cx.debug_bounds("failed-0").is_some(),
        "the list still shows the row its run failed"
    );
    std::fs::remove_dir_all(root).unwrap();
}

/// The sidebar opens on Convert, retabs on a verb, collapses on request, and
/// the resize-only preset lights as one of the named rows.
#[gpui_kit::test]
fn the_sidebar_tabs_switch_operations_and_collapse(cx: &mut TestAppContext) {
    let (audit, cx) = finding_audit(cx);
    audit.read_with(cx, |audit, _| {
        assert!(audit.sidebar_open, "the operations sidebar starts open");
        assert_eq!(audit.active_tab(), Rail::Convert);
    });
    cx.update(|window, cx| window.draw(cx).clear(cx));
    assert!(cx.debug_bounds("rail").is_some());

    audit.update(cx, |audit, cx| audit.open_rail(Rail::RemoveBackground, cx));
    audit.read_with(cx, |audit, _| {
        assert_eq!(audit.active_tab(), Rail::RemoveBackground)
    });

    audit.update(cx, |audit, cx| {
        audit.sidebar_open = false;
        cx.notify();
    });
    cx.update(|window, cx| window.draw(cx).clear(cx));
    assert!(
        cx.debug_bounds("rail").is_none(),
        "a collapsed sidebar leaves the window"
    );
    // A verb reopens the sidebar on its own tab.
    audit.update(cx, |audit, cx| audit.open_rail(Rail::Studio, cx));
    audit.read_with(cx, |audit, _| {
        assert!(audit.sidebar_open);
        assert_eq!(audit.active_tab(), Rail::Studio);
    });

    audit.update(cx, |audit, cx| {
        audit.format = Format::Same;
        audit.quality = Quality::lossy(80.);
        audit.max_edge = MaxEdge(Some(2400));
        cx.notify();
    });
    audit.read_with(cx, |audit, _| {
        assert_eq!(
            panel::active_preset(audit.format, audit.quality, audit.max_edge),
            Some(3),
            "keeping the container at a smaller edge is a named preset"
        );
    });
}

/// The new chrome states its facts in strings the tests can read: the output
/// plan in the status bar, the savings beside the selection, and the findings
/// behind the status-bar menu.
#[gpui_kit::test]
fn the_new_header_and_bar_facts_agree(cx: &mut TestAppContext) {
    let (audit, cx) = finding_audit(cx);
    audit.update(cx, |audit, _| {
        audit.format = Format::WebP;
        audit.quality = Quality::lossy(80.);
        audit.max_edge = MaxEdge::FULL;
        assert_eq!(audit.output_plan(), "WEBP q80 → optimized/");
        audit.max_edge = MaxEdge(Some(2400));
        assert_eq!(audit.output_plan(), "WEBP q80 · 2400px → optimized/");
        audit.quality = Quality::LOSSLESS;
        audit.max_edge = MaxEdge::FULL;
        assert_eq!(audit.output_plan(), "WEBP lossless → optimized/");

        audit.entries = vec![
            entry("a.png", 8, 8, 1000, ImageFormat::Png),
            entry("b.png", 8, 8, 1000, ImageFormat::Png),
        ];
        audit.visible = vec![0, 1];
        audit.selected = HashSet::from([0, 1]);
        audit.selected_target_count = 2;
        audit.selected_target_bytes = 2000;
        audit.estimate = Some((160, 2));
        assert_eq!(
            audit.savings_note(),
            Some("· ≈160 B output, 92% saved".to_string())
        );
        audit.estimate = Some((2200, 2));
        assert_eq!(
            audit.savings_note(),
            Some("· ≈2.1 KB output, 10% larger".to_string())
        );
        audit.estimate = None;
        assert_eq!(audit.savings_note(), None);

        audit.heavy = 0;
        audit.mislabelled = 3;
        let findings = audit.available_findings();
        assert_eq!(findings.len(), 1);
        assert_eq!(findings[0].0, Finding::Mislabelled);
        assert_eq!(findings[0].2, "3 mislabelled");
    });
    cx.update(|window, cx| window.draw(cx).clear(cx));
    assert!(cx.debug_bounds("status-bar").is_some());
    assert!(cx.debug_bounds("audit-header").is_some());
}
#[gpui_kit::test]
fn the_status_bar_names_folders_and_images(cx: &mut TestAppContext) {
    let (audit, cx) = finding_audit(cx);
    audit.update(cx, |audit, _| {
        audit.batch_folders = Some(2);
        audit.entries = vec![
            entry("a.png", 8, 8, 256, ImageFormat::Png),
            entry("b.png", 8, 8, 256, ImageFormat::Png),
        ];
        audit.visible = vec![0, 1];
        audit.visible_bytes = 512;
        assert_eq!(audit.status_line(2), "2 folders, 2 images · 512 B");
        audit.batch_folders = None;
        audit.folders = Vec::new();
        audit.browser_output_root = PathBuf::from("/nowhere");
        assert_eq!(audit.status_line(2), "2 images · 512 B");
        audit.visible = vec![0];
        audit.visible_bytes = 256;
        assert_eq!(audit.status_line(1), "1 of 2 images · 256 B");
    });
    cx.update(|window, cx| window.draw(cx).clear(cx));
    assert!(
        cx.debug_bounds("status-bar").is_some(),
        "the totals stay pinned to the window foot"
    );
}

#[gpui_kit::test]
fn installing_a_clean_dataset_toasts_its_totals(cx: &mut TestAppContext) {
    let (audit, cx) = notification_audit(cx, Vec::new());
    audit.update_in(cx, |audit, window, cx| {
        audit.install_dataset(
            scan::Scan {
                entries: vec![
                    entry("a.png", 8, 8, 256, ImageFormat::Png),
                    entry("b.png", 8, 8, 256, ImageFormat::Png),
                ],
                skipped_raw: 0,
                skipped_heic: 0,
                skipped_packages: 0,
                unreadable: Vec::new(),
                walk_errors: Vec::new(),
                existing_output: 0,
            },
            PathBuf::from("/photos"),
            false,
            None,
            window,
            cx,
        );
    });
    cx.run_until_parked();
    assert_eq!(notification_count(cx), 1);
    assert!(
        cx.debug_bounds("error-toast-message:2 images · 512 B")
            .is_some(),
        "a clean folder announces its counts once"
    );
}

/// The header keeps one persistent Sirv entry outside the Open menu, so
/// pairing is discoverable before the reconciliation strip exists. Unpaired
/// it opens the browser; while converting it is disabled like the menu item.
#[gpui_kit::test]
fn the_unpaired_header_sirv_button_opens_the_browser(cx: &mut TestAppContext) {
    let (audit, cx) = finding_audit(cx);
    audit.read_with(cx, |audit, _| assert!(audit.sirv_pairing.is_none()));
    cx.update(|window, cx| window.draw(cx).clear(cx));
    let button = cx
        .debug_bounds("sirv-pair-header")
        .expect("the unpaired header names its Sirv control");
    cx.simulate_click(button.center(), gpui_kit::Modifiers::none());
    cx.run_until_parked();
    audit.read_with(cx, |audit, _| {
        assert!(
            audit.sirv_browser.is_some(),
            "the enabled header button reaches the Sirv browser"
        )
    });

    // Disabled while converting: a click lands nowhere, the same rule the
    // Open menu's Pair item follows.
    audit.update(cx, |audit, cx| {
        audit.sirv_browser = None;
        audit.converting = true;
        cx.notify();
    });
    cx.update(|window, cx| window.draw(cx).clear(cx));
    assert!(
        cx.debug_bounds("sirv-pair-header").is_some(),
        "a disabled control stays visible rather than vanishing"
    );
    let button = cx
        .debug_bounds("sirv-pair-header")
        .expect("the disabled header control keeps its name");
    cx.simulate_click(button.center(), gpui_kit::Modifiers::none());
    cx.run_until_parked();
    audit.read_with(cx, |audit, _| {
        assert!(
            audit.sirv_browser.is_none(),
            "the converting header button does not open the browser"
        )
    });
}

/// Paired, the same header button stays put and reads selected, so the
/// paired state is visible above the reconciliation strip.
#[gpui_kit::test]
fn the_paired_header_sirv_button_stays_visible(cx: &mut TestAppContext) {
    let (audit, cx) = finding_audit(cx);
    audit.update(cx, |audit, cx| {
        audit.sirv_pairing = Some(test_pairing());
        cx.notify();
    });
    cx.update(|window, cx| window.draw(cx).clear(cx));
    assert!(
        cx.debug_bounds("sirv-pair-header").is_some(),
        "the paired header keeps its Sirv control"
    );
    // Selected follows the pairing: the button renders selected exactly
    // when this is set, so asserting the driver asserts the state.
    audit.read_with(cx, |audit, _| {
        assert!(
            audit.sirv_pairing.is_some(),
            "the paired header button reads selected"
        )
    });
}

fn sirv_folder(filename: &str) -> sirv::Node {
    sirv::Node {
        filename: filename.into(),
        size: 0,
        is_directory: true,
        kind: None,
    }
}

fn sirv_browser_audit(
    audit: &gpui_kit::Entity<Audit>,
    nodes: Option<Result<Vec<sirv::Node>, String>>,
    cx: &mut gpui_kit::VisualTestContext,
) {
    audit.update(cx, |audit, cx| {
        audit.sirv_browser = Some(SirvBrowser {
            client: Arc::new(parking_lot::Mutex::new(sirv::Client::new(
                sirv::Credentials {
                    client_id: String::new(),
                    client_secret: String::new(),
                },
            ))),
            path: "/photos".into(),
            needs_credentials: false,
            nodes,
            generation: 0,
            session: 1,
            focused: false,
            focus: cx.focus_handle(),
        });
        cx.notify();
    });
}

#[gpui_kit::test]
fn sirv_browser_filter_shows_only_matching_folders(cx: &mut TestAppContext) {
    let (audit, cx) = finding_audit(cx);
    sirv_browser_audit(
        &audit,
        Some(Ok(vec![
            sirv_folder("/photos/alpha"),
            sirv_folder("/photos/beta"),
            sirv_folder("/photos/alpine"),
        ])),
        cx,
    );
    audit.update(cx, |audit, cx| {
        // The box edits the string the rows read; typing never lists again.
        audit.sirv_browser_filter = "alp".into();
        cx.notify();
    });
    cx.update(|window, cx| window.draw(cx).clear(cx));
    assert!(cx.debug_bounds("sirv-filter").is_some());
    assert!(cx.debug_bounds("sirv-dir-0").is_some());
    assert!(cx.debug_bounds("sirv-dir-1").is_some());
    assert!(
        cx.debug_bounds("sirv-dir-2").is_none(),
        "beta is filtered out of the loaded listing, which is never re-listed"
    );
    audit.read_with(cx, |audit, _| {
        let nodes = audit
            .sirv_browser
            .as_ref()
            .and_then(|browser| browser.nodes.as_ref())
            .expect("the listing stays loaded under the filter");
        assert_eq!(
            nodes.as_ref().expect("the listing is still ready").len(),
            3,
            "filtering hides rows without touching the loaded listing"
        );
    });
}

#[gpui_kit::test]
fn sirv_browser_error_offers_a_retry(cx: &mut TestAppContext) {
    let (audit, cx) = finding_audit(cx);
    sirv_browser_audit(&audit, Some(Err("Sirv said 500: boom".into())), cx);
    cx.update(|window, cx| window.draw(cx).clear(cx));
    assert!(
        cx.debug_bounds("sirv-error").is_some(),
        "the message stays on screen next to the retry"
    );
    let retry = cx
        .debug_bounds("sirv-retry")
        .expect("a failed listing offers a retry");
    let generation = audit.read_with(cx, |audit, _| {
        audit
            .sirv_browser
            .as_ref()
            .map(|browser| browser.generation)
    });
    cx.simulate_click(retry.center(), gpui_kit::Modifiers::none());
    audit.read_with(cx, |audit, _| {
        let browser = audit.sirv_browser.as_ref().expect("the browser stays open");
        assert_eq!(
            Some(browser.generation),
            generation.map(|generation| generation.wrapping_add(1)),
            "retry re-runs the current path listing"
        );
        assert_eq!(browser.path, "/photos");
    });
}

#[gpui_kit::test]
fn sirv_filter_box_owns_its_keys(cx: &mut TestAppContext) {
    let (audit, cx) = finding_audit(cx);
    sirv_browser_audit(
        &audit,
        Some(Ok(vec![
            sirv_folder("/photos/alpha"),
            sirv_folder("/photos/beta"),
        ])),
        cx,
    );
    audit.update(cx, |audit, cx| {
        audit.selected.clear();
        audit.selected.insert(0);
        audit.selection_changed(cx);
        // A live browser has taken its one-time focus already; otherwise the
        // view's deferred focus would yank the focus below back out of the box.
        audit
            .sirv_browser
            .as_mut()
            .expect("the browser is open")
            .focused = true;
    });
    cx.update(|window, cx| window.draw(cx).clear(cx));

    // Click to focus, the way a user reaches the box. The click itself is a
    // separate path (the scrim does not occlude, so it also reaches the list
    // marquee underneath — reported separately), so the selection is reset
    // after it and this test owns the key path only.
    let filter = cx
        .debug_bounds("sirv-filter")
        .expect("the browser filter box is visible");
    cx.simulate_click(filter.center(), gpui_kit::Modifiers::none());
    audit.update_in(cx, |audit, window, cx| {
        assert!(
            audit
                .sirv_browser_filter_input
                .read(cx)
                .focus_handle(cx)
                .is_focused(window),
            "the click lands in the filter box"
        );
        audit.selected.clear();
        audit.selected.insert(0);
        audit.selection_changed(cx);
    });
    // Typed text proves the box itself still receives keys.
    cx.simulate_input("alp");
    // A bare space is a list shortcut with no text, so it proves the shield:
    // at the root it would toggle the row behind the modal.
    cx.simulate_keystrokes("space");

    audit.read_with(cx, |audit, cx| {
        assert_eq!(
            audit.selected,
            HashSet::from([0]),
            "a space typed in the filter box never reaches the list behind the modal"
        );
        assert_eq!(
            audit.sirv_browser_filter_input.read(cx).value(),
            "alp",
            "the filter box kept the keys it swallowed"
        );
    });
}

#[gpui_kit::test]
fn the_status_bar_carries_findings_under_one_icon(cx: &mut TestAppContext) {
    let (audit, cx) = finding_audit(cx);
    audit.read_with(cx, |audit, _| {
        assert!(audit.heavy > 0 && audit.mislabelled > 0);
    });
    cx.update(|window, cx| window.draw(cx).clear(cx));
    // One icon, not a chip per finding; nothing failed, so no failure wrapper.
    assert!(cx.debug_bounds("findings-menu").is_some());
    assert!(cx.debug_bounds("finding-failed").is_none());
    assert!(cx.debug_bounds("status-bar").is_some());
    // Narrowing through the menu's contract: the icon lights while one is in force.
    audit.update(cx, |audit, cx| audit.set_finding(Finding::Heavy, cx));
    cx.update(|window, cx| window.draw(cx).clear(cx));
    assert!(cx.debug_bounds("findings-menu").is_some());
}

#[gpui_kit::test]
fn a_finished_run_offers_publish_from_the_results_block(cx: &mut TestAppContext) {
    let (audit, cx) = finding_audit(cx);
    audit.update(cx, |audit, cx| {
        audit.record_result(0, Format::WebP, 500, PathBuf::from("/tmp/out.webp"));
        cx.notify();
    });
    cx.update(|window, cx| window.draw(cx).clear(cx));
    assert!(cx.debug_bounds("conversion-publish").is_some());
    audit.update(cx, |audit, cx| {
        audit.published_results = vec!["https://demo.sirv.com/a.webp".into()];
        cx.notify();
    });
    cx.update(|window, cx| window.draw(cx).clear(cx));
    assert!(cx.debug_bounds("conversion-copy-embed").is_some());
    assert!(cx.debug_bounds("conversion-publish").is_none());
}
