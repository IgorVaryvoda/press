//! What a run wrote, kept beside its outputs.
//!
//! Two things need a memory that outlives the process. A later run has to tell its
//! own earlier output from a file it is about to destroy — `plan_outputs`
//! de-duplicates inside one run and cannot see the last one, so converting
//! `shot.jpg` quietly replaced the `shot.webp` an earlier run made from `shot.png`.
//! And replace mode has to be undoable after a restart, which means the backup an
//! original moved into is a fact on disk rather than something the window is holding.
//!
//! One small JSON file per output root, appended to run after run.

use std::path::{Path, PathBuf};
use std::time::{SystemTime, UNIX_EPOCH};

use serde::{Deserialize, Serialize};

use crate::convert::{Format, MaxEdge, Quality, path_key};

/// Dot-prefixed so a file browser hides it, and named so the walk and the output
/// count can step over it rather than report it as an image.
pub const NAME: &str = ".press-manifest.json";

const VERSION: u32 = 1;

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
    pub format: String,
    pub quality: String,
    pub max_edge: Option<u32>,
    pub written: u64,
    /// Relative to the backup root. Only replace mode moves an original.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub backup: Option<PathBuf>,
}

impl Record {
    /// Whether `other` is the same claim written again: one source, one output.
    /// A rerun supersedes its own record; a different source never does.
    fn supersedes(&self, other: &Record) -> bool {
        path_key(&self.output) == path_key(&other.output)
            && path_key(&self.source) == path_key(&other.source)
    }
}

#[derive(Clone, Debug, Default, Deserialize, Serialize)]
pub struct Manifest {
    #[serde(default)]
    pub version: u32,
    #[serde(default)]
    pub outputs: Vec<Record>,
}

impl Manifest {
    /// Who owns an output now: the newest record naming it. A name written twice
    /// belongs to whoever wrote it last, and that is the only claim a later run
    /// must not walk over.
    pub fn claim(&self, output: &Path) -> Option<&Record> {
        let output = path_key(output);
        self.outputs
            .iter()
            .rev()
            .find(|record| path_key(&record.output) == output)
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

    /// One written output as a record, or `None` when its paths do not sit under
    /// the roots they were planned against — which would make the record a lie.
    ///
    /// The original is measured through `backup` when there is one: replace mode
    /// has already moved it, and the file it moved is the file being described.
    pub fn record(
        &self,
        root: &Path,
        out_dir: &Path,
        source: &Path,
        output: &Path,
        backup: Option<&Path>,
    ) -> Option<Record> {
        let relative_source = source.strip_prefix(root).ok()?;
        let relative_output = output.strip_prefix(out_dir).ok()?;
        let original = backup.unwrap_or(source);
        let metadata = std::fs::metadata(original).ok();
        Some(Record {
            source: relative_source.to_path_buf(),
            source_bytes: metadata.as_ref().map_or(0, |metadata| metadata.len()),
            source_modified: metadata
                .as_ref()
                .and_then(|metadata| metadata.modified().ok())
                .and_then(|time| time.duration_since(UNIX_EPOCH).ok())
                .map(|since| since.as_secs()),
            output: relative_output.to_path_buf(),
            format: self.format.clone(),
            quality: self.quality.clone(),
            max_edge: self.max_edge,
            written: self.written,
            backup: backup
                .and_then(|backup| backup.strip_prefix(backup_root(root)).ok())
                .map(Path::to_path_buf),
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

/// A missing, unreadable or hand-mangled manifest reads as an empty one. It is a
/// record of the past, and losing it must not stop the next run.
pub fn load(output_root: &Path) -> Manifest {
    std::fs::read_to_string(path(output_root))
        .ok()
        .and_then(|text| serde_json::from_str(&text).ok())
        .unwrap_or_default()
}

/// How many originals this folder could put back.
pub fn restorable(root: &Path) -> usize {
    load(root)
        .outputs
        .iter()
        .filter(|record| record.backup.is_some())
        .count()
}

pub fn save(output_root: &Path, manifest: &Manifest) -> Result<(), String> {
    let manifest = Manifest {
        version: VERSION,
        outputs: manifest.outputs.clone(),
    };
    let encoded = serde_json::to_vec_pretty(&manifest).map_err(|error| error.to_string())?;
    // The same stage-and-rename every output gets: a half-written manifest would
    // describe files that are not there and hide files that are.
    crate::convert::write_output(output_root, &path(output_root), &encoded).map_err(|failure| {
        failure
            .reason()
            .unwrap_or_else(|| "the run record could not be written".to_string())
    })
}

/// Add this run's records, newest last.
///
/// A record with the same source and the same output supersedes the one it
/// repeats; every other one is kept, because a chain of runs over one folder is
/// exactly what an undo has to walk back through.
pub fn append(output_root: &Path, records: Vec<Record>) -> Result<(), String> {
    if records.is_empty() {
        return Ok(());
    }
    let mut manifest = load(output_root);
    manifest
        .outputs
        .retain(|kept| !records.iter().any(|record| record.supersedes(kept)));
    manifest.outputs.extend(records);
    save(output_root, &manifest)
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
    let mut manifest = load(root);
    let mut restored = Vec::new();
    let mut failures = Vec::new();
    let mut kept = Vec::new();

    for record in manifest.outputs.iter().rev() {
        let Some(backup) = record.backup.as_ref() else {
            kept.push(record.clone());
            continue;
        };
        let original = root.join(&record.source);
        match restore_one(
            &backups.join(backup),
            &root.join(&record.output),
            &original,
            &backups,
        ) {
            Ok(()) => restored.push(original),
            Err(message) => {
                failures.push(format!("{} ({message})", record.source.display()));
                kept.push(record.clone());
            }
        }
    }

    kept.reverse();
    manifest.outputs = kept;
    let saved = if manifest.outputs.is_empty() {
        remove_if_present(&path(root))
    } else {
        save(root, &manifest)
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
    backups: &Path,
) -> Result<(), String> {
    if backup.symlink_metadata().is_err() {
        return Err(format!("its original is no longer at {}", backup.display()));
    }
    // The output goes first. A WebP converted to WebP kept its own name, so the
    // original lands back on top of the output, and removing that afterwards
    // would delete the file just put back.
    remove_if_present(output)?;
    if original.symlink_metadata().is_ok() {
        return Err(format!(
            "something else is already at {}",
            original.display()
        ));
    }
    if let Some(parent) = original.parent() {
        std::fs::create_dir_all(parent)
            .map_err(|error| format!("{} could not be recreated: {error}", parent.display()))?;
    }
    std::fs::rename(backup, original)
        .map_err(|error| format!("could not move it back: {error}"))?;
    prune_empty(backup.parent(), backups);
    Ok(())
}

fn remove_if_present(path: &Path) -> Result<(), String> {
    match std::fs::remove_file(path) {
        Ok(()) => Ok(()),
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => Ok(()),
        Err(error) => Err(format!("{} is still on disk: {error}", path.display())),
    }
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

fn now() -> u64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map_or(0, |since| since.as_secs())
}
