//! Press — audit and convert a folder of images locally.
//!
//! The browser tools on imageguide.dev post files to a worker to convert them. This
//! does the same work locally, so auditing and conversion send nothing away and the
//! folder size is bounded by the disk rather than by a tab. Files move over the
//! network only when the user explicitly uses optional Sirv or Studio actions.

mod assets;
mod audit;
mod avif;
mod compare;
mod convert;
mod crash;
mod jxl;
mod local_ai;
mod manifest;
mod menus;
mod output;
mod scan;
mod settings;
mod sirv;
mod studio;
mod thumbs;
#[cfg(feature = "updater")]
mod update;

use std::collections::HashMap;
use std::path::{Path, PathBuf};

use convert::{Format, MaxEdge, Quality};
use gpui_kit::component::{ActiveTheme, Root};
use gpui_kit::platform::application;
use gpui_kit::{
    App, Bounds, Window, WindowBounds, WindowHandle, WindowOptions, prelude::*, px, size,
};
use scan::{Entry, format_bytes};
use serde::Serialize;

/// Write `text` to `out`. `Ok(false)` means the reader closed the pipe.
fn write_text(out: &mut impl std::io::Write, text: &str) -> std::io::Result<bool> {
    match out.write_all(text.as_bytes()) {
        Ok(()) => Ok(true),
        Err(error) if error.kind() == std::io::ErrorKind::BrokenPipe => Ok(false),
        Err(error) => Err(error),
    }
}

/// Every line the CLI writes goes through here.
///
/// `press audit big-folder | head` closes the pipe while Press is still writing, and
/// `println!` panics on that. The panic hook then files a crash report and the next
/// launch offers to send it, over a shell pipeline that worked. A reader that left is
/// the end of the run, not a crash, so it exits 0 and the hook stays installed for
/// everything that really is one.
fn print_text(text: &str) {
    let stdout = std::io::stdout();
    let mut out = stdout.lock();
    match write_text(&mut out, text) {
        Ok(true) => {}
        Ok(false) => std::process::exit(0),
        Err(error) => {
            eprintln!("press: could not write to stdout: {error}");
            std::process::exit(1);
        }
    }
}

/// `println!` for the CLI, through the guard above.
macro_rules! outln {
    ($($argument:tt)*) => {
        crate::print_text(&format!("{}\n", format_args!($($argument)*)))
    };
}

/// The smallest compositor window that supports every production view.
const WINDOW_MIN_WIDTH: f32 = 760.;
const WINDOW_MIN_HEIGHT: f32 = 560.;
const WINDOW_DEFAULT_WIDTH: f32 = 900.;
const WINDOW_DEFAULT_HEIGHT: f32 = 640.;

const HELP: &str = concat!(
    "Press ",
    env!("CARGO_PKG_VERSION"),
    " — audit and optimise images locally\n\n",
    "Usage:\n",
    "  press [PATH] [OPTIONS]\n",
    "  press audit <PATH> [--json]\n",
    "  press convert <PATH> [OPTIONS]\n",
    "  press restore <PATH>\n",
    "  press skill\n",
    "  press update\n\n",
    "Commands:\n",
    "  audit      Read image headers without opening a window or writing files\n",
    "  convert    Re-encode a file or folder into optimized/ without a window\n",
    "  restore    Put back the originals a --replace run moved aside\n",
    "  skill      Print the bundled Agent Skill to stdout\n",
    "  update     Install the latest signed Press release\n",
    "  help       Print this help\n",
    "  version    Print the version\n\n",
    "Options:\n",
    "  --json                    Write one schema-versioned JSON document\n",
    "  --no-subfolders           Read the folder itself, not the folders below it\n",
    "  --format <webp|avif|jxl|jpeg|same>\n",
    "                            Output format (default: webp); same keeps each\n",
    "                            source's own format and name\n",
    "  --jpeg                    Same as --format jpeg\n",
    "  --quality <1..100>        Lossy quality (default: 80)\n",
    "  --lossless                Lossless WebP or JPEG XL\n",
    "  --max-edge <pixels>       Downscale the longest edge; never upscale\n",
    "  --replace                 Convert in place; originals move to press-originals/\n",
    "  -o, --output <dir>        Write converted files here instead of optimized/\n",
    "  --skip-existing           Skip a source whose output already matches this\n",
    "                            format, quality and max edge\n",
    "  --dry-run                 Plan and project a conversion, write nothing\n",
    "  --avif-speed <0..10>      libaom speed for AVIF output (default: 6);\n",
    "                            higher is faster and slightly larger\n",
    "  --grid                    Open the window in gallery view\n",
    "  -h, --help                Print this help\n",
    "  -V, --version             Print the version\n\n",
    "Compatibility:\n",
    "  --webp, --avif, --jxl and PATH --convert remain supported.\n\n",
    "Exit status:\n",
    "  0  Complete success\n",
    "  1  The requested operation failed or was incomplete\n",
    "  2  Invalid invocation or target\n\n",
    "With --json, stdout contains only JSON; diagnostics go to stderr.\n",
    "audit and convert walk subfolders unless --no-subfolders is given, and say so.\n"
);

const AGENT_SKILL: &str = include_str!("../.agents/skills/press-cli/SKILL.md");

/// A persisted size can be absent or corrupted. Keep restore policy pure so native
/// startup and tests agree about the supported window.
fn restored_window_size(width: Option<f32>, height: Option<f32>) -> (f32, f32) {
    let width = width
        .filter(|value| value.is_finite())
        .unwrap_or(WINDOW_DEFAULT_WIDTH)
        .max(WINDOW_MIN_WIDTH);
    let height = height
        .filter(|value| value.is_finite())
        .unwrap_or(WINDOW_DEFAULT_HEIGHT)
        .max(WINDOW_MIN_HEIGHT);
    (width, height)
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum Command {
    Window,
    Audit,
    Convert,
    Restore,
    Skill,
    Update,
    Help,
    Version,
}

struct Args {
    /// `None` when launched with no path: the window opens on its empty state.
    root: Option<PathBuf>,
    command: Command,
    format: Format,
    quality: Quality,
    max_edge: MaxEdge,
    /// `None` leaves the AVIF speed to the settings file, then to the default.
    avif_speed: Option<u8>,
    grid: bool,
    json: bool,
    /// Headless scope. The window has its own remembered chip for this.
    subfolders: bool,
    replace: bool,
    /// Where `convert` writes. `None` is the default `optimized/` beside the sources.
    /// The window has its own remembered Output setting.
    output: Option<PathBuf>,
    /// Leave a source alone when its planned output is already current.
    skip_existing: bool,
    /// Plan and project the conversion without writing anything.
    dry_run: bool,
    unknown: Vec<String>,
}

fn parse_args() -> Args {
    match parse_args_from(std::env::args().skip(1)) {
        Ok(args) => args,
        Err(message) => {
            eprintln!("press: {message}");
            std::process::exit(2);
        }
    }
}

/// Pop the value for `flag`, refusing to eat the next flag by accident.
/// `press convert photos --output --replace` used to create a folder named
/// `--replace`; a value starting with `-` is always that mistake, never a path.
fn next_value(
    rest: &mut impl Iterator<Item = String>,
    flag: &str,
    need: &str,
) -> Result<String, String> {
    match rest.next() {
        Some(value) if !value.is_empty() && !value.starts_with('-') => Ok(value),
        Some(got) => Err(format!("{flag} needs {need}, got {got:?}")),
        None => Err(format!("{flag} needs {need}")),
    }
}

fn parse_args_from(mut rest: impl Iterator<Item = String>) -> Result<Args, String> {
    let mut root = None;
    let mut command = Command::Window;
    let mut format = Format::WebP;
    let mut quality = Quality::lossy(80.);
    let mut max_edge = MaxEdge::FULL;
    let mut avif_speed = None;
    let mut grid = false;
    let mut json = false;
    let mut subfolders = true;
    let mut replace = false;
    let mut output = None;
    let mut skip_existing = false;
    let mut dry_run = false;
    let mut unknown = Vec::new();
    let mut conversion_option = false;

    while let Some(argument) = rest.next() {
        match argument.as_str() {
            "audit" if root.is_none() && command == Command::Window => command = Command::Audit,
            "convert" if root.is_none() && command == Command::Window => command = Command::Convert,
            "restore" if root.is_none() && command == Command::Window => command = Command::Restore,
            "skill" if root.is_none() && command == Command::Window => command = Command::Skill,
            "update" if root.is_none() && command == Command::Window => command = Command::Update,
            "help" if root.is_none() && command == Command::Window => command = Command::Help,
            "version" if root.is_none() && command == Command::Window => command = Command::Version,
            "--convert" => select_command(&mut command, Command::Convert, "--convert")?,
            "--audit" => select_command(&mut command, Command::Audit, "--audit")?,
            "--format" => {
                conversion_option = true;
                let value = next_value(&mut rest, "--format", "webp, avif, jxl, jpeg, or same")?;
                format = match value.as_str() {
                    "webp" => Format::WebP,
                    "avif" => Format::Avif,
                    "jxl" => Format::JpegXl,
                    "jpeg" | "jpg" => Format::Jpeg,
                    "same" => Format::Same,
                    _ => {
                        return Err(format!(
                            "--format needs webp, avif, jxl, jpeg, or same, got {value:?}"
                        ));
                    }
                };
            }
            "--avif" => {
                conversion_option = true;
                format = Format::Avif;
            }
            "--jxl" => {
                conversion_option = true;
                format = Format::JpegXl;
            }
            "--jpeg" => {
                conversion_option = true;
                format = Format::Jpeg;
            }
            "--max-edge" => {
                conversion_option = true;
                let value = next_value(&mut rest, "--max-edge", "a positive number")?;
                let edge = value
                    .parse()
                    .map_err(|_| format!("--max-edge needs a number, got {value:?}"))?;
                if edge == 0 {
                    return Err(format!("--max-edge needs a positive number, got {value:?}"));
                }
                max_edge = MaxEdge(Some(edge));
            }
            "-o" | "--output" => {
                conversion_option = true;
                let value = next_value(&mut rest, "--output", "a folder")?;
                output = Some(PathBuf::from(value));
            }
            "--avif-speed" => {
                conversion_option = true;
                let value = next_value(&mut rest, "--avif-speed", "a number from 0 to 10")?;
                let speed: u8 = value
                    .parse()
                    .map_err(|_| format!("--avif-speed needs a number, got {value:?}"))?;
                if speed > 10 {
                    return Err(format!("--avif-speed needs 0 to 10, got {value:?}"));
                }
                avif_speed = Some(speed);
            }
            "--webp" => {
                conversion_option = true;
                format = Format::WebP;
            }
            "--replace" => {
                conversion_option = true;
                replace = true;
            }
            "--skip-existing" => {
                conversion_option = true;
                skip_existing = true;
            }
            "--dry-run" => {
                conversion_option = true;
                dry_run = true;
            }
            "--grid" => grid = true,
            "--json" => json = true,
            "--no-subfolders" => subfolders = false,
            "--lossless" => {
                conversion_option = true;
                quality = Quality::LOSSLESS;
            }
            "--quality" => {
                conversion_option = true;
                let value = next_value(&mut rest, "--quality", "a number from 1 to 100")?;
                let quality_value: f32 = value
                    .parse()
                    .map_err(|_| format!("--quality needs a number, got {value:?}"))?;
                if !quality_value.is_finite() {
                    return Err(format!("--quality needs a finite number, got {value:?}"));
                }
                if !(1. ..=100.).contains(&quality_value) {
                    return Err(format!("--quality needs 1 to 100, got {value:?}"));
                }
                quality = Quality::lossy(quality_value);
            }
            "-h" | "--help" => command = Command::Help,
            "-V" | "--version" => command = Command::Version,
            "--" => {
                for argument in rest {
                    set_root(&mut root, argument)?;
                }
                break;
            }
            _ if argument.starts_with('-') => unknown.push(argument),
            _ => set_root(&mut root, argument)?,
        }
    }

    if matches!(command, Command::Help | Command::Version) {
        return Ok(Args {
            root,
            command,
            format,
            quality,
            max_edge,
            avif_speed,
            grid,
            json,
            subfolders,
            replace,
            output,
            skip_existing,
            dry_run,
            unknown,
        });
    }
    if command == Command::Restore && (conversion_option || grid) {
        return Err("restore takes only a folder".into());
    }
    if matches!(command, Command::Skill | Command::Update)
        && (root.is_some()
            || conversion_option
            || grid
            || json
            || !subfolders
            || !unknown.is_empty())
    {
        return Err(format!(
            "{} takes no arguments",
            if command == Command::Skill {
                "skill"
            } else {
                "update"
            }
        ));
    }
    if command == Command::Audit && conversion_option {
        return Err("audit does not accept conversion options".into());
    }
    if command == Command::Audit && grid {
        return Err("--grid is available only for the window".into());
    }
    if command == Command::Convert && grid {
        return Err("--grid is available only for the window".into());
    }
    if json && !matches!(command, Command::Audit | Command::Convert) {
        return Err("--json needs audit or convert".into());
    }
    if command == Command::Window && !subfolders {
        return Err(
            "--no-subfolders needs audit or convert; the window has a Subfolders chip".into(),
        );
    }
    if command == Command::Window && output.is_some() {
        return Err("--output needs convert; the window has its own Output setting".into());
    }
    if command == Command::Window && skip_existing {
        return Err("--skip-existing needs convert".into());
    }
    if command == Command::Window && dry_run {
        return Err("--dry-run needs convert".into());
    }
    if !format.supports_lossless() && quality == Quality::LOSSLESS {
        return Err("--lossless is available only with --webp or --jxl".into());
    }
    if replace && command != Command::Convert {
        return Err("--replace needs convert".into());
    }
    if replace && output.is_some() {
        return Err("--replace writes beside each source; it takes no --output".into());
    }

    Ok(Args {
        root,
        command,
        format,
        quality,
        max_edge,
        avif_speed,
        grid,
        json,
        subfolders,
        replace,
        output,
        skip_existing,
        dry_run,
        unknown,
    })
}

fn select_command(command: &mut Command, selected: Command, flag: &str) -> Result<(), String> {
    if *command != Command::Window && *command != selected {
        return Err(format!("{flag} conflicts with the selected command"));
    }
    *command = selected;
    Ok(())
}

fn set_root(root: &mut Option<PathBuf>, value: String) -> Result<(), String> {
    if root.is_some() {
        return Err("only one file or folder may be given".into());
    }
    *root = Some(PathBuf::from(value));
    Ok(())
}

#[derive(Serialize)]
struct ScanSummary {
    images: usize,
    bytes: u64,
    heavy: usize,
    mislabelled: usize,
    camera_raw_skipped: usize,
    heic_skipped: usize,
    macos_packages_skipped: usize,
    unreadable: usize,
    walk_errors: usize,
    existing_outputs: usize,
}

impl ScanSummary {
    fn from_scan(scanned: &scan::Scan) -> Self {
        Self {
            images: scanned.entries.len(),
            bytes: scanned.entries.iter().map(|entry| entry.bytes).sum(),
            heavy: scanned
                .entries
                .iter()
                .filter(|entry| audit::is_heavy(entry))
                .count(),
            mislabelled: scanned
                .entries
                .iter()
                .filter(|entry| entry.extension_lies())
                .count(),
            camera_raw_skipped: scanned.skipped_raw,
            heic_skipped: scanned.skipped_heic,
            macos_packages_skipped: scanned.skipped_packages,
            unreadable: scanned.unreadable.len(),
            walk_errors: scanned.walk_errors.len(),
            existing_outputs: scanned.existing_output,
        }
    }
}

#[derive(Serialize)]
struct AuditFile {
    path: String,
    format: String,
    width: u32,
    height: u32,
    bytes: u64,
    bytes_per_pixel: f32,
    heavy: bool,
    mislabelled: bool,
}

#[derive(Serialize)]
struct AuditReport {
    schema_version: u32,
    command: &'static str,
    target: String,
    /// Whether the walk went below the target. The window reads one level by
    /// default, so a report has to say which job it describes. `null` for one
    /// file, which has no scope, rather than a `false` that reads as a choice.
    subfolders: Option<bool>,
    summary: ScanSummary,
    files: Vec<AuditFile>,
    unreadable: Vec<String>,
    walk_errors: Vec<String>,
}

fn path_text(path: &Path) -> String {
    path.to_string_lossy().into_owned()
}

fn sorted_paths(paths: &[PathBuf]) -> Vec<String> {
    let mut paths: Vec<String> = paths.iter().map(|path| path_text(path)).collect();
    paths.sort();
    paths
}

fn audit_files(entries: &[Entry]) -> Vec<AuditFile> {
    let mut files: Vec<_> = entries
        .iter()
        .map(|entry| AuditFile {
            path: path_text(&entry.path),
            format: scan::format_name(entry.format)
                .to_ascii_lowercase()
                .replace(' ', "_"),
            width: entry.width,
            height: entry.height,
            bytes: entry.bytes,
            bytes_per_pixel: entry.bytes_per_pixel(),
            heavy: audit::is_heavy(entry),
            mislabelled: entry.extension_lies(),
        })
        .collect();
    files.sort_by(|left, right| left.path.cmp(&right.path));
    files
}

fn audit_report(target: &Path, scanned: &scan::Scan, subfolders: Option<bool>) -> AuditReport {
    AuditReport {
        schema_version: 1,
        command: "audit",
        target: path_text(target),
        subfolders,
        summary: ScanSummary::from_scan(scanned),
        files: audit_files(&scanned.entries),
        unreadable: sorted_paths(&scanned.unreadable),
        walk_errors: sorted_paths(&scanned.walk_errors),
    }
}

fn write_json(value: &impl Serialize) -> Result<(), String> {
    let document = serde_json::to_string(value).map_err(|error| error.to_string())?;
    print_text(&format!("{document}\n"));
    Ok(())
}

/// The scope, said on the summary line of both text outputs. A single file has
/// no scope to state.
fn scope_note(subfolders: Option<bool>) -> &'static str {
    match subfolders {
        Some(true) => ", subfolders included",
        Some(false) => ", subfolders excluded",
        None => "",
    }
}

fn print_audit(target: &Path, scanned: &scan::Scan, subfolders: Option<bool>) {
    let summary = ScanSummary::from_scan(scanned);
    outln!(
        "{} images, {} on disk, {} heavy, {} mislabelled{}",
        summary.images,
        format_bytes(summary.bytes),
        summary.heavy,
        summary.mislabelled,
        scope_note(subfolders)
    );
    // A phone folder is all HEIC, so the line above is four zeroes and the report
    // reads as a failure. The skipped count is the finding there.
    if summary.heic_skipped > 0 {
        outln!("{} HEIC skipped (not supported yet)", summary.heic_skipped);
    }
    for entry in &scanned.entries {
        let relative = entry.path.strip_prefix(target).unwrap_or(&entry.path);
        let mut findings = Vec::new();
        if audit::is_heavy(entry) {
            findings.push("heavy");
        }
        if entry.extension_lies() {
            findings.push("mislabelled");
        }
        outln!(
            "{:<52} {:<8} {:>5}x{:<5} {:>9}  {:>5.2} B/px  {}",
            relative.display(),
            scan::format_name(entry.format),
            entry.width,
            entry.height,
            format_bytes(entry.bytes),
            entry.bytes_per_pixel(),
            findings.join(", ")
        );
    }
}

fn print_scan_errors(scanned: &scan::Scan) {
    for path in &scanned.unreadable {
        eprintln!("press: could not read image {}", path.display());
    }
    for path in &scanned.walk_errors {
        eprintln!("press: could not enter {}", path.display());
    }
}

#[derive(Serialize)]
struct ConversionFile {
    source: String,
    status: &'static str,
    output: Option<String>,
    /// Where the plan sent this file, whether or not it was written. A skipped or
    /// dry-run file has no `output` of its own run to report, and this is the name a
    /// caller needs to find it.
    planned_output: Option<String>,
    source_bytes: u64,
    output_bytes: Option<u64>,
    width: Option<u32>,
    height: Option<u32>,
    error: Option<String>,
    skipped: bool,
    /// Why the file was skipped. Errors and skips are named, not counted.
    reason: Option<String>,
}

struct ConversionRun {
    before: u64,
    after: u64,
    failed: usize,
    files: Vec<ConversionFile>,
    /// The run record on disk, or `None` when it could not be written. Replace
    /// mode's undo reads it, so a report that named one that is not there would
    /// be promising something.
    written_manifest: Option<PathBuf>,
    /// What a dry run projects the whole queue would write, and how many real encodes
    /// stand behind that number. `None` on a run that actually wrote.
    projected: Option<(u64, usize)>,
}

#[derive(Serialize)]
struct ConversionOptions {
    format: &'static str,
    quality: Option<f32>,
    max_edge: Option<u32>,
}

#[derive(Serialize)]
struct ConversionSummary {
    attempted: usize,
    converted: usize,
    failed: usize,
    /// Sources left alone by `--skip-existing`. Never counted as converted or failed.
    skipped: usize,
    source_bytes: u64,
    output_bytes: u64,
    grew: bool,
    changed_bytes: u64,
    /// What a `--dry-run` projects the whole run would write, and how many real
    /// encodes stand behind that number. `null` on a run that actually wrote.
    projected_bytes: Option<u64>,
    projected_samples: Option<usize>,
}

#[derive(Serialize)]
struct ConversionReport {
    schema_version: u32,
    command: &'static str,
    target: String,
    output: String,
    /// Whether this run planned only. A dry run writes nothing and reports what it
    /// would have written.
    dry_run: bool,
    subfolders: Option<bool>,
    options: ConversionOptions,
    scan: ScanSummary,
    summary: ConversionSummary,
    manifest: Option<String>,
    backup: Option<String>,
    files: Vec<ConversionFile>,
    unreadable: Vec<String>,
    walk_errors: Vec<String>,
}

/// The folder the walk treats as this run's output.
///
/// A chosen `--output` under the audited folder holds what the last run wrote, and a
/// walk that does not skip it audits its own output and offers it back for
/// conversion. An output anywhere else is outside the walk already, and the default
/// boundary stands so `optimized/` keeps being skipped and counted.
fn walk_output(target: &Path, chosen: Option<&Path>) -> PathBuf {
    let default = target.join(scan::OUTPUT_DIR);
    let Some(chosen) = chosen else {
        return default;
    };
    let (Ok(root), Ok(output)) = (
        scan::canonical_boundary(target),
        scan::canonical_boundary(chosen),
    ) else {
        return default;
    };
    // An output that is the audited folder is refused when the context is established,
    // with a reason. Handing it to the walk here would empty the audit first and the
    // refusal would arrive with nothing left to refuse.
    if output != root && output.starts_with(&root) {
        output
    } else {
        default
    }
}

/// The counts a run only mentions when they happened. Zero skipped and zero failed
/// is the ordinary case and does not need saying.
fn run_tail(skipped: usize, failed: usize) -> String {
    let mut tail = String::new();
    if skipped > 0 {
        tail.push_str(&format!(", {skipped} skipped"));
    }
    if failed > 0 {
        tail.push_str(&format!(", {failed} failed"));
    }
    tail
}

/// The size already on disk when `written` is not older than `source`, or `None` when
/// it is missing, stale, or unreadable.
///
/// A build step re-runs over a tree that mostly has not changed, and re-encoding a
/// file whose output already answers for it buys nothing but the time. Modification
/// time is the comparison a build step already trusts for everything else.
fn current_output_bytes(source: &Path, written: &Path) -> Option<u64> {
    let output = std::fs::metadata(written).ok()?;
    let source = std::fs::metadata(source).ok()?;
    (output.modified().ok()? >= source.modified().ok()?).then_some(output.len())
}

/// A planned run split into what still has to be converted and what is already there.
struct Queued<'a> {
    entries: Vec<&'a Entry>,
    planned: Vec<Result<PathBuf, convert::Failure>>,
    skipped: Vec<ConversionFile>,
}

/// Decide, per file and after planning, which sources `--skip-existing` leaves alone.
///
/// The plan covers every audited source, so dropping a file here cannot hand its name
/// to a sibling that would otherwise have collided with it: the next run, with
/// nothing to skip, plans the same names again.
///
/// A current output is skipped only when the manifest shows a run wrote it at
/// these settings. Records store the requested format label, so `--format same`
/// compares as `same` against `same`: per-file resolution happens at encode
/// time and is never recorded. An output no record names keeps the old
/// mtime-only skip, and one a record names at other settings is rebuilt.
#[allow(clippy::too_many_arguments)]
fn queue_run<'a>(
    entries: &[&'a Entry],
    planned: &[Result<PathBuf, convert::Failure>],
    skip_existing: bool,
    json: bool,
    manifest: &manifest::Manifest,
    root: &Path,
    out_dir: &Path,
    format: Format,
    quality: Quality,
    max_edge: MaxEdge,
) -> Queued<'a> {
    debug_assert_eq!(entries.len(), planned.len(), "one plan per audited source");
    let wanted = (format.label().to_string(), quality.label(), max_edge.0);
    let matched = format!(
        "the output already matches {} {} {}",
        format.label(),
        quality.label(),
        max_edge.label()
    );
    let mut queued = Queued {
        entries: Vec::with_capacity(entries.len()),
        planned: Vec::with_capacity(entries.len()),
        skipped: Vec::new(),
    };
    for (entry, plan) in entries.iter().zip(planned) {
        let current = if skip_existing {
            plan.as_deref().ok().and_then(|written| {
                let bytes = current_output_bytes(&entry.path, written)?;
                Some((written, bytes))
            })
        } else {
            None
        };
        let Some((written, bytes)) = current else {
            queued.entries.push(entry);
            queued.planned.push(plan.clone());
            continue;
        };
        // A record at other settings is a stale file at this run's settings, so
        // only a matching record skips. An edited file is not the run's file at
        // all: rebuilding would eat the edit, so it keeps the legacy skip.
        let record = relative_pair(root, out_dir, &entry.path, written)
            .and_then(|(source, output)| manifest.latest(&source, &output));
        let (matches, stale) = match record {
            Some(record) if record.installed(written) => {
                let same = record.format == wanted.0
                    && record.quality == wanted.1
                    && record.max_edge == wanted.2;
                (same, !same)
            }
            _ => (false, false),
        };
        if stale {
            queued.entries.push(entry);
            queued.planned.push(plan.clone());
            continue;
        }
        let reason = if matches {
            matched.clone()
        } else {
            "the output is not older than the source".to_string()
        };
        let written = plan.as_deref().ok().map(path_text);
        if !json {
            outln!(
                "{:<52} {:>9}  skipped, {}",
                entry.name(),
                format_bytes(bytes),
                reason
            );
        }
        queued.skipped.push(ConversionFile {
            source: path_text(&entry.path),
            status: "skipped",
            output: written.clone(),
            planned_output: written,
            source_bytes: entry.bytes,
            output_bytes: Some(bytes),
            width: None,
            height: None,
            error: None,
            skipped: true,
            reason: Some(reason),
        });
    }
    queued
}

/// This source and output as manifest-relative paths, when both sit under the
/// roots the run was planned against.
fn relative_pair(
    root: &Path,
    out_dir: &Path,
    source: &Path,
    written: &Path,
) -> Option<(PathBuf, PathBuf)> {
    Some((
        source.strip_prefix(root).ok()?.to_path_buf(),
        written.strip_prefix(out_dir).ok()?.to_path_buf(),
    ))
}

/// Project what a run would write, by encoding a sample of it in memory.
///
/// The same sampling and the same projection the window's estimate uses, so a dry run
/// and the window cannot quote two different numbers for one folder. Nothing is
/// written: the samples are encoded and their lengths thrown away.
fn project_run(
    entries: &[&Entry],
    format: Format,
    quality: Quality,
    max_edge: MaxEdge,
) -> Option<(u64, usize)> {
    // `strata` reads a weight-sorted list: it takes one sample from the middle of each
    // slice and `project_total` scales that whole slice by the sample's ratio. In walk
    // order a slice mixes a 40MB photo with three thumbnails, and whichever one is
    // sampled speaks for bytes it has nothing to do with. The window sorts by weight
    // before it estimates; so does this, rather than trusting the caller's order.
    let mut entries: Vec<&Entry> = entries.to_vec();
    entries.sort_by_key(|entry| std::cmp::Reverse(entry.bytes));
    let weights: Vec<u64> = entries.iter().map(|entry| entry.bytes).collect();
    let slices = audit::sample_size(format).min(weights.len());
    let jobs = audit::strata(&weights, slices);
    // The window encodes its samples together, as many at once as a conversion
    // allows; a serial dry run made `--dry-run` feel like a conversion on
    // folders the window sizes in under a second. Indexed slots keep slice
    let sampled = &parking_lot::Mutex::new(vec![None; jobs.len()]);
    let next = &std::sync::atomic::AtomicUsize::new(0);
    let jobs = &jobs;
    let entries = &entries;
    let threads = convert::workers(format).min(jobs.len().max(1));
    std::thread::scope(|scope| {
        for _ in 0..threads {
            scope.spawn(move || {
                loop {
                    let job = next.fetch_add(1, std::sync::atomic::Ordering::Relaxed);
                    let Some(&(sample, slice_bytes)) = jobs.get(job) else {
                        return;
                    };
                    let entry = entries[sample];
                    let encoded = scan::decode_for_conversion(&entry.path, max_edge)
                        .ok()
                        .zip(format.resolve(&entry.path).ok())
                        .and_then(|((image, profile), format)| {
                            convert::encode(
                                &max_edge.apply(image),
                                format,
                                quality,
                                profile.as_deref(),
                            )
                            .ok()
                        })
                        .map(|encoded| encoded.len() as u64);
                    sampled.lock()[job] =
                        Some((slice_bytes, encoded.map(|encoded| (entry.bytes, encoded))));
                }
            });
        }
    });
    let sampled: Vec<(u64, Option<(u64, u64)>)> = sampled
        .lock()
        .iter()
        .map(|slot| slot.expect("every sample was encoded"))
        .collect();
    audit::project_total(&sampled)
}

/// Say what a conversion would do and what it would cost, without writing anything.
///
/// The plan is the real one, so the names reported here are the names a run would
/// write; a source whose plan was refused is named with its reason rather than
/// counted. `Format::resolve` runs here too, which is what catches `--format same`
/// over a container with no encoder and over a file whose extension lies.
///
/// The refusals a dry run cannot make are the ones that need the pixels: JPEG over a
/// source with real transparency, an animated GIF, PNG, WebP or JPEG XL, and a
/// lossless request over a depth the format cannot keep. Those still fail on the real
/// run, so a clean dry run is not a promise that every file converts.
fn dry_run_headless(
    queued: &Queued,
    format: Format,
    quality: Quality,
    max_edge: MaxEdge,
    json: bool,
) -> ConversionRun {
    let mut run = ConversionRun {
        before: 0,
        after: 0,
        failed: 0,
        files: Vec::with_capacity(queued.entries.len()),
        written_manifest: None,
        projected: None,
    };
    let mut writable: Vec<&Entry> = Vec::new();
    for (entry, plan) in queued.entries.iter().zip(&queued.planned) {
        // A file the encoder would refuse by name is a failure a dry run can see, and
        // reporting it as "planned" would send a caller off to look for an output that
        // was never going to exist.
        let resolved = plan
            .clone()
            .and_then(|written| format.resolve(&entry.path).map(|_| written));
        let (status, planned_output, error) = match resolved {
            Ok(written) => {
                run.before += entry.bytes;
                writable.push(entry);
                if !json {
                    outln!(
                        "{:<52} {:>9} -> {}",
                        entry.name(),
                        format_bytes(entry.bytes),
                        written.display()
                    );
                }
                ("planned", Some(path_text(&written)), None)
            }
            Err(failure) => {
                run.failed += 1;
                let reason = failure.reason();
                if !json {
                    outln!(
                        "{:<52} failed{}",
                        entry.name(),
                        reason
                            .as_deref()
                            .map(|reason| format!(": {reason}"))
                            .unwrap_or_default()
                    );
                }
                (
                    "failed",
                    None,
                    Some(reason.unwrap_or_else(|| "conversion failed".to_string())),
                )
            }
        };
        run.files.push(ConversionFile {
            source: path_text(&entry.path),
            status,
            output: None,
            planned_output,
            source_bytes: entry.bytes,
            output_bytes: None,
            width: None,
            height: None,
            error,
            skipped: false,
            reason: None,
        });
    }
    run.projected = project_run(&writable, format, quality, max_edge);
    run.files
        .sort_by(|left, right| left.source.cmp(&right.source));

    if !json {
        let projection = match run.projected {
            Some((bytes, samples)) => format!(
                "projected {} from {samples} {} ({:+.0}%)",
                format_bytes(bytes),
                if samples == 1 { "sample" } else { "samples" },
                (bytes as f64 - run.before as f64) / run.before.max(1) as f64 * 100.
            ),
            None => "nothing encoded, so nothing to project".to_string(),
        };
        outln!(
            "\n{} to convert to {} at {} ({}): {}, {projection}{}",
            writable.len(),
            format.label(),
            quality.label(),
            max_edge.label(),
            format_bytes(run.before),
            run_tail(queued.skipped.len(), run.failed)
        );
        outln!("nothing written (dry run)");
    }
    run
}

/// Convert without opening a window, so the same work is scriptable and testable.
///
/// Only `queued.entries` are converted, each to the name `queue_run` kept for it. The
/// plan was made against every audited source, so a file left out of the queue still
/// holds the name it was given.
fn convert_headless(
    root: &std::path::Path,
    queued: &Queued,
    destination: &convert::Destination,
    format: Format,
    quality: Quality,
    max_edge: MaxEdge,
    json: bool,
) -> ConversionRun {
    let out_dir = destination.out_dir;
    let entries = &queued.entries;
    let sources: Vec<PathBuf> = entries.iter().map(|entry| entry.path.clone()).collect();
    let by_path: HashMap<&Path, (&Entry, Option<&Path>)> = entries
        .iter()
        .zip(&queued.planned)
        .map(|(entry, plan)| (entry.path.as_path(), (*entry, plan.as_deref().ok())))
        .collect();

    // Lines arrive as files finish rather than in list order, which is what running
    // several at once looks like. The totals are the same either way.
    let totals = parking_lot::Mutex::new(ConversionRun {
        before: 0,
        after: 0,
        failed: 0,
        files: Vec::with_capacity(entries.len()),
        written_manifest: None,
        projected: None,
    });
    convert::convert_each(
        root,
        &sources,
        &queued.planned,
        destination,
        format,
        quality,
        max_edge,
        |source, converted| {
            let Some((entry, planned)) = by_path.get(source) else {
                return;
            };
            let mut totals = totals.lock();
            match converted {
                Ok(converted) => {
                    totals.before += entry.bytes;
                    totals.after += converted.bytes;
                    let delta = entry.bytes as i64 - converted.bytes as i64;
                    let percent = delta as f64 / entry.bytes.max(1) as f64 * 100.;
                    let resized = if converted.width == entry.width {
                        String::new()
                    } else {
                        format!("  {}x{}", converted.width, converted.height)
                    };
                    if !json {
                        outln!(
                            "{:<52} {:>9} -> {:>9}  {percent:+.0}%{resized}",
                            entry.name(),
                            format_bytes(entry.bytes),
                            format_bytes(converted.bytes)
                        );
                    }
                    totals.files.push(ConversionFile {
                        source: path_text(source),
                        status: "converted",
                        output: Some(path_text(&converted.written)),
                        planned_output: planned.map(path_text),
                        source_bytes: entry.bytes,
                        output_bytes: Some(converted.bytes),
                        width: Some(converted.width),
                        height: Some(converted.height),
                        error: None,
                        skipped: false,
                        reason: None,
                    });
                }
                Err(error) => {
                    totals.failed += 1;
                    let reason = error
                        .reason()
                        .map(|reason| format!(": {reason}"))
                        .unwrap_or_default();
                    if !json {
                        outln!("{:<52} failed{reason}", entry.name());
                    }
                    totals.files.push(ConversionFile {
                        source: path_text(source),
                        status: "failed",
                        output: None,
                        planned_output: planned.map(path_text),
                        source_bytes: entry.bytes,
                        output_bytes: None,
                        width: None,
                        height: None,
                        error: Some(
                            error
                                .reason()
                                .unwrap_or_else(|| "conversion failed".to_string()),
                        ),
                        skipped: false,
                        reason: None,
                    });
                }
            }
        },
    );
    let mut totals = totals.into_inner();
    totals
        .files
        .sort_by(|left, right| left.source.cmp(&right.source));
    // Each file appended its own line as it landed; this only names the file.
    let record = manifest::path(out_dir);
    totals.written_manifest = record.is_file().then_some(record);

    if !json {
        let growth = totals.after > totals.before;
        let delta = totals.before.abs_diff(totals.after);
        let percent = delta as f64 / totals.before.max(1) as f64 * 100.;
        outln!(
            "\n{} converted to {} at {} ({}): {} -> {}, {} {} ({percent:.0}%){}",
            entries.len() - totals.failed,
            format.label(),
            quality.label(),
            max_edge.label(),
            format_bytes(totals.before),
            format_bytes(totals.after),
            if growth { "grew" } else { "saved" },
            format_bytes(delta),
            run_tail(queued.skipped.len(), totals.failed)
        );
        if entries.len() - totals.failed > 0 {
            outln!("written to {}", out_dir.display());
            if let Some(backups) = destination.backups {
                outln!(
                    "originals moved to {}; press restore {} puts them back",
                    backups.display(),
                    root.display()
                );
            }
        }
    }
    totals
}

fn main() {
    let pending_crash = crash::pending_snapshot();
    crash::install();
    let args = parse_args();

    match args.command {
        Command::Help => {
            print_text(HELP);
            return;
        }
        Command::Version => {
            outln!("press {}", env!("CARGO_PKG_VERSION"));
            return;
        }
        Command::Skill => {
            print_text(AGENT_SKILL);
            return;
        }
        Command::Update => {
            update_headless();
            return;
        }
        Command::Window | Command::Audit | Command::Convert | Command::Restore => {}
    }

    if args.command == Command::Window {
        for argument in &args.unknown {
            eprintln!("press: ignoring unknown option {argument}");
        }
    } else if let Some(first) = args.unknown.first() {
        eprintln!("press: unknown option {first}");
        std::process::exit(2);
    }

    // Headless commands do not inherit window state. An agent should get the same
    // audit and output location on every machine, independent of what its user last
    // clicked in the app.
    let remembered = if args.command == Command::Window {
        settings::load()
    } else {
        settings::Settings::default()
    };
    // Set before anything can encode. The flag wins over the settings file, and a
    // window launched with the flag writes it back, so the choice sticks.
    avif::set_speed(
        args.avif_speed
            .or(remembered.avif_speed)
            .unwrap_or(avif::DEFAULT_SPEED),
    );

    let target = args.root.clone().or_else(|| {
        remembered
            .folder
            .clone()
            .filter(|folder| folder.is_dir() && args.command == Command::Window)
    });

    let Some(target) = target else {
        if args.command != Command::Window {
            eprintln!(
                "press: {} needs a file or folder",
                match args.command {
                    Command::Audit => "audit",
                    Command::Restore => "restore",
                    _ => "convert",
                }
            );
            std::process::exit(2);
        }
        // No path given: open the window on its empty state and let the user pick.
        return run_window(
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
                format: args.format,
                quality: args.quality,
                max_edge: args.max_edge,
                grid: args.grid,
                recent_folders: remembered.recent_folders.clone(),
                columns: remembered.columns,
                output: remembered.output.clone(),
                include_subfolders: remembered.include_subfolders,
                sidebar_open: !remembered.sidebar_collapsed,
            },
            None,
            pending_crash,
        );
    };

    if args.command == Command::Restore {
        if !target.is_dir() {
            eprintln!("press: {} is not a folder", target.display());
            std::process::exit(2);
        }
        std::process::exit(restore_headless(&target));
    }

    // A single file opens straight into the comparison. A folder opens the audit.
    let open_single = target.is_file();
    if !target.is_dir() && !open_single {
        eprintln!("press: {} is not a file or folder", target.display());
        std::process::exit(2);
    }

    if args.command == Command::Window {
        return run_window(
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
                format: args.format,
                quality: args.quality,
                max_edge: args.max_edge,
                grid: args.grid,
                recent_folders: remembered.recent_folders.clone(),
                columns: remembered.columns,
                output: remembered.output.clone(),
                include_subfolders: remembered.include_subfolders,
                sidebar_open: !remembered.sidebar_collapsed,
            },
            Some(target),
            pending_crash,
        );
    }

    let (scanned, root) = if open_single {
        let parent = target.parent().unwrap_or(Path::new(".")).to_path_buf();
        let Some(entry) = scan::probe(&target) else {
            eprintln!("press: {} is not an image", target.display());
            std::process::exit(2);
        };
        (
            scan::Scan {
                entries: vec![entry],
                skipped_raw: 0,
                skipped_heic: 0,
                skipped_packages: 0,
                unreadable: Vec::new(),
                walk_errors: Vec::new(),
                existing_output: 0,
            },
            parent,
        )
    } else if args.subfolders {
        let output = walk_output(&target, args.output.as_deref());
        (scan::scan(&target, &output), target.clone())
    } else {
        // The one-level read names files by their canonical root, so the root
        // follows it or the listing would lose its relative spelling.
        let output = walk_output(&target, args.output.as_deref());
        match scan::browse(&target, &output) {
            Ok(browsed) => (
                browsed.scan,
                scan::canonical_boundary(&target).unwrap_or_else(|_| target.clone()),
            ),
            Err(error) => {
                eprintln!("press: {}: {error}", target.display());
                std::process::exit(2);
            }
        }
    };
    print_scan_errors(&scanned);
    let scope = (!open_single).then_some(args.subfolders);
    let unread = scanned.unreadable.len() + scanned.walk_errors.len();

    if args.command == Command::Audit {
        let written = if args.json {
            write_json(&audit_report(&target, &scanned, scope))
        } else {
            print_audit(&root, &scanned, scope);
            Ok(())
        };
        if let Err(error) = written {
            eprintln!("press: could not write JSON: {error}");
            std::process::exit(1);
        }
        std::process::exit(if unread == 0 { 0 } else { 1 });
    }

    if !args.json {
        outln!(
            "{} images, {} on disk, {} camera raw skipped{}",
            scanned.entries.len(),
            format_bytes(scanned.entries.iter().map(|entry| entry.bytes).sum()),
            scanned.skipped_raw,
            scope_note(scope)
        );
        if scanned.skipped_heic > 0 {
            outln!("{} HEIC skipped (not supported yet)", scanned.skipped_heic);
        }
        match scanned.skipped_packages {
            0 => {}
            1 => outln!("1 macOS package skipped"),
            many => outln!("{many} macOS packages skipped"),
        }
    }
    // Headless writes the default `optimized/` unless `--replace` asks for the folder
    // itself or `--output` names another one. A chosen destination goes through the
    // boundary the window establishes, so the same refusals apply — an output that is
    // or contains the source, a symlinked final component. Refusing here names the
    // reason once instead of failing every file, and the report carries the canonical
    // root rather than what was typed.
    let destination = if args.replace {
        settings::Output::Replace
    } else {
        match args.output.clone() {
            Some(folder) => settings::Output::Folder(folder),
            None => settings::Output::Optimized,
        }
    };
    let context = match destination.context(&root) {
        Ok(context) => context,
        Err(message) => {
            eprintln!("press: {message}");
            std::process::exit(2);
        }
    };
    let out_dir = context.output_root().to_path_buf();
    let backups = args.replace.then(|| manifest::backup_root(&out_dir));
    let recorded = manifest::load(&out_dir);
    let destination = convert::Destination {
        out_dir: &out_dir,
        backups: backups.as_deref(),
        manifest: &recorded,
    };
    let audited: Vec<&Entry> = scanned.entries.iter().collect();
    let sources: Vec<PathBuf> = scanned
        .entries
        .iter()
        .map(|entry| entry.path.clone())
        .collect();
    // Plan against every audited source first. `--skip-existing` then decides one file
    // at a time, so the names a run leaves alone are the names it would have written.
    let planned = convert::plan_outputs(&root, &sources, &sources, &destination, args.format);
    let queued = queue_run(
        &audited,
        &planned,
        args.skip_existing,
        args.json,
        &recorded,
        &root,
        &out_dir,
        args.format,
        args.quality,
        args.max_edge,
    );
    let skipped = queued.skipped.len();

    let mut run = if args.dry_run {
        dry_run_headless(&queued, args.format, args.quality, args.max_edge, args.json)
    } else {
        convert_headless(
            &root,
            &queued,
            &destination,
            args.format,
            args.quality,
            args.max_edge,
            args.json,
        )
    };
    let failed = run.failed;
    let converted = if args.dry_run {
        0
    } else {
        queued.entries.len() - failed
    };
    // A dry run wrote nothing, so its output and change totals are zero. What it read
    // and what it projects are the numbers that mean anything.
    let (output_bytes, changed_bytes) = if args.dry_run {
        (0, 0)
    } else {
        (run.after, run.before.abs_diff(run.after))
    };
    run.files.extend(queued.skipped);
    run.files
        .sort_by(|left, right| left.source.cmp(&right.source));
    if args.json {
        let report = ConversionReport {
            schema_version: 1,
            command: "convert",
            target: path_text(&target),
            output: path_text(&out_dir),
            dry_run: args.dry_run,
            subfolders: scope,
            options: ConversionOptions {
                format: args.format.label(),
                quality: args.quality.0,
                max_edge: args.max_edge.0,
            },
            scan: ScanSummary::from_scan(&scanned),
            manifest: run.written_manifest.as_deref().map(path_text),
            backup: backups.as_deref().map(path_text),
            summary: ConversionSummary {
                attempted: scanned.entries.len(),
                converted,
                failed,
                skipped,
                source_bytes: run.before,
                output_bytes,
                grew: run.after > run.before,
                changed_bytes,
                projected_bytes: run.projected.map(|(bytes, _)| bytes),
                projected_samples: run.projected.map(|(_, samples)| samples),
            },
            files: run.files,
            unreadable: sorted_paths(&scanned.unreadable),
            walk_errors: sorted_paths(&scanned.walk_errors),
        };
        if let Err(error) = write_json(&report) {
            eprintln!("press: could not write JSON: {error}");
            std::process::exit(1);
        }
    }
    std::process::exit(if failed + unread == 0 { 0 } else { 1 });
}

/// Put back what a `--replace` run moved aside, and say what came back by name.
fn restore_headless(root: &Path) -> i32 {
    let restore = manifest::restore(root);
    if restore.restored.is_empty() && restore.failures.is_empty() {
        outln!("no originals to restore in {}", root.display());
        return 0;
    }
    for original in &restore.restored {
        outln!("restored {}", original.display());
    }
    for failure in &restore.failures {
        eprintln!("press: could not restore {failure}");
    }
    outln!(
        "{} restored, {} left in place",
        restore.restored.len(),
        restore.failures.len()
    );
    i32::from(!restore.failures.is_empty())
}

fn update_headless() {
    #[cfg(feature = "updater")]
    match update::install() {
        update::Outcome::Installed => {
            outln!("Press updated; start it again to use the new version")
        }
        update::Outcome::Current => outln!("Press {} is up to date", env!("CARGO_PKG_VERSION")),
        update::Outcome::Unsupported => {
            eprintln!(
                "press: this installation cannot self-update; use its package manager or replace it manually"
            );
            std::process::exit(1);
        }
        update::Outcome::Failed => std::process::exit(1),
    }

    #[cfg(not(feature = "updater"))]
    {
        eprintln!("press: this build has no self-updater; update it with its package manager");
        std::process::exit(1);
    }
}

fn reveal_path(path: &Path) -> std::io::Result<()> {
    open_with_desktop(path)
}

fn open_with_desktop(target: impl AsRef<std::ffi::OsStr>) -> std::io::Result<()> {
    let opener = if cfg!(target_os = "macos") {
        "open"
    } else if cfg!(target_os = "windows") {
        "explorer"
    } else {
        "xdg-open"
    };
    std::process::Command::new(opener)
        .arg(target)
        .spawn()
        .map(|_| ())
}

/// Build the audit view for a window. Shared by the app and the screenshot harness
/// so that what gets captured is the thing that ships.
fn init_theme(cx: &mut App) {
    // Must run before any component type is constructed.
    gpui_kit::component::init(cx);
    // Dark by default. Judging compression against a bright chrome is a bad idea,
    // and the comparison view is full-bleed imagery either way.
    gpui_kit::component::Theme::change(gpui_kit::component::ThemeMode::Dark, None, cx);
    // The stock dark theme paints `primary` white, which makes the one button that
    // commits work a white slab and leaves nothing to point at anything else. This
    // app already has a blue.
    //
    // Both halves of the theme have to be told. A button takes its fill from the
    // token set and its label from the colour set, so setting one and not the other
    // is how you get black text on a blue button.
    let theme = gpui_kit::component::Theme::global_mut(cx);
    // One neutral ramp, barely blue, so the imagery in the table carries the
    // colour and the chrome reads as an instrument panel rather than a website.
    // Flat and hairline-precise: surfaces are separated by a line or a stripe,
    // never by a gradient or a shadow, which is what a pro Mac app looks like.
    let background = gpui_kit::Hsla::from(gpui_kit::rgb(0x0b0e14));
    let surface = gpui_kit::Hsla::from(gpui_kit::rgb(0x121926));
    let table = gpui_kit::Hsla::from(gpui_kit::rgb(0x0b0e14));
    // Every other row. The zebra does the work the row hairlines used to.
    let stripe = gpui_kit::Hsla::from(gpui_kit::rgb(0x0d111a));
    let border = gpui_kit::Hsla::from(gpui_kit::rgb(0x1b2331));
    let foreground = gpui_kit::Hsla::from(gpui_kit::rgb(0xe6ecf4));
    let muted = gpui_kit::Hsla::from(gpui_kit::rgb(0x8b98ab));
    let base = gpui_kit::Hsla::from(gpui_kit::rgb(0x4c8dff));
    let hover = gpui_kit::Hsla::from(gpui_kit::rgb(0x65a0ff));
    let active = gpui_kit::Hsla::from(gpui_kit::rgb(0x3b79e6));
    let focus = gpui_kit::Hsla::from(gpui_kit::rgb(0x8fbcff));

    theme.background = background;
    theme.secondary = surface;
    theme.table = table;
    theme.input = border;
    theme.border = border;
    theme.foreground = foreground;
    theme.muted_foreground = muted;
    theme.group_box = surface;
    theme.group_box_foreground = foreground;
    theme.list_hover = gpui_kit::Hsla::from(gpui_kit::rgb(0x141c28));
    theme.list_active = gpui_kit::Hsla::from(gpui_kit::rgb(0x16233a));
    theme.list_active_border = base;
    theme.table_head = gpui_kit::Hsla::from(gpui_kit::rgb(0x10151d));
    theme.table_head_foreground = muted;
    theme.table_hover = gpui_kit::Hsla::from(gpui_kit::rgb(0x131a25));
    // The stripe separates the rows, so a hairline under every one of them was
    // the same edge drawn twice.
    theme.table_row_border = gpui_kit::transparent_black();
    theme.table_even = stripe;
    theme.ring = focus;

    // The table draws itself from the token set, not the colour set, so the
    // list stayed near-black under a blue-grey zebra until these were told too.
    theme.tokens.table = table.into();
    theme.tokens.table_head = gpui_kit::Hsla::from(gpui_kit::rgb(0x10151d)).into();
    theme.tokens.table_even = stripe.into();
    theme.tokens.table_hover = gpui_kit::Hsla::from(gpui_kit::rgb(0x131a25)).into();
    // Translucent on purpose: the table paints its selected row as an overlay
    // ON TOP of that row's cells, so an opaque colour here does not tint the
    // row, it hides it — tick, thumbnail, name and all.
    theme.tokens.table_active = gpui_kit::Hsla::from(gpui_kit::rgba(0x4c8dff1f)).into();
    theme.tokens.table_active_border = base.into();
    theme.tokens.table_row_border = gpui_kit::transparent_black().into();

    theme.primary = base;
    theme.primary_hover = hover;
    theme.primary_active = active;
    theme.primary_foreground = gpui_kit::white();
    theme.button_primary = base;
    theme.button_primary_hover = hover;
    theme.button_primary_active = active;
    theme.button_primary_foreground = gpui_kit::white();

    theme.tokens.button_primary = base.into();
    theme.tokens.button_primary_hover = hover.into();
    theme.tokens.button_primary_active = active.into();
    theme.tokens.button_primary_foreground = gpui_kit::white().into();

    // The two findings share one amber, and saved bytes are the one green.
    theme.yellow = gpui_kit::Hsla::from(gpui_kit::rgb(0xe0b054));
    theme.green = gpui_kit::Hsla::from(gpui_kit::rgb(0x4ade80));

    // SF Pro Text for words, Fira Code for every measured number. A column of
    // byte counts in a proportional face will not align down its right edge,
    // and an audit that will not align is not an audit.
    theme.font_family = "SF Pro Text".into();
    theme.mono_font_family = "Fira Code".into();
    theme.mono_font_size = px(12.);
}

/// Everything the window needs to open. A struct rather than nine positional
/// arguments, three of which are `usize` and two of which are `bool`.
struct Launch {
    root: PathBuf,
    entries: Vec<Entry>,
    skipped_raw: usize,
    skipped_heic: usize,
    skipped_packages: usize,
    unreadable: Vec<PathBuf>,
    walk_errors: Vec<PathBuf>,
    existing_output: usize,
    open_single: bool,
    format: Format,
    quality: Quality,
    max_edge: MaxEdge,
    grid: bool,
    recent_folders: Vec<PathBuf>,
    columns: settings::ColumnPrefs,
    output: settings::Output,
    include_subfolders: bool,
    sidebar_open: bool,
}

/// `Root` owns these overlays but leaves their placement to the app's content view.
struct WindowContent {
    audit: gpui_kit::Entity<audit::Audit>,
}

impl Render for WindowContent {
    fn render(
        &mut self,
        window: &mut gpui_kit::Window,
        cx: &mut gpui_kit::Context<Self>,
    ) -> impl IntoElement {
        let sheet = Root::render_sheet_layer(window, cx);
        let dialog = Root::render_dialog_layer(window, cx);
        let notifications = Root::render_notification_layer(window, cx);
        gpui_kit::div()
            .relative()
            .size_full()
            .child(self.audit.clone())
            .children(sheet)
            .children(dialog)
            .children(notifications)
    }
}

fn run_window(launch: Launch, startup_path: Option<PathBuf>, pending_crash: Option<PathBuf>) {
    application()
        // Every `IconName` is an SVG loaded through the app's asset source. Without
        // this the icons resolve to nothing and the toolbar renders as bare words.
        .with_assets(assets::Assets)
        .run(move |cx: &mut App| {
            init_theme(cx);
            // GPUI's macOS default keeps the process alive after the last window
            // closes, which suits a document-based app with a menu bar to come back
            // through. This is a single-window tool: the red light means quit.
            cx.set_quit_mode(gpui_kit::QuitMode::LastWindowClosed);

            // The thumbnail cache grows with every folder ever opened. Bound it once,
            // here, where a whole-directory pass costs a thread nobody waits for.
            cx.background_executor()
                .spawn(async { thumbs::trim_cache() })
                .detach();

            let remembered = settings::load();
            let (width, height) = restored_window_size(remembered.width, remembered.height);
            let bounds = Bounds::centered(None, size(px(width), px(height)), cx);
            let mut audit_slot = None;
            let root = cx
                .open_window(
                    WindowOptions {
                        window_bounds: Some(WindowBounds::Windowed(bounds)),
                        window_min_size: Some(size(px(WINDOW_MIN_WIDTH), px(WINDOW_MIN_HEIGHT))),
                        // Matches the desktop entry cargo-packager derives from
                        // product-name; a mismatch loses the icon under Wayland.
                        app_id: Some("press".to_string()),
                        ..Default::default()
                    },
                    |window, cx| {
                        let audit = audit::build_audit(launch, window, cx);
                        audit_slot = Some(audit.clone());
                        // Root owns modal state; WindowContent paints its overlays.
                        let content = cx.new(|_| WindowContent { audit });
                        cx.new(|cx| Root::new(content, window, cx).bg(cx.theme().background))
                    },
                )
                .unwrap();
            if let Some(audit) = audit_slot {
                if let Some(path) = startup_path {
                    audit.update(cx, |audit, cx| audit.request_path(path, cx));
                }
                #[cfg(feature = "updater")]
                {
                    let audit = audit.clone();
                    cx.spawn(async move |cx| {
                        let installed = cx
                            .background_executor()
                            .spawn(async { update::install_if_available() })
                            .await;
                        if installed {
                            let restart = audit.update(cx, |audit, _| {
                                if !audit.automatic_update_can_restart() {
                                    return false;
                                }
                                // A failed flush aborts the restart: relaunching
                                // into a half-remembered window is worse than
                                // staying on the installed update.
                                match audit.flush_settings() {
                                    settings::WriteOutcome::Failed { error, .. } => {
                                        eprintln!(
                                            "press: installed update but settings would not save: {error}"
                                        );
                                        false
                                    }
                                    _ => true,
                                }
                            });
                            if restart {
                                match update::relaunch() {
                                    Ok(()) => {
                                        cx.update(|cx| cx.quit());
                                        return;
                                    }
                                    Err(error) => {
                                        eprintln!(
                                            "press: installed update but could not restart: {error}"
                                        );
                                        audit.update(cx, |audit, cx| {
                                            audit.notify_error(
                                                "update",
                                                "Update installed, but restart failed",
                                                format!(
                                                    "Restart Press manually to finish updating: {error}"
                                                ),
                                                cx,
                                            );
                                        });
                                    }
                                }
                            }
                        }
                        audit.update(cx, |_, cx| cx.notify());
                    })
                    .detach();
                }
                // cfg! keeps the call compiled (and the module alive) on every
                // platform while running it only where a menu bar exists.
                if cfg!(target_os = "macos") {
                    menus::init(audit.clone(), cx);
                }
                audit::register_quit_flush(audit, cx);
            }
            cx.activate(true);
            schedule_pending_crash_prompt(&root, cx, pending_crash, crash::defer_prompt);
        });
}

/// The Root owns the dialog layer, so wait until its window exists before asking
/// crash reporting to defer its prompt onto that still-open window.
fn schedule_pending_crash_prompt(
    root: &WindowHandle<Root>,
    cx: &mut App,
    pending_crash: Option<PathBuf>,
    defer_prompt: impl FnOnce(&Window, &mut App, Option<PathBuf>),
) {
    root.update(cx, |_, window, cx| defer_prompt(window, cx, pending_crash))
        .ok();
}

#[cfg(test)]
mod tests {
    use super::*;
    use gpui_kit::{Context, IntoElement, Render, TestAppContext};

    /// A reader that has gone away, and a stdout that is genuinely broken.
    struct FailingWriter(std::io::ErrorKind);

    impl std::io::Write for FailingWriter {
        fn write(&mut self, _: &[u8]) -> std::io::Result<usize> {
            Err(std::io::Error::from(self.0))
        }

        fn flush(&mut self) -> std::io::Result<()> {
            Ok(())
        }
    }

    struct CrashWindowHarness;

    impl Render for CrashWindowHarness {
        fn render(
            &mut self,
            window: &mut gpui_kit::Window,
            cx: &mut Context<Self>,
        ) -> impl IntoElement {
            gpui_kit::div()
                .size_full()
                .children(Root::render_dialog_layer(window, cx))
        }
    }

    fn parse(arguments: &[&str]) -> Result<Args, String> {
        parse_args_from(arguments.iter().map(|argument| (*argument).to_string()))
    }

    /// macOS hands out `/var/folders/...`, and `/var` is a symlink to `/private/var`.
    /// `Context` canonicalizes its roots, so a fixture that starts from the aliased
    /// spelling compares two different names.
    fn temp_root(tag: &str) -> PathBuf {
        let root = std::env::temp_dir().join(format!("press-cli-{tag}-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&root);
        std::fs::create_dir_all(&root).unwrap();
        root.canonicalize().unwrap()
    }

    fn write_photo(path: &Path, width: u32, height: u32) {
        convert::tests::photo(width, height).save(path).unwrap();
    }

    fn probe_all(paths: &[PathBuf]) -> Vec<Entry> {
        paths
            .iter()
            .map(|path| scan::probe(path).expect("the fixture is an image"))
            .collect()
    }

    /// Plan a run over every entry, the way `main` does.
    fn plan_queue<'a>(
        root: &Path,
        destination: &convert::Destination,
        entries: &'a [Entry],
        format: Format,
        quality: Quality,
        max_edge: MaxEdge,
        skip_existing: bool,
    ) -> Queued<'a> {
        let sources: Vec<PathBuf> = entries.iter().map(|entry| entry.path.clone()).collect();
        let planned = convert::plan_outputs(root, &sources, &sources, destination, format);
        let audited: Vec<&Entry> = entries.iter().collect();
        queue_run(
            &audited,
            &planned,
            skip_existing,
            true,
            destination.manifest,
            root,
            destination.out_dir,
            format,
            quality,
            max_edge,
        )
    }

    /// The same plan against a destination with no history, which is every run that
    /// is not replacing.
    fn plan_run<'a>(
        root: &Path,
        out_dir: &Path,
        entries: &'a [Entry],
        format: Format,
        skip_existing: bool,
    ) -> Queued<'a> {
        plan_queue(
            root,
            &convert::tests::plain(out_dir),
            entries,
            format,
            Quality::lossy(80.),
            MaxEdge::FULL,
            skip_existing,
        )
    }

    fn convert_all(
        root: &Path,
        out_dir: &Path,
        entries: &[Entry],
        format: Format,
    ) -> ConversionRun {
        let destination = convert::tests::plain(out_dir);
        let queued = plan_queue(
            root,
            &destination,
            entries,
            format,
            Quality::lossy(80.),
            MaxEdge::FULL,
            false,
        );
        convert_headless(
            root,
            &queued,
            &destination,
            format,
            Quality::lossy(80.),
            MaxEdge::FULL,
            true,
        )
    }

    #[test]
    fn flags_parse_into_their_fields() {
        let cases = [
            (
                vec!["--convert", "--avif", "--quality", "40", "x"],
                Command::Convert,
                Format::Avif,
                Quality::lossy(40.),
                MaxEdge::FULL,
                false,
                false,
                "x",
            ),
            (
                vec!["--max-edge", "1600", "--lossless", "--grid", "x"],
                Command::Window,
                Format::WebP,
                Quality::LOSSLESS,
                MaxEdge(Some(1600)),
                true,
                false,
                "x",
            ),
            (
                vec!["convert", "x", "--format", "jxl", "--lossless", "--json"],
                Command::Convert,
                Format::JpegXl,
                Quality::LOSSLESS,
                MaxEdge::FULL,
                false,
                true,
                "x",
            ),
        ];

        for (arguments, command, format, quality, max_edge, grid, json, root) in cases {
            let args = parse(&arguments).unwrap();
            assert_eq!(args.command, command);
            assert_eq!(args.format, format);
            assert_eq!(args.quality, quality);
            assert_eq!(args.max_edge, max_edge);
            assert_eq!(args.grid, grid);
            assert_eq!(args.json, json);
            assert_eq!(args.root, Some(PathBuf::from(root)));
        }
    }

    #[test]
    fn the_jpeg_and_same_formats_parse_and_refuse_lossless() {
        assert_eq!(parse(&["--jpeg", "x"]).unwrap().format, Format::Jpeg);
        assert_eq!(
            parse(&["--format", "jpeg", "x"]).unwrap().format,
            Format::Jpeg
        );
        assert_eq!(
            parse(&["convert", "x", "--format", "same", "--max-edge", "1600"])
                .unwrap()
                .format,
            Format::Same
        );
        assert!(parse(&["--jpeg", "--lossless"]).is_err());
        assert!(parse(&["--format", "same", "--lossless"]).is_err());
        match parse(&["--format", "bmp"]) {
            Err(error) => assert!(error.contains("same"), "{error}"),
            Ok(_) => panic!("an unknown format must be an error"),
        }
    }

    #[test]
    fn a_bad_quality_value_is_an_error_not_a_default() {
        match parse(&["--quality", "abc"]) {
            Err(error) => assert!(error.contains("--quality")),
            Ok(_) => panic!("a bad quality value must be an error"),
        }
    }

    #[test]
    fn a_non_finite_quality_is_an_error() {
        for value in ["NaN", "inf", "-inf"] {
            assert!(parse(&["--quality", value]).is_err(), "accepted {value}");
        }
    }

    #[test]
    fn quality_outside_the_documented_range_is_an_error() {
        for value in ["0", "101", "-3"] {
            assert!(parse(&["--quality", value]).is_err(), "accepted {value}");
        }
    }

    #[test]
    fn lossless_avif_is_rejected_in_either_flag_order() {
        assert!(parse(&["--avif", "--lossless"]).is_err());
        assert!(parse(&["--lossless", "--avif"]).is_err());
    }

    #[test]
    fn a_bad_max_edge_value_is_an_error_not_a_default() {
        match parse(&["--max-edge", "abc"]) {
            Err(error) => assert!(error.contains("--max-edge")),
            Ok(_) => panic!("a bad max edge value must be an error"),
        }
    }

    #[test]
    fn a_zero_max_edge_is_an_error() {
        match parse(&["--max-edge", "0"]) {
            Err(error) => assert!(error.contains("--max-edge")),
            Ok(_) => panic!("a zero max edge must be an error"),
        }
    }

    /// Zero is a real speed, so it cannot be dropped as falsy, and the range is
    /// libaom's own. An unset flag stays unset: the settings file is asked next.
    #[test]
    fn the_avif_speed_flag_takes_zero_to_ten_and_nothing_else() {
        assert_eq!(
            parse(&["--avif-speed", "8", "x"]).unwrap().avif_speed,
            Some(8)
        );
        assert_eq!(
            parse(&["--avif-speed", "0", "x"]).unwrap().avif_speed,
            Some(0)
        );
        assert_eq!(parse(&["--avif", "x"]).unwrap().avif_speed, None);
        for value in ["11", "-1", "fast"] {
            match parse(&["--avif-speed", value]) {
                Err(error) => assert!(error.contains("--avif-speed"), "{value}: {error}"),
                Ok(_) => panic!("accepted --avif-speed {value}"),
            }
        }
        // It changes the output, so it is a conversion option like the rest.
        assert!(parse(&["audit", "x", "--avif-speed", "8"]).is_err());
    }

    #[test]
    fn a_missing_value_is_an_error() {
        for arguments in [
            ["--quality"].as_slice(),
            ["--max-edge"].as_slice(),
            ["--avif-speed"].as_slice(),
        ] {
            assert!(parse(arguments).is_err());
        }
    }

    #[test]
    fn an_unknown_option_is_collected_not_a_path() {
        let args = parse(&["--nope", "/tmp"]).unwrap();
        assert_eq!(args.root, Some(PathBuf::from("/tmp")));
        assert_eq!(args.unknown, ["--nope"]);
    }

    #[test]
    fn a_double_dash_ends_option_parsing() {
        let args = parse(&["--", "-photos"]).unwrap();
        assert_eq!(args.root, Some(PathBuf::from("-photos")));
        assert!(args.unknown.is_empty());
    }

    #[test]
    fn headless_commands_are_strict_and_take_one_target() {
        let audit = parse(&["audit", "/photos", "--json"]).unwrap();
        assert_eq!(audit.command, Command::Audit);
        assert!(audit.json);
        assert!(parse(&["audit", "/photos", "--quality", "80"]).is_err());
        assert!(parse(&["convert", "one", "two"]).is_err());
        assert!(parse(&["--json", "/photos"]).is_err());
    }

    /// Headless conversion establishes the same output context as the window, so the
    /// default destination has to keep working through it.
    #[test]
    fn headless_conversion_writes_into_the_established_default_output() {
        let root = std::env::temp_dir().join(format!("press-headless-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&root);
        std::fs::create_dir_all(&root).unwrap();
        // macOS hands out `/var/folders/...`, and `/var` is a symlink to
        // `/private/var`. `Context` canonicalizes its roots, so a fixture that
        // starts from the aliased spelling compares two different names.
        let root = root.canonicalize().unwrap();
        let source = root.join("photo.png");
        image::RgbImage::from_fn(64, 64, |x, y| {
            let hash = x.wrapping_mul(2_654_435_761) ^ y.wrapping_mul(2_246_822_519);
            image::Rgb([(hash >> 8) as u8, (hash >> 16) as u8, (hash >> 24) as u8])
        })
        .save(&source)
        .unwrap();
        let entries = probe_all(&[source]);

        let context = settings::Output::Optimized
            .context(&root)
            .expect("the default output establishes");
        let run = convert_all(&root, context.output_root(), &entries, Format::WebP);

        assert_eq!(run.failed, 0);
        assert_eq!(run.files.len(), 1);
        assert_eq!(run.files[0].status, "converted");
        assert!(root.join(scan::OUTPUT_DIR).join("photo.webp").is_file());
        std::fs::remove_dir_all(&root).unwrap();
    }

    #[test]
    fn the_output_folder_is_a_conversion_option_with_a_short_alias() {
        for flag in ["--output", "-o"] {
            assert_eq!(
                parse(&["convert", "/photos", flag, "/exports"])
                    .unwrap()
                    .output,
                Some(PathBuf::from("/exports")),
                "{flag}"
            );
        }
        assert!(parse(&["convert", "/photos"]).unwrap().output.is_none());
        assert!(parse(&["convert", "/photos", "--output"]).is_err());
        assert!(parse(&["convert", "/photos", "--output", ""]).is_err());
        assert!(parse(&["audit", "/photos", "--output", "/exports"]).is_err());
        assert!(parse(&["/photos", "--output", "/exports"]).is_err());
        assert!(parse(&["update", "-o", "/exports"]).is_err());
    }

    #[test]
    fn options_refuse_to_eat_the_next_flag_as_their_value() {
        assert!(parse(&["convert", "/photos", "--output", "--replace"]).is_err());
        assert!(parse(&["convert", "/photos", "-o", "--json"]).is_err());
        assert!(parse(&["convert", "/photos", "--format", "--replace"]).is_err());
        assert!(parse(&["convert", "/photos", "--max-edge", "--dry-run"]).is_err());
        assert!(parse(&["convert", "/photos", "--avif-speed", "--json"]).is_err());
        assert!(parse(&["convert", "/photos", "--quality", "--json"]).is_err());
        assert!(
            parse(&["convert", "/photos", "-o", "--output"])
                .is_err_and(|message| message.contains("--output needs a folder"))
        );
        assert_eq!(
            parse(&["convert", "/photos", "--output", "/exports"])
                .unwrap()
                .output,
            Some(PathBuf::from("/exports"))
        );
        assert!(matches!(
            parse(&["convert", "/photos", "--format", "avif"])
                .unwrap()
                .format,
            Format::Avif
        ));
        assert_eq!(
            parse(&["convert", "/photos", "--max-edge", "1600"])
                .unwrap()
                .max_edge
                .0,
            Some(1600)
        );
        assert_eq!(
            parse(&["convert", "/photos", "--avif-speed", "6"])
                .unwrap()
                .avif_speed,
            Some(6)
        );
    }

    /// `--output` establishes the window's own boundary, so a folder outside the
    /// audited tree has to take every file, subfolders and all.
    #[test]
    fn a_chosen_output_folder_outside_the_root_takes_every_file() {
        let base = temp_root("output");
        let root = base.join("photos");
        let album = root.join("album");
        std::fs::create_dir_all(&album).unwrap();
        let exports = base.join("exports");
        write_photo(&root.join("one.png"), 64, 64);
        write_photo(&album.join("two.png"), 48, 48);
        let entries = probe_all(&[root.join("one.png"), album.join("two.png")]);

        let context = settings::Output::Folder(exports.clone())
            .context(&root)
            .expect("a folder outside the root establishes");
        assert_eq!(context.output_root(), exports);
        let run = convert_all(&root, context.output_root(), &entries, Format::WebP);

        assert_eq!(run.failed, 0);
        assert_eq!(run.files.len(), 2);
        assert!(run.files.iter().all(|file| file.status == "converted"));
        assert!(exports.join("one.webp").is_file());
        assert!(exports.join("album").join("two.webp").is_file());
        assert!(!root.join(scan::OUTPUT_DIR).exists());
        std::fs::remove_dir_all(&base).unwrap();
    }

    #[test]
    fn skip_existing_is_a_conversion_option() {
        assert!(
            parse(&["convert", "/photos", "--skip-existing"])
                .unwrap()
                .skip_existing
        );
        assert!(!parse(&["convert", "/photos"]).unwrap().skip_existing);
        assert!(parse(&["audit", "/photos", "--skip-existing"]).is_err());
        assert!(parse(&["/photos", "--skip-existing"]).is_err());
        assert!(parse(&["skill", "--skip-existing"]).is_err());
    }

    /// The point of the flag: a re-run over a tree that mostly has not changed does
    /// the work only for the files that did.
    #[test]
    fn skip_existing_leaves_a_current_output_and_converts_a_stale_one() {
        let base = temp_root("skip");
        let root = base.join("photos");
        std::fs::create_dir_all(&root).unwrap();
        let out_dir = base.join("exports");
        std::fs::create_dir_all(&out_dir).unwrap();
        write_photo(&root.join("current.png"), 64, 64);
        write_photo(&root.join("stale.png"), 64, 64);
        let entries = probe_all(&[root.join("current.png"), root.join("stale.png")]);

        // One output is written now, so it is not older than its source. The other is
        // backdated, which is what an edited source looks like to the next run.
        std::fs::write(out_dir.join("current.webp"), b"already converted").unwrap();
        std::fs::write(out_dir.join("stale.webp"), b"out of date").unwrap();
        let stale = std::fs::File::options()
            .write(true)
            .open(out_dir.join("stale.webp"))
            .unwrap();
        stale
            .set_modified(std::time::SystemTime::UNIX_EPOCH)
            .unwrap();
        drop(stale);

        let queued = plan_run(&root, &out_dir, &entries, Format::WebP, true);

        assert_eq!(queued.skipped.len(), 1);
        assert_eq!(
            queued.skipped[0].source,
            path_text(&root.join("current.png"))
        );
        assert!(queued.skipped[0].skipped);
        assert_eq!(
            queued.skipped[0].reason.as_deref(),
            Some("the output is not older than the source")
        );
        assert_eq!(queued.skipped[0].output_bytes, Some(17));
        assert_eq!(queued.entries.len(), 1);
        assert_eq!(queued.entries[0].path, root.join("stale.png"));

        let run = convert_headless(
            &root,
            &queued,
            &convert::tests::plain(&out_dir),
            Format::WebP,
            Quality::lossy(80.),
            MaxEdge::FULL,
            true,
        );
        assert_eq!(run.failed, 0);
        assert_eq!(run.files.len(), 1);
        assert_eq!(run.files[0].status, "converted");
        // The skipped output is untouched; the stale one was rewritten.
        assert_eq!(
            std::fs::read(out_dir.join("current.webp")).unwrap(),
            b"already converted"
        );
        assert!(std::fs::metadata(out_dir.join("stale.webp")).unwrap().len() > 100);
        std::fs::remove_dir_all(&base).unwrap();
    }

    /// Plan with the manifest the out dir actually holds, the way `main` does.
    fn plan_history<'a>(
        root: &Path,
        out_dir: &Path,
        entries: &'a [Entry],
        format: Format,
        quality: Quality,
        max_edge: MaxEdge,
        skip_existing: bool,
    ) -> Queued<'a> {
        let recorded = manifest::load(out_dir);
        let destination = convert::Destination {
            out_dir,
            backups: None,
            manifest: &recorded,
        };
        let sources: Vec<PathBuf> = entries.iter().map(|entry| entry.path.clone()).collect();
        let planned = convert::plan_outputs(root, &sources, &sources, &destination, format);
        let audited: Vec<&Entry> = entries.iter().collect();
        queue_run(
            &audited,
            &planned,
            skip_existing,
            true,
            &recorded,
            root,
            out_dir,
            format,
            quality,
            max_edge,
        )
    }

    fn backdate(path: &Path) {
        std::fs::File::options()
            .write(true)
            .open(path)
            .expect("the fixture opens")
            .set_modified(
                std::time::SystemTime::UNIX_EPOCH + std::time::Duration::from_secs(1_000_000),
            )
            .expect("the mtime is set");
    }

    fn record_at(
        root: &Path,
        out_dir: &Path,
        source: &Path,
        output: &Path,
        format: Format,
        quality: Quality,
        max_edge: MaxEdge,
    ) {
        let record = manifest::Stamp::new(format, quality, max_edge)
            .record((root, out_dir), source, output, output, None)
            .expect("the run records");
        manifest::append_record(out_dir, &record).expect("the record appends");
    }

    #[test]
    fn skip_existing_rebuilds_when_the_quality_changed() {
        let base = temp_root("skip-quality");
        let root = base.join("photos");
        std::fs::create_dir_all(&root).unwrap();
        let out_dir = base.join("exports");
        std::fs::create_dir_all(&out_dir).unwrap();
        write_photo(&root.join("shot.png"), 64, 64);
        let entries = probe_all(&[root.join("shot.png")]);
        // An earlier run wrote this output at q60, and it is still current.
        std::fs::write(out_dir.join("shot.webp"), b"already converted").unwrap();
        backdate(&root.join("shot.png"));
        record_at(
            &root,
            &out_dir,
            &root.join("shot.png"),
            &out_dir.join("shot.webp"),
            Format::WebP,
            Quality::lossy(60.),
            MaxEdge::FULL,
        );
        let queued = plan_history(
            &root,
            &out_dir,
            &entries,
            Format::WebP,
            Quality::lossy(80.),
            MaxEdge::FULL,
            true,
        );
        assert!(queued.skipped.is_empty(), "a quality change rebuilds");
        assert_eq!(queued.entries.len(), 1);
        std::fs::remove_dir_all(&base).unwrap();
    }

    #[test]
    fn skip_existing_skips_when_settings_match() {
        let base = temp_root("skip-match");
        let root = base.join("photos");
        std::fs::create_dir_all(&root).unwrap();
        let out_dir = base.join("exports");
        std::fs::create_dir_all(&out_dir).unwrap();
        write_photo(&root.join("shot.png"), 64, 64);
        let entries = probe_all(&[root.join("shot.png")]);
        std::fs::write(out_dir.join("shot.webp"), b"already converted").unwrap();
        backdate(&root.join("shot.png"));
        record_at(
            &root,
            &out_dir,
            &root.join("shot.png"),
            &out_dir.join("shot.webp"),
            Format::WebP,
            Quality::lossy(80.),
            MaxEdge::FULL,
        );
        let queued = plan_history(
            &root,
            &out_dir,
            &entries,
            Format::WebP,
            Quality::lossy(80.),
            MaxEdge::FULL,
            true,
        );
        assert_eq!(queued.skipped.len(), 1);
        assert!(queued.entries.is_empty());
        assert_eq!(
            queued.skipped[0].reason.as_deref(),
            Some("the output already matches webp q80 full")
        );
        std::fs::remove_dir_all(&base).unwrap();
    }

    #[test]
    fn skip_existing_converts_a_stale_output() {
        let base = temp_root("skip-stale");
        let root = base.join("photos");
        std::fs::create_dir_all(&root).unwrap();
        let out_dir = base.join("exports");
        std::fs::create_dir_all(&out_dir).unwrap();
        write_photo(&root.join("shot.png"), 64, 64);
        let entries = probe_all(&[root.join("shot.png")]);
        // The output predates the source even though a record describes it.
        std::fs::write(out_dir.join("shot.webp"), b"out of date").unwrap();
        backdate(&out_dir.join("shot.webp"));
        record_at(
            &root,
            &out_dir,
            &root.join("shot.png"),
            &out_dir.join("shot.webp"),
            Format::WebP,
            Quality::lossy(80.),
            MaxEdge::FULL,
        );
        let queued = plan_history(
            &root,
            &out_dir,
            &entries,
            Format::WebP,
            Quality::lossy(80.),
            MaxEdge::FULL,
            true,
        );
        assert!(queued.skipped.is_empty(), "a stale output converts");
        assert_eq!(queued.entries.len(), 1);
        std::fs::remove_dir_all(&base).unwrap();
    }

    /// Without the flag nothing is skipped, however current the output is.
    #[test]
    fn a_run_without_skip_existing_queues_every_file() {
        let base = temp_root("no-skip");
        let root = base.join("photos");
        std::fs::create_dir_all(&root).unwrap();
        let out_dir = base.join("exports");
        std::fs::create_dir_all(&out_dir).unwrap();
        write_photo(&root.join("one.png"), 48, 48);
        let entries = probe_all(&[root.join("one.png")]);
        std::fs::write(out_dir.join("one.webp"), b"already converted").unwrap();

        let queued = plan_run(&root, &out_dir, &entries, Format::WebP, false);

        assert!(queued.skipped.is_empty());
        assert_eq!(queued.entries.len(), 1);
        assert_eq!(run_tail(0, 0), "");
        assert_eq!(run_tail(2, 1), ", 2 skipped, 1 failed");
        std::fs::remove_dir_all(&base).unwrap();
    }

    #[test]
    fn dry_run_is_a_conversion_option() {
        assert!(parse(&["convert", "/photos", "--dry-run"]).unwrap().dry_run);
        assert!(!parse(&["convert", "/photos"]).unwrap().dry_run);
        assert!(
            parse(&["convert", "/photos", "--dry-run", "--json"])
                .unwrap()
                .json
        );
        assert!(
            parse(&["convert", "/photos", "--dry-run", "--skip-existing"])
                .unwrap()
                .skip_existing
        );
        assert!(parse(&["audit", "/photos", "--dry-run"]).is_err());
        assert!(parse(&["/photos", "--dry-run"]).is_err());
        assert!(parse(&["update", "--dry-run"]).is_err());
    }

    /// A dry run has to be worth reading and cost nothing: real planned names, a
    /// projection off real encodes, and not one byte on disk.
    #[test]
    fn a_dry_run_plans_and_projects_without_writing_anything() {
        let base = temp_root("dry");
        let root = base.join("photos");
        std::fs::create_dir_all(&root).unwrap();
        let out_dir = base.join("exports");
        write_photo(&root.join("one.png"), 96, 96);
        write_photo(&root.join("two.png"), 96, 96);
        let entries = probe_all(&[root.join("one.png"), root.join("two.png")]);
        let queued = plan_run(&root, &out_dir, &entries, Format::WebP, false);

        let run = dry_run_headless(
            &queued,
            Format::WebP,
            Quality::lossy(80.),
            MaxEdge::FULL,
            true,
        );

        assert_eq!(run.failed, 0);
        assert_eq!(run.files.len(), 2);
        assert!(run.files.iter().all(|file| file.status == "planned"));
        assert!(run.files.iter().all(|file| file.output.is_none()));
        assert_eq!(
            run.files[0].planned_output,
            Some(path_text(&out_dir.join("one.webp")))
        );
        assert!(run.before > 0);
        assert_eq!(run.after, 0);
        let (projected, samples) = run.projected.expect("real encodes stand behind this");
        assert!(projected > 0, "a dry run projects a size");
        assert_eq!(samples, 2);
        assert!(!out_dir.exists(), "a dry run writes nothing");

        let report = ConversionReport {
            schema_version: 1,
            command: "convert",
            target: path_text(&root),
            output: path_text(&out_dir),
            dry_run: true,
            subfolders: Some(true),
            options: ConversionOptions {
                format: Format::WebP.label(),
                quality: Some(80.),
                max_edge: None,
            },
            scan: ScanSummary::from_scan(&scan::Scan {
                entries: entries.clone(),
                skipped_raw: 0,
                skipped_heic: 0,
                skipped_packages: 0,
                unreadable: vec![],
                walk_errors: vec![],
                existing_output: 0,
            }),
            summary: ConversionSummary {
                attempted: entries.len(),
                converted: 0,
                failed: 0,
                skipped: 0,
                source_bytes: run.before,
                output_bytes: 0,
                grew: false,
                changed_bytes: 0,
                projected_bytes: Some(projected),
                projected_samples: Some(samples),
            },
            manifest: None,
            backup: None,
            files: run.files,
            unreadable: vec![],
            walk_errors: vec![],
        };
        let json = serde_json::to_value(report).unwrap();
        assert_eq!(json["schema_version"], 1);
        assert_eq!(json["dry_run"], true);
        assert_eq!(json["summary"]["converted"], 0);
        assert_eq!(json["summary"]["projected_bytes"], projected);
        assert_eq!(json["files"][0]["status"], "planned");
        assert!(json["files"][0]["planned_output"].is_string());
        std::fs::remove_dir_all(&base).unwrap();
    }

    /// The sampler takes one file from the middle of each slice and scales the whole
    /// slice by it. Handed a list in walk order, a slice pairs a heavy photo with a
    /// thumbnail and the thumbnail's ratio speaks for both, so the projection follows
    /// the small files. Sorted by weight, each heavy slice is sampled by a heavy file.
    #[test]
    fn a_projection_weighs_the_heavy_files_by_their_own_ratio() {
        let base = temp_root("projection");
        std::fs::create_dir_all(&base).unwrap();
        // Two per slice, sampled at the second: heavy files sit on the even positions,
        // where walk order would leave every one of them unsampled.
        let mut paths = Vec::new();
        for index in 0..64 {
            let path = base.join(format!("photo-{index:02}.png"));
            if index < 16 && index % 2 == 0 {
                write_photo(&path, 160, 160);
            } else {
                write_photo(&path, 16, 16);
            }
            paths.push(path);
        }
        let entries = probe_all(&paths);
        let heavy: u64 = entries.iter().filter(|entry| entry.bytes > 10_000).count() as u64;
        assert_eq!(heavy, 8, "the fixture has a heavy tail to weigh");
        let walk_order: Vec<&Entry> = entries.iter().collect();

        let truth: u64 = entries
            .iter()
            .map(|entry| {
                let (image, profile) =
                    scan::decode_for_conversion(&entry.path, MaxEdge::FULL).unwrap();
                convert::encode(
                    &image,
                    Format::WebP,
                    Quality::lossy(80.),
                    profile.as_deref(),
                )
                .unwrap()
                .len() as u64
            })
            .sum();
        let (projected, samples) = project_run(
            &walk_order,
            Format::WebP,
            Quality::lossy(80.),
            MaxEdge::FULL,
        )
        .expect("real encodes stand behind this");

        assert_eq!(samples, 32);
        assert!(truth > 0);
        let error = projected.abs_diff(truth) as f64 / truth as f64;
        assert!(
            error < 0.2,
            "projected {projected} against a real {truth}, {:.0}% out",
            error * 100.
        );
        std::fs::remove_dir_all(&base).unwrap();
    }

    /// `--format same` refuses a file whose name and bytes disagree, and a dry run can
    /// see that without decoding anything. Calling it "planned" would send a caller
    /// looking for an output that was never going to exist.
    #[test]
    fn a_dry_run_names_a_file_the_encoder_would_refuse_by_name() {
        let base = temp_root("refused");
        let root = base.join("photos");
        std::fs::create_dir_all(&root).unwrap();
        let out_dir = base.join("exports");
        let liar = root.join("mislabelled.jpg");
        convert::tests::photo(48, 48)
            .save_with_format(&liar, image::ImageFormat::Png)
            .unwrap();
        write_photo(&root.join("honest.png"), 48, 48);
        let entries = probe_all(&[root.join("honest.png"), liar.clone()]);
        assert!(
            entries.iter().any(|entry| entry.extension_lies()),
            "the fixture lies about its extension"
        );

        let queued = plan_run(&root, &out_dir, &entries, Format::Same, false);
        let run = dry_run_headless(
            &queued,
            Format::Same,
            Quality::lossy(80.),
            MaxEdge::FULL,
            true,
        );

        assert_eq!(run.failed, 1);
        let refused = run
            .files
            .iter()
            .find(|file| file.source == path_text(&liar))
            .expect("the mislabelled file is reported");
        assert_eq!(refused.status, "failed");
        assert!(refused.planned_output.is_none());
        assert!(
            refused
                .error
                .as_deref()
                .is_some_and(|reason| reason.contains("named .jpg but the bytes are PNG")),
            "{:?}",
            refused.error
        );
        let planned = run
            .files
            .iter()
            .filter(|file| file.status == "planned")
            .count();
        assert_eq!(planned, 1);
        assert!(!out_dir.exists(), "a dry run writes nothing");
        std::fs::remove_dir_all(&base).unwrap();
    }

    /// A chosen output under the audited folder is this run's own output. The walk has
    /// to skip it, or the next run audits what the last one wrote.
    #[test]
    fn a_chosen_output_under_the_root_becomes_the_walk_boundary() {
        let base = temp_root("boundary");
        let root = base.join("photos");
        std::fs::create_dir_all(root.join("exports")).unwrap();
        let elsewhere = base.join("elsewhere");
        std::fs::create_dir_all(&elsewhere).unwrap();

        assert_eq!(
            walk_output(&root, Some(&root.join("exports"))),
            root.join("exports")
        );
        assert_eq!(
            walk_output(&root, Some(&elsewhere)),
            root.join(scan::OUTPUT_DIR)
        );
        assert_eq!(walk_output(&root, None), root.join(scan::OUTPUT_DIR));
        // The context refuses this one by name; the walk must not empty the audit
        // before that refusal arrives.
        assert_eq!(walk_output(&root, Some(&root)), root.join(scan::OUTPUT_DIR));
        std::fs::remove_dir_all(&base).unwrap();
    }

    /// The scriptable half of replace mode: convert in place, keep every original,
    /// and leave a record `press restore` can read on a later run.
    #[test]
    fn headless_replace_converts_in_place_and_restores_from_the_record() {
        let root = std::env::temp_dir().join(format!("press-replace-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&root);
        std::fs::create_dir_all(root.join("album")).unwrap();
        let root = root.canonicalize().unwrap();
        let source = root.join("album").join("photo.png");
        image::RgbImage::from_fn(64, 64, |x, y| {
            let hash = x.wrapping_mul(2_654_435_761) ^ y.wrapping_mul(2_246_822_519);
            image::Rgb([(hash >> 8) as u8, (hash >> 16) as u8, (hash >> 24) as u8])
        })
        .save(&source)
        .unwrap();
        let original = std::fs::read(&source).unwrap();
        let entries = vec![scan::probe(&source).expect("the fixture is an image")];

        let context = settings::Output::Replace
            .context(&root)
            .expect("replace mode establishes");
        let backups = manifest::backup_root(context.output_root());
        let recorded = manifest::load(context.output_root());
        let destination = convert::Destination {
            out_dir: context.output_root(),
            backups: Some(&backups),
            manifest: &recorded,
        };
        let queued = plan_queue(
            &root,
            &destination,
            &entries,
            Format::WebP,
            Quality::lossy(80.),
            MaxEdge::FULL,
            false,
        );
        let run = convert_headless(
            &root,
            &queued,
            &destination,
            Format::WebP,
            Quality::lossy(80.),
            MaxEdge::FULL,
            true,
        );

        assert_eq!(run.failed, 0);
        assert_eq!(run.files.len(), 1);
        assert_eq!(run.files[0].status, "converted");
        assert!(root.join("album").join("photo.webp").is_file());
        assert!(!source.exists(), "the original left its own name");
        assert_eq!(
            std::fs::read(backups.join("album").join("photo.png")).unwrap(),
            original,
            "the original is kept byte for byte"
        );
        assert_eq!(run.written_manifest, Some(manifest::path(&root)));
        let recorded = manifest::load(&root);
        assert_eq!(recorded.outputs.len(), 1);
        assert_eq!(recorded.outputs[0].source, Path::new("album/photo.png"));
        assert_eq!(
            recorded.outputs[0].backup.as_deref(),
            Some(Path::new("album/photo.png"))
        );
        assert_eq!(recorded.outputs[0].source_bytes, original.len() as u64);

        assert_eq!(restore_headless(&root), 0);
        assert_eq!(std::fs::read(&source).unwrap(), original);
        assert!(!root.join("album").join("photo.webp").exists());
        std::fs::remove_dir_all(&root).unwrap();
    }

    #[test]
    fn replacing_and_restoring_are_their_own_invocations() {
        let replace = parse(&["convert", "/photos", "--replace"]).unwrap();
        assert_eq!(replace.command, Command::Convert);
        assert!(replace.replace);
        assert!(!parse(&["convert", "/photos"]).unwrap().replace);

        let restore = parse(&["restore", "/photos"]).unwrap();
        assert_eq!(restore.command, Command::Restore);
        assert_eq!(restore.root.as_deref(), Some(Path::new("/photos")));

        assert!(
            parse(&["/photos", "--replace"]).is_err(),
            "replacing is never something the window inherits from a flag"
        );
        assert!(
            parse(&["convert", "/photos", "--replace", "--output", "/exports"]).is_err(),
            "a chosen output and replacing are two answers to the same question"
        );
        assert!(
            parse(&["convert", "/photos", "--replace", "--dry-run"])
                .unwrap()
                .dry_run,
            "a replace run can be planned without moving an original"
        );
        assert!(parse(&["restore", "/photos", "--json"]).is_err());
        assert!(parse(&["restore", "/photos", "--avif"]).is_err());
    }

    #[test]
    fn no_subfolders_narrows_the_headless_scope_and_is_stated_in_the_output() {
        assert!(parse(&["audit", "/photos"]).unwrap().subfolders);
        assert!(
            !parse(&["audit", "/photos", "--no-subfolders"])
                .unwrap()
                .subfolders
        );
        assert!(
            !parse(&["convert", "/photos", "--no-subfolders", "--avif"])
                .unwrap()
                .subfolders
        );
        assert!(parse(&["--no-subfolders", "/photos"]).is_err());
        assert!(parse(&["update", "--no-subfolders"]).is_err());
        assert_eq!(scope_note(Some(true)), ", subfolders included");
        assert_eq!(scope_note(Some(false)), ", subfolders excluded");
        assert_eq!(scope_note(None), "");
    }

    #[test]
    fn update_is_a_targetless_command() {
        assert_eq!(parse(&["update"]).unwrap().command, Command::Update);
        assert!(parse(&["update", "/tmp"]).is_err());
    }

    #[test]
    fn audit_json_has_a_stable_schema_and_the_ui_findings() {
        let scanned = scan::Scan {
            entries: vec![Entry {
                path: PathBuf::from("/photos/heavy.png"),
                format: image::ImageFormat::Png.into(),
                width: 100,
                height: 100,
                bytes: 40_000,
            }],
            skipped_raw: 2,
            skipped_heic: 0,
            skipped_packages: 0,
            unreadable: vec![],
            walk_errors: vec![],
            existing_output: 3,
        };

        let json =
            serde_json::to_value(audit_report(Path::new("/photos"), &scanned, Some(true))).unwrap();
        assert_eq!(json["schema_version"], 1);
        assert_eq!(json["subfolders"], true);
        assert_eq!(json["summary"]["heavy"], 1);
        assert_eq!(json["summary"]["camera_raw_skipped"], 2);
        assert_eq!(json["files"][0]["path"], "/photos/heavy.png");
        assert_eq!(json["files"][0]["heavy"], true);
    }

    #[test]
    fn a_heic_only_folder_reports_its_count_in_the_json_summary() {
        let scanned = scan::Scan {
            entries: vec![],
            skipped_raw: 0,
            skipped_heic: 4,
            skipped_packages: 0,
            unreadable: vec![],
            walk_errors: vec![],
            existing_output: 0,
        };

        let json = serde_json::to_value(audit_report(Path::new("/photos"), &scanned, Some(false)))
            .unwrap();
        assert_eq!(json["subfolders"], false);
        let single =
            serde_json::to_value(audit_report(Path::new("/photo.png"), &scanned, None)).unwrap();
        assert!(
            single["subfolders"].is_null(),
            "one file has no scope to claim"
        );
        // The whole point: zero images is not the whole story, and a caller that
        // reads only `images` would call an iPhone folder empty.
        assert_eq!(json["summary"]["images"], 0);
        assert_eq!(json["summary"]["heic_skipped"], 4);
    }

    /// `press audit big-folder | head` closes the pipe under Press. That is the reader
    /// leaving, and it must not reach the panic hook; anything else still has to be
    /// reported as the failure it is.
    #[test]
    fn a_closed_reader_ends_a_write_cleanly_and_a_real_failure_stays_an_error() {
        let mut closed = FailingWriter(std::io::ErrorKind::BrokenPipe);
        assert!(
            !write_text(&mut closed, "a line\n").expect("a closed reader is not an error"),
            "a closed pipe ends the run rather than failing it"
        );

        let mut broken = FailingWriter(std::io::ErrorKind::PermissionDenied);
        let error = write_text(&mut broken, "a line\n").expect_err("a real failure is an error");
        assert_eq!(error.kind(), std::io::ErrorKind::PermissionDenied);

        let mut written = Vec::new();
        assert!(write_text(&mut written, "a line\n").unwrap());
        assert_eq!(written, b"a line\n");
    }

    #[gpui_kit::test]
    fn windowed_startup_runs_the_crash_prompt_hook(cx: &mut TestAppContext) {
        cx.update(init_theme);
        let report = PathBuf::from("crash-00000000000000000001-42-0000.log");
        let scheduled = std::rc::Rc::new(std::cell::RefCell::new(None));
        let scheduled_prompt = scheduled.clone();
        let root = cx.add_window(|window, cx| {
            let harness = cx.new(|_| CrashWindowHarness);
            Root::new(harness, window, cx)
        });
        cx.update(|cx| {
            schedule_pending_crash_prompt(&root, cx, Some(report.clone()), move |_, _, actual| {
                *scheduled_prompt.borrow_mut() = actual
            });
        });
        assert_eq!(*scheduled.borrow(), Some(report));
    }
}
