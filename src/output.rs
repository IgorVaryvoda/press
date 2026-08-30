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
    SourceLookup { path: PathBuf, error: io::Error },
    OutputLookup { path: PathBuf, error: io::Error },
    OutputSymlink { path: PathBuf },
    OutputNotDirectory { path: PathBuf },
    OutputContainsSource,
    SourceNotCanonical,
    SourceOutsideRoot,
    SourceIsRoot,
    RelativePathEmpty,
    RelativePathNotNormal,
    FinalPathOutsideOutput,
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
            Self::SourceNotCanonical => write!(formatter, "the source path is not canonical"),
            Self::SourceOutsideRoot => {
                write!(formatter, "the source is outside the source directory")
            }
            Self::SourceIsRoot => write!(formatter, "the source is the source directory"),
            Self::RelativePathEmpty => write!(formatter, "the relative path is empty"),
            Self::RelativePathNotNormal => write!(formatter, "the relative path is not normal"),
            Self::FinalPathOutsideOutput => {
                write!(formatter, "the final path is outside the output directory")
            }
        }
    }
}

impl std::error::Error for Error {}

/// A validated source directory and output boundary.
#[derive(Debug)]
pub struct Context {
    source_root: PathBuf,
    output_root: PathBuf,
}

impl Context {
    /// Establish the boundary without creating an output directory or file.
    // First consumed by plan 1420; plan 1452 adds identity aliases.
    #[allow(dead_code)]
    pub fn establish(source: &Path, output: &Path) -> Result<Self, Error> {
        validate_raw(source)?;
        validate_raw(output)?;
        if !source.is_absolute() {
            return Err(Error::SourceNotAbsolute);
        }
        if !output.is_absolute() {
            return Err(Error::OutputNotAbsolute);
        }

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
            output_root,
        })
    }

    // First consumed by plan 1433 when CLI sources are normalized at capture time.
    #[allow(dead_code)]
    pub fn source_root(&self) -> &Path {
        &self.source_root
    }

    // First consumed by plan 1434 when GUI conversion adopts this boundary.
    #[allow(dead_code)]
    pub fn output_root(&self) -> &Path {
        &self.output_root
    }

    /// Return a source path only when it is already canonical and strictly below root.
    // First consumed by plan 1433 with the CLI conversion migration.
    #[allow(dead_code)]
    pub fn relative_source(&self, source: &Path) -> Result<PathBuf, Error> {
        if !source.is_absolute() {
            return Err(Error::SourceNotCanonical);
        }
        let canonical = fs::canonicalize(source).map_err(|error| Error::SourceLookup {
            path: source.to_path_buf(),
            error,
        })?;
        if canonical != source {
            return Err(Error::SourceNotCanonical);
        }
        let relative = source
            .strip_prefix(&self.source_root)
            .map_err(|_| Error::SourceOutsideRoot)?;
        if relative.as_os_str().is_empty() {
            return Err(Error::SourceIsRoot);
        }
        normal_relative(relative).map_err(|_| Error::SourceNotCanonical)?;
        Ok(relative.to_path_buf())
    }

    /// Join a normal source-relative path while retaining the proven output boundary.
    // First consumed by plan 1434 with the GUI conversion migration.
    #[allow(dead_code)]
    pub fn final_path(&self, relative: &Path) -> Result<PathBuf, Error> {
        normal_relative(relative)?;
        let final_path = self.output_root.join(relative);
        if !final_path.starts_with(&self.output_root) {
            return Err(Error::FinalPathOutsideOutput);
        }
        Ok(final_path)
    }
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
    Ok(())
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
    fn output_path_boundary_relative_source_and_final_path_are_strict() {
        let base = temp_dir("helpers");
        let source = base.join("source");
        let output = base.join("output");
        let inside = source.join("album/image.png");
        fs::create_dir_all(inside.parent().unwrap()).unwrap();
        fs::write(&inside, b"image").unwrap();
        let context = Context::establish(&source, &output).unwrap();
        let canonical_inside = fs::canonicalize(&inside).unwrap();
        assert_eq!(
            context.relative_source(&canonical_inside).unwrap(),
            Path::new("album/image.png")
        );
        assert!(matches!(
            context.relative_source(&source),
            Err(Error::SourceNotCanonical | Error::SourceIsRoot)
        ));
        assert!(matches!(
            context.relative_source(&base),
            Err(Error::SourceOutsideRoot)
        ));
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
}
