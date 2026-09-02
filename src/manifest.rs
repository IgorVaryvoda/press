//! What a run wrote, kept beside its outputs.
//!
//! Two things need a memory that outlives the process. A later run has to tell its
//! own earlier output from a file it is about to destroy — `plan_outputs`
//! de-duplicates inside one run and cannot see the last one, so converting
//! `shot.jpg` quietly replaced the `shot.webp` an earlier run made from `shot.png`.
//! And replace mode has to be undoable after a restart, which means the backup an
//! original moved into is a fact on disk rather than something the window is holding.
//!
//! One line of JSON per written output, appended before the original moves. A run
//! killed after four hundred of five hundred files leaves four hundred records and
//! four hundred restorable originals; a file rewritten at the end of the run would
//! have left none of either. Appending is also how eight workers, a window and a
//! command line share one file without a lock or a lost record.

use std::io::Write;
use std::path::{Path, PathBuf};
use std::time::{SystemTime, UNIX_EPOCH};

use serde::{Deserialize, Serialize};

use crate::convert::{Format, MaxEdge, Quality, path_key};

/// Dot-prefixed so a file browser hides it, and named so the walk and the output
/// count can step over it rather than report it as an image.
pub const NAME: &str = ".press-manifest.jsonl";

/// One output this folder holds, and where it came from.
#[derive(Clone, Debug, Deserialize, PartialEq, Serialize)]
pub struct Record {
    /// Relative to the audited root, so a folder that moves keeps its record.
    pub source: PathBuf,
    pub source_bytes: u64,
    /// Seconds since the Unix epoch; `None` when the filesystem would not say.
    pub source_modified: Option<u64>,
    /// Relative to the output root.
    pub output: PathBuf,
    /// What this run installed, so an undo can tell its own file from one
    /// somebody has edited since.
    pub output_bytes: u64,
    pub output_modified: Option<u64>,
    pub format: String,
    pub quality: String,
    pub max_edge: Option<u32>,
    pub written: u64,
    /// Relative to the backup root, and to where the original belongs when it
    /// comes back. Only replace mode moves an original.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub backup: Option<PathBuf>,
}

#[derive(Clone, Debug, Default)]
pub struct Manifest {
    pub outputs: Vec<Record>,
    /// Records whose paths could not be trusted, named. A manifest arrives with
    /// whatever folder it was in — a download, a shared drive — so a line saying
    /// `../../.ssh/id_rsa` is a file this app must refuse rather than delete.
    pub rejected: Vec<String>,
}

impl Manifest {
    /// Who owns an output now: the newest record naming it, ignoring records
    /// whose output is no longer there. A claim on a file somebody has already
    /// deleted is not a reason to rename anything.
    pub fn claim(&self, out_dir: &Path, output: &Path) -> Option<&Record> {
        let output = path_key(output);
        self.outputs.iter().rev().find(|record| {
            path_key(&record.output) == output
                && out_dir.join(&record.output).symlink_metadata().is_ok()
        })
    }

    /// The record that put an original away for a source that is itself an
    /// earlier run's output. Converting a folder twice is a chain, and the file
    /// worth keeping is the one at the start of it.
    pub fn chain(&self, source: &Path) -> Option<&Record> {
        let source = path_key(source);
        self.outputs
            .iter()
            .rev()
            .find(|record| record.backup.is_some() && path_key(&record.output) == source)
    }
}

/// The settings one run wrote with. Every record it appends repeats them, so a
/// folder can be read back without knowing which run made which file.
#[derive(Clone)]
pub struct Stamp {
    format: String,
    quality: String,
    max_edge: Option<u32>,
    written: u64,
}

impl Stamp {
    pub fn new(format: Format, quality: Quality, max_edge: MaxEdge) -> Self {
        Self {
            format: format.label().to_string(),
            quality: quality.label(),
            max_edge: max_edge.0,
            written: now(),
        }
    }

    /// One output as a record, or `None` when its paths do not sit under the roots
    /// they were planned against — which would make the record a lie.
    ///
    /// Written from the staged file, before anything moves: `staged` carries the
    /// bytes and the timestamp the installed file will have, and the source is
    /// still under its own name.
    pub fn record(
        &self,
        roots: (&Path, &Path),
        source: &Path,
        output: &Path,
        staged: &Path,
        backup: Option<&Path>,
    ) -> Option<Record> {
        let (root, out_dir) = roots;
        let relative_source = source.strip_prefix(root).ok()?;
        let relative_output = output.strip_prefix(out_dir).ok()?;
        let backup = match backup {
            Some(backup) => Some(backup.strip_prefix(backup_root(root)).ok()?.to_path_buf()),
            None => None,
        };
        let original = std::fs::metadata(source).ok();
        let installed = std::fs::metadata(staged).ok();
        Some(Record {
            source: relative_source.to_path_buf(),
            source_bytes: original.as_ref().map_or(0, |metadata| metadata.len()),
            source_modified: original.as_ref().and_then(modified),
            output: relative_output.to_path_buf(),
            output_bytes: installed.as_ref().map_or(0, |metadata| metadata.len()),
            output_modified: installed.as_ref().and_then(modified),
            format: self.format.clone(),
            quality: self.quality.clone(),
            max_edge: self.max_edge,
            written: self.written,
            backup,
        })
    }
}

/// Replace mode moves every original into one mirror of the audited tree, so a
/// record stores the path under it and stays portable.
pub fn backup_root(root: &Path) -> PathBuf {
    root.join(crate::scan::BACKUP_DIR)
}

pub fn path(output_root: &Path) -> PathBuf {
    output_root.join(NAME)
}

/// A missing or unreadable manifest reads as an empty one, and so does a line
/// that is not a record: a run killed mid-write leaves one torn line, always the
/// last, and losing it must not cost the four hundred before it.
pub fn load(output_root: &Path) -> Manifest {
    let mut manifest = Manifest::default();
    let Ok(text) = std::fs::read_to_string(path(output_root)) else {
        return manifest;
    };
    for (index, line) in text.lines().enumerate() {
        if line.trim().is_empty() {
            continue;
        }
        let Ok(record) = serde_json::from_str::<Record>(line) else {
            continue;
        };
        match untrusted(&record) {
            Some(reason) => manifest
                .rejected
                .push(format!("{NAME} line {} ({reason})", index + 1)),
            None => manifest.outputs.push(record),
        }
    }
    manifest
}

/// Why a record must not be acted on. Every path in it is joined onto a root and
/// then deleted from or moved to, so nothing but a plain relative path will do.
fn untrusted(record: &Record) -> Option<String> {
    let backup = record.backup.as_ref().map(|backup| ("backup", backup));
    [("source", &record.source), ("output", &record.output)]
        .into_iter()
        .chain(backup)
        .find_map(|(label, path)| {
            crate::output::normal_relative(path).err().map(|_| {
                format!(
                    "its {label} is not a plain relative path: {}",
                    path.display()
                )
            })
        })
}

/// How many originals this folder could put back.
pub fn restorable(root: &Path) -> usize {
    load(root)
        .outputs
        .iter()
        .filter(|record| record.backup.is_some())
        .count()
}

/// One line, appended and flushed before this file's original moves.
///
/// `O_APPEND` puts every worker's line whole at the end without a lock, and the
/// file is only ever rewritten by `restore`, which runs when nothing else is.
pub fn append_record(output_root: &Path, record: &Record) -> Result<(), String> {
    let mut line = serde_json::to_vec(record).map_err(|error| error.to_string())?;
    line.push(b'\n');
    // The output root already exists: the staged file that this record describes
    // was created inside it a moment ago, through the same directory checks.
    let mut file = std::fs::OpenOptions::new()
        .create(true)
        .append(true)
        .open(path(output_root))
        .map_err(|error| error.to_string())?;
    file.write_all(&line).map_err(|error| error.to_string())?;
    file.sync_data().map_err(|error| error.to_string())
}

/// Rewrite the file with the records that are left. Only `restore` does this, and
/// it goes through the same stage-and-rename as any output.
fn save(root: &Path, records: &[Record]) -> Result<(), String> {
    let mut encoded = Vec::new();
    for record in records {
        serde_json::to_writer(&mut encoded, record).map_err(|error| error.to_string())?;
        encoded.push(b'\n');
    }
    crate::convert::write_output(root, &path(root), &encoded).map_err(|failure| {
        failure
            .reason()
            .unwrap_or_else(|| "the run record could not be written".to_string())
    })
}

/// What an undo did, named on both sides.
pub struct Restore {
    pub restored: Vec<PathBuf>,
    pub failures: Vec<String>,
}

/// Move every backed-up original back over the file that replaced it.
///
/// Newest first. A folder converted twice has the second run's backup holding the
/// first run's output, so walking forward would put back an intermediate file and
/// then delete the original that was meant to survive.
pub fn restore(root: &Path) -> Restore {
    let backups = backup_root(root);
    let loaded = load(root);
    let mut restored = Vec::new();
    let mut failures = loaded.rejected.clone();
    let mut kept = Vec::new();

    for record in loaded.outputs.iter().rev() {
        let Some(backup) = record.backup.as_ref() else {
            kept.push(record.clone());
            continue;
        };
        // Belt and braces over `load`'s check: nothing is joined onto a root here
        // without landing under it.
        let (Some(from), Some(original), Some(output)) = (
            inside(&backups, backup),
            inside(root, backup),
            inside(root, &record.output),
        ) else {
            failures.push(format!(
                "{} (its paths leave the folder)",
                record.output.display()
            ));
            kept.push(record.clone());
            continue;
        };
        match restore_one(&from, &output, &original, record, &backups) {
            Ok(Some(original)) => restored.push(original),
            // Already back where it belongs, so the record has nothing left to say.
            Ok(None) => {}
            Err(message) => {
                failures.push(format!("{} ({message})", record.output.display()));
                kept.push(record.clone());
            }
        }
    }

    kept.reverse();
    let saved = if kept.is_empty() {
        remove_if_present(&path(root))
    } else {
        save(root, &kept)
    };
    if let Err(message) = saved {
        failures.push(format!("{NAME} ({message})"));
    }
    // Only ever succeeds once the tree it mirrored is empty again, which is the
    // one case where leaving it behind would be litter.
    let _ = std::fs::remove_dir(&backups);
    Restore { restored, failures }
}

fn restore_one(
    backup: &Path,
    output: &Path,
    original: &Path,
    record: &Record,
    backups: &Path,
) -> Result<Option<PathBuf>, String> {
    if backup.symlink_metadata().is_err() {
        // No backup and an original under its own name is a run that failed after
        // its record went down, or one already undone. Either way the original is
        // safe; only the file this run installed is still to go.
        if original.symlink_metadata().is_ok() {
            // Best effort on the way out: the name goes only if it holds the file
            // this record describes. Anything else was never installed by this run
            // — a failed install leaves whatever blocked it — and is not ours.
            let _ = remove_output(output, record);
            return Ok(None);
        }
        return Err(format!("its original is no longer at {}", backup.display()));
    }
    // The slot has to be free before anything is deleted. A WebP converted to
    // WebP is the exception: the output is standing on the original's own name,
    // and removing it is how the slot is freed.
    let same_name = path_key(original) == path_key(output);
    if !same_name && original.symlink_metadata().is_ok() {
        return Err(format!(
            "something else is already at {}",
            original.display()
        ));
    }
    remove_output(output, record)?;
    if let Some(parent) = original.parent() {
        std::fs::create_dir_all(parent)
            .map_err(|error| format!("{} could not be recreated: {error}", parent.display()))?;
    }
    std::fs::rename(backup, original)
        .map_err(|error| format!("could not move it back: {error}"))?;
    prune_empty(backup.parent(), backups);
    Ok(Some(original.to_path_buf()))
}

/// Remove the file the run installed, and only that file. A different size or a
/// later timestamp is somebody's edit, and an undo that eats it is not an undo.
fn remove_output(output: &Path, record: &Record) -> Result<(), String> {
    let Ok(metadata) = output.symlink_metadata() else {
        return Ok(());
    };
    if metadata.len() != record.output_bytes || modified(&metadata) != record.output_modified {
        return Err(format!(
            "{} has changed since the run wrote it",
            output.display()
        ));
    }
    remove_if_present(output)
}

fn remove_if_present(path: &Path) -> Result<(), String> {
    match std::fs::remove_file(path) {
        Ok(()) => Ok(()),
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => Ok(()),
        Err(error) => Err(format!("{} is still on disk: {error}", path.display())),
    }
}

fn inside(root: &Path, relative: &Path) -> Option<PathBuf> {
    let joined = root.join(relative);
    joined.starts_with(root).then_some(joined)
}

/// Walk the emptied mirror back up, never above the mirror itself. `remove_dir`
/// refuses a folder that still holds something, so this also stops on its own at
/// the first level still in use.
fn prune_empty(directory: Option<&Path>, backups: &Path) {
    let mut directory = directory.map(Path::to_path_buf);
    while let Some(path) = directory {
        if !path.starts_with(backups) || std::fs::remove_dir(&path).is_err() {
            return;
        }
        directory = path.parent().map(Path::to_path_buf);
    }
}

fn modified(metadata: &std::fs::Metadata) -> Option<u64> {
    metadata
        .modified()
        .ok()
        .and_then(|time| time.duration_since(UNIX_EPOCH).ok())
        .map(|since| since.as_secs())
}

fn now() -> u64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map_or(0, |since| since.as_secs())
}
