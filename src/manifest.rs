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
//!
//! A line that does not parse is stepped over rather than trusted, and it is not
//! necessarily the last one: a full disk or a network share that does not honour
//! `O_APPEND` atomically can leave a half-written line with whole records after it.
//! Each append starts a fresh line of its own if the file does not already end on
//! one, so a torn line can only ever swallow itself.

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
    /// The libaom speed the AVIF output was encoded at, when it was not the
    /// default. Another speed writes other bytes, so a record without it only
    /// matches a run that also used the default. Absent on lines written
    /// before the field existed, which therefore match default-speed runs.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub avif_speed: Option<u8>,
    pub written: u64,
    /// Relative to the backup root, and to where the original belongs when it
    /// comes back. Only replace mode moves an original.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub backup: Option<PathBuf>,
    /// A line that takes back the record above it. The record goes down before
    /// the original moves, so a file that fails after that leaves a claim on a
    /// name it never took; this is how the same run withdraws it.
    #[serde(default, skip_serializing_if = "is_false")]
    pub void: bool,
}

fn is_false(value: &bool) -> bool {
    !*value
}

impl Record {
    /// Whether the file at `path` is still the one this record installed. Size and
    /// timestamp, the only two facts the record kept about it.
    pub fn installed(&self, path: &Path) -> bool {
        std::fs::symlink_metadata(path).is_ok_and(|metadata| {
            metadata.len() == self.output_bytes && modified(&metadata) == self.output_modified
        })
    }

    /// The line that withdraws this one.
    pub fn voided(&self) -> Record {
        Record {
            void: true,
            ..self.clone()
        }
    }

    /// What identifies one record across the run that wrote it and the line that
    /// takes it back.
    fn identity(&self) -> (String, String, u64) {
        (path_key(&self.source), path_key(&self.output), self.written)
    }
}

#[derive(Clone, Debug, Default)]
pub struct Manifest {
    pub outputs: Vec<Record>,
    /// Records whose paths could not be trusted, named. A manifest arrives with
    /// whatever folder it was in — a download, a shared drive — so a line saying
    /// `../../.ssh/id_rsa` is a file this app must refuse rather than delete.
    pub rejected: Vec<Rejected>,
}

/// A line that was refused, and the line itself: it is the only evidence of what
/// was in the folder, so an undo puts it back rather than dropping it.
#[derive(Clone, Debug)]
pub struct Rejected {
    pub line: String,
    pub reason: String,
}

impl Manifest {
    /// The record that put an original away for a source that is itself an
    /// earlier run's output. Converting a folder twice is a chain, and the file
    /// worth keeping is the one at the start of it.
    ///
    /// Only while the file on disk is still that run's output. Somebody who has
    /// edited or replaced it since is holding an original of their own, and
    /// inheriting a backup for it would rename over the only copy.
    pub fn chain(&self, relative: &Path, on_disk: &Path) -> Option<&Record> {
        let relative = path_key(relative);
        self.outputs.iter().rev().find(|record| {
            record.backup.is_some()
                && path_key(&record.output) == relative
                && record.installed(on_disk)
        })
    }

    /// The newest record naming this source-and-output pair, when some run
    /// described it. Read-only: the skip decision asks, the write paths own.
    pub fn latest(&self, source: &Path, output: &Path) -> Option<&Record> {
        let (source, output) = (path_key(source), path_key(output));
        self.outputs
            .iter()
            .rev()
            .find(|record| path_key(&record.source) == source && path_key(&record.output) == output)
    }
}

/// The settings one run wrote with. Every record it appends repeats them, so a
/// folder can be read back without knowing which run made which file.
#[derive(Clone)]
pub struct Stamp {
    format: String,
    quality: String,
    max_edge: Option<u32>,
    avif_speed: Option<u8>,
    written: u64,
}

impl Stamp {
    pub fn new(format: Format, quality: Quality, max_edge: MaxEdge) -> Self {
        Self {
            format: format.label().to_string(),
            quality: quality.label(),
            max_edge: max_edge.0,
            avif_speed: crate::avif::configured_speed(),
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
            avif_speed: self.avif_speed,
            written: self.written,
            backup,
            void: false,
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
    let mut voided = std::collections::HashSet::new();
    for (index, line) in text.lines().enumerate() {
        if line.trim().is_empty() {
            continue;
        }
        let Ok(record) = serde_json::from_str::<Record>(line) else {
            continue;
        };
        if record.void {
            voided.insert(record.identity());
            continue;
        }
        match untrusted(&record) {
            Some(reason) => manifest.rejected.push(Rejected {
                line: line.to_string(),
                reason: format!("{NAME} line {} ({reason})", index + 1),
            }),
            None => manifest.outputs.push(record),
        }
    }
    // A withdrawn record describes a name its run never took. Acting on it would
    // delete somebody else's file and report an original that is not there.
    manifest
        .outputs
        .retain(|record| !voided.contains(&record.identity()));
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

/// How many originals this folder could put back. Counted by backup, not by
/// record: a folder converted twice has two records over one original, and
/// offering to restore it twice would be a lie about what is there.
pub fn restorable(root: &Path) -> usize {
    load(root)
        .outputs
        .iter()
        .filter_map(|record| record.backup.as_ref().map(|backup| path_key(backup)))
        .collect::<std::collections::HashSet<_>>()
        .len()
}

/// One line, appended and flushed before this file's original moves.
///
/// `O_APPEND` puts every worker's line whole at the end without a lock, and the
/// file is only ever rewritten by `restore`, which runs when nothing else is.
pub fn append_record(output_root: &Path, record: &Record) -> Result<(), String> {
    let mut line = serde_json::to_vec(record).map_err(|error| error.to_string())?;
    line.push(b'\n');
    // A full disk, or a share that does not honour `O_APPEND` atomically, can
    // leave a line with no newline on the end. Opening the next record with one
    // keeps that torn line from swallowing this one too.
    if unterminated(&path(output_root)) {
        line.insert(0, b'\n');
    }
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

/// Whether the last byte on disk is something other than a newline.
fn unterminated(path: &Path) -> bool {
    use std::io::{Read, Seek, SeekFrom};

    let Ok(mut file) = std::fs::File::open(path) else {
        return false;
    };
    let Ok(end) = file.seek(SeekFrom::End(0)) else {
        return false;
    };
    if end == 0 || file.seek(SeekFrom::Start(end - 1)).is_err() {
        return false;
    }
    let mut last = [0u8; 1];
    file.read_exact(&mut last).is_ok() && last[0] != b'\n'
}

/// Rewrite the file with the records that are left, and with every line the undo
/// refused to act on: those lines are the only evidence of what was claimed here,
/// and dropping them would quietly erase the thing being reported. Only `restore`
/// rewrites, and it goes through the same stage-and-rename as any output.
fn save(root: &Path, records: &[Record], rejected: &[Rejected]) -> Result<(), String> {
    let mut encoded = Vec::new();
    for record in records {
        serde_json::to_writer(&mut encoded, record).map_err(|error| error.to_string())?;
        encoded.push(b'\n');
    }
    for line in rejected {
        encoded.extend_from_slice(line.line.as_bytes());
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
    let mut failures: Vec<String> = loaded
        .rejected
        .iter()
        .map(|rejected| rejected.reason.clone())
        .collect();
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
    let saved = if kept.is_empty() && loaded.rejected.is_empty() {
        remove_if_present(&path(root))
    } else {
        save(root, &kept, &loaded.rejected)
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
    if output.symlink_metadata().is_err() {
        return Ok(());
    }
    if !record.installed(output) {
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

#[cfg(test)]
mod tests {
    use super::*;
    use crate::convert::{Format, MaxEdge, Quality};

    /// Every temp dir is per-process unique, so parallel threads and repeated
    /// runs never share a manifest file.
    fn test_dir(tag: &str) -> PathBuf {
        let dir = std::env::temp_dir().join(format!("press-manifest-{tag}-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).expect("the manifest fixture dir is created");
        dir.canonicalize()
            .expect("the manifest fixture has one filesystem identity")
    }

    fn record(source: &str, output: &str, backup: Option<&str>, written: u64) -> Record {
        Record {
            source: PathBuf::from(source),
            source_bytes: 0,
            source_modified: None,
            output: PathBuf::from(output),
            output_bytes: 0,
            output_modified: None,
            format: Format::WebP.label().to_string(),
            quality: Quality::lossy(80.).label(),
            max_edge: MaxEdge::FULL.0,
            avif_speed: None,
            written,
            backup: backup.map(PathBuf::from),
            void: false,
        }
    }

    fn stamp() -> Stamp {
        Stamp::new(Format::WebP, Quality::lossy(80.), MaxEdge::FULL)
    }

    #[test]
    fn appended_records_load_back_in_order() {
        let dir = test_dir("round-trip");
        let first = record("one.png", "one.webp", None, 1);
        let second = record("two.png", "two.webp", Some("two.png"), 2);
        let third = record("three.png", "three.webp", None, 3);
        for record in [&first, &second, &third] {
            append_record(&dir, record).expect("the record appends");
        }
        let loaded = load(&dir);
        assert_eq!(loaded.outputs, vec![first, second, third]);
        assert!(loaded.rejected.is_empty());
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn avif_speed_round_trips_and_old_lines_match_default() {
        let dir = test_dir("speed-round-trip");
        let mut fast = record("one.png", "one.avif", None, 1);
        fast.avif_speed = Some(9);
        append_record(&dir, &fast).expect("the fast record appends");
        // A line from before the field existed carries no speed key at all.
        // `skip_serializing_if` already omits a `None`, so this plain record
        // is exactly what an old writer left behind.
        let plain = record("two.png", "two.avif", None, 2);
        assert_eq!(plain.avif_speed, None);
        append_record(&dir, &plain).expect("the plain record appends");
        let raw = std::fs::read_to_string(path(&dir)).expect("the manifest reads");
        let mut lines = raw.lines();
        assert!(
            lines.next().is_some_and(|line| line.contains("avif_speed")),
            "an explicit speed is written down"
        );
        assert!(
            lines
                .next()
                .is_some_and(|line| !line.contains("avif_speed")),
            "a default run leaves no speed key behind"
        );
        let loaded = load(&dir);
        assert_eq!(loaded.outputs.len(), 2);
        assert_eq!(loaded.outputs[0].avif_speed, Some(9));
        assert_eq!(loaded.outputs[1].avif_speed, None);
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn a_torn_last_line_costs_only_itself() {
        let dir = test_dir("torn");
        append_record(&dir, &record("one.png", "one.webp", None, 1)).expect("append");
        append_record(&dir, &record("two.png", "two.webp", None, 2)).expect("append");
        std::fs::OpenOptions::new()
            .append(true)
            .open(path(&dir))
            .expect("the manifest opens")
            .write_all(b"{\"source\":")
            .expect("the torn line is written");
        let loaded = load(&dir);
        assert_eq!(loaded.outputs.len(), 2);
        assert!(loaded.rejected.is_empty());
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn a_missing_manifest_loads_as_empty() {
        let dir = test_dir("missing");
        let loaded = load(&dir);
        assert!(loaded.outputs.is_empty());
        assert!(loaded.rejected.is_empty());
        assert_eq!(restorable(&dir), 0);
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn absolute_and_parent_paths_are_rejected_not_acted_on() {
        let dir = test_dir("rejected");
        let absolute_output = record("/abs/evil.png", "/abs/out.webp", None, 1);
        let mut parent_source = record("ok.png", "ok.webp", None, 2);
        parent_source.source = PathBuf::from("../evil.png");
        let mut absolute_backup = record("other.png", "other.webp", None, 3);
        absolute_backup.backup = Some(PathBuf::from("/abs/other.png"));
        for record in [&absolute_output, &parent_source, &absolute_backup] {
            append_record(&dir, record).expect("the hostile line appends");
        }
        let loaded = load(&dir);
        assert!(loaded.outputs.is_empty());
        assert_eq!(loaded.rejected.len(), 3);
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn a_voided_record_is_withdrawn() {
        let dir = test_dir("void");
        let withdrawn = record("gone.png", "gone.webp", None, 1);
        let kept = record("kept.png", "kept.webp", None, 2);
        append_record(&dir, &withdrawn).expect("append");
        append_record(&dir, &withdrawn.voided()).expect("the withdrawal appends");
        append_record(&dir, &kept).expect("append");
        let loaded = load(&dir);
        assert_eq!(loaded.outputs, vec![kept]);
        assert!(loaded.rejected.is_empty());
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn an_edited_output_is_not_the_installed_one() {
        let dir = test_dir("edited");
        let staged = dir.join("staged.webp");
        std::fs::write(&staged, vec![7u8; 64]).expect("the staged output is written");
        std::fs::write(dir.join("source.png"), vec![1u8; 16]).expect("the source is written");
        let record = stamp()
            .record(
                (&dir, &dir),
                &dir.join("source.png"),
                &dir.join("staged.webp"),
                &staged,
                None,
            )
            .expect("plain relative paths record");
        assert!(record.installed(&staged));
        std::fs::OpenOptions::new()
            .append(true)
            .open(&staged)
            .expect("the output opens")
            .write_all(b"somebody edited this")
            .expect("the edit lands");
        assert!(!record.installed(&staged));
        assert!(remove_output(&staged, &record).is_err());
        assert!(staged.is_file(), "a refused undo leaves the edit alone");
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn restorable_counts_backups_not_records() {
        let dir = test_dir("restorable");
        append_record(&dir, &record("one.png", "one.webp", Some("shared.png"), 1)).expect("append");
        append_record(&dir, &record("two.png", "two.webp", Some("shared.png"), 2)).expect("append");
        assert_eq!(restorable(&dir), 1);
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn restore_walks_newest_first() {
        let dir = test_dir("restore-order");
        let backups = backup_root(&dir);
        std::fs::create_dir_all(&backups).expect("the backup mirror is created");
        // The true original. Run 1 turned it into run 1's output; run 2 turned
        // that into run 2's output, so run 2's backup holds run 1's output.
        std::fs::write(dir.join("photo.png"), vec![1u8; 32]).expect("the original");
        std::fs::write(dir.join("staged-one.webp"), vec![2u8; 48]).expect("run 1 stages");
        let first = stamp()
            .record(
                (&dir, &dir),
                &dir.join("photo.png"),
                &dir.join("photo.webp"),
                &dir.join("staged-one.webp"),
                Some(&backups.join("photo.png")),
            )
            .expect("run 1 records");
        std::fs::rename(dir.join("photo.png"), backups.join("photo.png")).expect("run 1 moves");
        std::fs::rename(dir.join("staged-one.webp"), dir.join("photo.webp"))
            .expect("run 1 installs");
        std::fs::write(dir.join("staged-two.webp"), vec![3u8; 40]).expect("run 2 stages");
        let second = stamp()
            .record(
                (&dir, &dir),
                &dir.join("photo.webp"),
                &dir.join("photo-two.webp"),
                &dir.join("staged-two.webp"),
                Some(&backups.join("photo.webp")),
            )
            .expect("run 2 records");
        std::fs::rename(dir.join("photo.webp"), backups.join("photo.webp")).expect("run 2 moves");
        std::fs::rename(dir.join("staged-two.webp"), dir.join("photo-two.webp"))
            .expect("run 2 installs");
        append_record(&dir, &first).expect("run 1 appends");
        append_record(&dir, &second).expect("run 2 appends");
        let restored = restore(&dir);
        assert!(restored.failures.is_empty(), "{:?}", restored.failures);
        assert_eq!(
            std::fs::read(dir.join("photo.png")).expect("the original came back"),
            vec![1u8; 32]
        );
        assert_eq!(load(&dir).outputs.len(), 0, "both records are spent");
        assert!(!backups.exists(), "the emptied mirror does not linger");
        let _ = std::fs::remove_dir_all(&dir);
    }
}
