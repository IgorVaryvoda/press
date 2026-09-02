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
use std::io::Write as _;
use std::path::{Path, PathBuf};

use convert::{Format, MaxEdge, Quality};
use gpui::{App, Bounds, Window, WindowBounds, WindowHandle, WindowOptions, prelude::*, px, size};
use gpui_component::{ActiveTheme, Root};
use gpui_platform::application;
use scan::{Entry, format_bytes};
use serde::Serialize;

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
    "  press skill\n",
    "  press update\n\n",
    "Commands:\n",
    "  audit      Read image headers without opening a window or writing files\n",
    "  convert    Re-encode a file or folder into optimized/ without a window\n",
    "  skill      Print the bundled Agent Skill to stdout\n",
    "  update     Install the latest signed Press release\n",
    "  help       Print this help\n",
    "  version    Print the version\n\n",
    "Options:\n",
    "  --json                    Write one schema-versioned JSON document\n",
    "  --no-subfolders           Read the folder itself, not the folders below it\n",
    "  --format <webp|avif|jxl>  Output format (default: webp)\n",
    "  --quality <1..100>        Lossy quality (default: 80)\n",
    "  --lossless                Lossless WebP or JPEG XL\n",
    "  --max-edge <pixels>       Downscale the longest edge; never upscale\n",
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
    grid: bool,
    json: bool,
    /// Headless scope. The window has its own remembered chip for this.
    subfolders: bool,
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

fn parse_args_from(mut rest: impl Iterator<Item = String>) -> Result<Args, String> {
    let mut root = None;
    let mut command = Command::Window;
    let mut format = Format::WebP;
    let mut quality = Quality::lossy(80.);
    let mut max_edge = MaxEdge::FULL;
    let mut grid = false;
    let mut json = false;
    let mut subfolders = true;
    let mut unknown = Vec::new();
    let mut conversion_option = false;

    while let Some(argument) = rest.next() {
        match argument.as_str() {
            "audit" if root.is_none() && command == Command::Window => command = Command::Audit,
            "convert" if root.is_none() && command == Command::Window => command = Command::Convert,
            "skill" if root.is_none() && command == Command::Window => command = Command::Skill,
            "update" if root.is_none() && command == Command::Window => command = Command::Update,
            "help" if root.is_none() && command == Command::Window => command = Command::Help,
            "version" if root.is_none() && command == Command::Window => command = Command::Version,
            "--convert" => select_command(&mut command, Command::Convert, "--convert")?,
            "--audit" => select_command(&mut command, Command::Audit, "--audit")?,
            "--format" => {
                conversion_option = true;
                let value = rest
                    .next()
                    .ok_or_else(|| "--format needs webp, avif, or jxl".to_string())?;
                format = match value.as_str() {
                    "webp" => Format::WebP,
                    "avif" => Format::Avif,
                    "jxl" => Format::JpegXl,
                    _ => return Err(format!("--format needs webp, avif, or jxl, got {value:?}")),
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
            "--max-edge" => {
                conversion_option = true;
                let value = rest
                    .next()
                    .ok_or_else(|| "--max-edge needs a positive number".to_string())?;
                let edge = value
                    .parse()
                    .map_err(|_| format!("--max-edge needs a number, got {value:?}"))?;
                if edge == 0 {
                    return Err(format!("--max-edge needs a positive number, got {value:?}"));
                }
                max_edge = MaxEdge(Some(edge));
            }
            "--webp" => {
                conversion_option = true;
                format = Format::WebP;
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
                let value = rest
                    .next()
                    .ok_or_else(|| "--quality needs a number from 1 to 100".to_string())?;
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
            grid,
            json,
            subfolders,
            unknown,
        });
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
    if command == Command::Window && json {
        return Err("--json needs audit or convert".into());
    }
    if command == Command::Window && !subfolders {
        return Err(
            "--no-subfolders needs audit or convert; the window has a Subfolders chip".into(),
        );
    }
    if !format.supports_lossless() && quality == Quality::LOSSLESS {
        return Err("--lossless is available only with --webp or --jxl".into());
    }

    Ok(Args {
        root,
        command,
        format,
        quality,
        max_edge,
        grid,
        json,
        subfolders,
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
    let stdout = std::io::stdout();
    let mut out = stdout.lock();
    serde_json::to_writer(&mut out, value).map_err(|error| error.to_string())?;
    writeln!(out).map_err(|error| error.to_string())
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
    println!(
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
        println!("{} HEIC skipped (not supported yet)", summary.heic_skipped);
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
        println!(
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
    source_bytes: u64,
    output_bytes: Option<u64>,
    width: Option<u32>,
    height: Option<u32>,
    error: Option<String>,
}

struct ConversionRun {
    before: u64,
    after: u64,
    failed: usize,
    files: Vec<ConversionFile>,
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
    source_bytes: u64,
    output_bytes: u64,
    grew: bool,
    changed_bytes: u64,
}

#[derive(Serialize)]
struct ConversionReport {
    schema_version: u32,
    command: &'static str,
    target: String,
    output: String,
    subfolders: Option<bool>,
    options: ConversionOptions,
    scan: ScanSummary,
    summary: ConversionSummary,
    files: Vec<ConversionFile>,
    unreadable: Vec<String>,
    walk_errors: Vec<String>,
}

/// Convert without opening a window, so the same work is scriptable and testable.
fn convert_headless(
    root: &std::path::Path,
    out_dir: &std::path::Path,
    entries: &[Entry],
    format: Format,
    quality: Quality,
    max_edge: MaxEdge,
    json: bool,
) -> ConversionRun {
    let sources: Vec<PathBuf> = entries.iter().map(|entry| entry.path.clone()).collect();
    let by_path: HashMap<&Path, &Entry> = entries
        .iter()
        .map(|entry| (entry.path.as_path(), entry))
        .collect();

    // Lines arrive as files finish rather than in list order, which is what running
    // several at once looks like. The totals are the same either way.
    let totals = parking_lot::Mutex::new(ConversionRun {
        before: 0,
        after: 0,
        failed: 0,
        files: Vec::with_capacity(entries.len()),
    });
    convert::convert_each(
        root,
        &sources,
        out_dir,
        format,
        quality,
        max_edge,
        |source, converted| {
            let Some(entry) = by_path.get(source) else {
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
                        println!(
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
                        source_bytes: entry.bytes,
                        output_bytes: Some(converted.bytes),
                        width: Some(converted.width),
                        height: Some(converted.height),
                        error: None,
                    });
                }
                Err(error) => {
                    totals.failed += 1;
                    let reason = error
                        .reason()
                        .map(|reason| format!(": {reason}"))
                        .unwrap_or_default();
                    if !json {
                        println!("{:<52} failed{reason}", entry.name());
                    }
                    totals.files.push(ConversionFile {
                        source: path_text(source),
                        status: "failed",
                        output: None,
                        source_bytes: entry.bytes,
                        output_bytes: None,
                        width: None,
                        height: None,
                        error: Some(
                            error
                                .reason()
                                .unwrap_or_else(|| "conversion failed".to_string()),
                        ),
                    });
                }
            }
        },
    );
    let mut totals = totals.into_inner();
    totals
        .files
        .sort_by(|left, right| left.source.cmp(&right.source));

    if !json {
        let growth = totals.after > totals.before;
        let delta = totals.before.abs_diff(totals.after);
        let percent = delta as f64 / totals.before.max(1) as f64 * 100.;
        println!(
            "\n{} converted to {} at {} ({}): {} -> {}, {} {} ({percent:.0}%){}",
            entries.len() - totals.failed,
            format.label(),
            quality.label(),
            max_edge.label(),
            format_bytes(totals.before),
            format_bytes(totals.after),
            if growth { "grew" } else { "saved" },
            format_bytes(delta),
            if totals.failed == 0 {
                String::new()
            } else {
                format!(", {} failed", totals.failed)
            }
        );
        if entries.len() - totals.failed > 0 {
            println!("written to {}", out_dir.display());
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
            print!("{HELP}");
            return;
        }
        Command::Version => {
            println!("press {}", env!("CARGO_PKG_VERSION"));
            return;
        }
        Command::Skill => {
            print!("{AGENT_SKILL}");
            return;
        }
        Command::Update => {
            update_headless();
            return;
        }
        Command::Window | Command::Audit | Command::Convert => {}
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
                if args.command == Command::Audit {
                    "audit"
                } else {
                    "convert"
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
            },
            None,
            pending_crash,
        );
    };

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
        let output = target.join(scan::OUTPUT_DIR);
        (scan::scan(&target, &output), target.clone())
    } else {
        // The one-level read names files by their canonical root, so the root
        // follows it or the listing would lose its relative spelling.
        let output = target.join(scan::OUTPUT_DIR);
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
        println!(
            "{} images, {} on disk, {} camera raw skipped{}",
            scanned.entries.len(),
            format_bytes(scanned.entries.iter().map(|entry| entry.bytes).sum()),
            scanned.skipped_raw,
            scope_note(scope)
        );
        if scanned.skipped_heic > 0 {
            println!("{} HEIC skipped (not supported yet)", scanned.skipped_heic);
        }
        match scanned.skipped_packages {
            0 => {}
            1 => println!("1 macOS package skipped"),
            many => println!("{many} macOS packages skipped"),
        }
    }
    // Headless always writes the default `optimized/`, but it still has to be a usable
    // folder. Refusing here names the reason once instead of failing every file.
    let context = match settings::Output::Optimized.context(&root) {
        Ok(context) => context,
        Err(message) => {
            eprintln!("press: {message}");
            std::process::exit(2);
        }
    };
    let out_dir = context.output_root().to_path_buf();
    let run = convert_headless(
        &root,
        &out_dir,
        &scanned.entries,
        args.format,
        args.quality,
        args.max_edge,
        args.json,
    );
    let failed = run.failed;
    if args.json {
        let report = ConversionReport {
            schema_version: 1,
            command: "convert",
            target: path_text(&target),
            output: path_text(&out_dir),
            subfolders: scope,
            options: ConversionOptions {
                format: args.format.label(),
                quality: args.quality.0,
                max_edge: args.max_edge.0,
            },
            scan: ScanSummary::from_scan(&scanned),
            summary: ConversionSummary {
                attempted: scanned.entries.len(),
                converted: scanned.entries.len() - run.failed,
                failed: run.failed,
                source_bytes: run.before,
                output_bytes: run.after,
                grew: run.after > run.before,
                changed_bytes: run.before.abs_diff(run.after),
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

fn update_headless() {
    #[cfg(feature = "updater")]
    match update::install() {
        update::Outcome::Installed => {
            println!("Press updated; start it again to use the new version")
        }
        update::Outcome::Current => println!("Press {} is up to date", env!("CARGO_PKG_VERSION")),
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
    // Must run before any gpui-component type is constructed.
    gpui_component::init(cx);
    // Dark by default. Judging compression against a bright chrome is a bad idea,
    // and the comparison view is full-bleed imagery either way.
    gpui_component::Theme::change(gpui_component::ThemeMode::Dark, None, cx);
    // The stock dark theme paints `primary` white, which makes the one button that
    // commits work a white slab and leaves nothing to point at anything else. This
    // app already has a blue.
    //
    // Both halves of the theme have to be told. A button takes its fill from the
    // token set and its label from the colour set, so setting one and not the other
    // is how you get black text on a blue button.
    let theme = gpui_component::Theme::global_mut(cx);
    // One neutral ramp, barely blue, so the imagery in the table carries the
    // colour and the chrome reads as an instrument panel rather than a website.
    // Flat and hairline-precise: surfaces are separated by a line or a stripe,
    // never by a gradient or a shadow, which is what a pro Mac app looks like.
    let background = gpui::Hsla::from(gpui::rgb(0x0b0e14));
    let surface = gpui::Hsla::from(gpui::rgb(0x121926));
    let table = gpui::Hsla::from(gpui::rgb(0x0b0e14));
    // Every other row. The zebra does the work the row hairlines used to.
    let stripe = gpui::Hsla::from(gpui::rgb(0x0d111a));
    let border = gpui::Hsla::from(gpui::rgb(0x1b2331));
    let foreground = gpui::Hsla::from(gpui::rgb(0xe6ecf4));
    let muted = gpui::Hsla::from(gpui::rgb(0x8b98ab));
    let base = gpui::Hsla::from(gpui::rgb(0x4c8dff));
    let hover = gpui::Hsla::from(gpui::rgb(0x65a0ff));
    let active = gpui::Hsla::from(gpui::rgb(0x3b79e6));
    let focus = gpui::Hsla::from(gpui::rgb(0x8fbcff));

    theme.background = background;
    theme.secondary = surface;
    theme.table = table;
    theme.input = border;
    theme.border = border;
    theme.foreground = foreground;
    theme.muted_foreground = muted;
    theme.group_box = surface;
    theme.group_box_foreground = foreground;
    theme.list_hover = gpui::Hsla::from(gpui::rgb(0x141c28));
    theme.list_active = gpui::Hsla::from(gpui::rgb(0x16233a));
    theme.list_active_border = base;
    theme.table_head = gpui::Hsla::from(gpui::rgb(0x10151d));
    theme.table_head_foreground = muted;
    theme.table_hover = gpui::Hsla::from(gpui::rgb(0x131a25));
    // The stripe separates the rows, so a hairline under every one of them was
    // the same edge drawn twice.
    theme.table_row_border = gpui::transparent_black();
    theme.table_even = stripe;
    theme.ring = focus;

    // The table draws itself from the token set, not the colour set, so the
    // list stayed near-black under a blue-grey zebra until these were told too.
    theme.tokens.table = table.into();
    theme.tokens.table_head = gpui::Hsla::from(gpui::rgb(0x10151d)).into();
    theme.tokens.table_even = stripe.into();
    theme.tokens.table_hover = gpui::Hsla::from(gpui::rgb(0x131a25)).into();
    // Translucent on purpose: the table paints its selected row as an overlay
    // ON TOP of that row's cells, so an opaque colour here does not tint the
    // row, it hides it — tick, thumbnail, name and all.
    theme.tokens.table_active = gpui::Hsla::from(gpui::rgba(0x4c8dff1f)).into();
    theme.tokens.table_active_border = base.into();
    theme.tokens.table_row_border = gpui::transparent_black().into();

    theme.primary = base;
    theme.primary_hover = hover;
    theme.primary_active = active;
    theme.primary_foreground = gpui::white();
    theme.button_primary = base;
    theme.button_primary_hover = hover;
    theme.button_primary_active = active;
    theme.button_primary_foreground = gpui::white();

    theme.tokens.button_primary = base.into();
    theme.tokens.button_primary_hover = hover.into();
    theme.tokens.button_primary_active = active.into();
    theme.tokens.button_primary_foreground = gpui::white().into();

    // The two findings share one amber, and saved bytes are the one green.
    theme.yellow = gpui::Hsla::from(gpui::rgb(0xe0b054));
    theme.green = gpui::Hsla::from(gpui::rgb(0x4ade80));

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
}

/// `Root` owns these overlays but leaves their placement to the app's content view.
struct WindowContent {
    audit: gpui::Entity<audit::Audit>,
}

impl Render for WindowContent {
    fn render(
        &mut self,
        window: &mut gpui::Window,
        cx: &mut gpui::Context<Self>,
    ) -> impl IntoElement {
        let sheet = Root::render_sheet_layer(window, cx);
        let dialog = Root::render_dialog_layer(window, cx);
        let notifications = Root::render_notification_layer(window, cx);
        gpui::div()
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
            cx.set_quit_mode(gpui::QuitMode::LastWindowClosed);

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
                                if let Err(error) = audit.flush_settings() {
                                    eprintln!("press: could not save settings: {error}");
                                }
                                true
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
    use gpui::{Context, IntoElement, Render, TestAppContext};

    struct CrashWindowHarness;

    impl Render for CrashWindowHarness {
        fn render(
            &mut self,
            window: &mut gpui::Window,
            cx: &mut Context<Self>,
        ) -> impl IntoElement {
            gpui::div()
                .size_full()
                .children(Root::render_dialog_layer(window, cx))
        }
    }

    fn parse(arguments: &[&str]) -> Result<Args, String> {
        parse_args_from(arguments.iter().map(|argument| (*argument).to_string()))
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

    #[test]
    fn a_missing_value_is_an_error() {
        for arguments in [["--quality"].as_slice(), ["--max-edge"].as_slice()] {
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
        let entries = vec![scan::probe(&source).expect("the fixture is an image")];

        let context = settings::Output::Optimized
            .context(&root)
            .expect("the default output establishes");
        let run = convert_headless(
            &root,
            context.output_root(),
            &entries,
            Format::WebP,
            Quality::lossy(80.),
            MaxEdge::FULL,
            true,
        );

        assert_eq!(run.failed, 0);
        assert_eq!(run.files.len(), 1);
        assert_eq!(run.files[0].status, "converted");
        assert!(root.join(scan::OUTPUT_DIR).join("photo.webp").is_file());
        std::fs::remove_dir_all(&root).unwrap();
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

    #[gpui::test]
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
