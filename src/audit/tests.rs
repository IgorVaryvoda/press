use super::local_ai_actions::local_ai_landing_applies;
use super::media::comparison_landing_applies;
use super::sirv_actions::{browser_landing_applies, remember_failure, walk_landing_applies};
use super::studio_actions::studio_landing_applies;
use super::*;
use crate::{
    Launch, WINDOW_DEFAULT_HEIGHT, WINDOW_DEFAULT_WIDTH, WINDOW_MIN_HEIGHT, WINDOW_MIN_WIDTH,
    init_theme, restored_window_size,
};
use gpui::{HeadlessAppContext, TestAppContext, size};
use gpui_component::Root;
use image::ImageFormat;
use std::path::PathBuf;

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
        folder.clone()
    };

    // A real platform, only for its text system: glyph metrics decide every
    // width in the window, so a fake one would measure a different layout.
    let text_system = gpui_platform::current_platform(true).text_system();
    let mut cx = HeadlessAppContext::with_platform(
        text_system,
        std::sync::Arc::new(gpui_component_assets::Assets),
        gpui_platform::current_headless_renderer,
    );

    cx.update(init_theme);

    let window = cx
        .open_window(size(px(1100.), px(720.)), |window, cx| {
            let audit = build_audit(
                Launch {
                    root: root.clone(),
                    entries: scanned.entries,
                    skipped_raw: scanned.skipped_raw,
                    skipped_packages: scanned.skipped_packages,
                    unreadable: scanned.unreadable,
                    walk_errors: scanned.walk_errors,
                    existing_output: scanned.existing_output,
                    open_single: mode == "compare",
                    format: Format::WebP,
                    quality: Quality::lossy(80.),
                    max_edge: MaxEdge::FULL,
                    grid: mode == "grid",
                    columns: ColumnPrefs::default(),
                    output: crate::settings::Output::default(),
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
    std::thread::sleep(Duration::from_millis(1200));
    cx.run_until_parked();

    let image = cx
        .capture_screenshot(window.into())
        .expect("frame renders to an image");
    image.save(&out).expect("png writes");
    println!("wrote {out} ({}x{})", image.width(), image.height());
}
struct AuditHarness {
    audit: gpui::Entity<Audit>,
}

impl gpui::Render for AuditHarness {
    fn render(&mut self, _window: &mut Window, _cx: &mut Context<Self>) -> impl IntoElement {
        self.audit.clone()
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
    assert_eq!(conversion_targets(&visible, &HashSet::new()), vec![2, 0, 1]);

    let selected = HashSet::from([0, 3]);
    assert_eq!(conversion_targets(&visible, &selected), vec![0]);

    let hidden = HashSet::from([3]);
    assert!(conversion_targets(&visible, &hidden).is_empty());
}

#[gpui::test]
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
    assert_eq!(bounds.origin, gpui::point(px(20.), px(30.)));
    assert_eq!(bounds.size, gpui::size(px(70.), px(50.)));
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

#[gpui::test]
fn table_select_all_follows_the_visible_rows(cx: &mut TestAppContext) {
    let (audit, cx) = finding_audit(cx);
    audit.update(cx, |audit, cx| {
        audit.visible = vec![2, 0];
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
    entries.sort_by(|a, b| compare_entries(a, b, sort));
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
#[gpui::test]
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
#[gpui::test]
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

#[gpui::test]
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
}

#[gpui::test]
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

#[gpui::test]
fn an_armed_overwrite_is_withdrawn_by_unpair(cx: &mut TestAppContext) {
    let (audit, cx) = finding_audit(cx);

    audit.update(cx, |audit, cx| {
        audit.sirv_confirm = Some(SirvJobKind::PushChanged);
        audit.unpair_sirv(cx);
        assert!(audit.sirv_confirm.is_none());
    });
}

#[gpui::test]
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

#[gpui::test]
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
#[gpui::test]
fn opening_another_folder_clears_the_finding(cx: &mut TestAppContext) {
    let (audit, cx) = finding_audit(cx);
    audit.update(cx, |audit, cx| audit.set_finding(Finding::Heavy, cx));

    cx.update(|window, cx| {
        audit.update(cx, |audit, cx| {
            audit.install_dataset(
                scan::Scan {
                    entries: vec![entry("new.png", 10, 10, 100, ImageFormat::Png)],
                    skipped_raw: 0,
                    skipped_packages: 0,
                    unreadable: Vec::new(),
                    walk_errors: Vec::new(),
                    existing_output: 0,
                },
                PathBuf::from("/elsewhere"),
                false,
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

#[gpui::test]
fn a_progressive_scan_appends_rows_without_replacing_the_dataset(cx: &mut TestAppContext) {
    let (audit, cx) = finding_audit(cx);
    let generation = audit.read_with(cx, |audit, _| audit.dataset_generation);

    audit.update(cx, |audit, cx| {
        audit.scanning = Some("more-images".into());
        audit.scan_partial = true;
        audit.schedule_estimate(cx);
        audit.append_scan_batch(
            vec![entry("new-heavy.png", 100, 100, 400_000, ImageFormat::Png)],
            cx,
        );
    });

    audit.read_with(cx, |audit, _| {
        assert_eq!(audit.dataset_generation, generation);
        assert_eq!(audit.entries.len(), 4);
        assert_eq!(audit.visible.len(), 4);
        assert_eq!(audit.entries[audit.visible[0]].name(), "new-heavy.png");
        assert!(audit.estimate.is_none());
    });
}

#[gpui::test]
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
                    unreadable: Vec::new(),
                    walk_errors: Vec::new(),
                    existing_output: 0,
                    skipped_packages: 0,
                },
                PathBuf::from("/elsewhere"),
                false,
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

fn finding_audit(cx: &mut TestAppContext) -> (gpui::Entity<Audit>, &mut gpui::VisualTestContext) {
    cx.update(init_theme);
    // A PNG named `.webp` is the mislabelled one. The screenshot is 30 bytes per
    // pixel; the photo is a tenth of one.
    let launch = Launch {
        root: PathBuf::new(),
        entries: vec![
            entry("photo.jpg", 1000, 1000, 100_000, ImageFormat::Jpeg),
            entry("screenshot.png", 100, 100, 300_000, ImageFormat::Png),
            entry("liar.webp", 100, 100, 1_000, ImageFormat::Png),
        ],
        skipped_raw: 0,
        skipped_packages: 0,
        unreadable: Vec::new(),
        walk_errors: Vec::new(),
        existing_output: 0,
        open_single: false,
        format: Format::WebP,
        quality: Quality::lossy(80.),
        max_edge: MaxEdge::FULL,
        grid: false,
        columns: ColumnPrefs::default(),
        output: crate::settings::Output::default(),
    };
    let (harness, cx) = cx.add_window_view(move |window, cx| AuditHarness {
        audit: build_audit(launch, window, cx),
    });
    let audit = harness.read_with(cx, |harness, _| harness.audit.clone());
    (audit, cx)
}

#[gpui::test]
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

#[gpui::test]
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

#[gpui::test]
fn render_totals_change_with_selection_and_results(cx: &mut TestAppContext) {
    let (audit, cx) = finding_audit(cx);
    audit.update(cx, |audit, _| {
        assert_eq!(audit.heavy, 1);
        assert_eq!(audit.mislabelled, 1);
        assert_eq!(audit.target_count(), 3);
        assert_eq!(audit.conversion_action_label(), "Convert all 3 to WEBP");
        assert_eq!(audit.target_bytes(), 401_000);

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
        assert_eq!(
            audit.conversion_action_label(),
            "Replace 2 selected WEBP outputs"
        );
        audit.record_result(0, Format::WebP, 40_000, PathBuf::from("/tmp/out.webp"));
        assert_eq!(audit.converted_totals(), (101_000, 40_500));
        audit.clear_results();
        assert_eq!(audit.converted_totals(), (0, 0));
        assert_eq!(
            audit.conversion_action_label(),
            "Replace 2 selected WEBP outputs",
            "changing settings clears old measurements, not known output files"
        );
    });
}

#[gpui::test]
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

#[gpui::test]
fn named_filter_clear_restores_the_audit(cx: &mut TestAppContext) {
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

    let clear = cx
        .debug_bounds("clear-filter")
        .expect("a populated filter has a named clear action");
    cx.simulate_click(clear.center(), gpui::Modifiers::none());

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

#[gpui::test]
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

#[gpui::test]
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
    assert_eq!(thumb_overscan_rows(20..30, 100), 0..70);
    assert_eq!(thumb_overscan_rows(0..10, 100), 0..50);
    assert_eq!(thumb_overscan_rows(90..100, 100), 50..100);
}

#[gpui::test]
fn thumbnail_decodes_use_independent_native_and_fallback_slots(cx: &mut TestAppContext) {
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
            });
        }
        audit.start_thumb_jobs(cx);
        assert_eq!(audit.thumb_inflight, THUMB_SLOW_WORKERS + THUMB_WORKERS);
        assert_eq!(audit.thumb_slow_inflight, THUMB_SLOW_WORKERS);
        assert!(audit.thumb_queue.is_empty());
    });
    cx.run_until_parked();
}

#[gpui::test]
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

#[gpui::test]
fn flushing_settings_clears_the_pending_debounce(cx: &mut TestAppContext) {
    let (audit, cx) = finding_audit(cx);
    audit.update(cx, |audit, cx| {
        let mut settings = audit.settings.clone();
        settings.width = Some(1280.);
        audit.remember_settings(settings, cx);
        assert!(audit.settings_save_pending);

        audit.flush_settings();
        assert!(!audit.settings_save_pending);
    });
}

fn pointer_checkbox_audit(
    grid: bool,
    cx: &mut TestAppContext,
) -> (gpui::Entity<Audit>, &mut gpui::VisualTestContext) {
    cx.update(init_theme);
    let launch = Launch {
        root: PathBuf::new(),
        entries: vec![
            entry("first.png", 10, 10, 100, ImageFormat::Png),
            entry("second.png", 10, 10, 200, ImageFormat::Png),
        ],
        skipped_raw: 0,
        skipped_packages: 0,
        unreadable: Vec::new(),
        walk_errors: Vec::new(),
        existing_output: 0,
        open_single: false,
        format: Format::WebP,
        quality: Quality::lossy(80.),
        max_edge: MaxEdge::FULL,
        grid,
        columns: ColumnPrefs::default(),
        output: crate::settings::Output::default(),
    };
    let (harness, cx) = cx.add_window_view(move |window, cx| {
        let built = build_audit(launch, window, cx);
        AuditHarness { audit: built }
    });
    let audit = harness.read_with(cx, |harness, _| harness.audit.clone());
    audit.update(cx, |audit, _| {
        audit.selected.extend([0, 1]);
        audit.refresh_target_summary();
        audit.estimate = Some((123, 2));
    });
    cx.update(|window, cx| window.draw(cx).clear(cx));
    (audit, cx)
}

#[gpui::test]
fn gallery_exposes_sorting_and_a_separate_compare_action(cx: &mut TestAppContext) {
    let (audit, cx) = pointer_checkbox_audit(true, cx);
    assert!(cx.debug_bounds("gallery-sort").is_some());

    let compare = cx
        .debug_bounds("grid-compare-0")
        .expect("each gallery image has a named comparison action");
    cx.simulate_click(compare.center(), gpui::Modifiers::none());

    audit.read_with(cx, |audit, _| {
        assert_eq!(
            audit.compare.as_ref().map(|comparison| comparison.index),
            Some(0)
        );
        assert_eq!(audit.selected, [0, 1].into_iter().collect());
    });
}

#[gpui::test]
fn double_click_opens_a_source_preview_before_comparison(cx: &mut TestAppContext) {
    let (audit, cx) = finding_audit(cx);
    let click = gpui::ClickEvent::Mouse(gpui::MouseClickEvent {
        down: gpui::MouseDownEvent {
            button: gpui::MouseButton::Left,
            click_count: 2,
            ..Default::default()
        },
        up: gpui::MouseUpEvent {
            button: gpui::MouseButton::Left,
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

#[gpui::test]
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

#[gpui::test]
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

#[gpui::test]
fn marquee_selects_intersecting_visible_items(cx: &mut TestAppContext) {
    let (audit, cx) = finding_audit(cx);
    audit.update(cx, |audit, cx| {
        audit.selection_surface.set(gpui::Bounds::from_corners(
            gpui::point(px(0.), px(0.)),
            gpui::point(px(300.), px(300.)),
        ));
        audit.selection_bounds.borrow_mut().extend([
            (
                0,
                gpui::Bounds::from_corners(
                    gpui::point(px(70.), px(70.)),
                    gpui::point(px(100.), px(100.)),
                ),
            ),
            (
                1,
                gpui::Bounds::from_corners(
                    gpui::point(px(200.), px(200.)),
                    gpui::point(px(230.), px(230.)),
                ),
            ),
        ]);
        audit.start_marquee(
            &gpui::MouseDownEvent {
                button: gpui::MouseButton::Left,
                position: gpui::point(px(50.), px(50.)),
                ..Default::default()
            },
            cx,
        );
        assert!(audit.marquee.is_none(), "the table header owns its drags");
        audit.start_marquee(
            &gpui::MouseDownEvent {
                button: gpui::MouseButton::Left,
                position: gpui::point(px(50.), px(120.)),
                ..Default::default()
            },
            cx,
        );
        audit.move_marquee(
            &gpui::MouseMoveEvent {
                position: gpui::point(px(120.), px(50.)),
                pressed_button: Some(gpui::MouseButton::Left),
                ..Default::default()
            },
            cx,
        );

        assert_eq!(audit.selected, HashSet::from([0]));
        audit.finish_marquee(cx);
        assert!(audit.marquee.is_none());

        audit.start_marquee(
            &gpui::MouseDownEvent {
                button: gpui::MouseButton::Left,
                position: gpui::point(px(50.), px(120.)),
                modifiers: gpui::Modifiers {
                    control: true,
                    ..Default::default()
                },
                ..Default::default()
            },
            cx,
        );
        audit.move_marquee(
            &gpui::MouseMoveEvent {
                position: gpui::point(px(120.), px(50.)),
                pressed_button: Some(gpui::MouseButton::Left),
                ..Default::default()
            },
            cx,
        );
        assert!(audit.selected.is_empty(), "control-drag toggles a hit off");
        audit.finish_marquee(cx);
    });
}

#[gpui::test]
fn action_bar_clicks_do_not_replace_the_marquee_selection(cx: &mut TestAppContext) {
    let (audit, cx) = pointer_checkbox_audit(false, cx);
    let selected = audit.read_with(cx, |audit, _| audit.selected.clone());
    let convert = cx
        .debug_bounds("action-bar")
        .expect("the audit action bar is visible");

    cx.simulate_click(
        gpui::point(convert.left() + px(2.), convert.center().y),
        gpui::Modifiers::none(),
    );

    audit.read_with(cx, |audit, _| {
        assert_eq!(audit.selected, selected);
        assert!(audit.marquee.is_none());
    });
}

#[gpui::test]
fn ai_operations_target_the_context_image(cx: &mut TestAppContext) {
    let (audit, cx) = finding_audit(cx);
    audit.update(cx, |audit, cx| {
        audit.selected.extend([0, 1]);
        audit.rail = Rail::Studio;

        audit.open_ai_operations(2, None, cx);

        assert_eq!(audit.selected, HashSet::from([2]));
        assert_eq!(audit.cursor, audit.row_of(2).unwrap());
        assert_eq!(audit.rail, Rail::Studio);
    });
}

#[gpui::test]
fn studio_prompt_typing_does_not_toggle_the_audit_selection(cx: &mut TestAppContext) {
    let (audit, cx) = finding_audit(cx);
    audit.update(cx, |audit, cx| {
        audit.selected.insert(0);
        audit.rail = Rail::Studio;
        audit.selection_changed(cx);
    });
    cx.update(|window, cx| window.draw(cx).clear(cx));

    let input = cx
        .debug_bounds("studio-prompt-input")
        .expect("the Studio prompt is visible");
    cx.simulate_click(input.center(), gpui::Modifiers::none());
    cx.update(|window, cx| assert!(audit.read(cx).studio_input_focused(window, cx)));
    cx.simulate_keystrokes("space");

    audit.read_with(cx, |audit, _| {
        assert_eq!(audit.selected, HashSet::from([0]));
    });
}

#[gpui::test]
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
#[gpui::test]
fn toggling_a_column_reaches_the_table(cx: &mut TestAppContext) {
    let (audit, cx) = finding_audit(cx);
    cx.update(|window, cx| window.draw(cx).clear(cx));

    let columns = |cx: &mut gpui::VisualTestContext| {
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

#[gpui::test]
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
#[gpui::test]
fn one_ticked_file_is_what_single_image_tools_act_on(cx: &mut TestAppContext) {
    let (audit, cx) = finding_audit(cx);
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

#[gpui::test]
fn custom_output_and_destination_are_named_in_the_panel(cx: &mut TestAppContext) {
    let (audit, cx) = finding_audit(cx);
    audit.update(cx, |audit, _| audit.quality = Quality::lossy(57.));
    cx.update(|window, cx| window.draw(cx).clear(cx));

    assert!(cx.debug_bounds("output-destination").is_some());
    assert!(cx.debug_bounds("custom-settings-active").is_some());
}

#[gpui::test]
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
    audit: &gpui::Entity<Audit>,
    selector: &'static str,
    cx: &mut gpui::VisualTestContext,
) {
    let checkbox = cx
        .debug_bounds(selector)
        .expect("the checkbox must be rendered in its parent event tree");
    let before = audit.read_with(cx, |audit, _| audit.estimate_generation);

    cx.simulate_click(checkbox.center(), gpui::Modifiers::none());
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
    cx.simulate_click(checkbox.center(), gpui::Modifiers::none());
    audit.read_with(cx, |audit, _| {
        assert_eq!(audit.selected, [0, 1].into_iter().collect());
        assert!(audit.compare.is_none());
        assert_eq!(audit.estimate_generation, before + 2);
        assert_eq!(audit.estimate, None);
    });
}

#[gpui::test]
fn grid_checkbox_pointer_click_stays_inside_checkbox(cx: &mut TestAppContext) {
    let (audit, cx) = pointer_checkbox_audit(true, cx);
    assert_pointer_checkbox_toggle(&audit, "grid-checkbox-0", cx);
}

#[gpui::test]
fn table_checkbox_pointer_click_stays_inside_checkbox(cx: &mut TestAppContext) {
    let (audit, cx) = pointer_checkbox_audit(false, cx);
    assert_pointer_checkbox_toggle(&audit, "table-checkbox-0", cx);
}

#[gpui::test]
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
        let event = gpui::KeyDownEvent {
            keystroke: gpui::Keystroke {
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

    let other = gpui::KeyDownEvent {
        keystroke: gpui::Keystroke {
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

#[gpui::test]
fn gallery_scroll_resets_only_when_the_production_column_count_changes(
    cx: &mut gpui::TestAppContext,
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
                skipped_packages: 0,
                unreadable: Vec::new(),
                walk_errors: Vec::new(),
                existing_output: 0,
                open_single: false,
                format: Format::WebP,
                quality: Quality::lossy(80.),
                max_edge: MaxEdge::FULL,
                grid: true,
                columns: ColumnPrefs::default(),
                output: crate::settings::Output::default(),
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

#[gpui::test]
fn opening_another_large_folder_resets_gallery_scroll_at_the_same_column_count(
    cx: &mut gpui::TestAppContext,
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
                skipped_packages: 0,
                unreadable: Vec::new(),
                walk_errors: Vec::new(),
                existing_output: 0,
                open_single: false,
                format: Format::WebP,
                quality: Quality::lossy(80.),
                max_edge: MaxEdge::FULL,
                grid: true,
                columns: ColumnPrefs::default(),
                output: crate::settings::Output::default(),
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
        audit.install_dataset(first_scan, first_folder.clone(), false, window, cx);
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
        audit.install_dataset(second_scan, second_folder.clone(), false, window, cx);
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

#[gpui::test]
fn opening_another_large_folder_resets_table_scroll(cx: &mut gpui::TestAppContext) {
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
                skipped_packages: 0,
                unreadable: Vec::new(),
                walk_errors: Vec::new(),
                existing_output: 0,
                open_single: false,
                format: Format::WebP,
                quality: Quality::lossy(80.),
                max_edge: MaxEdge::FULL,
                grid: false,
                columns: ColumnPrefs::default(),
                output: crate::settings::Output::default(),
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
        skipped_packages: 0,
        unreadable: Vec::new(),
        walk_errors: Vec::new(),
        existing_output: 0,
    };
    audit.update_in(cx, |audit, window, cx| {
        audit.install_dataset(scanned, PathBuf::from("new"), false, window, cx);
    });
    cx.run_until_parked();
    cx.simulate_resize(size(px(900.), px(640.)));
    cx.run_until_parked();

    assert_eq!(
        table.read_with(cx, |table, _| table.visible_range().rows().start),
        0
    );
}
