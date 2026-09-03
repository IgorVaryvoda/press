//! Remember where the window was and what you were looking at.
//!
//! A tiny key=value file rather than a config crate. There are a handful of values, and a
//! dependency that walks platform config directories costs more than the ten lines
//! it would save.

use std::io::{self, Write};
use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::sync::atomic::{AtomicU64, Ordering};

use parking_lot::Mutex;

#[derive(Clone, Debug, Default, PartialEq)]
pub struct Settings {
    pub width: Option<f32>,
    pub height: Option<f32>,
    pub folder: Option<PathBuf>,
    pub recent_folders: Vec<PathBuf>,
    pub columns: ColumnPrefs,
    pub output: Output,
    /// Whether the window walks the whole tree like the command line does. Off
    /// by default so a folder opens exactly as it did before the choice existed.
    pub include_subfolders: bool,
    /// libaom's speed dial for AVIF output. `None` is the built-in default; the
    /// window has no control for it, so this and `--avif-speed` are the two ways
    /// to say anything else. Range-checked once, in `avif::set_speed`.
    pub avif_speed: Option<u8>,
}

pub const MAX_RECENT_FOLDERS: usize = 5;

/// Where converted files and local-model results land.
///
/// The default keeps them beside the originals in `optimized/`, which is what
/// makes "originals unchanged" true and easy to check. A chosen folder is for
/// people whose output belongs somewhere else entirely — a staging directory, a
/// share, a build tree.
///
/// `Replace` is the job most people actually came for: make this folder's images
/// smaller, in place, without merging a second tree back over the first by hand.
/// It is opt-in and it still keeps every original — each one moves into
/// `press-originals/` before its replacement takes its name.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub enum Output {
    #[default]
    Optimized,
    Folder(PathBuf),
    Replace,
}

impl Output {
    /// The folder outputs are written into, for a given audited root.
    pub fn root(&self, audited: &Path) -> PathBuf {
        match self {
            Output::Optimized => audited.join(crate::scan::OUTPUT_DIR),
            Output::Folder(path) => path.clone(),
            Output::Replace => audited.to_path_buf(),
        }
    }

    /// Establish the selected output boundary without creating a path or file.
    pub fn context(&self, audited: &Path) -> Result<Arc<crate::output::Context>, String> {
        let working_directory = std::env::current_dir().map_err(|error| error.to_string())?;
        self.context_with_working_directory(audited, working_directory)
    }

    fn context_with_working_directory(
        &self,
        audited: &Path,
        working_directory: PathBuf,
    ) -> Result<Arc<crate::output::Context>, String> {
        let audited = if audited.as_os_str().is_empty() {
            working_directory.clone()
        } else {
            crate::output::lexical_normalize_against(audited, &working_directory)
                .map_err(|error| error.to_string())?
        };
        let context = match self {
            Output::Optimized => {
                crate::output::Context::establish_default_child_with_working_directory(
                    &audited,
                    Path::new(crate::scan::OUTPUT_DIR),
                    working_directory,
                )
            }
            Output::Folder(output) => {
                let output = crate::output::lexical_normalize_against(output, &working_directory)
                    .map_err(|error| error.to_string())?;
                crate::output::Context::establish_with_working_directory(
                    &audited,
                    &output,
                    working_directory,
                )
            }
            Output::Replace => crate::output::Context::establish_replace_with_working_directory(
                &audited,
                working_directory,
            ),
        };
        context.map(Arc::new).map_err(|error| error.to_string())
    }

    /// How the destination reads in the window: a name for the default, the
    /// path itself for anywhere else.
    pub fn label(&self) -> String {
        match self {
            Output::Optimized => format!("{}/", crate::scan::OUTPUT_DIR),
            Output::Folder(path) => path.display().to_string(),
            Output::Replace => "beside the originals".to_string(),
        }
    }
}

/// Which optional table columns are on. Sirv and Result are not here: they
/// appear only when a pairing or a conversion exists, which is a fact about the
/// folder rather than a preference.
///
/// B/px is off by default. It is the audit's sharpest number and its least
/// legible one; a file carrying too many bytes per pixel says `heavy` in its own
/// row instead, and anyone who wants the figure ticks it back on.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct ColumnPrefs {
    pub thumb: bool,
    pub format: bool,
    pub pixels: bool,
    pub density: bool,
    pub weight: bool,
}

impl Default for ColumnPrefs {
    fn default() -> Self {
        Self {
            thumb: true,
            format: true,
            pixels: true,
            density: false,
            weight: true,
        }
    }
}

impl ColumnPrefs {
    /// Written and read as the list of columns that are ON, so a settings file
    /// from an older build has no `columns` line and gets the defaults.
    fn render(&self) -> String {
        [
            ("thumb", self.thumb),
            ("format", self.format),
            ("pixels", self.pixels),
            ("density", self.density),
            ("weight", self.weight),
        ]
        .into_iter()
        .filter_map(|(key, on)| on.then_some(key))
        .collect::<Vec<_>>()
        .join(",")
    }

    fn parse(text: &str) -> Self {
        let mut prefs = Self {
            thumb: false,
            format: false,
            pixels: false,
            density: false,
            weight: false,
        };
        for key in text.split(',') {
            match key.trim() {
                "thumb" => prefs.thumb = true,
                "format" => prefs.format = true,
                "pixels" => prefs.pixels = true,
                "density" => prefs.density = true,
                "weight" => prefs.weight = true,
                _ => {}
            }
        }
        prefs
    }
}

/// Where the file lives on each platform. `None` when the environment gives us
/// nothing usable, in which case nothing is remembered and nothing breaks.
pub fn path() -> Option<PathBuf> {
    let base = if cfg!(target_os = "windows") {
        std::env::var_os("APPDATA").map(PathBuf::from)
    } else if cfg!(target_os = "macos") {
        std::env::var_os("HOME")
            .map(PathBuf::from)
            .map(|home| home.join("Library/Application Support"))
    } else {
        std::env::var_os("XDG_CONFIG_HOME")
            .map(PathBuf::from)
            .or_else(|| {
                std::env::var_os("HOME")
                    .map(PathBuf::from)
                    .map(|home| home.join(".config"))
            })
    }?;

    // The folder keeps the old name on purpose. Renaming it to `press` would
    // orphan every existing settings file and Sirv credential pair without a
    // migration, and losing a saved secret to a cosmetic change is not a trade.
    Some(base.join("imageguide").join("settings"))
}

pub fn load() -> Settings {
    let Some(text) = path().and_then(|path| std::fs::read_to_string(path).ok()) else {
        return Settings::default();
    };
    parse(&text)
}

/// The path a new writer persists to: the real config file, except in tests,
/// where a render must not touch the user's real config file. Test audits get
/// a unique temp path each, so stray debounced saves land complete and silent
/// instead of failing or escaping; tests that assert contents inject their own.
pub(crate) fn default_writer_path() -> Option<PathBuf> {
    #[cfg(not(test))]
    return path();
    #[cfg(test)]
    {
        let unique = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .map_or(0, |since| since.as_nanos());
        Some(std::env::temp_dir().join(format!("press-test-settings-{unique}/settings")))
    }
}

/// Why a settings write did not land. Stage errors keep their `io::Error` and
/// its kind; a missing config directory is the only refusal without one.
#[derive(Debug)]
pub enum SaveError {
    NoConfigDir,
    ParentCreation { path: PathBuf, error: io::Error },
    Stage { error: io::Error },
    Write { error: io::Error },
    Sync { error: io::Error },
    Replace { error: io::Error },
}

impl std::fmt::Display for SaveError {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::NoConfigDir => write!(formatter, "no config directory is available"),
            Self::ParentCreation { path, error } => {
                write!(
                    formatter,
                    "could not create the settings folder {}: {error}",
                    path.display()
                )
            }
            Self::Stage { error } => {
                write!(formatter, "could not stage the settings: {error}")
            }
            Self::Write { error } => {
                write!(formatter, "could not write the staged settings: {error}")
            }
            Self::Sync { error } => {
                write!(formatter, "could not sync the staged settings: {error}")
            }
            Self::Replace { error } => {
                write!(formatter, "could not install the staged settings: {error}")
            }
        }
    }
}

impl std::error::Error for SaveError {
    fn source(&self) -> Option<&(dyn std::error::Error + 'static)> {
        match self {
            Self::NoConfigDir => None,
            Self::ParentCreation { error, .. }
            | Self::Stage { error }
            | Self::Write { error }
            | Self::Sync { error }
            | Self::Replace { error } => Some(error),
        }
    }
}

/// What one ordered write did. Every variant carries the revision it acted on,
/// so a caller can tell a superseded drag from a failed disk.
// Revisions are asserted by the contract tests; production routes on the
// variant and only the notice path reads the error.
#[allow(dead_code)]
#[derive(Debug)]
pub enum WriteOutcome {
    Written {
        revision: u64,
        warning: Option<crate::output::DurabilityWarning>,
    },
    Superseded {
        revision: u64,
    },
    Failed {
        revision: u64,
        error: SaveError,
    },
}

/// One lock plus a revision counter for every settings write the process does.
/// The mutex serializes filesystem writes; the counter retires overlapped
/// work: after taking the lock, a revision older than the latest claimed one
/// returns `Superseded` without touching the disk.
pub struct SettingsWriter {
    path: Option<PathBuf>,
    lock: Mutex<()>,
    latest: AtomicU64,
}

impl SettingsWriter {
    pub fn new(path: Option<PathBuf>) -> Arc<Self> {
        Arc::new(Self {
            path,
            lock: Mutex::new(()),
            latest: AtomicU64::new(0),
        })
    }

    /// Claim the next revision. The debounced save calls this at every change
    /// and writes only the newest claim; the synchronous flush calls it and
    /// always writes, so an older task can neither overlap nor land afterward.
    pub fn next_revision(&self) -> u64 {
        self.latest.fetch_add(1, Ordering::SeqCst) + 1
    }

    /// Write unless a newer revision was claimed meanwhile. Runs off the UI
    /// thread; concurrent calls serialize on the writer lock.
    pub fn write_if_latest(&self, revision: u64, settings: &Settings) -> WriteOutcome {
        let _guard = self.lock.lock();
        if revision < self.latest.load(Ordering::SeqCst) {
            return WriteOutcome::Superseded { revision };
        }
        self.write_locked(revision, settings)
    }

    /// Claim a newer revision and write it now, waiting for the same lock.
    /// Unconditional: quit and updater restart must persist, and the fresh
    /// revision keeps any older task superseded after it.
    pub fn flush(&self, settings: &Settings) -> WriteOutcome {
        let revision = self.next_revision();
        let _guard = self.lock.lock();
        self.write_locked(revision, settings)
    }

    fn write_locked(&self, revision: u64, settings: &Settings) -> WriteOutcome {
        let Some(path) = self.path.as_ref() else {
            return WriteOutcome::Failed {
                revision,
                error: SaveError::NoConfigDir,
            };
        };
        match save_to(path, settings, Fault::None) {
            Ok(warning) => WriteOutcome::Written { revision, warning },
            Err(error) => WriteOutcome::Failed { revision, error },
        }
    }
}

/// Test-only failure injection for [`save_to`]. Each variant fails exactly one
/// stage; production always runs `Fault::None`.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
enum Fault {
    #[default]
    None,
    Parent,
    Temp,
    Write,
    Sync,
    Replace,
    /// Never constructed on Windows: directory sync is an explicit no-warning
    /// platform outcome there, and only the non-Windows warning test builds it.
    #[cfg_attr(windows, allow(dead_code))]
    ParentSync,
}

fn injected(stage: &str) -> io::Error {
    io::Error::other(format!("injected {stage} failure"))
}

fn save_to(
    path: &Path,
    settings: &Settings,
    fault: Fault,
) -> Result<Option<crate::output::DurabilityWarning>, SaveError> {
    if fault == Fault::Parent {
        return Err(SaveError::ParentCreation {
            path: path.to_path_buf(),
            error: injected("parent creation"),
        });
    }
    if let Some(parent) = path.parent() {
        std::fs::create_dir_all(parent).map_err(|error| SaveError::ParentCreation {
            path: parent.to_path_buf(),
            error,
        })?;
    }
    // Unique per call, not per process: two writers in one process (or a new
    // process reusing a killed one's pid name) stage side by side, and the
    // atomic commit still lands exactly one complete file.
    static STAGE_COUNTER: AtomicU64 = AtomicU64::new(0);
    let mut temporary = path.as_os_str().to_owned();
    temporary.push(format!(
        ".{}.{}.part",
        std::process::id(),
        STAGE_COUNTER.fetch_add(1, Ordering::SeqCst)
    ));
    let temporary = PathBuf::from(temporary);
    let _ = std::fs::remove_file(&temporary);
    let result = (|| {
        if fault == Fault::Temp {
            return Err(SaveError::Stage {
                error: injected("temp creation"),
            });
        }
        let mut file = std::fs::OpenOptions::new()
            .write(true)
            .create_new(true)
            .open(&temporary)
            .map_err(|error| SaveError::Stage { error })?;
        file.write_all(render(settings).as_bytes())
            .map_err(|error| SaveError::Write { error })?;
        if fault == Fault::Write {
            return Err(SaveError::Write {
                error: injected("write"),
            });
        }
        if fault == Fault::Sync {
            return Err(SaveError::Sync {
                error: injected("file sync"),
            });
        }
        file.sync_all().map_err(|error| SaveError::Sync { error })?;
        drop(file);
        if fault == Fault::Replace {
            return Err(SaveError::Replace {
                error: injected("replace"),
            });
        }
        replace(&temporary, path, fault)
    })();
    if result.is_err() {
        let _ = std::fs::remove_file(&temporary);
    }
    result
}

/// Atomically replace the old settings with the staged file: the previous
/// contents stay byte-identical on every injected or real failure, and temp
/// residue is cleaned by the caller. Unix renames over the old file and syncs
/// the parent best-effort with a warning; Windows replaces atomically first
/// and only falls back to a non-replacing move when the destination is
/// missing, retrying the replace once on a race. Never delete-first: a crash
/// between delete and rename would leave no settings at all.
#[cfg(not(windows))]
fn replace(
    from: &Path,
    to: &Path,
    fault: Fault,
) -> Result<Option<crate::output::DurabilityWarning>, SaveError> {
    std::fs::rename(from, to).map_err(|error| SaveError::Replace { error })?;
    if fault == Fault::ParentSync {
        return Ok(Some(crate::output::DurabilityWarning(format!(
            "could not sync the settings directory: {}",
            injected("parent sync")
        ))));
    }
    let parent = to.parent().unwrap_or(to);
    match std::fs::File::open(parent).and_then(|file| file.sync_all()) {
        Ok(()) => Ok(None),
        Err(error) => Ok(Some(crate::output::DurabilityWarning(format!(
            "could not sync the settings directory {}: {error}",
            parent.display()
        )))),
    }
}

#[cfg(windows)]
fn replace(
    from: &Path,
    to: &Path,
    fault: Fault,
) -> Result<Option<crate::output::DurabilityWarning>, SaveError> {
    let _ = fault;
    windows_replace(from, to).map_err(|error| SaveError::Replace { error })?;
    Ok(None)
}

/// Atomic replacement through the Win32 API: `ReplaceFileW` first, a
/// non-replacing `MoveFileExW` only when the destination is missing, and one
/// `ReplaceFileW` retry when that move reports a race. A second race stays a
/// typed error with the old-or-new complete file intact.
#[cfg(windows)]
fn windows_replace(from: &Path, to: &Path) -> io::Result<()> {
    use std::os::windows::ffi::OsStrExt;

    fn wide(path: &Path) -> Vec<u16> {
        path.as_os_str()
            .encode_wide()
            .chain(std::iter::once(0))
            .collect()
    }

    let from_wide = wide(from);
    let to_wide = wide(to);
    // SAFETY: both buffers outlive the calls; backup, exclude and reserved
    // stay null, which the API documents as no backup and no exclusion.
    let replaced = unsafe {
        windows_sys::Win32::Storage::FileSystem::ReplaceFileW(
            to_wide.as_ptr(),
            from_wide.as_ptr(),
            std::ptr::null(),
            0,
            std::ptr::null(),
            std::ptr::null(),
        )
    } != 0;
    if replaced {
        return Ok(());
    }
    let error = io::Error::last_os_error();
    if error.kind() != io::ErrorKind::NotFound {
        return Err(error);
    }
    let moved = unsafe {
        windows_sys::Win32::Storage::FileSystem::MoveFileExW(
            from_wide.as_ptr(),
            to_wide.as_ptr(),
            0,
        )
    } != 0;
    if moved {
        return Ok(());
    }
    let error = io::Error::last_os_error();
    if error.kind() != io::ErrorKind::AlreadyExists {
        return Err(error);
    }
    let replaced = unsafe {
        windows_sys::Win32::Storage::FileSystem::ReplaceFileW(
            to_wide.as_ptr(),
            from_wide.as_ptr(),
            std::ptr::null(),
            0,
            std::ptr::null(),
            std::ptr::null(),
        )
    } != 0;
    if replaced {
        Ok(())
    } else {
        Err(io::Error::last_os_error())
    }
}

fn parse(text: &str) -> Settings {
    let mut settings = Settings::default();
    for line in text.lines() {
        let Some((key, value)) = line.split_once('=') else {
            continue;
        };
        match key.trim() {
            "width" => settings.width = value.trim().parse().ok(),
            "height" => settings.height = value.trim().parse().ok(),
            "folder" => settings.folder = Some(PathBuf::from(value.trim())),
            "recent_folder" => settings.recent_folders.push(PathBuf::from(value.trim())),
            "columns" => settings.columns = ColumnPrefs::parse(value),
            "subfolders" => settings.include_subfolders = value.trim() == "1",
            // Range-checked in `avif::set_speed`, which is where every source of this
            // value goes through. Parsing only has to reject what is not a number.
            "avif_speed" => settings.avif_speed = value.trim().parse().ok(),
            "output" => {
                let value = value.trim();
                // Replace mode is never inherited from a file. It rewrites the
                // folder it is pointed at, and a launch is not the moment to
                // discover that; a line an older build wrote reads as the default.
                settings.output = match value {
                    "" | "replace" => Output::Optimized,
                    path => Output::Folder(PathBuf::from(path)),
                };
            }
            _ => {}
        }
    }
    settings
}

fn render(settings: &Settings) -> String {
    let mut out = String::new();
    if let Some(width) = settings.width {
        out.push_str(&format!("width={width}\n"));
    }
    if let Some(height) = settings.height {
        out.push_str(&format!("height={height}\n"));
    }
    if let Some(folder) = settings.folder.as_ref() {
        out.push_str(&format!("folder={}\n", folder.display()));
    }
    for folder in settings.recent_folders.iter().take(MAX_RECENT_FOLDERS) {
        out.push_str(&format!("recent_folder={}\n", folder.display()));
    }
    // Always written, including when every column is off: an empty value is a
    // choice, and leaving the line out would restore the defaults next launch.
    out.push_str(&format!("columns={}\n", settings.columns.render()));
    // The default writes no path, so a settings file says nothing about where
    // output goes until somebody chooses somewhere else.
    match &settings.output {
        Output::Optimized | Output::Replace => {}
        Output::Folder(path) => out.push_str(&format!("output={}\n", path.display())),
    }
    // Same shape as `output`: the default writes nothing, so a file from before the
    // choice existed keeps opening one level at a time.
    if settings.include_subfolders {
        out.push_str("subfolders=1\n");
    }
    if let Some(speed) = settings.avif_speed {
        out.push_str(&format!("avif_speed={speed}\n"));
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    fn temp_dir(name: &str) -> PathBuf {
        let unique = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .unwrap()
            .as_nanos();
        let path = std::env::temp_dir().join(format!("press-settings-{name}-{unique}"));
        std::fs::create_dir_all(&path).unwrap();
        // macOS aliases `/var` to `/private/var`; `Context` canonicalizes, so
        // the fixture has to start from the canonical spelling too.
        path.canonicalize().unwrap()
    }

    #[test]
    fn dot_audit_root_is_normalized_before_context_establishment() {
        let base = temp_dir("dot-audit-root");
        let context = Output::Optimized
            .context_with_working_directory(Path::new("."), base.clone())
            .unwrap();
        assert_eq!(context.source_root(), base);
        assert_eq!(context.output_root(), base.join(crate::scan::OUTPUT_DIR));
        assert!(!context.output_root().exists());
        std::fs::remove_dir_all(base).unwrap();
    }

    #[test]
    fn parent_relative_audit_root_is_normalized_before_context_establishment() {
        let base = temp_dir("parent-relative-audit-root");
        let working_directory = base.join("working");
        let source = base.join("folder");
        std::fs::create_dir(&working_directory).unwrap();
        std::fs::create_dir(&source).unwrap();
        let context = Output::Optimized
            .context_with_working_directory(Path::new("../folder"), working_directory)
            .unwrap();
        assert_eq!(context.source_root(), source);
        assert_eq!(context.output_root(), source.join(crate::scan::OUTPUT_DIR));
        assert!(!context.output_root().exists());
        std::fs::remove_dir_all(base).unwrap();
    }

    #[test]
    fn relative_custom_output_uses_the_same_working_directory() {
        let base = temp_dir("relative-custom-output");
        let working_directory = base.join("working");
        let source = base.join("photos");
        std::fs::create_dir(&working_directory).unwrap();
        std::fs::create_dir(&source).unwrap();
        let context = Output::Folder(PathBuf::from("exports"))
            .context_with_working_directory(Path::new("../photos"), working_directory.clone())
            .unwrap();
        assert_eq!(context.source_root(), source);
        assert_eq!(context.output_root(), working_directory.join("exports"));
        assert!(!context.output_root().exists());
        std::fs::remove_dir_all(base).unwrap();
    }

    #[cfg(unix)]
    #[test]
    fn default_output_for_a_symlinked_audit_uses_the_canonical_root() {
        let base = temp_dir("symlinked-audit-root");
        let target = base.join("target");
        let alias = base.join("alias");
        std::fs::create_dir(&target).unwrap();
        std::os::unix::fs::symlink(&target, &alias).unwrap();
        let context = Output::Optimized
            .context_with_working_directory(&alias, base.clone())
            .unwrap();
        assert_eq!(context.source_root(), target);
        assert_eq!(context.output_root(), target.join(crate::scan::OUTPUT_DIR));
        assert_eq!(
            context.relative_source(&alias.join("image.png")).unwrap(),
            Path::new("image.png")
        );
        assert!(!context.output_root().exists());
        std::fs::remove_dir_all(base).unwrap();
    }

    #[test]
    fn a_saved_file_reads_back_the_same() {
        let settings = Settings {
            width: Some(1280.),
            height: Some(720.5),
            folder: Some(PathBuf::from("/photos/library")),
            recent_folders: vec![
                PathBuf::from("/photos/library"),
                PathBuf::from("/photos/exports"),
            ],
            columns: ColumnPrefs::default(),
            output: Output::Folder(PathBuf::from("/exports/web")),
            include_subfolders: true,
            avif_speed: Some(8),
        };
        assert_eq!(parse(&render(&settings)), settings);
    }

    /// The window has no control for the AVIF speed, so the file is the only place a
    /// chosen one lives between launches. The default writes nothing, and junk in the
    /// line leaves the default alone rather than encoding at some invented speed.
    #[test]
    fn an_avif_speed_survives_the_settings_file() {
        let chosen = Settings {
            avif_speed: Some(9),
            ..Settings::default()
        };
        assert_eq!(parse(&render(&chosen)).avif_speed, Some(9));

        assert!(!render(&Settings::default()).contains("avif_speed"));
        assert_eq!(parse("avif_speed=0\n").avif_speed, Some(0));
        // Out of range is carried, not dropped: `avif::set_speed` clamps it, and the
        // window then writes the clamped value back.
        assert_eq!(parse("avif_speed=11\n").avif_speed, Some(11));
        assert_eq!(parse("avif_speed=fast\n").avif_speed, None);
        assert_eq!(parse("avif_speed=999\n").avif_speed, None);
    }

    /// The default writes no `output` line, so an older settings file — and a
    /// hand-edited one that drops it — keeps writing beside the originals.
    #[test]
    fn a_chosen_output_folder_survives_a_round_trip() {
        let chosen = Settings {
            output: Output::Folder(PathBuf::from("/exports/web assets")),
            ..Settings::default()
        };
        assert_eq!(parse(&render(&chosen)).output, chosen.output);

        let default = Settings::default();
        assert!(!render(&default).contains("output="));
        assert_eq!(parse(&render(&default)).output, Output::Optimized);
        assert_eq!(
            Output::Optimized.root(Path::new("/photos")),
            Path::new("/photos/optimized")
        );
    }

    /// Replace mode is asked for per folder, never remembered. A settings file
    /// that carried it would rewrite the next folder somebody opened.
    #[test]
    fn replacing_in_place_is_never_persisted_and_never_read_back() {
        let replacing = Settings {
            output: Output::Replace,
            ..Settings::default()
        };
        assert!(!render(&replacing).contains("output="));
        assert_eq!(parse(&render(&replacing)).output, Output::Optimized);
        assert_eq!(parse("output=replace\n").output, Output::Optimized);
        assert_eq!(
            Output::Replace.root(Path::new("/photos")),
            Path::new("/photos")
        );
    }

    /// The boundary every other destination has to clear is "does not contain the
    /// source". Replace mode is the audited folder, so it needs its own door.
    #[test]
    fn replace_mode_establishes_the_audited_folder_as_its_own_output() {
        let base = temp_dir("replace-context");
        let context = Output::Replace
            .context_with_working_directory(&base, base.clone())
            .expect("replace mode establishes");
        assert_eq!(context.source_root(), base);
        assert_eq!(context.output_root(), base);
        assert_eq!(
            context.final_path(Path::new("shot.webp")).unwrap(),
            base.join("shot.webp")
        );
        assert!(
            Output::Folder(base.clone())
                .context_with_working_directory(&base, base.clone())
                .is_err(),
            "choosing the audited folder by hand is still refused"
        );
        std::fs::remove_dir_all(base).unwrap();
    }

    /// A hand-edited or half-written file must not stop the app opening.
    #[test]
    fn nonsense_is_ignored_rather_than_fatal() {
        let settings = parse("width=not-a-number\nnokeyhere\n\nheight=600\n");
        assert_eq!(settings.width, None);
        assert_eq!(settings.height, Some(600.));
        assert_eq!(settings.folder, None);
    }

    #[test]
    fn the_subfolders_choice_round_trips() {
        let on = Settings {
            include_subfolders: true,
            ..Settings::default()
        };
        assert!(render(&on).contains("subfolders=1\n"));
        assert!(parse(&render(&on)).include_subfolders);

        let default = Settings::default();
        assert!(!render(&default).contains("subfolders="));
        assert!(!parse(&render(&default)).include_subfolders);
        assert!(!parse("subfolders=maybe\n").include_subfolders);
    }

    #[test]
    fn a_folder_with_spaces_survives() {
        let settings = Settings {
            folder: Some(PathBuf::from("/photos/My Holiday")),
            ..Settings::default()
        };
        assert_eq!(parse(&render(&settings)).folder, settings.folder);
    }

    #[test]
    fn a_save_replaces_the_whole_file_without_leaving_a_partial() {
        // A Rust test thread is named after its module path, so the old name
        // carried `::` and Windows rejected the directory outright.
        let dir = temp_dir("save-replaces-whole-file");
        let path = dir.join("settings");
        std::fs::write(&path, "width=1\ntrailing=old\n").unwrap();

        let settings = Settings {
            width: Some(900.),
            ..Settings::default()
        };
        save_to(&path, &settings, Fault::None).unwrap();

        assert_eq!(
            std::fs::read_to_string(&path).unwrap(),
            "width=900\ncolumns=thumb,format,pixels,weight\n"
        );
        assert_eq!(std::fs::read_dir(&dir).unwrap().count(), 1);
        let _ = std::fs::remove_dir_all(dir);
    }

    fn widths(width: f32) -> Settings {
        Settings {
            width: Some(width),
            ..Settings::default()
        }
    }

    #[test]
    fn serialized_settings_writer_contract() {
        let dir = temp_dir("writer-contract");
        let path = dir.join("settings");
        let writer = SettingsWriter::new(Some(path.clone()));

        let first = writer.next_revision();
        match writer.write_if_latest(first, &widths(100.)) {
            WriteOutcome::Written { revision, warning } => {
                assert_eq!(revision, first);
                assert!(warning.is_none());
            }
            outcome => panic!("the first claim writes: {outcome:?}"),
        }
        assert_eq!(
            parse(&std::fs::read_to_string(&path).unwrap()).width,
            Some(100.)
        );

        // A newer claim retires the older one without touching the disk.
        let second = writer.next_revision();
        match writer.write_if_latest(first, &widths(200.)) {
            WriteOutcome::Superseded { revision } => assert_eq!(revision, first),
            outcome => panic!("the older claim is retired: {outcome:?}"),
        }
        assert_eq!(
            parse(&std::fs::read_to_string(&path).unwrap()).width,
            Some(100.)
        );
        writer.write_if_latest(second, &widths(200.));
        assert_eq!(
            parse(&std::fs::read_to_string(&path).unwrap()).width,
            Some(200.)
        );

        // The synchronous flush always writes its own newer revision.
        match writer.flush(&widths(300.)) {
            WriteOutcome::Written { revision, .. } => assert!(revision > second),
            outcome => panic!("the flush writes: {outcome:?}"),
        }
        assert_eq!(
            parse(&std::fs::read_to_string(&path).unwrap()).width,
            Some(300.)
        );

        // Without a config directory the revision still comes back, as a failure.
        let homeless = SettingsWriter::new(None);
        match homeless.write_if_latest(homeless.next_revision(), &widths(1.)) {
            WriteOutcome::Failed { error, .. } => {
                assert!(matches!(error, SaveError::NoConfigDir), "typed: {error}")
            }
            outcome => panic!("no directory fails: {outcome:?}"),
        }
        let _ = std::fs::remove_dir_all(dir);
    }

    #[test]
    fn concurrent_settings_writes_leave_only_new_bytes() {
        let dir = temp_dir("writer-races");
        let path = dir.join("settings");
        let writer = SettingsWriter::new(Some(path.clone()));
        let flushed = widths(300.);

        // An older revision queued behind the lock while a flush claims newer
        // and queues behind it. Whichever acquires first, the file ends on the
        // flush: the old revision is retired either at its check or before it
        // ever runs, so join alone orders the assertions, with no sleeps.
        let old = writer.next_revision();
        let _ = writer.next_revision();
        let _held = writer.lock.lock();
        let queued = {
            let writer = writer.clone();
            std::thread::spawn(move || writer.write_if_latest(old, &widths(100.)))
        };
        let flushing = {
            let writer = writer.clone();
            let flushed = flushed.clone();
            std::thread::spawn(move || writer.flush(&flushed))
        };
        drop(_held);
        let delayed = queued.join().expect("the delayed write finishes");
        let outcome = flushing.join().expect("the flush finishes");
        assert!(
            matches!(delayed, WriteOutcome::Superseded { .. }),
            "the delayed write never lands: {delayed:?}"
        );
        assert!(
            matches!(outcome, WriteOutcome::Written { .. }),
            "the flush lands: {outcome:?}"
        );
        assert_eq!(
            parse(&std::fs::read_to_string(&path).unwrap()).width,
            Some(300.)
        );
        let _ = std::fs::remove_dir_all(dir);
    }

    #[test]
    fn settings_replace_keeps_the_old_file_on_every_injected_failure() {
        let dir = temp_dir("save-faults");
        let path = dir.join("settings");
        std::fs::write(&path, "width=1\n").unwrap();
        for (fault, expected) in [
            (Fault::Parent, "ParentCreation"),
            (Fault::Temp, "Stage"),
            (Fault::Write, "Write"),
            (Fault::Sync, "Sync"),
            (Fault::Replace, "Replace"),
        ] {
            let error = save_to(&path, &widths(2.), fault).unwrap_err();
            let actual = match &error {
                SaveError::ParentCreation { .. } => "ParentCreation",
                SaveError::Stage { .. } => "Stage",
                SaveError::Write { .. } => "Write",
                SaveError::Sync { .. } => "Sync",
                SaveError::Replace { .. } => "Replace",
                other => panic!("{fault:?} failed as {other:?} instead of {expected}"),
            };
            assert_eq!(actual, expected);
            assert_eq!(
                std::fs::read_to_string(&path).unwrap(),
                "width=1\n",
                "{expected} keeps the old file byte-identical"
            );
        }
        assert_eq!(
            std::fs::read_dir(&dir).unwrap().count(),
            1,
            "no staged temp lingers"
        );
        let _ = std::fs::remove_dir_all(dir);
    }

    #[test]
    fn concurrent_settings_saves_never_tear() {
        let dir = temp_dir("save-races");
        let path = dir.join("settings");
        let candidates: Vec<String> = (0..8).map(|index| render(&widths(index as f32))).collect();
        let outcomes: Vec<_> = std::thread::scope(|scope| {
            candidates
                .iter()
                .map(|rendered| scope.spawn(|| save_to(&path, &parse(rendered), Fault::None)))
                .collect::<Vec<_>>()
                .into_iter()
                .map(|handle| handle.join().expect("a saver finishes"))
                .collect()
        });
        // A lost race stays a typed replace error with the old-or-new complete
        // file intact — Windows cannot atomically replace one file from eight
        // writers at once, and must not tear it trying.
        for outcome in &outcomes {
            match outcome {
                Ok(_) => {}
                Err(SaveError::Replace { .. }) => {}
                Err(error) => panic!("a race stays typed: {error:?}"),
            }
        }
        let landed = std::fs::read_to_string(&path).unwrap();
        assert!(
            candidates.iter().any(|candidate| candidate == &landed),
            "the file is one whole render, never a mixture"
        );
        let _ = std::fs::remove_dir_all(dir);
    }

    #[test]
    #[cfg(not(windows))]
    fn a_parent_sync_failure_warns_without_failing() {
        let dir = temp_dir("save-warning");
        let path = dir.join("settings");
        let outcome = save_to(&path, &widths(1.), Fault::ParentSync).unwrap();
        let warning = outcome.expect("a sync failure is not a save failure");
        assert!(
            warning.0.contains("sync"),
            "the warning says what: {warning}"
        );
        assert_eq!(
            parse(&std::fs::read_to_string(&path).unwrap()).width,
            Some(1.)
        );
        let _ = std::fs::remove_dir_all(dir);
    }

    #[test]
    fn unicode_and_long_settings_paths_save() {
        let dir = temp_dir("save-names");
        let long: String = std::iter::repeat_n('n', 150).collect();
        let path = dir.join("sättings ünicode").join(long).join("settings");
        save_to(&path, &widths(1.), Fault::None).unwrap();
        assert_eq!(
            parse(&std::fs::read_to_string(&path).unwrap()).width,
            Some(1.)
        );
        let _ = std::fs::remove_dir_all(dir);
    }

    #[test]
    #[cfg(unix)]
    fn a_read_only_folder_refuses_the_stage() {
        use std::os::unix::fs::PermissionsExt;

        let dir = temp_dir("save-permissions");
        let path = dir.join("settings");
        std::fs::write(&path, "width=1\n").unwrap();
        std::fs::set_permissions(&dir, std::fs::Permissions::from_mode(0o555)).unwrap();
        let error = save_to(&path, &widths(2.), Fault::None).unwrap_err();
        match &error {
            SaveError::Stage { error } => {
                assert_eq!(error.kind(), std::io::ErrorKind::PermissionDenied)
            }
            other => panic!("a read-only folder fails the stage: {other:?}"),
        }
        std::fs::set_permissions(&dir, std::fs::Permissions::from_mode(0o755)).unwrap();
        assert_eq!(
            std::fs::read_to_string(&path).unwrap(),
            "width=1\n",
            "the old file is untouched"
        );
        let _ = std::fs::remove_dir_all(dir);
    }
}
