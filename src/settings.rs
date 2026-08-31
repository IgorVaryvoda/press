//! Remember where the window was and what you were looking at.
//!
//! A tiny key=value file rather than a config crate. There are four values, and a
//! dependency that walks platform config directories costs more than the ten lines
//! it would save.

use std::io::Write;
use std::path::{Path, PathBuf};
use std::sync::Arc;

#[derive(Clone, Debug, Default, PartialEq)]
pub struct Settings {
    pub width: Option<f32>,
    pub height: Option<f32>,
    pub folder: Option<PathBuf>,
    pub columns: ColumnPrefs,
    pub output: Output,
}

/// Where converted files and local-model results land.
///
/// The default keeps them beside the originals in `optimized/`, which is what
/// makes "originals unchanged" true and easy to check. A chosen folder is for
/// people whose output belongs somewhere else entirely — a staging directory, a
/// share, a build tree.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub enum Output {
    #[default]
    Optimized,
    Folder(PathBuf),
}

impl Output {
    /// The folder outputs are written into, for a given audited root.
    pub fn root(&self, audited: &Path) -> PathBuf {
        match self {
            Output::Optimized => audited.join(crate::scan::OUTPUT_DIR),
            Output::Folder(path) => path.clone(),
        }
    }

    /// Establish the selected output boundary without creating a path or file.
    #[allow(dead_code)]
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
        };
        context.map(Arc::new).map_err(|error| error.to_string())
    }

    /// How the destination reads in the window: a name for the default, the
    /// path itself for anywhere else.
    pub fn label(&self) -> String {
        match self {
            Output::Optimized => format!("{}/", crate::scan::OUTPUT_DIR),
            Output::Folder(path) => path.display().to_string(),
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

pub fn save(settings: &Settings) -> std::io::Result<()> {
    let Some(path) = path() else {
        return Err(std::io::Error::new(
            std::io::ErrorKind::NotFound,
            "no config directory is available",
        ));
    };
    save_to(&path, settings)
}

fn save_to(path: &Path, settings: &Settings) -> std::io::Result<()> {
    if let Some(parent) = path.parent() {
        std::fs::create_dir_all(parent)?;
    }
    let mut temporary = path.as_os_str().to_owned();
    temporary.push(format!(".{}.part", std::process::id()));
    let temporary = PathBuf::from(temporary);
    let _ = std::fs::remove_file(&temporary);
    let result = (|| {
        let mut file = std::fs::OpenOptions::new()
            .write(true)
            .create_new(true)
            .open(&temporary)?;
        file.write_all(render(settings).as_bytes())?;
        file.sync_all()?;
        replace(&temporary, path)
    })();
    if result.is_err() {
        let _ = std::fs::remove_file(&temporary);
    }
    result
}

fn replace(from: &Path, to: &Path) -> std::io::Result<()> {
    match std::fs::rename(from, to) {
        Ok(()) => Ok(()),
        #[cfg(windows)]
        Err(error) if to.exists() => {
            std::fs::remove_file(to)?;
            std::fs::rename(from, to).map_err(|_| error)
        }
        Err(error) => Err(error),
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
            "columns" => settings.columns = ColumnPrefs::parse(value),
            "output" => {
                let value = value.trim();
                settings.output = if value.is_empty() {
                    Output::Optimized
                } else {
                    Output::Folder(PathBuf::from(value))
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
    // Always written, including when every column is off: an empty value is a
    // choice, and leaving the line out would restore the defaults next launch.
    out.push_str(&format!("columns={}\n", settings.columns.render()));
    // The default writes no path, so a settings file says nothing about where
    // output goes until somebody chooses somewhere else.
    if let Output::Folder(path) = &settings.output {
        out.push_str(&format!("output={}\n", path.display()));
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
            columns: ColumnPrefs::default(),
            output: Output::Folder(PathBuf::from("/exports/web")),
        };
        assert_eq!(parse(&render(&settings)), settings);
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

    /// A hand-edited or half-written file must not stop the app opening.
    #[test]
    fn nonsense_is_ignored_rather_than_fatal() {
        let settings = parse("width=not-a-number\nnokeyhere\n\nheight=600\n");
        assert_eq!(settings.width, None);
        assert_eq!(settings.height, Some(600.));
        assert_eq!(settings.folder, None);
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
        save_to(&path, &settings).unwrap();

        assert_eq!(
            std::fs::read_to_string(&path).unwrap(),
            "width=900\ncolumns=thumb,format,pixels,weight\n"
        );
        assert_eq!(std::fs::read_dir(&dir).unwrap().count(), 1);
        let _ = std::fs::remove_dir_all(dir);
    }
}
