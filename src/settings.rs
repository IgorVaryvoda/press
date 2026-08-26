//! Remember where the window was and what you were looking at.
//!
//! A tiny key=value file rather than a config crate. There are four values, and a
//! dependency that walks platform config directories costs more than the ten lines
//! it would save.

use std::io::Write;
use std::path::{Path, PathBuf};

#[derive(Clone, Debug, Default, PartialEq)]
pub struct Settings {
    pub width: Option<f32>,
    pub height: Option<f32>,
    pub folder: Option<PathBuf>,
    pub columns: ColumnPrefs,
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

pub fn save(settings: &Settings) {
    let Some(path) = path() else {
        return;
    };
    let _ = save_to(&path, settings);
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
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_saved_file_reads_back_the_same() {
        let settings = Settings {
            width: Some(1280.),
            height: Some(720.5),
            folder: Some(PathBuf::from("/photos/library")),
            columns: ColumnPrefs::default(),
        };
        assert_eq!(parse(&render(&settings)), settings);
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
        let dir = std::env::temp_dir().join(format!(
            "imageguide-settings-{}-{}",
            std::process::id(),
            std::thread::current().name().unwrap_or("test")
        ));
        let _ = std::fs::remove_dir_all(&dir);
        let path = dir.join("settings");
        std::fs::create_dir_all(&dir).unwrap();
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
