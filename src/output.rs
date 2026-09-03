//! Lexical and canonical boundaries for conversion output.
//!
//! Every producer — normal conversion, local AI, Studio — establishes one context per
//! run and writes inside the output root it proves. Establishing never creates a path
//! and never follows an output symlink, so a destination can be refused before any
//! state moves and before any file is written.

use std::ffi::OsStr;
use std::fmt;
use std::fs;
use std::io;
use std::io::Write as _;
use std::path::{Component, Path, PathBuf};

use sha2::{Digest, Sha256};

#[cfg(unix)]
use std::os::unix::fs::PermissionsExt;

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

    /// Establish the boundary for replace mode, where the output root *is* the
    /// audited root.
    ///
    /// Every other destination is refused when it contains the source, and that
    /// rule is what stops a chosen folder from re-encoding originals onto
    /// themselves. Replace mode does exactly that on purpose, one file at a time
    /// and only after the original is safe in the backup, so it gets its own door
    /// rather than a hole in the rule everything else relies on.
    pub(crate) fn establish_replace_with_working_directory(
        source: &Path,
        working_directory: PathBuf,
    ) -> Result<Self, Error> {
        validate_raw(source)?;
        if !source.is_absolute() {
            return Err(Error::SourceNotAbsolute);
        }
        validate_existing_windows_components(source, false)?;
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
        validate_raw(&working_directory)?;
        if !working_directory.is_absolute() {
            return Err(Error::SourceNotAbsolute);
        }
        Ok(Self {
            output_root: source_root.clone(),
            source_root,
            lexical_source_root: source.to_path_buf(),
            working_directory,
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

/// What a create-new install wrote back: where the complete file is, what it
/// hashes to, and how many bytes it holds.
// First consumed by plan 1421; plan 1401 adds owned replacement beside it.
#[allow(dead_code)]
#[derive(Debug, PartialEq, Eq)]
pub struct InstalledOutput {
    pub path: PathBuf,
    pub sha256: String,
    pub bytes: u64,
}

/// must never read as a file that did not.
// First consumed by plan 1421 with its receipt.
#[allow(dead_code)]
#[derive(Debug, PartialEq, Eq)]
pub struct DurabilityWarning(pub String);

impl std::fmt::Display for DurabilityWarning {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(formatter, "{}", self.0)
    }
}

/// the name, which is [`InstallError::ForeignOutput`] instead of a failure.
// First consumed by plan 1421; variants stay distinct for its diagnostics.
#[allow(dead_code)]
#[derive(Debug)]
pub enum InstallError {
    OutsideOutput { path: PathBuf },
    ParentSymlink { path: PathBuf },
    ParentNotDirectory { path: PathBuf },
    ParentCreation { path: PathBuf, error: io::Error },
    TempCreation { error: io::Error },
    Write { error: io::Error },
    FileSync { error: io::Error },
    Commit { error: io::Error },
    ForeignOutput { path: PathBuf },
}

impl InstallError {
    /// The `io::ErrorKind` behind the failure: the preserved kind for every
    /// I/O stage, `AlreadyExists` for a name somebody else owns, and
    /// `InvalidInput` for a path the boundary refuses.
    // First consumed by plan 1421 with its error.
    #[allow(dead_code)]
    pub fn kind(&self) -> io::ErrorKind {
        match self {
            Self::OutsideOutput { .. }
            | Self::ParentSymlink { .. }
            | Self::ParentNotDirectory { .. } => io::ErrorKind::InvalidInput,
            Self::ParentCreation { error, .. }
            | Self::TempCreation { error }
            | Self::Write { error }
            | Self::FileSync { error }
            | Self::Commit { error } => error.kind(),
            Self::ForeignOutput { .. } => io::ErrorKind::AlreadyExists,
        }
    }
}

impl std::fmt::Display for InstallError {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::OutsideOutput { path } => {
                write!(
                    formatter,
                    "the target is outside the output folder: {}",
                    path.display()
                )
            }
            Self::ParentSymlink { path } => {
                write!(
                    formatter,
                    "a folder on the way to the target is a symlink: {}",
                    path.display()
                )
            }
            Self::ParentNotDirectory { path } => {
                write!(
                    formatter,
                    "a folder on the way to the target is not a directory: {}",
                    path.display()
                )
            }
            Self::ParentCreation { path, error } => {
                write!(
                    formatter,
                    "could not create the folder {}: {error}",
                    path.display()
                )
            }
            Self::TempCreation { error } => {
                write!(formatter, "could not stage the output: {error}")
            }
            Self::Write { error } => {
                write!(formatter, "could not write the staged output: {error}")
            }
            Self::FileSync { error } => {
                write!(formatter, "could not sync the staged output: {error}")
            }
            Self::Commit { error } => {
                write!(formatter, "could not install the staged output: {error}")
            }
            Self::ForeignOutput { path } => {
                write!(formatter, "an earlier run already wrote {}", path.display())
            }
        }
    }
}

impl std::error::Error for InstallError {
    fn source(&self) -> Option<&(dyn std::error::Error + 'static)> {
        match self {
            Self::ParentCreation { error, .. }
            | Self::TempCreation { error }
            | Self::Write { error }
            | Self::FileSync { error }
            | Self::Commit { error } => Some(error),
            _ => None,
        }
    }
}

/// Test-only failure injection for [`install`]. Each variant fails exactly one
/// stage; production always runs `Fault::None`.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
enum Fault {
    #[default]
    None,
    Parent,
    Temp,
    Write,
    FileSync,
    Persist,
    /// Never constructed on Windows: directory sync is an explicit no-warning
    /// platform outcome there, and only the non-Windows warning test builds it.
    #[cfg_attr(windows, allow(dead_code))]
    ParentSync,
    /// Not a failure: creates the final after staging, so the commit really
    /// races and only `AlreadyExists` may map to [`InstallError::ForeignOutput`].
    CommitConflict,
}

fn injected(stage: &str) -> io::Error {
    io::Error::other(format!("injected {stage} failure"))
}

/// Install finished bytes as a new file that appears complete or not at all.
///
/// Parents are created component by component inside the context's output root
/// and rechecked immediately before staging; an existing final — found before
/// staging or raced in after it — is refused as [`InstallError::ForeignOutput`]
/// and left byte-identical. Routing a relative source onto its final name stays
/// with the producers (plans 1433/1434); this owns the last mile.
// First consumed by plan 1421; tested directly until then.
#[allow(dead_code)]
pub fn install(
    context: &Context,
    final_path: &Path,
    bytes: &[u8],
) -> Result<(InstalledOutput, Option<DurabilityWarning>), InstallError> {
    install_with_fault(context, final_path, bytes, Fault::None)
}

fn install_with_fault(
    context: &Context,
    final_path: &Path,
    bytes: &[u8],
    fault: Fault,
) -> Result<(InstalledOutput, Option<DurabilityWarning>), InstallError> {
    let output_root = context.output_root();
    let relative =
        final_path
            .strip_prefix(output_root)
            .map_err(|_| InstallError::OutsideOutput {
                path: final_path.to_path_buf(),
            })?;
    if relative.as_os_str().is_empty() {
        return Err(InstallError::OutsideOutput {
            path: final_path.to_path_buf(),
        });
    }
    for component in relative.components() {
        if !matches!(component, Component::Normal(_)) {
            return Err(InstallError::OutsideOutput {
                path: final_path.to_path_buf(),
            });
        }
    }
    let parent = final_path
        .parent()
        .ok_or_else(|| InstallError::OutsideOutput {
            path: final_path.to_path_buf(),
        })?;
    ensure_parent_dir(output_root, parent, fault)?;
    if fs::symlink_metadata(final_path).is_ok() {
        return Err(InstallError::ForeignOutput {
            path: final_path.to_path_buf(),
        });
    }

    if fault == Fault::Temp {
        return Err(InstallError::TempCreation {
            error: injected("temp creation"),
        });
    }
    let mut staged = tempfile::NamedTempFile::new_in(parent)
        .map_err(|error| InstallError::TempCreation { error })?;
    #[cfg(unix)]
    staged
        .as_file()
        .set_permissions(fs::Permissions::from_mode(0o666))
        .map_err(|error| InstallError::TempCreation { error })?;
    if fault == Fault::Write {
        return Err(InstallError::Write {
            error: injected("write"),
        });
    }
    staged
        .write_all(bytes)
        .map_err(|error| InstallError::Write { error })?;
    staged
        .flush()
        .map_err(|error| InstallError::Write { error })?;
    if fault == Fault::FileSync {
        return Err(InstallError::FileSync {
            error: injected("file sync"),
        });
    }
    staged
        .as_file()
        .sync_all()
        .map_err(|error| InstallError::FileSync { error })?;
    if fault == Fault::CommitConflict {
        fs::write(final_path, b"somebody else won the race").ok();
    }

    if fault == Fault::Persist {
        return Err(InstallError::Commit {
            error: injected("commit"),
        });
    }
    if let Err(persisted) = staged.persist_noclobber(final_path) {
        let kind = persisted.error.kind();
        drop(persisted.file);
        if kind == io::ErrorKind::AlreadyExists {
            return Err(InstallError::ForeignOutput {
                path: final_path.to_path_buf(),
            });
        }
        return Err(InstallError::Commit {
            error: persisted.error,
        });
    }

    let mut hasher = Sha256::new();
    hasher.update(bytes);
    let receipt = InstalledOutput {
        path: final_path.to_path_buf(),
        sha256: format!("{:x}", hasher.finalize()),
        bytes: bytes.len() as u64,
    };
    Ok((receipt, sync_parent_warning(parent, fault)))
}
/// Create the output root and every parent below it, refusing symlinks and
/// non-directories before each creation and use. Levels grow top-down from the
/// filesystem root, so a missing output root grows its own chain one level at
/// a time; the last check stats the staged file's parent immediately before
/// the commit, narrowing the swap window to the commit itself. Every ancestor
/// of a canonical path is canonical, so the walk above the output root meets
/// no symlinks it did not already prove away at establishment.
fn ensure_parent_dir(output_root: &Path, parent: &Path, fault: Fault) -> Result<(), InstallError> {
    if fault == Fault::Parent {
        return Err(InstallError::ParentCreation {
            path: parent.to_path_buf(),
            error: injected("parent creation"),
        });
    }
    if parent.strip_prefix(output_root).is_err() {
        return Err(InstallError::OutsideOutput {
            path: parent.to_path_buf(),
        });
    }
    let mut current = PathBuf::new();
    for component in parent.components() {
        match component {
            Component::Prefix(_) | Component::RootDir => current.push(component),
            Component::Normal(_) => {
                current.push(component);
                ensure_one_dir(&current)?;
            }
            Component::CurDir | Component::ParentDir => {
                return Err(InstallError::OutsideOutput {
                    path: parent.to_path_buf(),
                });
            }
        }
    }
    Ok(())
}

fn ensure_one_dir(path: &Path) -> Result<(), InstallError> {
    match fs::symlink_metadata(path) {
        Ok(metadata) => check_dir(path, &metadata),
        Err(error) if error.kind() == io::ErrorKind::NotFound => {
            match fs::create_dir(path) {
                Ok(()) => Ok(()),
                Err(error) if error.kind() == io::ErrorKind::AlreadyExists => {
                    // Raced with another creator; judge what is there now.
                    let metadata = fs::symlink_metadata(path).map_err(|error| {
                        InstallError::ParentCreation {
                            path: path.to_path_buf(),
                            error,
                        }
                    })?;
                    check_dir(path, &metadata)
                }
                Err(error) => Err(InstallError::ParentCreation {
                    path: path.to_path_buf(),
                    error,
                }),
            }
        }
        Err(error) => Err(InstallError::ParentCreation {
            path: path.to_path_buf(),
            error,
        }),
    }
}

fn check_dir(path: &Path, metadata: &fs::Metadata) -> Result<(), InstallError> {
    if metadata.file_type().is_symlink() {
        return Err(InstallError::ParentSymlink {
            path: path.to_path_buf(),
        });
    }
    if !metadata.is_dir() {
        return Err(InstallError::ParentNotDirectory {
            path: path.to_path_buf(),
        });
    }
    Ok(())
}

/// Sync the finished file's parent so the rename survives an OS crash. A
/// failure keeps the install successful and comes back as a warning instead:
/// the bytes are complete, and a warning must never read as a file that did
/// not convert. Directory sync is not supported on Windows, which is an
/// explicit no-warning platform outcome rather than a silent skip.
fn sync_parent_warning(parent: &Path, fault: Fault) -> Option<DurabilityWarning> {
    #[cfg(windows)]
    {
        let _ = (parent, fault);
        None
    }
    #[cfg(not(windows))]
    {
        if fault == Fault::ParentSync {
            return Some(DurabilityWarning(format!(
                "could not sync the output directory {}: {}",
                parent.display(),
                injected("parent sync")
            )));
        }
        match fs::File::open(parent).and_then(|file| file.sync_all()) {
            Ok(()) => None,
            Err(error) => Some(DurabilityWarning(format!(
                "could not sync the output directory {}: {error}",
                parent.display()
            ))),
        }
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
        // Only the destination itself may not be a symlink. Ordinary places are
        // routinely reached through one — macOS spells `/var` as a link to
        // `/private/var`, so every temporary folder has an aliased ancestor — and
        // refusing those left the person who picked one with no way to proceed.
        // Canonicalizing the surviving prefix below still binds the boundary to the
        // real directory, whatever it was reached through.
        let last = index + 1 == normal.len();
        let observed = if last {
            fs::symlink_metadata(&candidate)
        } else {
            fs::metadata(&candidate)
        };
        match observed {
            Ok(metadata) if last && metadata.file_type().is_symlink() => {
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

/// A path that may be joined onto a proven root: components only, no root, no
/// dots. Also the rule a stored run record's paths have to pass before anything
/// is joined onto a folder and deleted.
pub(crate) fn normal_relative(path: &Path) -> Result<(), Error> {
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

    // A prefix spells dots as ordinary text: `\\.\PhysicalDrive0` is a device
    // namespace, not a dot segment. Scan only what follows the prefix, so
    // `validate_windows_lexical` keeps the namespace verdict.
    let prefix = match Path::new(path).components().next() {
        Some(Component::Prefix(prefix)) => prefix.as_os_str().encode_wide().count(),
        _ => 0,
    };
    // `encode_wide` is an iterator, and `split` belongs to slices. One owned
    // buffer per path keeps the raw units exact; this runs once per context,
    // not once per file.
    let units: Vec<u16> = path.encode_wide().skip(prefix).collect();
    units
        .split(|unit| *unit == b'/' as u16 || *unit == b'\\' as u16)
        .filter(|segment| !segment.is_empty())
        .map(std::ffi::OsString::from_wide)
        .collect::<Vec<_>>()
        .into_iter()
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
        // macOS hands out `/var/folders/...`, and `/var` is a symlink to
        // `/private/var`. `Context` canonicalizes its roots, so a fixture that
        // starts from the aliased spelling compares two different names.
        path.canonicalize().unwrap()
    }

    /// `PathBuf::push` strips `.` and `..` when the base is a Windows verbatim
    /// path, and `temp_dir` canonicalizes into exactly that form. Append the raw
    /// spelling so the segment under test survives to `validate_raw`.
    fn raw_join(base: &Path, tail: &str) -> PathBuf {
        let mut path = base.to_path_buf().into_os_string();
        for segment in tail.split('/') {
            path.push(std::path::MAIN_SEPARATOR_STR);
            path.push(segment);
        }
        PathBuf::from(path)
    }

    #[test]
    fn output_path_boundary_rejects_raw_dot_and_parent_segments_without_creating_paths() {
        let base = temp_dir("raw");
        let source = base.join("source");
        fs::create_dir(&source).unwrap();
        for output in [
            raw_join(&base, "missing/./output"),
            raw_join(&base, "missing/../output"),
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
                Context::establish(&source, &base.join("link")),
                Err(Error::OutputSymlink { .. })
            ));
        }
        fs::remove_dir_all(base).unwrap();
    }

    /// Temporary and home folders are routinely reached through a link — macOS
    /// spells `/var` as one — so an aliased ancestor has to resolve rather than
    /// leave the chosen folder unusable.
    #[cfg(unix)]
    #[test]
    fn an_output_under_a_symlinked_ancestor_establishes_at_its_canonical_path() {
        let base = temp_dir("symlinked-ancestor");
        let source = base.join("source");
        let target = base.join("target");
        fs::create_dir(&source).unwrap();
        fs::create_dir(&target).unwrap();
        std::os::unix::fs::symlink(&target, base.join("link")).unwrap();

        let context = Context::establish(&source, &base.join("link").join("output")).unwrap();

        assert_eq!(context.output_root(), target.join("output"));
        assert!(!context.output_root().exists());
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

    fn dir_names(dir: &Path) -> Vec<String> {
        let mut names: Vec<String> = fs::read_dir(dir)
            .unwrap()
            .map(|entry| entry.unwrap().file_name().to_string_lossy().into_owned())
            .collect();
        names.sort();
        names
    }

    fn boundary(source: &Path, out: &Path) -> Context {
        Context::establish(source, out).expect("the fixture boundary establishes")
    }

    #[test]
    fn create_new_output_writes_bytes_and_reports_the_receipt() {
        let base = temp_dir("install-first");
        let source = base.join("source");
        fs::create_dir(&source).unwrap();
        let out = base.join("out").join("nested");
        let context = boundary(&source, &out);

        let bytes = b"complete webp bytes";
        let final_path = out.join("pic.webp");
        let (receipt, warning) = install(&context, &final_path, bytes).expect("first install");
        assert!(warning.is_none(), "a clean install warns about nothing");
        assert_eq!(receipt.path, final_path);
        assert_eq!(receipt.bytes, bytes.len() as u64);
        assert_eq!(receipt.sha256.len(), 64, "hex sha256");
        assert_eq!(fs::read(&final_path).unwrap(), bytes);

        // The empty string's hash is a fixed vector, so this pins the digest
        // rather than recomputing it with the same code under test.
        let empty = out.join("empty.webp");
        let (receipt, _) = install(&context, &empty, b"").expect("empty install");
        assert_eq!(
            receipt.sha256,
            "e3b0c44298fc1c149afbf4c8996fb92427ae41e4649b934ca495991b7852b855"
        );

        #[cfg(unix)]
        assert_eq!(
            fs::metadata(&final_path).unwrap().permissions().mode() & 0o777,
            0o666,
            "umask owns the final permissions"
        );
        fs::remove_dir_all(base).unwrap();
    }

    #[test]
    fn an_existing_final_is_left_byte_identical_and_reported_foreign() {
        let base = temp_dir("install-foreign");
        let source = base.join("source");
        fs::create_dir(&source).unwrap();
        let out = base.join("out");
        let context = boundary(&source, &out);

        let final_path = out.join("taken.webp");
        fs::create_dir_all(out).unwrap();
        fs::write(&final_path, b"somebody else's bytes").unwrap();
        let error = install(&context, &final_path, b"new bytes").unwrap_err();
        assert!(
            matches!(error, InstallError::ForeignOutput { .. }),
            "an owned name is refused, not a failure: {error}"
        );
        assert_eq!(error.kind(), io::ErrorKind::AlreadyExists);
        assert_eq!(
            fs::read(&final_path).unwrap(),
            b"somebody else's bytes",
            "the foreign file is byte-identical"
        );
        fs::remove_dir_all(base).unwrap();
    }

    #[test]
    fn a_final_created_after_staging_maps_only_already_exists_to_foreign() {
        let base = temp_dir("install-raced");
        let source = base.join("source");
        fs::create_dir(&source).unwrap();
        let out = base.join("out");
        let context = boundary(&source, &out);
        let final_path = out.join("raced.webp");

        let error = install_with_fault(&context, &final_path, b"new bytes", Fault::CommitConflict)
            .unwrap_err();
        assert!(
            matches!(error, InstallError::ForeignOutput { .. }),
            "a raced commit is refused: {error}"
        );
        assert_eq!(
            fs::read(&final_path).unwrap(),
            b"somebody else won the race",
            "the winner's bytes stand"
        );
        assert_eq!(dir_names(&out), ["raced.webp"], "the staged temp is gone");

        // Any other commit failure stays a failure and stages nothing visible.
        let error = install_with_fault(
            &context,
            &out.join("broke.webp"),
            b"new bytes",
            Fault::Persist,
        )
        .unwrap_err();
        assert!(
            matches!(error, InstallError::Commit { .. }),
            "a non-conflict commit fails: {error}"
        );
        assert_eq!(dir_names(&out), ["raced.webp"]);
        fs::remove_dir_all(base).unwrap();
    }

    #[test]
    fn unicode_and_space_parents_are_created_component_by_component() {
        let base = temp_dir("install-parents");
        let source = base.join("source");
        fs::create_dir(&source).unwrap();
        let out = base.join("out");
        let context = boundary(&source, &out);

        let final_path = out
            .join("Album ünicode")
            .join("nested deep")
            .join("pic.webp");
        let (receipt, _) = install(&context, &final_path, b"bytes").expect("nested install");
        assert_eq!(receipt.bytes, 5);
        assert!(final_path.parent().unwrap().is_dir());
        assert_eq!(fs::read(&final_path).unwrap(), b"bytes");
        fs::remove_dir_all(base).unwrap();
    }

    #[test]
    #[cfg(unix)]
    fn a_symlinked_parent_is_refused_without_creating_anything() {
        let base = temp_dir("install-symlink");
        let source = base.join("source");
        fs::create_dir(&source).unwrap();
        let elsewhere = base.join("elsewhere");
        fs::create_dir(&elsewhere).unwrap();
        let out = base.join("out");
        let context = boundary(&source, &out);

        let link = out.join("link");
        fs::create_dir(&out).unwrap();
        std::os::unix::fs::symlink(&elsewhere, &link).unwrap();
        let final_path = link.join("pic.webp");
        let error = install(&context, &final_path, b"bytes").unwrap_err();
        assert!(
            matches!(error, InstallError::ParentSymlink { .. }),
            "a symlinked parent is refused: {error}"
        );
        assert!(!final_path.exists(), "nothing was written through the link");
        assert_eq!(dir_names(&elsewhere), Vec::<String>::new());
        fs::remove_dir_all(base).unwrap();
    }

    #[test]
    fn unrelated_temp_residue_survives_an_install() {
        let base = temp_dir("install-residue");
        let source = base.join("source");
        fs::create_dir(&source).unwrap();
        let out = base.join("out");
        let context = boundary(&source, &out);

        fs::create_dir(&out).unwrap();
        fs::write(out.join(".tmpleftover"), b"not mine").unwrap();
        let final_path = out.join("pic.webp");
        install(&context, &final_path, b"bytes").expect("install beside residue");
        assert_eq!(fs::read(out.join(".tmpleftover")).unwrap(), b"not mine");
        assert_eq!(dir_names(&out), [".tmpleftover", "pic.webp"]);
        fs::remove_dir_all(base).unwrap();
    }

    #[test]
    fn each_injected_stage_failure_writes_no_final_and_leaves_no_temp() {
        let base = temp_dir("install-faults");
        let source = base.join("source");
        fs::create_dir(&source).unwrap();
        let out = base.join("out");
        fs::create_dir(&out).unwrap();
        fs::write(out.join("residue.tmp"), b"not mine").unwrap();
        let context = boundary(&source, &out);

        for (fault, expected) in [
            (Fault::Parent, "ParentCreation"),
            (Fault::Temp, "TempCreation"),
            (Fault::Write, "Write"),
            (Fault::FileSync, "FileSync"),
            (Fault::Persist, "Commit"),
        ] {
            let final_path = out.join(format!("{expected}.webp"));
            let error = install_with_fault(&context, &final_path, b"bytes", fault).unwrap_err();
            let actual = match &error {
                InstallError::ParentCreation { .. } => "ParentCreation",
                InstallError::TempCreation { .. } => "TempCreation",
                InstallError::Write { .. } => "Write",
                InstallError::FileSync { .. } => "FileSync",
                InstallError::Commit { .. } => "Commit",
                other => panic!("{fault:?} failed as {other:?} instead of {expected}"),
            };
            assert_eq!(actual, expected);
            assert!(!final_path.exists(), "{expected} stages no final");
        }
        assert_eq!(dir_names(&out), ["residue.tmp"], "no staged temp lingers");
        fs::remove_dir_all(base).unwrap();
    }

    #[test]
    #[cfg(not(windows))]
    fn a_failed_parent_sync_still_reports_the_receipt_with_a_warning() {
        let base = temp_dir("install-warning");
        let source = base.join("source");
        fs::create_dir(&source).unwrap();
        let out = base.join("out");
        let context = boundary(&source, &out);
        let final_path = out.join("pic.webp");

        let (receipt, warning) =
            install_with_fault(&context, &final_path, b"bytes", Fault::ParentSync)
                .expect("a sync failure is not a conversion failure");
        assert_eq!(receipt.bytes, 5);
        let warning = warning.expect("the sync failure is reported");
        assert!(
            warning.0.contains("sync"),
            "the warning says what: {warning}"
        );
        assert_eq!(fs::read(&final_path).unwrap(), b"bytes");
        fs::remove_dir_all(base).unwrap();
    }

    #[test]
    fn an_escaping_final_is_refused_outside_the_output() {
        let base = temp_dir("install-outside");
        let source = base.join("source");
        fs::create_dir(&source).unwrap();
        let out = base.join("out");
        let context = boundary(&source, &out);

        for final_path in [base.join("evil.webp"), out.clone()] {
            let error = install(&context, &final_path, b"bytes").unwrap_err();
            assert!(
                matches!(error, InstallError::OutsideOutput { .. }),
                "an escaping final is refused: {error}"
            );
            assert_eq!(error.kind(), io::ErrorKind::InvalidInput);
        }
        assert!(!base.join("evil.webp").exists());
        assert!(!out.exists(), "refusal creates nothing");
        fs::remove_dir_all(base).unwrap();
    }

    #[test]
    #[cfg(windows)]
    fn long_windows_paths_install_within_runner_policy() {
        let base = temp_dir("install-long");
        let source = base.join("source");
        fs::create_dir(&source).unwrap();
        let out = base.join("out");
        let context = boundary(&source, &out);

        let long: String = std::iter::repeat_n('n', 180).collect();
        let final_path = out.join(&long).join("pic.webp");
        let (receipt, _) = install(&context, &final_path, b"bytes").expect("long install");
        assert_eq!(receipt.bytes, 5);
        assert_eq!(fs::read(&final_path).unwrap(), b"bytes");
        fs::remove_dir_all(base).unwrap();
    }
}
