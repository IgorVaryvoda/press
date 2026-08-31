//! Lexical and canonical boundaries for conversion output.
//!
//! The conversion callers still use their legacy relative path handling. This module
//! defines the stricter contract they will adopt once source capture and writing move
//! together, so validation itself never creates a path or follows an output symlink.

use std::ffi::OsStr;
use std::fmt;
use std::fs;
use std::io;
use std::path::{Component, Path, PathBuf};

/// Why a source/output boundary could not be established.
#[derive(Debug)]
pub enum Error {
    EmptyPath,
    DotSegment,
    ParentSegment,
    SourceNotAbsolute,
    OutputNotAbsolute,
    SourceNotDirectory,
    WorkingDirectory {
        error: io::Error,
    },
    SourceLookup {
        path: PathBuf,
        error: io::Error,
    },
    OutputLookup {
        path: PathBuf,
        error: io::Error,
    },
    OutputSymlink {
        path: PathBuf,
    },
    OutputNotDirectory {
        path: PathBuf,
    },
    OutputContainsSource,
    SourceOutsideRoot,
    SourceIsRoot,
    RelativePathEmpty,
    RelativePathNotNormal,
    FinalPathOutsideOutput,
    #[cfg_attr(not(windows), allow(dead_code))]
    WindowsNamespace,
    #[cfg_attr(not(windows), allow(dead_code))]
    WindowsComponent,
    #[cfg_attr(not(windows), allow(dead_code))]
    WindowsDevice,
    #[cfg_attr(not(windows), allow(dead_code))]
    WindowsReparse {
        path: PathBuf,
    },
}

impl fmt::Display for Error {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::EmptyPath => write!(formatter, "a path is empty"),
            Self::DotSegment => write!(formatter, "a path contains a dot segment"),
            Self::ParentSegment => write!(formatter, "a path contains a parent segment"),
            Self::SourceNotAbsolute => write!(formatter, "the source directory is not absolute"),
            Self::OutputNotAbsolute => write!(formatter, "the output directory is not absolute"),
            Self::SourceNotDirectory => write!(formatter, "the source path is not a directory"),
            Self::WorkingDirectory { error } => {
                write!(
                    formatter,
                    "could not determine the working directory: {error}"
                )
            }
            Self::SourceLookup { path, error } => {
                write!(
                    formatter,
                    "could not inspect source {}: {error}",
                    path.display()
                )
            }
            Self::OutputLookup { path, error } => {
                write!(
                    formatter,
                    "could not inspect output {}: {error}",
                    path.display()
                )
            }
            Self::OutputSymlink { path } => {
                write!(
                    formatter,
                    "output component is a symlink: {}",
                    path.display()
                )
            }
            Self::OutputNotDirectory { path } => {
                write!(
                    formatter,
                    "output component is not a directory: {}",
                    path.display()
                )
            }
            Self::OutputContainsSource => {
                write!(formatter, "the output directory contains the source")
            }
            Self::SourceOutsideRoot => {
                write!(formatter, "the source is outside the source directory")
            }
            Self::SourceIsRoot => write!(formatter, "the source is the source directory"),
            Self::RelativePathEmpty => write!(formatter, "the relative path is empty"),
            Self::RelativePathNotNormal => write!(formatter, "the relative path is not normal"),
            Self::FinalPathOutsideOutput => {
                write!(formatter, "the final path is outside the output directory")
            }
            Self::WindowsNamespace => write!(formatter, "the path is not a disk filesystem path"),
            Self::WindowsComponent => {
                write!(formatter, "the path contains an invalid Windows component")
            }
            Self::WindowsDevice => write!(
                formatter,
                "the path contains a reserved Windows device name"
            ),
            Self::WindowsReparse { path } => {
                write!(
                    formatter,
                    "path component is a Windows reparse point: {}",
                    path.display()
                )
            }
        }
    }
}

impl std::error::Error for Error {}

/// A validated source directory and output boundary.
#[derive(Debug)]
pub struct Context {
    source_root: PathBuf,
    lexical_source_root: PathBuf,
    working_directory: PathBuf,
    output_root: PathBuf,
}

impl Context {
    /// Establish the boundary without creating an output directory or file.
    // First consumed by plan 1420; plan 1452 adds identity aliases.
    #[allow(dead_code)]
    pub fn establish(source: &Path, output: &Path) -> Result<Self, Error> {
        Self::establish_with_current_dir(source, output, std::env::current_dir)
    }

    pub(crate) fn establish_with_current_dir(
        source: &Path,
        output: &Path,
        current_dir: impl FnOnce() -> io::Result<PathBuf>,
    ) -> Result<Self, Error> {
        let working_directory = current_dir().map_err(|error| Error::WorkingDirectory { error })?;
        Self::establish_with_working_directory(source, output, working_directory)
    }

    pub(crate) fn establish_with_working_directory(
        source: &Path,
        output: &Path,
        working_directory: PathBuf,
    ) -> Result<Self, Error> {
        validate_raw(source)?;
        validate_raw(output)?;
        if !source.is_absolute() {
            return Err(Error::SourceNotAbsolute);
        }
        if !output.is_absolute() {
            return Err(Error::OutputNotAbsolute);
        }
        validate_existing_windows_components(source, false)?;
        validate_existing_windows_components(output, true)?;

        let source_root = fs::canonicalize(source).map_err(|error| Error::SourceLookup {
            path: source.to_path_buf(),
            error,
        })?;
        let source_metadata = fs::metadata(&source_root).map_err(|error| Error::SourceLookup {
            path: source_root.clone(),
            error,
        })?;
        if !source_metadata.is_dir() {
            return Err(Error::SourceNotDirectory);
        }

        let output_root = canonical_output(output)?;
        if source_root.starts_with(&output_root) {
            return Err(Error::OutputContainsSource);
        }

        Ok(Self {
            source_root,
            lexical_source_root: source.to_path_buf(),
            working_directory,
            output_root,
        })
    }

    pub(crate) fn establish_default_child_with_working_directory(
        source: &Path,
        child: &Path,
        working_directory: PathBuf,
    ) -> Result<Self, Error> {
        Self::establish_default_child_with_hook(source, child, working_directory, || {})
    }

    fn establish_default_child_with_hook(
        source: &Path,
        child: &Path,
        working_directory: PathBuf,
        after_source_lookup: impl FnOnce(),
    ) -> Result<Self, Error> {
        validate_raw(source)?;
        if !source.is_absolute() {
            return Err(Error::SourceNotAbsolute);
        }
        validate_existing_windows_components(source, false)?;
        normal_relative(child)?;

        let source_root = fs::canonicalize(source).map_err(|error| Error::SourceLookup {
            path: source.to_path_buf(),
            error,
        })?;
        let source_metadata = fs::metadata(&source_root).map_err(|error| Error::SourceLookup {
            path: source_root.clone(),
            error,
        })?;
        if !source_metadata.is_dir() {
            return Err(Error::SourceNotDirectory);
        }
        after_source_lookup();

        validate_raw(&working_directory)?;
        if !working_directory.is_absolute() {
            return Err(Error::SourceNotAbsolute);
        }
        let output_root = canonical_output_child(&source_root, child)?;
        if source_root.starts_with(&output_root) {
            return Err(Error::OutputContainsSource);
        }

        Ok(Self {
            source_root,
            lexical_source_root: source.to_path_buf(),
            working_directory,
            output_root,
        })
    }

    // First consumed by plan 1433 when GUI sources are normalized at capture time.
    #[allow(dead_code)]
    pub fn source_root(&self) -> &Path {
        &self.source_root
    }

    // First consumed by plan 1434 when CLI conversion adopts this boundary.
    #[allow(dead_code)]
    pub fn output_root(&self) -> &Path {
        &self.output_root
    }

    /// Return a normalized source-relative path from the audited path spelling.
    ///
    /// Relative source spellings resolve from the working directory captured when the
    /// context was established. This is lexical only: the source need not still exist.
    // First consumed by plan 1433 for GUI routing.
    #[allow(dead_code)]
    pub fn relative_source(&self, source: &Path) -> Result<PathBuf, Error> {
        if source.as_os_str().is_empty() {
            return Err(Error::EmptyPath);
        }
        let source = if source.is_absolute() {
            source.to_path_buf()
        } else {
            // `current_dir` follows source-root aliases on Unix. Restore the audited
            // spelling before resolving a relative source so its output name remains
            // lexical even when the process was started through that alias.
            let working_directory = self
                .working_directory
                .strip_prefix(&self.source_root)
                .map(|relative| self.lexical_source_root.join(relative))
                .unwrap_or_else(|_| self.working_directory.clone());
            working_directory.join(source)
        };
        let root = lexical_normalize(&self.lexical_source_root)?;
        let source = lexical_normalize(&source)?;
        let relative = source
            .strip_prefix(root)
            .map_err(|_| Error::SourceOutsideRoot)?;
        if relative.as_os_str().is_empty() {
            return Err(Error::SourceIsRoot);
        }
        normal_relative(relative)?;
        Ok(relative.to_path_buf())
    }

    /// Join a normal source-relative path while retaining the proven output boundary.
    // First consumed by plan 1434 for CLI routing.
    #[allow(dead_code)]
    pub fn final_path(&self, relative: &Path) -> Result<PathBuf, Error> {
        normal_relative(relative)?;
        let final_path = self.output_root.join(relative);
        validate_raw(&final_path)?;
        validate_existing_windows_components(&final_path, true)?;
        if !final_path.starts_with(&self.output_root) {
            return Err(Error::FinalPathOutsideOutput);
        }
        Ok(final_path)
    }
}

pub(crate) fn lexical_normalize_against(path: &Path, base: &Path) -> Result<PathBuf, Error> {
    if !base.is_absolute() {
        return Err(Error::SourceNotAbsolute);
    }
    let path = if path.is_absolute() {
        path.to_path_buf()
    } else {
        base.join(path)
    };
    lexical_normalize(&path)
}

fn lexical_normalize(path: &Path) -> Result<PathBuf, Error> {
    let mut normalized = PathBuf::new();
    let mut prefixed = false;
    let mut rooted = false;
    for component in path.components() {
        match component {
            Component::Prefix(prefix) => {
                if !normalized.as_os_str().is_empty() {
                    return Err(Error::SourceOutsideRoot);
                }
                normalized.push(prefix.as_os_str());
                prefixed = true;
            }
            Component::RootDir => {
                if rooted {
                    return Err(Error::SourceOutsideRoot);
                }
                normalized.push(std::path::MAIN_SEPARATOR_STR);
                rooted = true;
            }
            Component::CurDir => {}
            Component::Normal(part) => normalized.push(part),
            Component::ParentDir => {
                if !normalized.pop() {
                    return Err(Error::SourceOutsideRoot);
                }
            }
        }
    }
    if prefixed && !rooted {
        return Err(Error::SourceOutsideRoot);
    }
    Ok(normalized)
}

fn canonical_output_child(parent: &Path, child: &Path) -> Result<PathBuf, Error> {
    let output = parent.join(child);
    validate_raw(&output)?;
    validate_existing_windows_components(&output, true)?;
    for component in child.components() {
        let Component::Normal(part) = component else {
            return Err(Error::RelativePathNotNormal);
        };
        let candidate = parent.join(part);
        match fs::symlink_metadata(&candidate) {
            Ok(metadata) if metadata.file_type().is_symlink() => {
                return Err(Error::OutputSymlink { path: candidate });
            }
            Ok(metadata) if !metadata.is_dir() => {
                return Err(Error::OutputNotDirectory { path: candidate });
            }
            Ok(_) => {}
            Err(error) if error.kind() == io::ErrorKind::NotFound => break,
            Err(error) => {
                return Err(Error::OutputLookup {
                    path: candidate,
                    error,
                });
            }
        }
    }
    Ok(output)
}

fn canonical_output(output: &Path) -> Result<PathBuf, Error> {
    let mut ancestor = PathBuf::new();
    let mut components = output.components().peekable();
    if let Some(Component::Prefix(prefix)) = components.peek() {
        ancestor.push(prefix.as_os_str());
        components.next();
    }
    if !matches!(components.next(), Some(Component::RootDir)) {
        return Err(Error::OutputNotAbsolute);
    }
    ancestor.push(std::path::MAIN_SEPARATOR_STR);

    let normal: Vec<_> = components
        .filter_map(|component| match component {
            Component::Normal(part) => Some(part),
            _ => None,
        })
        .collect();
    let mut missing = None;
    for (index, part) in normal.iter().enumerate() {
        if missing.is_some() {
            continue;
        }
        let candidate = ancestor.join(part);
        match fs::symlink_metadata(&candidate) {
            Ok(metadata) if metadata.file_type().is_symlink() => {
                return Err(Error::OutputSymlink { path: candidate });
            }
            Ok(metadata) if !metadata.is_dir() => {
                return Err(Error::OutputNotDirectory { path: candidate });
            }
            Ok(_) => ancestor = candidate,
            Err(error) if error.kind() == io::ErrorKind::NotFound => missing = Some(index),
            Err(error) => {
                return Err(Error::OutputLookup {
                    path: candidate,
                    error,
                });
            }
        }
    }
    let existing = fs::canonicalize(&ancestor).map_err(|error| Error::OutputLookup {
        path: ancestor.clone(),
        error,
    })?;
    let mut canonical = existing;
    if let Some(start) = missing {
        for part in &normal[start..] {
            canonical.push(part);
        }
    }
    Ok(canonical)
}

fn normal_relative(path: &Path) -> Result<(), Error> {
    if path.as_os_str().is_empty() {
        return Err(Error::RelativePathEmpty);
    }
    validate_raw(path).map_err(|_| Error::RelativePathNotNormal)?;
    if path.is_absolute()
        || path
            .components()
            .any(|component| !matches!(component, Component::Normal(_)))
    {
        return Err(Error::RelativePathNotNormal);
    }
    Ok(())
}

fn validate_raw(path: &Path) -> Result<(), Error> {
    if path.as_os_str().is_empty() {
        return Err(Error::EmptyPath);
    }
    for segment in raw_segments(path.as_os_str()) {
        if segment == OsStr::new(".") {
            return Err(Error::DotSegment);
        }
        if segment == OsStr::new("..") {
            return Err(Error::ParentSegment);
        }
    }
    validate_windows_lexical(path)?;
    Ok(())
}

#[cfg(not(windows))]
fn validate_windows_lexical(_path: &Path) -> Result<(), Error> {
    Ok(())
}

#[cfg(windows)]
fn validate_windows_lexical(path: &Path) -> Result<(), Error> {
    use std::path::Prefix;

    if path.has_root() {
        let Some(Component::Prefix(prefix)) = path.components().next() else {
            return Err(Error::WindowsNamespace);
        };
        match prefix.kind() {
            Prefix::Disk(_) | Prefix::VerbatimDisk(_) => {}
            Prefix::UNC(_, share) | Prefix::VerbatimUNC(_, share) => {
                if is_ipc_share(share) {
                    return Err(Error::WindowsNamespace);
                }
            }
            Prefix::DeviceNS(_) | Prefix::Verbatim(_) => return Err(Error::WindowsNamespace),
        }
    }

    for component in path.components() {
        if let Component::Normal(part) = component {
            validate_windows_component(part)?;
        }
    }
    Ok(())
}

#[cfg(windows)]
fn is_ipc_share(share: &OsStr) -> bool {
    use std::os::windows::ffi::OsStrExt;

    let share: Vec<_> = share.encode_wide().collect();
    ascii_eq_ignore_case(&share, b"pipe")
        || ascii_eq_ignore_case(&share, b"mailslot")
        || ascii_eq_ignore_case(&share, b"ipc$")
}

#[cfg(windows)]
fn validate_windows_component(part: &OsStr) -> Result<(), Error> {
    use std::os::windows::ffi::OsStrExt;

    let units: Vec<_> = part.encode_wide().collect();
    if matches!(units.first(), Some(unit) if *unit == 0x0020)
        || matches!(units.last(), Some(0x0020 | 0x002e))
    {
        return Err(Error::WindowsComponent);
    }
    if units.iter().any(|unit| {
        *unit <= 0x001f
            || matches!(
                *unit,
                0x0022 | 0x002a | 0x002f | 0x003a | 0x003c | 0x003e | 0x003f | 0x005c | 0x007c
            )
    }) {
        return Err(Error::WindowsComponent);
    }

    let device = units.split(|unit| *unit == 0x002e).next().unwrap_or(&units);
    let device = trim_ascii_spaces_end(device);
    if is_windows_device(device) {
        return Err(Error::WindowsDevice);
    }
    Ok(())
}

#[cfg(windows)]
fn ascii_eq_ignore_case(units: &[u16], ascii: &[u8]) -> bool {
    units.len() == ascii.len()
        && units.iter().zip(ascii).all(|(unit, expected)| {
            *unit <= 0x7f
                && if (0x0041..=0x005a).contains(unit) {
                    *unit + 0x0020
                } else {
                    *unit
                } == expected.to_ascii_lowercase() as u16
        })
}

#[cfg(windows)]
fn trim_ascii_spaces_end(units: &[u16]) -> &[u16] {
    let end = units
        .iter()
        .rposition(|unit| *unit != 0x0020)
        .map_or(0, |index| index + 1);
    &units[..end]
}

#[cfg(windows)]
fn is_windows_device(name: &[u16]) -> bool {
    [
        b"CON".as_slice(),
        b"PRN",
        b"AUX",
        b"NUL",
        b"CLOCK$",
        b"CONIN$",
        b"CONOUT$",
    ]
    .into_iter()
    .any(|device| ascii_eq_ignore_case(name, device))
        || is_windows_port(name, b"COM")
        || is_windows_port(name, b"LPT")
}

#[cfg(windows)]
fn is_windows_port(name: &[u16], prefix: &[u8]) -> bool {
    name.len() == prefix.len() + 1
        && ascii_eq_ignore_case(&name[..prefix.len()], prefix)
        && matches!(
            name[prefix.len()],
            0x0031..=0x0039 | 0x00b9 | 0x00b2 | 0x00b3
        )
}

#[cfg(not(windows))]
fn validate_existing_windows_components(_path: &Path, _output: bool) -> Result<(), Error> {
    Ok(())
}

#[cfg(windows)]
fn validate_existing_windows_components(path: &Path, output: bool) -> Result<(), Error> {
    use std::os::windows::fs::MetadataExt;

    validate_existing_windows_components_with(path, output, |candidate| {
        fs::symlink_metadata(candidate).map(|metadata| metadata.file_attributes())
    })
}

#[cfg(windows)]
fn validate_existing_windows_components_with(
    path: &Path,
    output: bool,
    mut metadata: impl FnMut(&Path) -> io::Result<u32>,
) -> Result<(), Error> {
    for candidate in windows_absolute_ancestors(path) {
        match metadata(&candidate) {
            Ok(attributes) if attributes & 0x400 != 0 => {
                return Err(Error::WindowsReparse { path: candidate });
            }
            Ok(_) => {}
            Err(error) if error.kind() == io::ErrorKind::NotFound => return Ok(()),
            Err(error) => return Err(metadata_error(candidate, output, error)),
        }
    }
    Ok(())
}

#[cfg(windows)]
fn windows_absolute_ancestors(path: &Path) -> Vec<PathBuf> {
    let mut components = path.components();
    let Some(Component::Prefix(prefix)) = components.next() else {
        return Vec::new();
    };
    let Some(Component::RootDir) = components.next() else {
        return Vec::new();
    };

    let mut ancestor = PathBuf::from(prefix.as_os_str());
    ancestor.push(std::path::MAIN_SEPARATOR_STR);
    let mut ancestors = vec![ancestor.clone()];
    for component in components {
        if let Component::Normal(part) = component {
            ancestor.push(part);
            ancestors.push(ancestor.clone());
        }
    }
    ancestors
}

#[cfg(windows)]
fn metadata_error(path: PathBuf, output: bool, error: io::Error) -> Error {
    if output {
        Error::OutputLookup { path, error }
    } else {
        Error::SourceLookup { path, error }
    }
}

#[cfg(unix)]
fn raw_segments(path: &OsStr) -> impl Iterator<Item = std::ffi::OsString> {
    use std::os::unix::ffi::OsStrExt;

    path.as_bytes()
        .split(|byte| *byte == b'/')
        .filter(|segment| !segment.is_empty())
        .map(OsStr::from_bytes)
        .map(OsStr::to_os_string)
}

#[cfg(windows)]
fn raw_segments(path: &OsStr) -> impl Iterator<Item = std::ffi::OsString> {
    use std::os::windows::ffi::{OsStrExt, OsStringExt};

    path.encode_wide()
        .split(|unit| *unit == b'/' as u16 || *unit == b'\\' as u16)
        .filter(|segment| !segment.is_empty())
        .map(|segment| std::ffi::OsString::from_wide(segment))
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::time::{SystemTime, UNIX_EPOCH};

    #[cfg(windows)]
    use std::os::windows::ffi::OsStringExt;

    fn temp_dir(name: &str) -> PathBuf {
        let unique = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .unwrap()
            .as_nanos();
        let path = std::env::temp_dir().join(format!("press-output-{name}-{unique}"));
        fs::create_dir_all(&path).unwrap();
        path
    }

    #[test]
    fn output_path_boundary_rejects_raw_dot_and_parent_segments_without_creating_paths() {
        let base = temp_dir("raw");
        let source = base.join("source");
        fs::create_dir(&source).unwrap();
        for output in [
            base.join("missing/./output"),
            base.join("missing/../output"),
        ] {
            assert!(matches!(
                Context::establish(&source, &output),
                Err(Error::DotSegment | Error::ParentSegment)
            ));
            assert!(!base.join("missing").exists());
        }
        fs::remove_dir_all(base).unwrap();
    }

    #[test]
    fn output_path_boundary_accepts_descendants_and_disjoint_paths_without_creating_them() {
        let base = temp_dir("valid");
        let source = base.join("source with spaces");
        fs::create_dir(&source).unwrap();
        let child = source.join("optimized ünicode");
        let disjoint = base.join("exports").join("nested");
        assert_eq!(
            Context::establish(&source, &child).unwrap().output_root(),
            child
        );
        assert_eq!(
            Context::establish(&source, &disjoint)
                .unwrap()
                .output_root(),
            disjoint
        );
        assert!(!child.exists());
        assert!(!disjoint.exists());
        fs::remove_dir_all(base).unwrap();
    }

    #[test]
    fn output_path_boundary_rejects_output_equal_to_or_above_source() {
        let base = temp_dir("ancestor");
        let source = base.join("source");
        fs::create_dir(&source).unwrap();
        for output in [&source, &base] {
            assert!(matches!(
                Context::establish(&source, output),
                Err(Error::OutputContainsSource)
            ));
        }
        fs::remove_dir_all(base).unwrap();
    }

    #[test]
    fn output_path_boundary_rejects_existing_symlink_and_file_components() {
        let base = temp_dir("components");
        let source = base.join("source");
        let target = base.join("target");
        fs::create_dir(&source).unwrap();
        fs::create_dir(&target).unwrap();
        fs::write(base.join("file"), b"x").unwrap();
        assert!(matches!(
            Context::establish(&source, &base.join("file/output")),
            Err(Error::OutputNotDirectory { .. })
        ));
        #[cfg(unix)]
        {
            std::os::unix::fs::symlink(&target, base.join("link")).unwrap();
            assert!(matches!(
                Context::establish(&source, &base.join("link/output")),
                Err(Error::OutputSymlink { .. })
            ));
        }
        fs::remove_dir_all(base).unwrap();
    }

    #[test]
    fn output_path_boundary_final_path_is_strict() {
        let base = temp_dir("helpers");
        let source = base.join("source");
        let output = base.join("output");
        fs::create_dir(&source).unwrap();
        let context = Context::establish(&source, &output).unwrap();
        assert_eq!(
            context.final_path(Path::new("album/image.webp")).unwrap(),
            output.join("album/image.webp")
        );
        assert!(matches!(
            context.final_path(Path::new("../escape.webp")),
            Err(Error::RelativePathNotNormal)
        ));
        fs::remove_dir_all(base).unwrap();
    }

    #[test]
    fn context_rejects_a_relative_source_root() {
        let base = temp_dir("relative-root");
        assert!(matches!(
            Context::establish(Path::new("relative"), &base.join("output")),
            Err(Error::SourceNotAbsolute)
        ));
        fs::remove_dir_all(base).unwrap();
    }

    #[test]
    fn context_reports_a_working_directory_failure() {
        let base = temp_dir("working-directory");
        let source = base.join("source");
        fs::create_dir(&source).unwrap();
        assert!(matches!(
            Context::establish_with_current_dir(&source, &base.join("output"), || {
                Err(io::Error::new(
                    io::ErrorKind::PermissionDenied,
                    "injected failure",
                ))
            }),
            Err(Error::WorkingDirectory { .. })
        ));
        fs::remove_dir_all(base).unwrap();
    }

    #[cfg(unix)]
    #[test]
    fn default_context_keeps_one_source_lookup_during_symlink_retarget() {
        let base = temp_dir("default-context-retarget");
        let first = base.join("first");
        let second = base.join("second");
        let alias = base.join("alias");
        fs::create_dir(&first).unwrap();
        fs::create_dir(&second).unwrap();
        std::os::unix::fs::symlink(&first, &alias).unwrap();

        let context = Context::establish_default_child_with_hook(
            &alias,
            Path::new("optimized"),
            base.clone(),
            || {
                fs::remove_file(&alias).unwrap();
                std::os::unix::fs::symlink(&second, &alias).unwrap();
            },
        )
        .unwrap();

        assert_eq!(context.source_root(), first);
        assert_eq!(context.output_root(), first.join("optimized"));
        fs::remove_dir_all(base).unwrap();
    }

    #[test]
    fn relative_source_preserves_an_absolute_name() {
        let base = temp_dir("absolute-source");
        let source = base.join("source");
        let entry = source.join("album/image.png");
        fs::create_dir_all(entry.parent().unwrap()).unwrap();
        let context =
            Context::establish_with_current_dir(&source, &base.join("output"), || Ok(base.clone()))
                .unwrap();
        assert_eq!(
            context.relative_source(&entry).unwrap(),
            Path::new("album/image.png")
        );
        fs::remove_dir_all(base).unwrap();
    }

    #[test]
    fn relative_source_preserves_a_bare_relative_file() {
        let base = temp_dir("bare-relative");
        let source = base.join("source");
        fs::create_dir(&source).unwrap();
        let context = Context::establish_with_current_dir(&source, &base.join("output"), || {
            Ok(source.clone())
        })
        .unwrap();
        assert_eq!(
            context.relative_source(Path::new("image.png")).unwrap(),
            Path::new("image.png")
        );
        fs::remove_dir_all(base).unwrap();
    }

    #[test]
    fn relative_source_preserves_a_dot_relative_file() {
        let base = temp_dir("dot-relative");
        let source = base.join("source");
        fs::create_dir(&source).unwrap();
        let context = Context::establish_with_current_dir(&source, &base.join("output"), || {
            Ok(source.clone())
        })
        .unwrap();
        assert_eq!(
            context.relative_source(Path::new("./image.png")).unwrap(),
            Path::new("image.png")
        );
        fs::remove_dir_all(base).unwrap();
    }

    #[test]
    fn relative_source_preserves_a_parent_relative_folder_entry() {
        let base = temp_dir("parent-relative");
        let source = base.join("source");
        let working_directory = source.join("album");
        fs::create_dir_all(&working_directory).unwrap();
        let context = Context::establish_with_current_dir(&source, &base.join("output"), || {
            Ok(working_directory.clone())
        })
        .unwrap();
        assert_eq!(
            context.relative_source(Path::new("../cover.png")).unwrap(),
            Path::new("cover.png")
        );
        fs::remove_dir_all(base).unwrap();
    }

    #[test]
    fn relative_source_rejects_an_empty_path() {
        let base = temp_dir("empty-relative");
        let source = base.join("source");
        fs::create_dir(&source).unwrap();
        let context = Context::establish(&source, &base.join("output")).unwrap();
        assert!(matches!(
            context.relative_source(Path::new("")),
            Err(Error::EmptyPath)
        ));
        fs::remove_dir_all(base).unwrap();
    }

    #[test]
    fn relative_source_rejects_a_lexical_escape() {
        let base = temp_dir("lexical-escape");
        let source = base.join("source");
        fs::create_dir(&source).unwrap();
        let context = Context::establish_with_current_dir(&source, &base.join("output"), || {
            Ok(source.clone())
        })
        .unwrap();
        assert!(matches!(
            context.relative_source(Path::new("../outside.png")),
            Err(Error::SourceOutsideRoot)
        ));
        fs::remove_dir_all(base).unwrap();
    }

    #[test]
    fn relative_source_does_not_require_the_source_to_still_exist() {
        let base = temp_dir("removed-source");
        let source = base.join("source");
        let entry = source.join("removed/image.png");
        fs::create_dir_all(entry.parent().unwrap()).unwrap();
        fs::write(&entry, b"image").unwrap();
        let context = Context::establish(&source, &base.join("output")).unwrap();
        fs::remove_file(&entry).unwrap();
        assert_eq!(
            context.relative_source(&entry).unwrap(),
            Path::new("removed/image.png")
        );
        fs::remove_dir_all(base).unwrap();
    }

    #[cfg(unix)]
    #[test]
    fn relative_source_under_a_symlinked_root_keeps_the_audited_name() {
        let base = temp_dir("symlinked-source");
        let target = base.join("target");
        let source = base.join("source");
        fs::create_dir(&target).unwrap();
        std::os::unix::fs::symlink(&target, &source).unwrap();
        let context = Context::establish_with_current_dir(&source, &base.join("output"), || {
            Ok(target.clone())
        })
        .unwrap();
        assert_eq!(
            context.relative_source(Path::new("image.png")).unwrap(),
            Path::new("image.png")
        );
        fs::remove_dir_all(base).unwrap();
    }

    #[cfg(unix)]
    #[test]
    fn relative_source_preserves_non_utf8_components() {
        use std::os::unix::ffi::OsStringExt;

        let base = temp_dir("non-utf8-source");
        let source = base.join("source");
        fs::create_dir(&source).unwrap();
        let name =
            std::ffi::OsString::from_vec(vec![b'i', b'm', b'g', 0xff, b'.', b'p', b'n', b'g']);
        let entry = source.join(&name);
        let context = Context::establish(&source, &base.join("output")).unwrap();
        assert_eq!(context.relative_source(&entry).unwrap(), Path::new(&name));
        fs::remove_dir_all(base).unwrap();
    }

    #[cfg(windows)]
    #[test]
    fn ordinary_windows_source_spelling_keeps_its_relative_name() {
        let base = temp_dir("windows-source-spelling");
        let source = base.join("source");
        let entry = source.join("album").join("image.png");
        fs::create_dir_all(entry.parent().unwrap()).unwrap();
        let context = Context::establish(&source, &base.join("output")).unwrap();
        assert_eq!(
            context.relative_source(&entry).unwrap(),
            Path::new(r"album\image.png")
        );
        fs::remove_dir_all(base).unwrap();
    }

    #[cfg(windows)]
    #[test]
    fn windows_filesystem_path_admits_only_disk_namespaces() {
        for path in [
            r"C:\photos\image.png",
            r"\\server\share\image.png",
            r"\\?\C:\photos\image.png",
            r"\\?\UNC\server\share\image.png",
            r"\\?\UNC\server\normal\image.png",
        ] {
            assert!(validate_raw(Path::new(path)).is_ok(), "{path}");
        }
        for path in [
            r"\\.\PhysicalDrive0",
            r"\\?\GLOBALROOT\Device\HarddiskVolume1",
            r"\\?\Volume{01234567-89ab-cdef-0123-456789abcdef}\image.png",
            r"\\?\pipe\name",
            r"\\?\mailslot\name",
            r"\\?\arbitrary\name",
            r"\\.\PIPE\name",
            r"\\.\MAILSLOT\name",
            r"\??\Device\HarddiskVolume1",
            r"\??\C:\photos\image.png",
            r"\\server\PiPe\name",
            r"\\server\MAILSLOT\name",
            r"\\server\iPc$\name",
            r"\\?\UNC\server\PiPe\name",
            r"\\?\UNC\server\MAILSLOT\name",
            r"\\?\UNC\server\iPc$\name",
        ] {
            assert!(
                matches!(validate_raw(Path::new(path)), Err(Error::WindowsNamespace)),
                "{path}"
            );
        }
    }

    #[cfg(windows)]
    #[test]
    fn windows_filesystem_path_rejects_normalization_and_device_forms() {
        for path in [
            r"C:\ leading\image.png",
            r"C:\trailing \image.png",
            r"C:\trailing.\image.png",
            r"C:\image:stream.png",
            r"C:\CON.txt",
            r"C:\aux .txt",
            r"C:\LPT³.log",
            r"\\?\C:\PRN.txt",
        ] {
            assert!(
                matches!(
                    validate_raw(Path::new(path)),
                    Err(Error::WindowsComponent | Error::WindowsDevice)
                ),
                "{path}"
            );
        }
        for path in [r"C:\COM0", r"C:\COM10", r"C:\LPT0", r"C:\LPT10"] {
            assert!(validate_raw(Path::new(path)).is_ok(), "{path}");
        }
    }

    #[cfg(windows)]
    #[test]
    fn windows_filesystem_path_checks_raw_wide_components() {
        fn wide(units: &[u16]) -> std::ffi::OsString {
            std::ffi::OsString::from_wide(units)
        }

        for device in [
            "CON", "PRN", "AUX", "NUL", "CLOCK$", "CONIN$", "CONOUT$", "COM1", "COM2", "COM3",
            "COM4", "COM5", "COM6", "COM7", "COM8", "COM9", "LPT1", "LPT2", "LPT3", "LPT4", "LPT5",
            "LPT6", "LPT7", "LPT8", "LPT9", "COM¹", "COM²", "COM³", "LPT¹", "LPT²", "LPT³",
        ] {
            for name in [
                device.to_owned(),
                device.to_ascii_lowercase(),
                format!("{device}.txt"),
                format!("{device} .txt"),
            ] {
                assert!(
                    matches!(
                        validate_windows_component(&wide(&name.encode_utf16().collect::<Vec<_>>())),
                        Err(Error::WindowsDevice)
                    ),
                    "{name}"
                );
            }
        }

        for unit in (0x0000..=0x001f).chain([
            b'<' as u16,
            b'>' as u16,
            b':' as u16,
            b'"' as u16,
            b'/' as u16,
            b'\\' as u16,
            b'|' as u16,
            b'?' as u16,
            b'*' as u16,
        ]) {
            assert!(
                matches!(
                    validate_windows_component(&wide(&[b'a' as u16, unit])),
                    Err(Error::WindowsComponent)
                ),
                "{unit:#06x}"
            );
        }
        for units in [
            vec![b' ' as u16, b'a' as u16],
            vec![b'a' as u16, b' ' as u16],
            vec![b'a' as u16, b'.' as u16],
        ] {
            assert!(matches!(
                validate_windows_component(&wide(&units)),
                Err(Error::WindowsComponent)
            ));
        }
        for units in [
            vec![
                0x00fc,
                b'n' as u16,
                b'i' as u16,
                b'c' as u16,
                b'o' as u16,
                b'd' as u16,
                b'e' as u16,
            ],
            vec![0xd800, b'n' as u16, b'a' as u16, b'm' as u16, b'e' as u16],
            "COM0".encode_utf16().collect(),
            "COM10".encode_utf16().collect(),
            "LPT0".encode_utf16().collect(),
            "LPT10".encode_utf16().collect(),
        ] {
            assert!(
                validate_windows_component(&wide(&units)).is_ok(),
                "{units:?}"
            );
            assert!(validate_raw(Path::new(&wide(&units))).is_ok(), "{units:?}");
        }
        assert!(is_ipc_share(&wide(&[
            b'P' as u16,
            b'i' as u16,
            b'P' as u16,
            b'e' as u16,
        ])));
        assert!(!is_ipc_share(&wide(&[
            b'p' as u16,
            b'i' as u16,
            b'p' as u16,
            b'e' as u16,
            0xd800,
        ])));
    }

    #[cfg(windows)]
    #[test]
    fn windows_filesystem_path_metadata_errors_fail_closed() {
        let error = io::Error::new(io::ErrorKind::PermissionDenied, "injected failure");
        let result = metadata_error(PathBuf::from(r"C:\blocked"), false, error);
        assert!(matches!(result, Error::SourceLookup { .. }));
        let error = io::Error::new(io::ErrorKind::InvalidInput, "injected failure");
        let result = metadata_error(PathBuf::from(r"C:\blocked"), true, error);
        assert!(matches!(result, Error::OutputLookup { .. }));
    }

    #[cfg(windows)]
    #[test]
    fn windows_absolute_ancestor_walk() {
        let base = temp_dir("absolute-ancestor");
        let source = base.join("source");
        let output = base.join("output");
        fs::create_dir(&source).unwrap();
        assert!(Context::establish(&source, &output).is_ok());
        fs::remove_dir_all(&base).unwrap();

        for (path, expected) in [
            (
                r"C:\album\missing",
                vec![r"C:\", r"C:\album", r"C:\album\missing"],
            ),
            (
                r"\\?\C:\album\missing",
                vec![r"\\?\C:\", r"\\?\C:\album", r"\\?\C:\album\missing"],
            ),
            (
                r"\\server\share\album\missing",
                vec![
                    r"\\server\share\",
                    r"\\server\share\album",
                    r"\\server\share\album\missing",
                ],
            ),
            (
                r"\\?\UNC\server\share\album\missing",
                vec![
                    r"\\?\UNC\server\share\",
                    r"\\?\UNC\server\share\album",
                    r"\\?\UNC\server\share\album\missing",
                ],
            ),
        ] {
            assert_eq!(
                windows_absolute_ancestors(Path::new(path)),
                expected.into_iter().map(PathBuf::from).collect::<Vec<_>>(),
                "{path}"
            );
        }

        let path = Path::new(r"C:\existing\missing\child");
        let expected = windows_absolute_ancestors(path);
        let mut seen = Vec::new();
        assert!(
            validate_existing_windows_components_with(path, false, |candidate| {
                seen.push(candidate.to_path_buf());
                if candidate == Path::new(r"C:\existing\missing") {
                    Err(io::Error::new(io::ErrorKind::NotFound, "missing suffix"))
                } else {
                    Ok(0)
                }
            })
            .is_ok()
        );
        assert_eq!(seen, expected[..3].to_vec());

        for (reparse, expected_error) in [
            (Path::new(r"C:\"), PathBuf::from(r"C:\")),
            (Path::new(r"C:\existing"), PathBuf::from(r"C:\existing")),
        ] {
            assert!(matches!(
                validate_existing_windows_components_with(path, true, |candidate| {
                    Ok(if candidate == reparse { 0x400 } else { 0 })
                }),
                Err(Error::WindowsReparse { path }) if path == expected_error
            ));
        }

        let mut seen = Vec::new();
        assert!(matches!(
            validate_existing_windows_components_with(path, false, |candidate| {
                seen.push(candidate.to_path_buf());
                Err(io::Error::new(io::ErrorKind::PermissionDenied, "denied"))
            }),
            Err(Error::SourceLookup { .. })
        ));
        assert_eq!(seen, expected[..1].to_vec());
    }

    #[cfg(windows)]
    #[test]
    fn windows_filesystem_path_rejects_junctions_and_relative_sources() {
        let base = temp_dir("junction");
        let source = base.join("source");
        let target = base.join("target");
        let junction = base.join("junction");
        fs::create_dir(&source).unwrap();
        fs::create_dir(&target).unwrap();
        let status = std::process::Command::new("cmd")
            .args([
                "/C",
                "mklink",
                "/J",
                junction.to_str().unwrap(),
                target.to_str().unwrap(),
            ])
            .status()
            .unwrap();
        assert!(status.success());
        assert!(matches!(
            Context::establish(&source, &junction.join("output")),
            Err(Error::WindowsReparse { .. })
        ));
        assert!(matches!(
            Context::establish(Path::new("relative"), &base.join("output")),
            Err(Error::SourceNotAbsolute)
        ));
        fs::remove_dir_all(base).unwrap();
    }
}
