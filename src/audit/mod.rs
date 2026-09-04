//! Audit window state, background jobs, rendering, and tests.

mod acquisition;
mod browser;
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
mod studio_actions;
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

use std::borrow::Cow;
use std::cell::{Cell, RefCell};
use std::collections::{HashMap, HashSet, VecDeque};
use std::ops::Range;
use std::path::{Path, PathBuf};
use std::rc::Rc;
use std::sync::{
    Arc,
    atomic::{AtomicBool, Ordering},
};
use std::time::Duration;

use crate::compare::{Pair, Preview};
use crate::convert::{Format, MaxEdge, Quality};
use crate::scan::{Entry, format_bytes, format_name};
use crate::{Launch, compare, convert, local_ai, scan, settings, sirv, studio, thumbs};
use futures::StreamExt;
use futures::future::select_all;
use gpui_kit::component::alert::Alert;
use gpui_kit::component::breadcrumb::{Breadcrumb, BreadcrumbItem};
use gpui_kit::component::button::{Button, ButtonGroup, ButtonVariants};
use gpui_kit::component::checkbox::Checkbox;
use gpui_kit::component::input::{Input, InputContentType, InputEvent, InputState};
use gpui_kit::component::list::ListItem;
use gpui_kit::component::menu::{ContextMenuExt, DropdownMenu, PopupMenu, PopupMenuItem};
use gpui_kit::component::notification::{Notification, NotificationType};
use gpui_kit::component::popover::Popover;
use gpui_kit::component::progress::Progress;
use gpui_kit::component::scroll::{Scrollbar, ScrollbarMode};
use gpui_kit::component::slider::{Slider, SliderEvent, SliderState};
use gpui_kit::component::switch::Switch;
use gpui_kit::component::table::{
    Column as TableCol, ColumnSort, DataTable, TableDelegate, TableState,
};
use gpui_kit::component::tag::Tag;
use gpui_kit::component::tooltip::Tooltip;
use gpui_kit::component::tree::{TreeEvent, TreeItem, TreeState, tree};
use gpui_kit::component::{
    ActiveTheme, Disableable, ElementExt, Icon, IconName, Selectable, Sizable, WindowExt,
};
use gpui_kit::{
    App, Context, Decorations, FocusHandle, Focusable as _, FontWeight, RenderImage,
    ScrollStrategy, UniformListScrollHandle, Window, div, img, prelude::*, px, rgb, rgba,
    uniform_list,
};
use image::DynamicImage;

struct ErrorToast;

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
/// Below these, bytes per pixel stops meaning anything. A 44-byte 1×2 sliver
/// carries an enormous ratio and has nothing to give back: the finding is a
/// claim that converting would win something, so it needs a file big enough for
/// that to be true.
const HEAVY_MIN_BYTES: u64 = 32_768;
const HEAVY_MIN_PIXELS: u64 = 64 * 64;

/// A folder browse owns one token so a newer request can stop it between files.
struct ScanCancellation {
    token: Arc<AtomicBool>,
}

impl ScanCancellation {
    fn new() -> Self {
        Self {
            token: Arc::new(AtomicBool::new(false)),
        }
    }

    fn cancel(&mut self) {
        self.token.store(true, Ordering::Release);
    }
}

/// Shared with the headless audit so its finding is exactly the one shown here.
pub(super) fn is_heavy(entry: &Entry) -> bool {
    entry.bytes >= HEAVY_MIN_BYTES
        && u64::from(entry.width) * u64::from(entry.height) >= HEAVY_MIN_PIXELS
        && entry.bytes_per_pixel() > DENSITY_HEAVY
}

/// Gallery rows stay uniform for virtualisation, but the tile itself grows to use the
/// available surface instead of leaving a dead strip beside three tiny cards.
const TILE_MIN: f32 = 168.;
const TILE_MAX: f32 = 224.;
const TILE_GAP: f32 = 8.;
const GALLERY_MIN_COLUMNS: usize = 1;
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
pub(crate) fn sample_size(format: Format) -> usize {
    match format {
        // A kept-format folder mixes containers, so it needs the wide sample more
        // than any single format does; three files would not project a mixture.
        Format::WebP | Format::Jpeg | Format::Png | Format::Same => 32,
        Format::Avif | Format::JpegXl => 3,
    }
}

/// The decoded pixels behind the estimate's sample, keyed by the dataset, the source
/// path and the max edge: the three things that change what a decode produces.
type SampledDecodes = Arc<parking_lot::Mutex<HashMap<(u64, PathBuf, MaxEdge), SampledDecode>>>;
/// One decoded sample: its pixels and the colour profile the writer will attach.
type SampledDecode = Arc<(DynamicImage, Option<Vec<u8>>)>;

/// The most decoded sample the estimate may hold on to. A sample is at most 32
/// images, and re-decoding one costs a fraction of a second, so past this the cache
/// would buy a slider stop with a gigabyte of resident pixels.
const ESTIMATE_DECODE_BYTES: u64 = 256 * 1024 * 1024;

/// Resident cost of one decoded sample.
fn decoded_bytes(sample: &SampledDecode) -> u64 {
    let image = &sample.0;
    u64::from(image.width())
        * u64::from(image.height())
        * u64::from(image.color().bytes_per_pixel())
}

/// Settling time before sampling, so dragging the slider does not start a run per pixel.
const ESTIMATE_DELAY: Duration = Duration::from_millis(400);
/// Settling time before building a comparison, so a held arrow key does not queue one
/// full decode and encode per repeat.
const COMPARE_DELAY: Duration = Duration::from_millis(120);
/// A source preview can start much sooner than an encode, but still waits out a held
/// arrow key so obsolete full-resolution decodes never crowd the useful one.
const PREVIEW_DELAY: Duration = Duration::from_millis(50);
/// The most media built ahead of the cursor may cost. A pair holds two full-size
/// RGBA buffers and a preview holds one; past this size the sweep would buy its head
/// start with a quarter gigabyte.
const PREFETCH_BUDGET: u64 = 128 * 1024 * 1024;
/// Settling time before the window state reaches disk, so a resize drag is one write.
const SETTINGS_SAVE_DELAY: Duration = Duration::from_millis(500);

/// Approximate upper bound for decoded thumbnail pixels. The table can retain many
/// more 96px rows than the 224px gallery without turning either mode into an
/// unbounded GPU cache.
const THUMB_CACHE_BYTES: usize = 64 * 1024 * 1024;
/// Native JPEG/WebP scaling avoids full-image allocations, so four jobs fill a
/// viewport quickly without recreating the old full-decode CPU spike.
const THUMB_WORKERS: usize = 4;
const THUMB_SLOW_WORKERS: usize = 2;
const THUMB_SLOW_SETTLE: Duration = Duration::from_millis(300);
const THUMB_REDRAW_DELAY: Duration = Duration::from_millis(8);
/// Start the next rows while the current ones are still on screen. The requested range
/// is narrowed on large grids so it never exceeds the decoded-pixel budget.
const THUMB_OVERSCAN_VIEWPORTS: usize = 4;

fn thumb_overscan_rows(visible: Range<usize>, total: usize, limit: usize) -> Range<usize> {
    let start = visible.start.min(total);
    let end = visible.end.min(total).max(start);
    let extra = end
        .saturating_sub(start)
        .saturating_mul(THUMB_OVERSCAN_VIEWPORTS);
    let wanted = start.saturating_sub(extra)..end.saturating_add(extra).min(total);
    let capacity = limit.max(end - start).min(total);
    if wanted.len() <= capacity {
        return wanted;
    }

    let before = (capacity - (end - start)) / 2;
    let mut bounded_start = start.saturating_sub(before);
    let bounded_end = bounded_start.saturating_add(capacity).min(total);
    bounded_start = bounded_end.saturating_sub(capacity);
    bounded_start..bounded_end
}

fn thumb_cache_limit(edge: u32) -> usize {
    let bytes = (edge as usize)
        .saturating_mul(edge as usize)
        .saturating_mul(4)
        .max(1);
    (THUMB_CACHE_BYTES / bytes).max(THUMB_WORKERS)
}

/// The open rail. Every operation with settings owns one, so the action bar
/// can hold verbs alone and no operation borrows another's controls.
#[derive(Clone, Copy, PartialEq, Eq, Debug, Default)]
pub(super) enum Rail {
    #[default]
    None,
    Convert,
    RemoveBackground,
    Upscale,
    Studio,
}

impl Rail {
    fn title(self) -> &'static str {
        match self {
            Rail::None => "",
            Rail::Convert => "Convert",
            Rail::RemoveBackground => "Remove background",
            Rail::Upscale => "Upscale 4×",
            Rail::Studio => "AI operations",
        }
    }
}

#[derive(Clone)]
struct ThumbRequest {
    index: usize,
    dataset_generation: u64,
    edge: u32,
    path: PathBuf,
    native_scaled: bool,
    fallback: bool,
}

struct Marquee {
    start: (f32, f32),
    current: (f32, f32),
    base: HashSet<usize>,
    toggle: bool,
}

impl Marquee {
    fn bounds(&self) -> gpui_kit::Bounds<gpui_kit::Pixels> {
        let left = self.start.0.min(self.current.0);
        let top = self.start.1.min(self.current.1);
        let right = self.start.0.max(self.current.0);
        let bottom = self.start.1.max(self.current.1);
        gpui_kit::Bounds::from_corners(
            gpui_kit::point(px(left), px(top)),
            gpui_kit::point(px(right), px(bottom)),
        )
    }
}

fn is_checkbox_activation_key(event: &gpui_kit::KeyDownEvent) -> bool {
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
    // No column cap: the tile size is the constraint, and a hard maximum of
    // five left a third of a wide window empty while the tiles stayed small.
    let columns = ((available + TILE_GAP) / (TILE_MIN + TILE_GAP)) as usize;
    let columns = columns.max(GALLERY_MIN_COLUMNS);
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
    let paddings = gpui_kit::component::window_paddings(window);
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
fn density_colour(density: f32, cx: &App) -> gpui_kit::Hsla {
    if density <= DENSITY_GOOD {
        cx.theme().green
    } else if density <= DENSITY_HEAVY {
        cx.theme().yellow
    } else {
        cx.theme().red
    }
}

pub(crate) struct Audit {
    window: gpui_kit::AnyWindowHandle,
    root: PathBuf,
    /// Filesystem identity of the current output, cached outside render so aliases
    /// and case-folded paths cannot expose the destination as an input folder.
    browser_output_root: PathBuf,
    /// Present when the dataset is an explicit file batch rather than every image
    /// found under `root`.
    batch_size: Option<usize>,
    /// Present when that explicit batch came from several sibling folders.
    batch_folders: Option<usize>,
    /// Direct child folders shown beside the direct image rows.
    folders: Vec<PathBuf>,
    /// File-manager shortcuts, newest first and bounded by settings.
    recent_folders: Vec<PathBuf>,
    /// The narrow/work-panel form of the folder browser is an overlay.
    browser_overlay: bool,
    /// Backs the loaded-folder search in the browser sidebar.
    folder_filter_input: gpui_kit::Entity<InputState>,
    tree_state: gpui_kit::Entity<TreeState>,
    tree_anchor: PathBuf,
    tree_children: HashMap<PathBuf, Vec<PathBuf>>,
    tree_loaded: HashSet<PathBuf>,
    tree_loading: HashSet<PathBuf>,
    tree_expanded: HashSet<PathBuf>,
    tree_paths: HashMap<String, PathBuf>,
    entries: Vec<Entry>,
    skipped_raw: usize,
    /// HEIC/HEIF files the scan counted but could not read, for the same reason:
    /// no decoder is linked, and a phone folder that lists nothing looks broken.
    skipped_heic: usize,
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
    thumb_slow_inflight: usize,
    thumb_prefetch_pending: bool,
    thumb_notify_pending: bool,
    /// The order `thumbs` filled up in, so the oldest decode is the one that leaves
    /// when the cache reaches the current mode's decoded-pixel limit.
    thumb_order: VecDeque<usize>,
    format: Format,
    quality: Quality,
    max_edge: MaxEdge,
    /// The size typed beside the presets. A preset click empties it; a typed size
    /// that happens to be a preset lights that preset and stays in the box.
    max_edge_input: gpui_kit::Entity<InputState>,
    /// Drives the quality slider. Its own entity, because that is how the component
    /// reports drags.
    quality_slider: gpui_kit::Entity<SliderState>,
    /// Rows explicitly ticked for conversion.
    selected: HashSet<usize>,
    /// Bounds of the rendered rows or tiles. Marquee selection only needs the
    /// visible objects, so virtualised items never get measured eagerly.
    selection_bounds: Rc<RefCell<HashMap<usize, gpui_kit::Bounds<gpui_kit::Pixels>>>>,
    selection_surface: Rc<Cell<gpui_kit::Bounds<gpui_kit::Pixels>>>,
    marquee: Option<Marquee>,
    /// Encoded size per row, filled in as conversion progresses.
    results: HashMap<usize, u64>,
    /// Where each of those rows was written. Kept because a run renames on a
    /// name clash, so the path cannot be recomputed from the source afterwards.
    result_paths: HashMap<usize, PathBuf>,
    /// Outputs successfully written in this session, retained when settings change
    /// so the next action can say that it will replace them.
    completed_outputs: HashSet<(usize, Format)>,
    /// Originals this folder's run record could put back. Read from disk when the
    /// dataset changes and after a run, never during a render, and it survives a
    /// restart because the record does.
    restorable: usize,
    /// Source and output bytes for `results`. Conversion progress redraws often,
    /// so rebuilding these totals from every completed row would get slower as
    /// the job advances.
    converted_totals: (u64, u64),
    converting: bool,
    /// The running conversion's stop flag, read between files. Sirv transfers and
    /// both AI jobs already owned one; the app's headline verb was the only long
    /// job with no way out of it.
    convert_cancel: Option<Arc<AtomicBool>>,
    /// The immutable denominator owned by the active conversion.
    active_target_count: Option<usize>,
    /// The target count of a run the user stopped, so the summary can say how far
    /// it got. The files it wrote are real and stay; the ones it never started are
    /// not failures.
    stopped_run: Option<usize>,
    /// Why a conversion could not read or write a row, keyed by that row like
    /// `results`. Kept rather than counted, because "3 failed" without saying which
    /// is not a report — and keyed rather than listed, because the row itself is
    /// where a reader looks for the reason once the toast is gone.
    failures: HashMap<usize, String>,
    /// The first few of those, named, as the notices line says them. Built when the
    /// map changes rather than per frame: that line is on screen for the whole run.
    failure_summary: String,
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
    /// One direct Studio API run, with the same stale-dataset and cancellation
    /// ownership as a local model run.
    studio_job: Option<StudioJob>,
    studio_tool: studio::Tool,
    studio_key: Option<String>,
    studio_key_input: gpui_kit::Entity<InputState>,
    studio_prompt: gpui_kit::Entity<InputState>,
    studio_key_checking: bool,
    studio_status: Option<(bool, String)>,
    /// A local result that opened the Studio rail. Kept with its source row so key
    /// setup or prompt editing does not silently switch the job back to the original.
    studio_source: Option<(usize, PathBuf)>,
    /// The paired Sirv folder, if any: the client, the remote path, and its
    /// listing keyed by the same relative keys the local rows use.
    sirv_pairing: Option<SirvPairing>,
    /// How the local dataset stands against it: files to push, files that
    /// differ, files to pull. Recomputed when the dataset or the listing
    /// changes, never per frame.
    sirv_counts: Option<(usize, usize, usize)>,
    /// The selected reconciliation category. Conversion ticks still mean conversion;
    /// this only narrows which difference the audit is showing.
    sirv_scope: Option<SirvScope>,
    /// Names in the paired listing that have no local file. They cannot use `visible`,
    /// whose indices deliberately refer only to immutable local entries.
    sirv_remote_only: Vec<String>,
    /// A snapshot from the last listing, patched by completed pulls. A file made
    /// by hand between listings stays stale until the next one, like the remote map.
    sirv_local_presence: HashSet<String>,
    /// A running or finished Sirv transfer, shown in the notices line.
    sirv_job: Option<SirvJob>,
    /// The destructive transfer awaiting its second click.
    sirv_confirm: Option<SirvJobKind>,
    /// Bumped whenever a running transfer stops being wanted.
    sirv_generation: u64,
    /// Bumped whenever a pairing changes, so an old paginated listing cannot
    /// land under a newly selected remote folder.
    sirv_pairing_generation: u64,
    /// Lets a superseded listing stop between pages instead of consuming the whole
    /// bounded request budget before its result is discarded.
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
    filter_input: gpui_kit::Entity<InputState>,
    /// Row the keyboard is on, as a position in `visible`.
    cursor: usize,
    /// High-rate key repeats update the cursor faster than a display can present.
    /// One next-frame callback draws the latest position instead of rebuilding
    /// the table once for every queued key event.
    cursor_redraw_pending: bool,
    /// Fixed end of a Shift range. Plain pointer or keyboard movement moves it;
    /// Shift movement leaves it in place while the cursor extends from it.
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
    /// The sample's decoded pixels, kept between runs. Only the encode depends on
    /// quality, and decoding the whole sample again for each one made dragging
    /// q80 → q60 → q80 decode 96 full images to answer a question about 32. Pruned
    /// to the sample being taken now, so it holds at most `sample_size` of them.
    estimate_decodes: SampledDecodes,
    /// Invalidates detached work when a new folder or file is installed.
    dataset_generation: u64,
    /// Invalidates older folder-open requests while a newer scan is pending.
    scan_generation: u64,
    /// The path currently being scanned, if any.
    scanning: Option<String>,
    /// Images the running tree walk has found so far. `None` for a one-level read,
    /// which is over before a count would help.
    scan_found: Option<usize>,
    /// The cancellable folder scan currently allowed to publish into this audit.
    scan_cancellation: Option<ScanCancellation>,
    /// Walk the whole tree when a folder opens, as `press audit` does. Off, the
    /// window reads one level and the tree is how you reach the rest. This is the
    /// chip; `dataset_subfolders` is the list, and the two agree except while a
    /// walk is running.
    include_subfolders: bool,
    /// Whether the installed rows came from a tree walk. Labels and sort order
    /// follow this, not the chip, so a cancelled walk cannot relabel the list.
    dataset_subfolders: bool,
    /// The list is one file opened straight into its comparison. Changing scope
    /// there would replace the file with its whole folder.
    single_file: bool,
    /// Keyboard target. Without one the window gets no key events at all.
    focus: FocusHandle,
    /// Last title pushed to the compositor, so render does not set it every frame.
    titled: String,
    /// Last state render asked to store, so render only schedules a write when it
    /// changes.
    settings: settings::Settings,
    /// Ordered, serialized persistence for the remembered state. The writer
    /// carries the injected path tests point at a temp file; production uses
    /// the real config path, so a render never touches it in tests.
    settings_writer: Arc<settings::SettingsWriter>,
    /// The newest debounced snapshot and its revision. Each change claims a
    /// newer revision and replaces this, so a whole resize drag needs one task
    /// and one write, and an older task can never land after a flush.
    pending_settings: Option<(u64, settings::Settings)>,
    /// The last full-resolution preview or pair, kept so reopening it is instant.
    // ponytail: one entry. A pair holds two full-size RGBA buffers — 165 MB for a
    // 5568x3712 photo — so a bigger cache would need a byte budget, not a count.
    cached: Option<(compare::Key, CachedMedia)>,
    /// The media for the file the arrow key is about to ask for, built while you
    /// look at the current one. `PREFETCH_BUDGET` bounds this second slot.
    ahead: Option<(compare::Key, CachedMedia)>,
    /// Which way the arrows last stepped, so the media built ahead is the one the
    /// sweep wants rather than the one behind it.
    compare_step: isize,
    /// The build running ahead of the cursor. Holding the task lets a replacement
    /// cancel it during its settle before it reaches the encoder.
    prefetch: Option<gpui_kit::Task<()>>,
    /// Identity of that build. Navigation can adopt an in-flight decode instead of
    /// cancelling it and starting the same file again.
    prefetch_key: Option<(compare::Key, MediaMode)>,
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
    /// Files outside the one marketplace preflight Press can prove from headers.
    marketplace: usize,
    /// Numbered sequences found once per scan, ready or with a named issue.
    spins: Vec<acquisition::SpinSet>,
    /// Copy/publish acknowledgements stay visible until the dataset or results change.
    report_copied: bool,
    published_results: Vec<String>,
    published_spins: Vec<String>,
    /// The visible part of a non-empty selection. Cached because the output panel
    /// is rebuilt by cursor, thumbnail and comparison interaction.
    selected_target_count: usize,
    selected_target_bytes: u64,
    /// The list, which the component library owns. It holds a weak handle back to
    /// this audit and reads its rows through that, so it cannot be built until this
    /// audit is a live entity: `TableState::new` asks the delegate for its row and
    /// column counts straight away, and answering that means reading the audit.
    table: Option<gpui_kit::Entity<TableState<AuditTable>>>,
    /// Width/preferences/result/Sirv signature last handed to the component table.
    table_signature: Option<(u32, ColumnPrefs, bool, bool)>,
    /// Which optional columns the picker has on.
    column_prefs: ColumnPrefs,
    /// Where conversions and local-model results are written.
    output: Output,
    /// The open rail, if any. A folder opens on Convert: it is the app's job,
    /// and an empty right-hand edge on launch would hide it.
    rail: Rail,
    /// The keyboard shortcut list, open over the workspace like settings.
    shortcuts_open: bool,
    /// The notices row spills past one line. Collapsed, it holds its height
    /// and the list below never moves; expanded on request for the full names.
    notices_expanded: bool,
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
fn compare_entries(
    a: &Entry,
    b: &Entry,
    sort: Sort,
    a_name: &str,
    b_name: &str,
    a_folded: &str,
    b_folded: &str,
) -> std::cmp::Ordering {
    let ordering = match sort.column {
        // Folded once per refresh by the caller: folding here would redo Unicode
        // lowercase per comparison, once per pair the sort touches.
        Column::Name => a_folded.cmp(b_folded),
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
    .then_with(|| a_name.cmp(b_name));

    if sort.descending {
        ordering.reverse()
    } else {
        ordering
    }
}

/// A paired Sirv folder. `files` maps the relative keys `sirv::relative_key`
/// produces for local rows onto the remote listing, so the diff column is a
/// lookup, never a walk. `None` while the listing is in flight —
/// a pairing that just happened does not know its diff yet.
/// What the paired folder's remote listing knows.
///
/// This was an `Option<HashMap<..>>`, so `None` meant both "the listing is running"
/// and "the listing failed". The window showed the first and reported the second as a pull
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
    /// Publish converted results under optimized/.
    Publish,
    /// Publish complete numbered sequences under press-spins/.
    Spin,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum SirvScope {
    OnlyLocal,
    Changed,
    OnlyRemote,
}

#[derive(Clone)]
enum UploadCompletion {
    None,
    Results(Vec<String>),
    Spins(Vec<String>),
}

struct SirvJob {
    kind: SirvJobKind,
    done: usize,
    total: usize,
    /// Total failures. Only the first few messages are retained below.
    failed: usize,
    failures: Vec<String>,
    /// The file currently crossing the network. `done` counts completed files.
    current: Option<String>,
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
    focus: gpui_kit::FocusHandle,
}

/// The settings overlay: the CDN credentials, and nothing else. Inputs are entities
/// so the framework owns their editing state.
struct SettingsPanel {
    client_id: gpui_kit::Entity<InputState>,
    client_secret: gpui_kit::Entity<InputState>,
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

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum MediaMode {
    Preview,
    Compare,
}

#[derive(Clone)]
enum CachedMedia {
    Preview(Arc<Preview>),
    Pair(Arc<Pair>),
}

impl CachedMedia {
    fn mode(&self) -> MediaMode {
        match self {
            Self::Preview(_) => MediaMode::Preview,
            Self::Pair(_) => MediaMode::Compare,
        }
    }
}

struct Comparison {
    index: usize,
    dataset_generation: u64,
    mode: MediaMode,
    /// Take focus once after the comparison tree exists. Re-focusing on every
    /// render steals keyboard ownership from the comparison buttons.
    focused: bool,
    key: compare::Key,
    /// The source-only image. It lands without running an encoder.
    preview: Option<Arc<Preview>>,
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
    /// The output being examined, when this is a finished result rather than a
    /// preview. Set means both sides came off disk and the bytes are real.
    written: Option<PathBuf>,
    /// The model that produced it. Its output is one file
    /// made on purpose, so it is offered for keeping or throwing away rather
    /// than filed silently and reported in a line of green text.
    produced_by: Option<ProducedBy>,
}

impl Comparison {
    fn dimensions(&self) -> Option<(u32, u32)> {
        self.pair
            .as_ref()
            .map(|pair| (pair.width, pair.height))
            .or_else(|| {
                self.preview
                    .as_ref()
                    .map(|preview| (preview.width, preview.height))
            })
    }
}

#[derive(Clone, Copy)]
enum ProducedBy {
    Local(local_ai::Tool),
    Studio(studio::Tool),
}

impl ProducedBy {
    fn result_label(self) -> &'static str {
        match self {
            Self::Local(local_ai::Tool::RemoveBackground) => "background removed",
            Self::Local(local_ai::Tool::Upscale) => "upscaled 4×",
            Self::Studio(tool) => tool.result_label(),
        }
    }
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

enum StudioJobState {
    Preparing,
    AwaitingConfirmation(studio::PreparedUpload),
    Running,
    Done(PathBuf),
    Failed(String),
}

struct StudioJob {
    tool: studio::Tool,
    index: usize,
    dataset_generation: u64,
    source_name: String,
    output_source: PathBuf,
    prompt: String,
    state: StudioJobState,
    cancelled: Arc<std::sync::atomic::AtomicBool>,
}

fn batch_root(paths: &[PathBuf]) -> Option<PathBuf> {
    let parent = paths.first()?.parent()?;
    paths
        .iter()
        .all(|path| path.parent() == Some(parent))
        .then(|| parent.to_path_buf())
}

fn root_needs_custom_output(root: &Path, output: &Output) -> bool {
    // Replace mode counts too: its backup mirror would be created at the
    // filesystem root, which is the same unwritable place for the same reason.
    root.has_root()
        && root.parent().is_none()
        && matches!(output, Output::Optimized | Output::Replace)
}

/// A path as the window names it: relative to the audited folder when it is
/// inside it, and whatever it is otherwise.
fn entry_name(root: &Path, path: &Path) -> String {
    path.strip_prefix(root)
        .unwrap_or(path)
        .to_string_lossy()
        .into_owned()
}

fn entry_label(root: &Path, show_parent: bool, entry: &Entry) -> String {
    entry_label_lossy(root, show_parent, entry).into_owned()
}

/// The same label without the owned copy. Filenames are valid UTF-8 in
/// practice, so this borrows instead of allocating; both the owned display
/// label and the folded sort key build on it.
fn entry_label_lossy<'a>(root: &Path, show_parent: bool, entry: &'a Entry) -> Cow<'a, str> {
    if show_parent {
        entry
            .path
            .strip_prefix(root)
            .unwrap_or(&entry.path)
            .to_string_lossy()
    } else {
        entry.name_lossy()
    }
}

/// The label the filter matches and the Name column sorts by, lowercased once.
/// `refresh_visible` builds one per entry and shares it between both, instead of
/// folding per row per keystroke and per pair per comparison.
fn folded_label(root: &Path, show_parent: bool, entry: &Entry) -> String {
    entry_label_lossy(root, show_parent, entry).to_lowercase()
}

fn navigation_path(path: PathBuf) -> PathBuf {
    if path.as_os_str().is_empty() {
        return path;
    }
    if let Ok(identity) = scan::canonical_boundary(&path) {
        return identity;
    }
    std::env::current_dir()
        .ok()
        .and_then(|base| crate::output::lexical_normalize_against(&path, &base).ok())
        .unwrap_or(path)
}

fn output_identity(output: &Output, root: &Path) -> PathBuf {
    navigation_path(output.root(root))
}

impl Audit {
    fn update_notifications(
        &self,
        update: impl FnOnce(&mut Window, &mut App) + 'static,
        cx: &mut Context<Self>,
    ) {
        let window = self.window;
        cx.spawn(async move |_, cx| {
            let _ = window.update(cx, |_, window, cx| {
                // Focused tests may render Audit without the production Root.
                if window
                    .root::<gpui_kit::component::Root>()
                    .flatten()
                    .is_some()
                {
                    update(window, cx);
                }
            });
        })
        .detach();
    }

    /// Keep the detailed failure beside the work that owns it, and announce it once
    /// where it cannot be missed. A scope replaces its previous toast so a failing
    /// batch never turns into one popup per file.
    pub(crate) fn notify_error(
        &self,
        scope: &'static str,
        title: impl Into<gpui_kit::SharedString>,
        message: impl Into<gpui_kit::SharedString>,
        cx: &mut Context<Self>,
    ) {
        self.notify_toast(NotificationType::Error, scope, title, message, cx);
    }

    /// The same toast for something that worked. An undo that names the files it
    /// put back is the only proof the user gets that it did.
    pub(crate) fn notify_success(
        &self,
        scope: &'static str,
        title: impl Into<gpui_kit::SharedString>,
        message: impl Into<gpui_kit::SharedString>,
        cx: &mut Context<Self>,
    ) {
        self.notify_toast(NotificationType::Success, scope, title, message, cx);
    }

    fn notify_toast(
        &self,
        kind: NotificationType,
        scope: &'static str,
        title: impl Into<gpui_kit::SharedString>,
        message: impl Into<gpui_kit::SharedString>,
        cx: &mut Context<Self>,
    ) {
        let failed = matches!(kind, NotificationType::Error);
        let message = message.into();
        let notification = Notification::new()
            .with_type(kind)
            .id1::<ErrorToast>(scope)
            .title(title)
            .content(move |_, _, _| {
                let selector = format!("error-toast-message:{message}");
                div()
                    .debug_selector(move || selector.clone())
                    .text_sm()
                    .child(message.clone())
                    .into_any_element()
            })
            // A failure waits to be read; a confirmation gets out of the way.
            .autohide(!failed);
        self.update_notifications(
            move |window, cx| window.push_notification(notification, cx),
            cx,
        );
    }

    /// A new attempt and a successful completion both make the previous failure
    /// stale. Remove only that operation's toast; unrelated failures stay visible.
    pub(crate) fn clear_error(&self, scope: &'static str, cx: &mut Context<Self>) {
        self.update_notifications(
            move |window, cx| window.remove_notification1::<ErrorToast>(scope, cx),
            cx,
        );
    }

    pub(super) fn scan_blocks_delivery(&self) -> bool {
        self.scanning.is_some()
    }

    /// Rows are named relative to the root whenever one list can hold files from
    /// more than one folder: a dropped batch, or a folder walked with its subfolders.
    /// The same label sorts the list, so what you read is what you sorted by.
    pub(super) fn show_parent(&self) -> bool {
        self.batch_folders.is_some() || self.dataset_subfolders
    }

    /// Flip the scope and read the current folder again under it. A dropped batch
    /// keeps its exact set; the choice applies to the next folder opened.
    pub(super) fn toggle_subfolders(&mut self, cx: &mut Context<Self>) {
        if self.converting || self.single_file {
            return;
        }
        self.include_subfolders = !self.include_subfolders;
        if self.batch_size.is_none() && self.root.is_dir() {
            self.request_folder(self.root.clone(), cx);
        } else {
            cx.notify();
        }
    }

    pub(super) fn cancel_retained_scan(&mut self) {
        if let Some(cancellation) = self.scan_cancellation.as_mut() {
            cancellation.cancel();
        }
    }

    /// Stop the running scan and keep what was on screen. The request is disowned
    /// before the token is raised: a completion racing the click then fails the
    /// ownership check, and no partial dataset is ever installed. The chip goes
    /// back to the scope the list still has, so it claims nothing the rows lack
    /// and the settings write remembers the truth.
    pub(super) fn cancel_scan(&mut self, cx: &mut Context<Self>) {
        self.scan_generation = self.scan_generation.wrapping_add(1);
        self.cancel_retained_scan();
        self.scan_cancellation = None;
        self.scanning = None;
        self.scan_found = None;
        self.include_subfolders = self.dataset_subfolders;
        cx.notify();
    }

    fn owns_scan_request(&self, request: u64, token: Option<&Arc<AtomicBool>>) -> bool {
        self.scan_generation == request
            && match token {
                Some(token) => self
                    .scan_cancellation
                    .as_ref()
                    .is_some_and(|current| Arc::ptr_eq(&current.token, token)),
                None => self.scan_cancellation.is_none(),
            }
    }

    /// Install a completed scan. This is the one state transition that replaces the
    /// dataset and invalidates every detached job derived from the old rows.
    fn install_dataset(
        &mut self,
        scanned: scan::Scan,
        root: PathBuf,
        single: bool,
        batch_size: Option<usize>,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let root_changed = self.root != root;
        self.dataset_generation = self.dataset_generation.wrapping_add(1);
        self.estimate_generation = self.estimate_generation.wrapping_add(1);
        self.estimate = None;
        self.estimate_decodes.lock().clear();
        self.converting = false;
        self.active_target_count = None;
        self.stopped_run = None;
        // The dataset guard already dropped a stale run's results; now the run
        // itself stops, rather than writing out the rest of a folder nobody is
        // looking at any more.
        if let Some(cancel) = self.convert_cancel.take() {
            cancel.store(true, Ordering::Release);
        }
        if let Some(job) = self.local_ai_job.take() {
            job.cancelled.store(true, Ordering::Relaxed);
        }
        if let Some(job) = self.studio_job.take() {
            job.cancelled.store(true, Ordering::Relaxed);
        }
        self.studio_source = None;
        self.root = root;
        self.browser_output_root = output_identity(&self.output, &self.root);
        self.batch_size = batch_size;
        self.batch_folders = None;
        self.dataset_subfolders = false;
        self.single_file = single;
        self.folders.clear();
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
        self.marketplace = scanned
            .entries
            .iter()
            .filter(|entry| acquisition::marketplace_fails(entry))
            .count();
        self.spins = acquisition::detect_spins(&self.root, &scanned.entries);
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
        self.skipped_heic = scanned.skipped_heic;
        self.skipped_packages = scanned.skipped_packages;
        self.unreadable = scanned.unreadable;
        self.walk_errors = scanned.walk_errors;
        self.existing_output = scanned.existing_output;
        // A new folder answers this question itself: the previous one's backups are
        // not something this one can put back. Reading the answer is a file read,
        // so it happens off the main thread and lands under its own generation.
        self.restorable = 0;
        let dataset = self.dataset_generation;
        let root = self.root.clone();
        cx.spawn(async move |this, cx| {
            let restorable = cx
                .background_executor()
                .spawn(async move { crate::manifest::restorable(&root) })
                .await;
            let _ = this.update(cx, |audit, cx| {
                if audit.dataset_generation == dataset {
                    audit.restorable = restorable;
                    cx.notify();
                }
            });
        })
        .detach();
        self.thumbs.clear();
        self.thumb_order.clear();
        self.requested.clear();
        self.thumb_queue.clear();
        self.selected.clear();
        self.marquee = None;
        self.selection_bounds.borrow_mut().clear();
        self.clear_results();
        self.completed_outputs.clear();
        self.compare = None;
        self.cached = None;
        self.ahead = None;
        self.compare_step = 1;
        self.prefetch = None;
        self.prefetch_key = None;
        self.report_copied = false;
        self.published_spins.clear();
        self.filter.clear();
        // A finding belongs to the folder it was found in. Carrying it over would show
        // the new folder narrowed to something nobody asked about.
        self.finding = None;
        // The notices belong to the last folder too, and an open dialog or an
        // expanded warning must not outlive the dataset it described.
        self.shortcuts_open = false;
        self.notices_expanded = false;
        self.filter_input.update(cx, |input, cx| {
            input.set_value("", window, cx);
        });
        self.cursor = 0;
        self.anchor = 0;
        self.refresh_visible();
        // A pairing maps one local root to one remote folder. A rescan of that root
        // keeps it; replacing the root retires it before the new rows can be pushed.
        if root_changed {
            // Replacing originals is a decision about one folder. Carrying it into
            // the next folder somebody opens would rewrite that one on a click
            // nobody made there, so the destination goes back to the default.
            if self.output == Output::Replace {
                self.output = Output::Optimized;
                self.browser_output_root = output_identity(&self.output, &self.root);
            }
            self.unpair_sirv(cx);
        } else {
            self.cancel_sirv_transfer();
            if let Some(pairing) = self.sirv_pairing.as_mut() {
                self.sirv_local_presence.clear();
                pairing.files = Listing::Walking;
                self.sirv_counts = None;
                self.sirv_remote_only.clear();
                self.refresh_visible();
                self.walk_sirv_pairing(cx);
            } else {
                self.refresh_sirv_counts();
            }
        }
        // Last, because retiring a pairing resets the Sirv scope and refreshes the
        // list again. Ticking before that would tick the rows a stale scope was
        // still hiding, and open the folder with nothing selected.
        self.select_all_visible();
        self.schedule_estimate(cx);
        cx.notify();

        if single {
            self.open_preview(0, cx);
        }
    }

    /// Open a requested folder or exact file away from the UI thread. A newer
    /// request wins, while a failed current request leaves the last usable dataset.
    pub(super) fn request_path(&mut self, path: PathBuf, cx: &mut Context<Self>) {
        if self.converting {
            return;
        }
        let path = navigation_path(path);
        if path.is_dir() {
            if root_needs_custom_output(&path, &self.output) {
                self.notify_error(
                    "open-image",
                    "Couldn’t open selection",
                    "Choose a custom output folder before opening an item from the filesystem root.",
                    cx,
                );
                return;
            }
            self.request_folder(path, cx);
            return;
        }
        if !path.is_file() {
            return;
        }
        let root = path.parent().unwrap_or(&path).to_path_buf();
        if root_needs_custom_output(&root, &self.output) {
            self.notify_error(
                "open-image",
                "Couldn’t open selection",
                "Choose a custom output folder before opening an item from the filesystem root.",
                cx,
            );
            return;
        }
        self.clear_error("open-image", cx);
        self.scan_generation = self.scan_generation.wrapping_add(1);
        let request = self.scan_generation;
        if let Some(cancellation) = self.scan_cancellation.as_mut() {
            cancellation.cancel();
        }
        self.scan_cancellation = None;
        self.scan_found = None;
        self.scanning = Some(
            path.file_name()
                .map(|name| name.to_string_lossy().into_owned())
                .unwrap_or_else(|| path.display().to_string()),
        );
        let requested = path.clone();
        cx.notify();

        cx.spawn(async move |this, cx| {
            let scan_path = path.clone();
            let (entry, folders) = cx
                .background_executor()
                .spawn(async move {
                    let folders = scan_path
                        .parent()
                        .and_then(|parent| scan::child_folders(parent).ok())
                        .unwrap_or_default();
                    (scan::probe(&scan_path), folders)
                })
                .await;
            let _ = this.update_in(cx, |audit, window, cx| {
                if !audit.owns_scan_request(request, None) {
                    return;
                }
                audit.scanning = None;
                if let Some(entry) = entry {
                    audit.install_dataset(
                        scan::Scan {
                            entries: vec![entry],
                            skipped_raw: 0,
                            skipped_heic: 0,
                            skipped_packages: 0,
                            unreadable: Vec::new(),
                            walk_errors: Vec::new(),
                            existing_output: 0,
                        },
                        root.clone(),
                        true,
                        None,
                        window,
                        cx,
                    );
                    audit.install_browser_page(root, folders, cx);
                } else {
                    audit.notify_error(
                        "open-image",
                        "Couldn’t open image",
                        format!(
                            "{} is damaged, unsupported, or not an image.",
                            requested.display()
                        ),
                        cx,
                    );
                }
                cx.notify();
            });
        })
        .detach();
    }

    fn request_folder(&mut self, path: PathBuf, cx: &mut Context<Self>) {
        self.clear_error("open-image", cx);
        self.scan_generation = self.scan_generation.wrapping_add(1);
        let request = self.scan_generation;
        if let Some(cancellation) = self.scan_cancellation.as_mut() {
            cancellation.cancel();
        }
        let cancellation = ScanCancellation::new();
        let token = cancellation.token.clone();
        self.scan_cancellation = Some(cancellation);
        self.scan_found = None;
        self.scanning = Some(
            path.file_name()
                .map(|name| name.to_string_lossy().into_owned())
                .unwrap_or_else(|| path.display().to_string()),
        );
        let output_root = self.output.root(&path);
        let requested = path.clone();
        let include_subfolders = self.include_subfolders;
        cx.notify();

        cx.spawn(async move |this, cx| {
            let browse_token = token.clone();
            let browsed = if include_subfolders {
                // The walker publishes from its own thread, so the count crosses to
                // the window through a channel; the sender goes away with the scan
                // and the loop below ends before the result is read.
                let (progress, mut found) = futures::channel::mpsc::unbounded();
                let scan = cx.background_executor().spawn(async move {
                    let mut count = 0;
                    scan::browse_tree_cancellable(&path, &output_root, browse_token, |batch| {
                        count += batch.len();
                        let _ = progress.unbounded_send(count);
                        std::ops::ControlFlow::Continue(())
                    })
                    .transpose()
                });
                while let Some(count) = found.next().await {
                    let shown = this.update(cx, |audit, cx| {
                        if audit.owns_scan_request(request, Some(&token)) {
                            audit.scan_found = Some(count);
                            cx.notify();
                        }
                    });
                    if shown.is_err() {
                        break;
                    }
                }
                scan.await
            } else {
                cx.background_executor()
                    .spawn(async move {
                        scan::browse_cancellable(&path, &output_root, &browse_token).transpose()
                    })
                    .await
            };
            let _ = this.update_in(cx, |audit, window, cx| {
                if !audit.owns_scan_request(request, Some(&token)) {
                    return;
                }
                // The count stays until the next request starts: it is only drawn
                // while `scanning` is set, and a completed walk leaves its total
                // where a test can read it.
                audit.scanning = None;
                audit.scan_cancellation = None;
                match browsed {
                    Some(Ok(browsed)) => {
                        audit.install_browse(browsed, requested, window, cx);
                        // Installed like `batch_folders`: the install clears batch
                        // context, and the scope that labelled these rows goes back
                        // before the list is sorted by it.
                        if include_subfolders {
                            audit.dataset_subfolders = true;
                            audit.refresh_visible();
                        }
                    }
                    Some(Err(error)) => {
                        audit.include_subfolders = audit.dataset_subfolders;
                        audit.notify_error(
                            "open-image",
                            "Couldn’t open folder",
                            format!("{}: {error}", requested.display()),
                            cx,
                        )
                    }
                    // Stopped from outside the button, so the chip is put back here.
                    None => audit.include_subfolders = audit.dataset_subfolders,
                }
                cx.notify();
            });
        })
        .detach();
    }

    /// Accept folders or an exact batch of files from the desktop's external-file
    /// drop event. Several folders share one parent so one output root remains safe.
    pub(super) fn request_paths(
        &mut self,
        mut paths: Vec<PathBuf>,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        if self.converting || paths.is_empty() {
            return;
        }
        paths = paths.into_iter().map(navigation_path).collect();
        paths.sort();
        paths.dedup();
        if paths.len() == 1 {
            let path = paths.pop().unwrap();
            if path.is_file() || path.is_dir() {
                self.request_path(path, cx);
            } else {
                window.push_notification(
                    Notification::warning("Drop one or more folders, or any number of images.")
                        .title("Couldn’t open selection"),
                    cx,
                );
            }
            return;
        }
        if paths.iter().all(|path| path.is_dir()) {
            let Some(root) = batch_root(&paths) else {
                window.push_notification(
                    Notification::warning("Choose folders from one parent at a time.")
                        .title("Couldn’t open selection"),
                    cx,
                );
                return;
            };
            if root_needs_custom_output(&root, &self.output) {
                window.push_notification(
                    Notification::warning(
                        "Choose a custom output folder before opening root-level folders.",
                    )
                    .title("Couldn’t open selection"),
                    cx,
                );
                return;
            }
            self.request_folders(paths, root, cx);
            return;
        }
        if !paths.iter().all(|path| path.is_file()) {
            window.push_notification(
                Notification::warning("Drop folders or images, not both.")
                    .title("Couldn’t open selection"),
                cx,
            );
            return;
        }
        let Some(root) = batch_root(&paths) else {
            window.push_notification(
                Notification::warning("Choose images from one folder at a time.")
                    .title("Couldn’t open selection"),
                cx,
            );
            return;
        };
        if root_needs_custom_output(&root, &self.output) {
            window.push_notification(
                Notification::warning(
                    "Choose a custom output folder before opening root-level images.",
                )
                .title("Couldn’t open selection"),
                cx,
            );
            return;
        }
        self.request_files(paths, root, cx);
    }

    fn request_folders(&mut self, paths: Vec<PathBuf>, root: PathBuf, cx: &mut Context<Self>) {
        self.scan_generation = self.scan_generation.wrapping_add(1);
        let request = self.scan_generation;
        if let Some(cancellation) = self.scan_cancellation.as_mut() {
            cancellation.cancel();
        }
        let cancellation = ScanCancellation::new();
        let token = cancellation.token.clone();
        self.scan_cancellation = Some(cancellation);
        self.scan_found = None;
        self.scanning = Some(format!("{} folders", paths.len()));
        let folder_count = paths.len();
        let output_root = self.output.root(&root);
        cx.notify();

        cx.spawn(async move |this, cx| {
            let scan_token = token.clone();
            let scanned = cx
                .background_executor()
                .spawn(async move {
                    scan::browse_folders_cancellable(&paths, &output_root, &scan_token)
                })
                .await;
            let _ = this.update_in(cx, |audit, window, cx| {
                if !audit.owns_scan_request(request, Some(&token)) {
                    return;
                }
                audit.scanning = None;
                audit.scan_cancellation = None;
                match scanned {
                    Ok(Some(scanned)) => {
                        let batch_size = scanned.entries.len();
                        audit.install_dataset(scanned, root, false, Some(batch_size), window, cx);
                        audit.batch_folders = Some(folder_count);
                        if audit.sirv_pairing.is_some() {
                            audit.unpair_sirv(cx);
                        }
                        audit.refresh_visible();
                        audit.browser_overlay = false;
                        cx.notify();
                    }
                    Ok(None) => {}
                    Err(error) => audit.notify_error(
                        "open-image",
                        "Couldn’t open folders",
                        error.to_string(),
                        cx,
                    ),
                }
            });
        })
        .detach();
    }

    fn request_files(&mut self, paths: Vec<PathBuf>, root: PathBuf, cx: &mut Context<Self>) {
        self.scan_generation = self.scan_generation.wrapping_add(1);
        let request = self.scan_generation;
        if let Some(cancellation) = self.scan_cancellation.as_mut() {
            cancellation.cancel();
        }
        self.scan_cancellation = None;
        self.scan_found = None;
        self.scanning = Some(format!("{} images", paths.len()));
        cx.notify();

        cx.spawn(async move |this, cx| {
            let scanned = cx
                .background_executor()
                .spawn(async move { scan::scan_files(&paths) })
                .await;
            let batch_size = scanned.entries.len();
            let _ = this.update_in(cx, |audit, window, cx| {
                if !audit.owns_scan_request(request, None) {
                    return;
                }
                audit.scanning = None;
                audit.install_dataset(scanned, root, false, Some(batch_size), window, cx);
            });
        })
        .detach();
    }

    /// Hand the output folder to the desktop's file manager.
    // ponytail: three names for one idea, and no crate needed for it.
    fn reveal_output(&self, cx: &mut Context<Self>) {
        self.clear_error("reveal-output", cx);
        let path = self.output.root(&self.root);
        if !path.exists() {
            self.notify_error(
                "reveal-output",
                "Output folder is unavailable",
                format!("{} does not exist yet.", path.display()),
                cx,
            );
            return;
        }
        self.reveal_path(&path, "Couldn’t show output folder", cx);
    }

    fn reveal_path(&self, path: &Path, title: &'static str, cx: &mut Context<Self>) {
        self.clear_error("reveal-path", cx);
        if let Err(error) = crate::reveal_path(path) {
            self.notify_error(
                "reveal-path",
                title,
                format!("{}: {error}", path.display()),
                cx,
            );
        }
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
                    audit.set_output(Output::Folder(path), cx);
                    window.refresh();
                });
            }
        })
        .detach();
    }

    pub(super) fn reset_output(&mut self, cx: &mut Context<Self>) {
        if root_needs_custom_output(&self.root, &Output::Optimized) {
            self.notify_error(
                "output",
                "Couldn’t reset output",
                "A filesystem-root selection requires a custom output folder.",
                cx,
            );
            return;
        }
        self.set_output(Output::Optimized, cx);
    }

    /// Convert in place. Opt-in, because it is the one destination that changes
    /// the folder you audited — and it still keeps every original, in the backup
    /// the Restore button reads.
    pub(super) fn use_replace_output(&mut self, cx: &mut Context<Self>) {
        if self.converting {
            return;
        }
        if root_needs_custom_output(&self.root, &Output::Replace) {
            self.notify_error(
                "output",
                "Couldn’t replace in place",
                "A filesystem-root selection requires a custom output folder.",
                cx,
            );
            return;
        }
        self.set_output(Output::Replace, cx);
    }

    /// Put back every original a replace run moved aside.
    ///
    /// The run record on disk is what makes this work after a restart, and what
    /// makes the report truthful: the files that came back are named, and so are
    /// the ones that could not.
    pub(super) fn restore_originals(&mut self, cx: &mut Context<Self>) {
        if self.converting || self.restorable == 0 {
            return;
        }
        self.clear_error("restore", cx);
        let root = self.root.clone();
        cx.spawn(async move |this, cx| {
            let (restored, restorable) = cx
                .background_executor()
                .spawn(async move {
                    let restored = crate::manifest::restore(&root);
                    let restorable = crate::manifest::restorable(&root);
                    (restored, restorable)
                })
                .await;
            let _ = this.update(cx, |audit, cx| {
                audit.restorable = restorable;
                if restored.failures.is_empty() {
                    audit.notify_success(
                        "restore",
                        "Originals restored",
                        format!(
                            "{} put back: {}",
                            restored.restored.len(),
                            named(
                                restored
                                    .restored
                                    .iter()
                                    .map(|path| entry_name(&audit.root, path))
                            )
                        ),
                        cx,
                    );
                } else {
                    audit.notify_error(
                        "restore",
                        "Some originals stayed put",
                        format!(
                            "{} put back; {} did not: {}",
                            restored.restored.len(),
                            restored.failures.len(),
                            named(restored.failures.iter().cloned())
                        ),
                        cx,
                    );
                }
                // The folder is not what the list is showing any more: the outputs
                // are gone and the originals are back under their own names.
                let root = audit.root.clone();
                audit.request_folder(root, cx);
                cx.notify();
            });
        })
        .detach();
    }

    /// Take the destination only if it can actually hold output. This refuses a folder
    /// that is or contains the audited one, which would re-encode originals onto
    /// themselves and report the loss as a saving. A folder *inside* the audited tree
    /// is still allowed — `optimized/` is one — so the run also refuses, per file, any
    /// output that would land on an audited original.
    fn set_output(&mut self, output: Output, cx: &mut Context<Self>) {
        self.clear_error("output", cx);
        if let Err(message) = output.context(&self.root) {
            self.notify_error(
                "output",
                "Couldn’t use that output folder",
                format!("{}: {message}", output.label()),
                cx,
            );
            return;
        }
        self.output = output;
        self.browser_output_root = output_identity(&self.output, &self.root);
        self.rebuild_tree(cx);
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
                        dialog.pick_folder().into_iter().collect()
                    } else {
                        dialog.pick_files().unwrap_or_default()
                    }
                })
                .await;

            if !chosen.is_empty() {
                let _ = this.update_in(cx, |audit, window, cx| {
                    audit.request_paths(chosen, window, cx);
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
    /// Objective marketplace file checks. Background colour remains a visual check.
    Marketplace,
    /// Rows the last run could not convert. The only finding about the run rather
    /// than the file, so a folder of 6,000 images with 40 failures has somewhere to
    /// put them instead of three names on a toast nobody kept.
    Failed,
}

impl Finding {
    /// The one place a finding about a file is decided, so the count in the toolbar,
    /// the filter it applies, and the chip in the row can never disagree. `Failed`
    /// is not one of those: it is keyed by row, and `refresh_visible` answers it
    /// from the failure map instead.
    pub(super) fn holds(self, entry: &Entry) -> bool {
        match self {
            Finding::Mislabelled => entry.extension_lies(),
            Finding::Heavy => is_heavy(entry),
            Finding::Marketplace => acquisition::marketplace_fails(entry),
            Finding::Failed => false,
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

/// One sample per slice of a weight-sorted list, taken from the middle of the slice:
/// the first file of a slice is its heaviest and the least like the rest of it.
///
/// Returns the position to sample and that whole slice's bytes, the sample included.
/// The window's estimate and a headless dry run both project from this, so the two
/// cannot quote numbers built from different samples.
pub(crate) fn strata(weights: &[u64], slices: usize) -> Vec<(usize, u64)> {
    (0..slices)
        .filter_map(|slice| {
            let start = slice * weights.len() / slices;
            let end = (slice + 1) * weights.len() / slices;
            let sample = (start + end) / 2;
            weights.get(sample)?;
            Some((sample, weights.get(start..end)?.iter().sum()))
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
pub(crate) fn project_total(slices: &[(u64, Option<(u64, u64)>)]) -> Option<(u64, usize)> {
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
    visible
        .iter()
        .copied()
        .filter(|index| selected.contains(index))
        .collect()
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
) -> gpui_kit::Entity<Audit> {
    let Launch {
        root,
        entries,
        skipped_raw,
        skipped_heic,
        skipped_packages,
        unreadable,
        walk_errors,
        existing_output,
        open_single,
        format,
        quality,
        max_edge,
        grid,
        recent_folders,
        columns: column_prefs,
        output,
        include_subfolders,
    } = launch;
    let root = navigation_path(root);
    let recent_folders = recent_folders
        .into_iter()
        .map(navigation_path)
        .collect::<Vec<_>>();
    let browser_output_root = output_identity(&output, &root);
    let restorable = crate::manifest::restorable(&root);

    let audit = cx.new(|cx| {
        let focus = cx.focus_handle();
        focus.focus(window, cx);

        let filter_input =
            cx.new(|cx| InputState::new(window, cx).placeholder("Filter by name (Ctrl+K)"));
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

        let studio_key = studio::load_key();
        let studio_key_input = cx.new(|cx| {
            InputState::new(window, cx)
                .masked(true)
                .placeholder("sk_live_…")
        });
        let studio_prompt =
            cx.new(|cx| InputState::new(window, cx).placeholder("Describe the result"));
        for input in [&studio_key_input, &studio_prompt] {
            cx.subscribe(input, |_, _, event: &InputEvent, cx| {
                if matches!(event, InputEvent::Change) {
                    cx.notify();
                }
            })
            .detach();
        }

        let quality_slider = cx.new(|_| {
            SliderState::new()
                .min(1.)
                .max(100.)
                .step(1.)
                .default_value(quality.0.unwrap_or(80.))
        });
        let max_edge_input = cx.new(|cx| {
            let typed = match max_edge {
                edge if MaxEdge::PRESETS.contains(&edge) => String::new(),
                MaxEdge(Some(edge)) => edge.to_string(),
                MaxEdge(None) => String::new(),
            };
            InputState::new(window, cx)
                .placeholder("Custom")
                .default_value(typed)
        });
        cx.subscribe(
            &max_edge_input,
            |audit: &mut Audit, input, event: &InputEvent, cx| {
                // On Enter or leaving the box, not every keystroke: "1600" typed
                // one digit at a time is not four sizes and four dropped results.
                if matches!(event, InputEvent::PressEnter { .. } | InputEvent::Blur) {
                    let value = input.read(cx).value().to_string();
                    audit.apply_custom_max_edge(&value, cx);
                }
            },
        )
        .detach();
        let folder_filter_input =
            cx.new(|cx| InputState::new(window, cx).placeholder("Search folders"));
        cx.subscribe(
            &folder_filter_input,
            |audit: &mut Audit, _, event: &InputEvent, cx| {
                if matches!(event, InputEvent::Change) {
                    audit.rebuild_tree(cx);
                    cx.notify();
                }
            },
        )
        .detach();
        let tree_state = cx.new(|cx| TreeState::new(cx));
        cx.subscribe(
            &tree_state,
            |audit: &mut Audit, _, event: &TreeEvent, cx| {
                audit.tree_event(event, cx);
            },
        )
        .detach();
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
        let marketplace = entries
            .iter()
            .filter(|entry| acquisition::marketplace_fails(entry))
            .count();
        let spins = acquisition::detect_spins(&root, &entries);
        let mut audit = Audit {
            window: window.window_handle(),
            table: None,
            table_signature: None,
            root,
            browser_output_root,
            batch_size: None,
            batch_folders: None,
            folders: Vec::new(),
            recent_folders,
            browser_overlay: false,
            folder_filter_input,
            tree_state,
            tree_anchor: PathBuf::new(),
            tree_children: HashMap::new(),
            tree_loaded: HashSet::new(),
            tree_loading: HashSet::new(),
            tree_expanded: HashSet::new(),
            tree_paths: HashMap::new(),
            entries,
            skipped_raw,
            skipped_heic,
            skipped_packages,
            heaviest: 0,
            visible_bytes: 0,
            heavy,
            mislabelled,
            marketplace,
            spins,
            report_copied: false,
            published_results: Vec::new(),
            published_spins: Vec::new(),
            studio_job: None,
            studio_tool: studio::Tool::default(),
            studio_key,
            studio_key_input,
            studio_prompt,
            studio_key_checking: false,
            studio_status: None,
            studio_source: None,
            selected_target_count: 0,
            selected_target_bytes: 0,
            thumbs: HashMap::new(),
            requested: HashSet::new(),
            thumb_queue: VecDeque::new(),
            thumb_inflight: 0,
            thumb_slow_inflight: 0,
            thumb_prefetch_pending: false,
            thumb_notify_pending: false,
            thumb_order: VecDeque::new(),
            format,
            quality,
            max_edge,
            max_edge_input,
            quality_slider,
            selected: HashSet::new(),
            selection_bounds: Rc::new(RefCell::new(HashMap::new())),
            selection_surface: Rc::new(Cell::new(gpui_kit::Bounds::default())),
            marquee: None,
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
            estimate_decodes: Arc::new(parking_lot::Mutex::new(HashMap::new())),
            dataset_generation: 0,
            scan_generation: 0,
            scanning: None,
            scan_found: None,
            scan_cancellation: None,
            settings: settings::Settings::default(),
            settings_writer: settings::SettingsWriter::new(settings::default_writer_path()),
            pending_settings: None,
            include_subfolders,
            dataset_subfolders: false,
            single_file: open_single,
            focus,
            titled: String::new(),
            cached: None,
            ahead: None,
            compare_step: 1,
            prefetch: None,
            prefetch_key: None,
            results: HashMap::new(),
            result_paths: HashMap::new(),
            completed_outputs: HashSet::new(),
            restorable,
            converted_totals: (0, 0),
            converting: false,
            convert_cancel: None,
            active_target_count: None,
            stopped_run: None,
            failures: HashMap::new(),
            failure_summary: String::new(),
            unreadable,
            walk_errors,
            existing_output,
            drag_over: false,
            sirv_pairing: None,
            sirv_counts: None,
            sirv_scope: None,
            sirv_remote_only: Vec::new(),
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
            rail: Rail::None,
            shortcuts_open: false,
            notices_expanded: false,
        };
        audit.refresh_visible();
        audit.select_all_visible();
        audit.schedule_estimate(cx);
        audit.seed_tree_for_current_folder(cx);
        if open_single {
            audit.open_preview(0, cx);
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

    audit.update(cx, |_, cx| {
        cx.on_release(|audit, _| {
            audit.cancel_retained_scan();
        })
        .detach();
    });
    let window_id = window.window_handle().window_id();
    let weak_audit = audit.downgrade();
    cx.on_window_closed(move |cx, closed_window_id| {
        if closed_window_id == window_id {
            let _ = weak_audit.update(cx, |audit, _| {
                audit.cancel_retained_scan();
            });
        }
    })
    .detach();

    audit
}

/// Flush the debounced settings write when the app quits, whatever path
/// the quit took — menu, Cmd+W on the last window, or the close button.
pub(crate) fn register_quit_flush(audit: gpui_kit::Entity<Audit>, cx: &mut gpui_kit::App) {
    cx.on_app_quit(move |cx| {
        // Quit cannot reliably be cancelled, so a failed flush never stops it:
        let outcome = audit.update(cx, |audit, _| audit.flush_settings());
        match outcome {
            settings::WriteOutcome::Failed { error, .. } => {
                eprintln!("press: could not save settings while quitting: {error}");
                crate::crash::note_diagnostic(&format!(
                    "settings flush failed while quitting: {error}"
                ));
            }
            settings::WriteOutcome::Written {
                warning: Some(warning),
                ..
            } => {
                eprintln!("press: {warning}");
            }
            _ => {}
        }
        async {}
    })
    .detach();
}
