//! Audit window state, background jobs, rendering, and tests.

mod compare_view;
mod convert_job;
mod gallery;
mod header;
mod local_ai_actions;
mod media;
mod panel;
mod sirv_actions;
mod sirv_view;
mod state;
mod statusbar;
mod table;
#[cfg(test)]
mod tests;
mod toolbar;
mod view;

use crate::settings::{ColumnPrefs, Output};
use table::AuditTable;
#[cfg(test)]
use table::{TableColumn, W_NAME_MIN};
use view::meter;

use std::collections::{HashMap, HashSet, VecDeque};
use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::time::Duration;

use crate::compare::Pair;
use crate::convert::{Format, MaxEdge, Quality};
use crate::scan::{Entry, format_bytes, format_name};
use crate::{Launch, compare, convert, local_ai, scan, settings, sirv, thumbs};
use futures::future::select_all;
use gpui::{
    App, Context, Decorations, FocusHandle, Focusable as _, FontWeight, RenderImage,
    ScrollStrategy, UniformListScrollHandle, Window, div, img, prelude::*, px, rgb, rgba,
    uniform_list,
};
use gpui_component::alert::Alert;
use gpui_component::button::{Button, ButtonGroup, ButtonVariants};
use gpui_component::checkbox::Checkbox;
use gpui_component::input::{Input, InputContentType, InputEvent, InputState};
use gpui_component::popover::Popover;
use gpui_component::progress::Progress;
use gpui_component::scroll::{Scrollbar, ScrollbarMode};
use gpui_component::slider::{Slider, SliderEvent, SliderState};
use gpui_component::switch::Switch;
use gpui_component::table::{Column as TableCol, ColumnSort, DataTable, TableDelegate, TableState};
use gpui_component::tag::Tag;
use gpui_component::{ActiveTheme, Disableable, IconName, Selectable, Sizable};

// Colours come from `cx.theme()` rather than a private palette. The window is
// built out of this library's buttons, inputs and tags, and a hand-picked set of
// greys sitting behind them agreed with nothing — the chrome and the controls were
// two designs in one window.

/// Bytes per output pixel, banded. A photographic JPEG lands near 0.2; a
/// screenshot saved as PNG can be ten times that. The number was already in the
/// list and every row printed it in the same grey, which made the app's one
/// diagnostic something you had to read rather than see.
const DENSITY_GOOD: f32 = 0.5;
const DENSITY_HEAVY: f32 = 1.5;
/// Gallery rows stay uniform for virtualisation, but the tile itself grows to use the
/// available surface instead of leaving a dead strip beside three tiny cards.
const TILE_MIN: f32 = 168.;
const TILE_MAX: f32 = 224.;
const TILE_GAP: f32 = 8.;
const GALLERY_MIN_COLUMNS: usize = 1;
const GALLERY_MAX_COLUMNS: usize = 5;
const ROOT_PADDING: f32 = 12.;
const ROOT_BORDER: f32 = 2.;
const GALLERY_PADDING: f32 = 8.;
const GALLERY_BORDER: f32 = 1.;
/// Files encoded to project a total.
///
/// Measured against a real 3.0GB folder that converts to 422.9MB, sweeping which file
/// each slice offers up: 16 slices land anywhere in −53%..+59%, and 32 slices tighten
/// that to −36%..+10%. Samples run together, so 32 of them cost 0.9s on that folder.
/// AVIF and JPEG XL settle for three and stay rough numbers instead of making each
/// slider stop feel like a conversion.
fn sample_size(format: Format) -> usize {
    match format {
        Format::WebP => 32,
        Format::Avif | Format::JpegXl => 3,
    }
}
/// Settling time before sampling, so dragging the slider does not start a run per pixel.
const ESTIMATE_DELAY: Duration = Duration::from_millis(400);
/// Settling time before building a comparison, so a held arrow key does not queue one
/// full decode and encode per repeat.
const COMPARE_DELAY: Duration = Duration::from_millis(120);
/// Settling time before the window state reaches disk, so a resize drag is one write.
const SETTINGS_SAVE_DELAY: Duration = Duration::from_millis(500);

/// Decoded thumbnails kept in memory at once. A viewport holds a few dozen, so this is
/// still far more than scrolling needs; without it a 5,000-image folder scrolled end to
/// end retains 5,000 decoded thumbnails and a GPU texture for each.
///
/// Lower than it was, because `THUMB_EDGE` grew to fill a gallery tile. A 3:2 thumbnail
/// is about 150KB of texture at that size against 25KB before, so 512 of them would be
/// 75MB of video memory for rows nobody is looking at.
const THUMB_CACHE: usize = 192;
const THUMB_SETTLE: Duration = Duration::from_millis(300);
/// Cold thumbnail work runs beside the eight-file WebP estimator. Two decodes keep
/// enough CPU free for the window while still filling a viewport in one short wave.
const THUMB_WORKERS: usize = 2;

/// The open rail. Every operation with settings owns one, so the action bar
/// can hold verbs alone and no operation borrows another's controls.
#[derive(Clone, Copy, PartialEq, Eq, Debug, Default)]
pub(super) enum Rail {
    #[default]
    None,
    Convert,
    RemoveBackground,
    Upscale,
}

impl Rail {
    fn title(self) -> &'static str {
        match self {
            Rail::None => "",
            Rail::Convert => "Convert",
            Rail::RemoveBackground => "Remove background",
            Rail::Upscale => "Upscale 4×",
        }
    }
}

struct ThumbRequest {
    index: usize,
    dataset_generation: u64,
    edge: u32,
    path: PathBuf,
}

fn is_checkbox_activation_key(event: &gpui::KeyDownEvent) -> bool {
    matches!(event.keystroke.key.as_str(), "space" | "enter")
        && !event.keystroke.modifiers.modified()
}

/// Geometry for one virtualised gallery row. Band ranges are calculated on demand,
/// so only the bands requested by `uniform_list` allocate tiles.
#[derive(Clone, Copy, Debug, PartialEq)]
struct GalleryLayout {
    columns: usize,
    rows: usize,
    entries: usize,
    tile: f32,
}

impl GalleryLayout {
    fn band_range(self, band: usize) -> std::ops::Range<usize> {
        let first = band.saturating_mul(self.columns).min(self.entries);
        let last = first.saturating_add(self.columns).min(self.entries);
        first..last
    }

    #[cfg(test)]
    fn bands(self) -> impl Iterator<Item = std::ops::Range<usize>> {
        (0..self.rows).map(move |band| self.band_range(band))
    }
}

fn gallery_layout(
    viewport_width: f32,
    root_left: f32,
    root_right: f32,
    entries: usize,
) -> GalleryLayout {
    let chrome = root_left
        + root_right
        + 2. * (ROOT_PADDING + ROOT_BORDER + GALLERY_PADDING + GALLERY_BORDER);
    let available = (viewport_width - chrome).max(0.);
    let columns = ((available + TILE_GAP) / (TILE_MIN + TILE_GAP)) as usize;
    let columns = columns.clamp(GALLERY_MIN_COLUMNS, GALLERY_MAX_COLUMNS);
    let tile = ((available - (columns.saturating_sub(1) as f32 * TILE_GAP)) / columns as f32)
        .clamp(TILE_MIN, TILE_MAX);
    GalleryLayout {
        columns,
        rows: entries.div_ceil(columns),
        entries,
        tile,
    }
}

/// Root owns a one-pixel border on every non-tiled client-decoration edge.
fn root_horizontal_chrome(window: &Window) -> (f32, f32) {
    let paddings = gpui_component::window_paddings(window);
    let (left_border, right_border) = match window.window_decorations() {
        Decorations::Client { tiling } => {
            ((!tiling.left) as u8 as f32, (!tiling.right) as u8 as f32)
        }
        Decorations::Server => (0., 0.),
    };
    (
        f32::from(paddings.left) + left_border,
        f32::from(paddings.right) + right_border,
    )
}

/// Which band a file's byte density falls in. Green is carrying its weight, amber
/// is suspicious, red is a screenshot saved as a PNG.
fn density_colour(density: f32, cx: &App) -> gpui::Hsla {
    if density <= DENSITY_GOOD {
        cx.theme().green
    } else if density <= DENSITY_HEAVY {
        cx.theme().yellow
    } else {
        cx.theme().red
    }
}

pub(crate) struct Audit {
    root: PathBuf,
    entries: Vec<Entry>,
    skipped_raw: usize,
    /// macOS packages the scan skipped whole, counted like raw: excluded by
    /// design, so the total says so.
    skipped_packages: usize,
    /// Decoded thumbnails, keyed by their row. Only rows that have been on screen are
    /// in here; a folder of 5,000 images never decodes 5,000 files.
    thumbs: HashMap<usize, Arc<RenderImage>>,
    /// Rows already handed to a background thread, so scrolling past one twice does
    /// not decode it twice.
    requested: HashSet<usize>,
    /// Settled, still-visible rows waiting for a bounded decode slot.
    thumb_queue: VecDeque<ThumbRequest>,
    thumb_inflight: usize,
    /// The order `thumbs` filled up in, so the oldest decode is the one that leaves
    /// when the cache reaches `THUMB_CACHE`.
    thumb_order: VecDeque<usize>,
    format: Format,
    quality: Quality,
    max_edge: MaxEdge,
    /// Drives the quality slider. Its own entity, because that is how the component
    /// reports drags.
    quality_slider: gpui::Entity<SliderState>,
    /// Rows ticked for conversion. Empty means "all of them".
    selected: HashSet<usize>,
    /// Encoded size per row, filled in as conversion progresses.
    results: HashMap<usize, u64>,
    /// Outputs successfully written in this session, retained when settings change
    /// so the next action can say that it will replace them.
    completed_outputs: HashSet<(usize, Format)>,
    /// Source and output bytes for `results`. Conversion progress redraws often,
    /// so rebuilding these totals from every completed row would get slower as
    /// the job advances.
    converted_totals: (u64, u64),
    converting: bool,
    /// The immutable denominator owned by the active conversion.
    active_target_count: Option<usize>,
    /// Names of files a conversion could not read or write. Kept rather than counted,
    /// because "3 failed" without saying which is not a report.
    failures: Vec<String>,
    /// Files in the folder that claim to be images and will not decode, by name. A
    /// count alone says a folder has a problem and gives you nowhere to look.
    unreadable: Vec<PathBuf>,
    /// Directories the scan could not enter, by path. Every number in the header is
    /// short while one of these exists, so they are named like `unreadable`.
    walk_errors: Vec<PathBuf>,
    /// Files already sitting in the output folder when this one was scanned.
    existing_output: usize,
    /// A drag is hovering over the window.
    drag_over: bool,
    /// The open side-by-side view, if any.
    compare: Option<Comparison>,
    /// One local inference job. It stays visible after the comparison closes so a
    /// completed file or named failure never disappears with the view that started it.
    local_ai_job: Option<LocalAiJob>,
    /// The paired Sirv folder, if any: the client, the remote path, and its
    /// listing keyed by the same relative keys the local rows use.
    sirv_pairing: Option<SirvPairing>,
    /// How the local dataset stands against it: files to push, files that
    /// differ, files to pull. Recomputed when the dataset or the listing
    /// changes, never per frame.
    sirv_counts: Option<(usize, usize, usize)>,
    /// A snapshot from the last walk, patched by completed pulls. A file made
    /// by hand between walks stays stale until the next walk, like the listing.
    sirv_local_presence: HashSet<String>,
    /// A running or finished Sirv transfer, shown in the notices line.
    sirv_job: Option<SirvJob>,
    /// The destructive transfer awaiting its second click.
    sirv_confirm: Option<SirvJobKind>,
    /// Bumped whenever a running transfer stops being wanted.
    sirv_generation: u64,
    /// Bumped whenever a pairing changes, so an old recursive listing cannot
    /// land under a newly selected remote folder.
    sirv_pairing_generation: u64,
    /// Lets a superseded recursive walk stop between pages and directories instead
    /// of consuming the whole bounded request budget before its result is discarded.
    sirv_walk_cancel: Option<Arc<std::sync::atomic::AtomicBool>>,
    /// Identity of the current browser instance. Per-path request numbers restart
    /// when the overlay reopens, so they cannot identify the instance by themselves.
    sirv_browser_generation: u64,
    /// The open remote-folder browser.
    sirv_browser: Option<SirvBrowser>,
    /// The open settings overlay.
    settings_panel: Option<SettingsPanel>,
    /// How the list is ordered.
    sort: Sort,
    /// Indices into `entries`, filtered and sorted. `entries` itself never moves, so
    /// thumbnails, ticks and results stay attached to their file through both.
    visible: Vec<usize>,
    /// Substring the name must contain, lowercased. Empty shows everything.
    filter: String,
    /// The finding the list is narrowed to, if any. Sits alongside the name filter
    /// rather than replacing it, so you can search within one.
    finding: Option<Finding>,
    /// Backs the filter box.
    filter_input: gpui::Entity<InputState>,
    /// Row the keyboard is on, as a position in `visible`.
    cursor: usize,
    /// High-rate key repeats update the cursor faster than a display can present.
    /// One next-frame callback draws the latest position instead of rebuilding
    /// the table once for every queued key event.
    cursor_redraw_pending: bool,
    /// Where the last plain click landed, which is the fixed end of a shift-click
    /// range. Separate from `cursor` so arrowing around does not move the anchor.
    anchor: usize,
    /// The last quality the slider was set to, so turning Lossless off goes back to
    /// where you were rather than to an arbitrary default.
    slider_quality: f32,
    /// List or gallery.
    grid: bool,
    /// The gallery scroll state survives renders so a width transition can reset it.
    gallery_scroll: UniformListScrollHandle,
    /// The column count laid out last frame. `None` deliberately leaves initial layout alone.
    gallery_columns: Option<usize>,
    /// Bands GPUI asked the virtualised gallery to render this frame.
    gallery_visible: std::ops::Range<usize>,
    /// Projected output size for the current settings, and how many files were
    /// actually encoded to get it.
    estimate: Option<(u64, usize)>,
    /// Bumped on every settings change so a slow sample can tell it is stale. Dragging
    /// the quality slider fires dozens of these.
    estimate_generation: u64,
    /// Invalidates detached work when a new folder or file is installed.
    dataset_generation: u64,
    /// Invalidates older folder-open requests while a newer scan is pending.
    scan_generation: u64,
    /// The path currently being scanned, if any.
    scanning: Option<String>,
    /// Keyboard target. Without one the window gets no key events at all.
    focus: FocusHandle,
    /// Last title pushed to the compositor, so render does not set it every frame.
    titled: String,
    /// Last state render asked to store, so render only schedules a write when it
    /// changes.
    settings: settings::Settings,
    /// A delayed save is already waiting; it reads `settings` when it fires, so a
    /// whole resize drag needs one task and one write.
    settings_save_pending: bool,
    /// The last pair built, kept so closing and reopening the same image is instant.
    // ponytail: one entry. A pair holds two full-size RGBA buffers — 165 MB for a
    // 5568x3712 photo — so a bigger cache would need a byte budget, not a count.
    cached: Option<(compare::Key, Arc<Pair>)>,
    /// Bytes of the heaviest visible file, so every row's weight bar is drawn
    /// against the same scale. Cached because the alternative is a scan of the
    /// whole list once per row.
    heaviest: u64,
    /// Cached with `heaviest`; progress and thumbnail redraws must not rescan the
    /// entire visible folder just to rebuild the header.
    visible_bytes: u64,
    /// Counted once when the folder is read; findings do not change between scans.
    heavy: usize,
    /// Files whose extension disagrees with their contents, also fixed for a scan.
    mislabelled: usize,
    /// The visible part of a non-empty selection. Cached because the output panel
    /// is rebuilt by cursor, thumbnail and comparison interaction.
    selected_target_count: usize,
    selected_target_bytes: u64,
    /// The list, which the component library owns. It holds a weak handle back to
    /// this audit and reads its rows through that, so it cannot be built until this
    /// audit is a live entity: `TableState::new` asks the delegate for its row and
    /// column counts straight away, and answering that means reading the audit.
    table: Option<gpui::Entity<TableState<AuditTable>>>,
    /// Width/preferences/result/Sirv signature last handed to the component table.
    table_signature: Option<(u32, ColumnPrefs, bool, bool)>,
    /// Which optional columns the picker has on.
    column_prefs: ColumnPrefs,
    /// Where conversions and local-model results are written.
    output: Output,
    /// The open rail, if any. A folder opens on Convert: it is the app's job,
    /// and an empty right-hand edge on launch would hide it.
    rail: Rail,
}

/// List order. Every column is sortable, and clicking the active one reverses it.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Sort {
    column: Column,
    descending: bool,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Column {
    Name,
    Format,
    Pixels,
    Density,
    Weight,
}

/// Ties fall back to the filename so the order is stable between runs — a list that
/// reshuffles itself is worse than one sorted badly.
fn compare_entries(a: &Entry, b: &Entry, sort: Sort) -> std::cmp::Ordering {
    let a_name = a.name_lossy();
    let b_name = b.name_lossy();
    let ordering = match sort.column {
        Column::Name => a_name
            .chars()
            .flat_map(char::to_lowercase)
            .cmp(b_name.chars().flat_map(char::to_lowercase)),
        Column::Format => format_name(a.format).cmp(format_name(b.format)),
        Column::Pixels => {
            (a.width as u64 * a.height as u64).cmp(&(b.width as u64 * b.height as u64))
        }
        Column::Density => a
            .bytes_per_pixel()
            .partial_cmp(&b.bytes_per_pixel())
            .unwrap_or(std::cmp::Ordering::Equal),
        Column::Weight => a.bytes.cmp(&b.bytes),
    }
    .then_with(|| a_name.as_ref().cmp(b_name.as_ref()));

    if sort.descending {
        ordering.reverse()
    } else {
        ordering
    }
}

/// A paired Sirv folder. `files` maps the relative keys `sirv::relative_key`
/// produces for local rows onto the remote listing, so the diff column is a
/// lookup, never a walk. `None` while the recursive listing is in flight —
/// a pairing that just happened does not know its diff yet.
/// What the paired folder's remote listing knows.
///
/// This was an `Option<HashMap<..>>`, so `None` meant both "the walk is running" and
/// "the walk failed". The window showed the first and reported the second as a pull
/// that transferred nothing — the same confusion the comparison view had between
/// loading and failed, and the same fix.
enum Listing {
    Walking,
    Failed(String),
    Ready(HashMap<String, sirv::Node>),
}

enum CdnHost {
    Loading,
    Failed(String),
    Ready(String),
}

struct SirvPairing {
    dir: String,
    files: Listing,
    cdn_host: CdnHost,
    client: Arc<parking_lot::Mutex<sirv::Client>>,
}

/// What a background Sirv job is doing, and how far it got. Failures keep
/// names, because "2 failed" is not a report.
#[derive(Clone, Copy, PartialEq)]
enum SirvJobKind {
    Pull,
    /// Deliberately overwrite the differing local copy.
    PullChanged,
    Push,
    /// Deliberately overwrite the differing remote copy.
    PushChanged,
}

struct SirvJob {
    kind: SirvJobKind,
    done: usize,
    total: usize,
    /// Total failures. Only the first few messages are retained below.
    failed: usize,
    failures: Vec<String>,
    finished: bool,
    /// A stop has been requested; the in-flight file still has to acknowledge it.
    stopping: bool,
    /// The transfer generation this job belongs to. Unpairing or opening another
    /// folder bumps `sirv_generation`, and the loop stops at its next file rather
    /// than uploading the rest of a folder nobody is paired to any more.
    generation: u64,
}

/// The remote-folder browser: one path, its listing, and its own focus so
/// Escape closes it rather than the thing underneath.
struct SirvBrowser {
    client: Arc<parking_lot::Mutex<sirv::Client>>,
    path: String,
    /// This browser is the credential setup route, not a remote listing.
    needs_credentials: bool,
    /// `None` while the listing is in flight.
    nodes: Option<Result<Vec<sirv::Node>, String>>,
    /// Bumped per request, so a listing for a folder the user has already left
    /// cannot overwrite the one they are looking at.
    generation: u64,
    /// The audit-owned identity of this browser instance.
    session: u64,
    /// Take focus once after the overlay tree exists, then leave child controls alone.
    focused: bool,
    focus: gpui::FocusHandle,
}

/// The settings overlay: the CDN credentials, and nothing else. Inputs are entities
/// so the framework owns their editing state.
struct SettingsPanel {
    client_id: gpui::Entity<InputState>,
    client_secret: gpui::Entity<InputState>,
    /// (ok?, message) per section.
    cdn_status: Option<(bool, String)>,
    /// Which form field holds focus, as an index into the field list.
    focus_ix: usize,
    /// The panel has taken focus already. Without this the next render put focus back
    /// in the first field, so Tab and a click into another field both came undone the
    /// moment a save or a status message redrew the audit.
    focused: bool,
}

fn credentials_complete(client_id: &str, client_secret: &str) -> bool {
    !client_id.trim().is_empty() && !client_secret.trim().is_empty()
}

struct Comparison {
    index: usize,
    dataset_generation: u64,
    /// Take focus once after the comparison tree exists. Re-focusing on every
    /// render steals keyboard ownership from the comparison buttons.
    focused: bool,
    key: compare::Key,
    /// `None` while the two sides are still decoding.
    pair: Option<Arc<Pair>>,
    /// A completed build can fail after the initial loading frame.
    failed: bool,
    /// Where the divider sits, 0 to 1 across the viewport.
    split: f32,
    /// How far the image is dragged from centre, in pixels.
    pan: (f32, f32),
    /// Display scale. `None` means fit the window; `Some(1.0)` is one image pixel per
    /// screen pixel. Kept separate so resizing the window keeps "fit" fitting.
    zoom: Option<f32>,
    /// Pointer position when the current drag began, and the pan it started from.
    drag: Option<((f32, f32), (f32, f32))>,
}

enum LocalAiJobState {
    SettingUp,
    Running,
    Done(PathBuf),
    Failed(String),
}

struct LocalAiJob {
    tool: local_ai::Tool,
    index: usize,
    dataset_generation: u64,
    source_name: String,
    first_setup: bool,
    state: LocalAiJobState,
    cancelled: Arc<std::sync::atomic::AtomicBool>,
}

/// Write the remembered state, except in tests, where a render must not touch the
/// user's real config file.
fn write_settings(settings: &settings::Settings) {
    #[cfg(not(test))]
    settings::save(settings);
    #[cfg(test)]
    let _ = (settings, settings::save as fn(&settings::Settings));
}

impl Audit {
    /// Install a completed scan. This is the one state transition that replaces the
    /// dataset and invalidates every detached job derived from the old rows.
    fn install_dataset(
        &mut self,
        scanned: scan::Scan,
        root: PathBuf,
        single: bool,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let root_changed = self.root != root;
        self.dataset_generation = self.dataset_generation.wrapping_add(1);
        self.estimate_generation = self.estimate_generation.wrapping_add(1);
        self.estimate = None;
        self.converting = false;
        self.active_target_count = None;
        if let Some(job) = self.local_ai_job.take() {
            job.cancelled
                .store(true, std::sync::atomic::Ordering::Relaxed);
        }
        self.root = root;
        self.mislabelled = scanned
            .entries
            .iter()
            .filter(|entry| entry.extension_lies())
            .count();
        self.heavy = scanned
            .entries
            .iter()
            .filter(|entry| Finding::Heavy.holds(entry))
            .count();
        self.entries = scanned.entries;
        // The scroll handle belongs to the gallery rather than its data. A new folder
        // can have the same column count, so a render-time column transition cannot
        // be relied on to bring its first image into view.
        self.gallery_scroll
            .scroll_to_item_strict(0, ScrollStrategy::Top);
        if let Some(table) = self.table.clone() {
            cx.defer(move |cx| {
                table.update(cx, |table, cx| table.scroll_to_row(0, cx));
            });
        }
        self.skipped_raw = scanned.skipped_raw;
        self.skipped_packages = scanned.skipped_packages;
        self.unreadable = scanned.unreadable;
        self.walk_errors = scanned.walk_errors;
        self.existing_output = scanned.existing_output;
        self.thumbs.clear();
        self.thumb_order.clear();
        self.requested.clear();
        self.thumb_queue.clear();
        self.selected.clear();
        self.clear_results();
        self.completed_outputs.clear();
        self.failures.clear();
        self.compare = None;
        self.cached = None;
        self.filter.clear();
        // A finding belongs to the folder it was found in. Carrying it over would show
        // the new folder narrowed to something nobody asked about.
        self.finding = None;
        self.filter_input.update(cx, |input, cx| {
            input.set_value("", window, cx);
        });
        self.cursor = 0;
        self.anchor = 0;
        self.refresh_visible();
        // A pairing maps one local root to one remote folder. A rescan of that root
        // keeps it; replacing the root retires it before the new rows can be pushed.
        if root_changed {
            self.unpair_sirv(cx);
        } else {
            self.cancel_sirv_transfer();
            if let Some(pairing) = self.sirv_pairing.as_mut() {
                self.sirv_local_presence.clear();
                pairing.files = Listing::Walking;
                self.sirv_counts = None;
                self.walk_sirv_pairing(cx);
            } else {
                self.refresh_sirv_counts();
            }
        }
        self.schedule_estimate(cx);
        cx.notify();

        if single {
            self.open_compare(0, cx);
        }
    }

    /// Scan a requested path away from the UI thread. A newer request wins, while a
    /// failed current request leaves the last usable dataset in place.
    pub(super) fn request_path(&mut self, path: PathBuf, cx: &mut Context<Self>) {
        if self.converting {
            return;
        }
        let single = path.is_file();
        if !single && !path.is_dir() {
            return;
        }
        self.scan_generation = self.scan_generation.wrapping_add(1);
        let request = self.scan_generation;
        let label = path
            .file_name()
            .map(|name| name.to_string_lossy().into_owned())
            .unwrap_or_else(|| path.display().to_string());
        self.scanning = Some(label);
        // A chosen destination follows the audit to the new folder: it is a
        // preference about where output belongs, not about this folder.
        let output = self.output.clone();
        cx.notify();

        cx.spawn(async move |this, cx| {
            let result = cx
                .background_executor()
                .spawn(async move {
                    if single {
                        let entry = scan::probe(&path)?;
                        let root = path.parent().unwrap_or(Path::new(".")).to_path_buf();
                        Some((
                            scan::Scan {
                                entries: vec![entry],
                                skipped_raw: 0,
                                skipped_packages: 0,
                                unreadable: Vec::new(),
                                walk_errors: Vec::new(),
                                existing_output: 0,
                            },
                            root,
                            true,
                        ))
                    } else {
                        Some((scan::scan(&path, &output.root(&path)), path, false))
                    }
                })
                .await;

            let _ = this.update_in(cx, |audit, window, cx| {
                if audit.scan_generation != request {
                    return;
                }
                audit.scanning = None;
                if let Some((scanned, root, single)) = result {
                    audit.install_dataset(scanned, root, single, window, cx);
                } else {
                    cx.notify();
                }
            });
        })
        .detach();
    }

    /// Hand the output folder to the desktop's file manager.
    // ponytail: three names for one idea, and no crate needed for it.
    fn reveal_output(&self) {
        let path = self.output.root(&self.root);
        if !path.exists() {
            return;
        }
        let opener = if cfg!(target_os = "macos") {
            "open"
        } else if cfg!(target_os = "windows") {
            "explorer"
        } else {
            "xdg-open"
        };
        let _ = std::process::Command::new(opener).arg(path).spawn();
    }

    /// Ask the desktop for a folder or a file. The dialog runs off the main thread so
    /// the window keeps drawing while it is open.
    /// Choose where output lands. The originals never move, so this only ever
    /// changes the destination of new files — which is why it can be changed
    /// mid-audit without invalidating anything already on screen.
    pub(super) fn pick_output(&mut self, cx: &mut Context<Self>) {
        if self.converting {
            return;
        }
        let start = self.output.root(&self.root);
        let start = if start.is_dir() {
            start
        } else {
            self.root.clone()
        };
        cx.spawn(async move |this, cx| {
            let chosen = cx
                .background_executor()
                .spawn(async move { rfd::FileDialog::new().set_directory(&start).pick_folder() })
                .await;
            if let Some(path) = chosen {
                let _ = this.update_in(cx, |audit, window, cx| {
                    audit.output = Output::Folder(path);
                    cx.notify();
                    window.refresh();
                });
            }
        })
        .detach();
    }

    pub(super) fn reset_output(&mut self, cx: &mut Context<Self>) {
        self.output = Output::Optimized;
        cx.notify();
    }

    pub(crate) fn pick(&mut self, folders: bool, cx: &mut Context<Self>) {
        if self.converting {
            return;
        }
        let start = self.root.clone();
        cx.spawn(async move |this, cx| {
            let chosen = cx
                .background_executor()
                .spawn(async move {
                    let dialog = rfd::FileDialog::new().set_directory(&start);
                    if folders {
                        dialog.pick_folder()
                    } else {
                        dialog.pick_file()
                    }
                })
                .await;

            if let Some(path) = chosen {
                let _ = this.update_in(cx, |audit, window, cx| {
                    audit.request_path(path, cx);
                    window.refresh();
                });
            }
        })
        .detach();
    }
}

/// What the audit found, as something the list can be narrowed to.
///
/// The window used to state these as numbers and stop there. A folder of 5,739 images
/// saying "5 files are not the format their extension claims" is a finding you cannot
/// reach: the whole point of an audit is to end up looking at those five.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum Finding {
    /// The extension disagrees with the bytes inside the file.
    Mislabelled,
    /// More bytes per pixel than a photograph needs. These are the files a conversion
    /// is actually for.
    Heavy,
}

impl Finding {
    fn holds(self, entry: &Entry) -> bool {
        match self {
            Finding::Mislabelled => entry.extension_lies(),
            Finding::Heavy => entry.bytes_per_pixel() > DENSITY_HEAVY,
        }
    }
}

/// A few names and then a count, rather than a count alone. Used wherever the window
/// reports a set of files it could not handle.
fn named(names: impl Iterator<Item = String>) -> String {
    let all: Vec<String> = names.collect();
    let shown = all.iter().take(3).cloned().collect::<Vec<_>>().join(", ");
    match all.len().saturating_sub(3) {
        0 => shown,
        rest => format!("{shown} and {rest} more"),
    }
}

fn sirv_push_plan(
    root: &Path,
    entries: &[scan::Entry],
    files: &HashMap<String, sirv::Node>,
    accept: sirv::SyncState,
) -> Vec<(String, PathBuf)> {
    entries
        .iter()
        .filter_map(|entry| {
            let key = sirv::relative_key(root, &entry.path)?;
            (sirv::classify(entry.bytes, files.get(&key)) == accept)
                .then(|| (key, entry.path.clone()))
        })
        .collect()
}

/// One sampled file and the slice of the list it speaks for.
struct Stratum {
    path: PathBuf,
    /// The sampled file's own size on disk.
    bytes: u64,
    /// Every file in its slice, that one included.
    slice_bytes: u64,
}

/// Project the encoded size of a whole list from a few real encodes.
///
/// Each entry is one slice's bytes and, when its sample encoded, that sample's own
/// source and encoded size. A slice is scaled by its own sample; a slice whose sample
/// would not decode is scaled by the average of the ones that did. Returns the total
/// and how many samples stood behind it, or `None` when nothing encoded at all.
///
/// The old version divided the summed sample bytes by the summed source bytes and
/// applied that one ratio to the folder. On a weight-sorted list of 5,739 photos the
/// heaviest file was 109MB of a 110MB sample, so its 300:1 compression became the
/// forecast for all 3GB and the window promised "3.0 GB to save, −100%".
fn project_total(slices: &[(u64, Option<(u64, u64)>)]) -> Option<(u64, usize)> {
    let ratio = |(source, encoded): (u64, u64)| encoded as f64 / source.max(1) as f64;
    let sampled: Vec<f64> = slices
        .iter()
        .filter_map(|(_, sample)| sample.map(ratio))
        .collect();
    if sampled.is_empty() {
        return None;
    }

    let average = sampled.iter().sum::<f64>() / sampled.len() as f64;
    let projected: f64 = slices
        .iter()
        .map(|(slice_bytes, sample)| *slice_bytes as f64 * sample.map_or(average, ratio))
        .sum();
    Some((projected as u64, sampled.len()))
}

fn conversion_targets(visible: &[usize], selected: &HashSet<usize>) -> Vec<usize> {
    if selected.is_empty() {
        visible.to_vec()
    } else {
        visible
            .iter()
            .copied()
            .filter(|index| selected.contains(index))
            .collect()
    }
}

fn progress_batch_ready(completed: usize, workers: usize, work_remaining: bool) -> bool {
    completed >= workers || !work_remaining
}

/// Build the audit view for a window. Shared by the app and the screenshot harness
/// so that what gets captured is the thing that ships.
pub(crate) fn build_audit(
    launch: Launch,
    window: &mut Window,
    cx: &mut App,
) -> gpui::Entity<Audit> {
    let Launch {
        root,
        entries,
        skipped_raw,
        skipped_packages,
        unreadable,
        walk_errors,
        existing_output,
        open_single,
        format,
        quality,
        max_edge,
        grid,
        columns: column_prefs,
        output,
    } = launch;

    let audit = cx.new(|cx| {
        let focus = cx.focus_handle();
        focus.focus(window, cx);

        let filter_input = cx.new(|cx| InputState::new(window, cx).placeholder("Filter by name"));
        cx.subscribe(
            &filter_input,
            |audit: &mut Audit, input, event: &InputEvent, cx| {
                if matches!(event, InputEvent::Change) {
                    let value = input.read(cx).value().to_string();
                    audit.set_filter(value, cx);
                }
            },
        )
        .detach();

        let quality_slider = cx.new(|_| {
            SliderState::new()
                .min(1.)
                .max(100.)
                .step(1.)
                .default_value(quality.0.unwrap_or(80.))
        });
        // Dragging the slider is the only thing that changes quality now,
        // so results from the old value stop being true the moment it moves.
        cx.subscribe(
            &quality_slider,
            |audit: &mut Audit, _, event: &SliderEvent, cx| {
                let SliderEvent::Change(value) = event else {
                    return;
                };
                if audit.converting {
                    return;
                }
                audit.quality = Quality::lossy(value.start());
                audit.slider_quality = value.start();
                audit.clear_results();
                audit.schedule_estimate(cx);
                cx.notify();
            },
        )
        .detach();
        let mislabelled = entries
            .iter()
            .filter(|entry| entry.extension_lies())
            .count();
        let heavy = entries
            .iter()
            .filter(|entry| Finding::Heavy.holds(entry))
            .count();
        let mut audit = Audit {
            table: None,
            table_signature: None,
            root,
            entries,
            skipped_raw,
            skipped_packages,
            heaviest: 0,
            visible_bytes: 0,
            heavy,
            mislabelled,
            selected_target_count: 0,
            selected_target_bytes: 0,
            thumbs: HashMap::new(),
            requested: HashSet::new(),
            thumb_queue: VecDeque::new(),
            thumb_inflight: 0,
            thumb_order: VecDeque::new(),
            format,
            quality,
            max_edge,
            quality_slider,
            selected: HashSet::new(),
            sort: Sort {
                column: Column::Weight,
                descending: true,
            },
            visible: Vec::new(),
            filter: String::new(),
            finding: None,
            filter_input,
            cursor: 0,
            cursor_redraw_pending: false,
            anchor: 0,
            slider_quality: quality.0.unwrap_or(80.),
            grid,
            gallery_scroll: UniformListScrollHandle::new(),
            gallery_columns: None,
            gallery_visible: 0..0,
            estimate: None,
            estimate_generation: 0,
            dataset_generation: 0,
            scan_generation: 0,
            scanning: None,
            focus,
            titled: String::new(),
            settings: settings::Settings::default(),
            settings_save_pending: false,
            cached: None,
            results: HashMap::new(),
            completed_outputs: HashSet::new(),
            converted_totals: (0, 0),
            converting: false,
            active_target_count: None,
            failures: Vec::new(),
            unreadable,
            walk_errors,
            existing_output,
            drag_over: false,
            sirv_pairing: None,
            sirv_counts: None,
            sirv_local_presence: HashSet::new(),
            sirv_job: None,
            sirv_confirm: None,
            sirv_generation: 0,
            sirv_pairing_generation: 0,
            sirv_walk_cancel: None,
            sirv_browser_generation: 0,
            sirv_browser: None,
            settings_panel: None,
            compare: None,
            local_ai_job: None,
            column_prefs,
            output,
            rail: Rail::Convert,
        };
        audit.refresh_visible();
        audit.schedule_estimate(cx);
        if open_single {
            audit.open_compare(0, cx);
        }
        audit
    });

    // Only now that the audit is a live entity, because building the
    // table asks the delegate how many rows there are and answering
    // that means reading the audit.
    let table = {
        let delegate = AuditTable::new(audit.downgrade(), window);
        cx.new(|cx| TableState::new(delegate, window, cx))
    };
    audit.update(cx, |audit, _| audit.table = Some(table));

    audit
}

/// Flush the debounced settings write when the app quits, whatever path
/// the quit took — menu, Cmd+W on the last window, or the close button.
pub(crate) fn register_quit_flush(audit: gpui::Entity<Audit>, cx: &mut gpui::App) {
    cx.on_app_quit(move |cx| {
        audit.update(cx, |audit, _| audit.flush_settings());
        async {}
    })
    .detach();
}
