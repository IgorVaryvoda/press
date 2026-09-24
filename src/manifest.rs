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
//! `O_APPEND` atomically can leave a half-written line with whole records after it,
//! or tear one mid-character. Each append starts a fresh line of its own if the
//! file does not already end on one, and reading splits the file into lines before
//! decoding any of them, so a torn line — UTF-8 or JSON — can only ever swallow
//! itself.

use std::io::Write;
use std::path::{Path, PathBuf};
use std::time::{SystemTime, UNIX_EPOCH};

use serde::{Deserialize, Serialize};

use crate::convert::{Format, MaxEdge, Quality, path_key};

/// The bytes a decoder consumed at a processing boundary.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct SourceIdentity {
    pub bytes: u64,
    pub hash: String,
}

impl SourceIdentity {
    pub fn from_bytes(bytes: &[u8]) -> Self {
        Self {
            bytes: bytes.len() as u64,
            hash: hash_bytes(bytes),
        }
    }

    /// Read the current source for a reuse check. A failed read is a mismatch.
    pub fn from_path(path: &Path) -> std::io::Result<Self> {
        crate::scan::read_source_bytes(path)
            .map(|bytes| Self::from_bytes(&bytes))
            .map_err(|error| {
                std::io::Error::new(
                    std::io::ErrorKind::InvalidData,
                    format!("could not read source snapshot: {error:?}"),
                )
            })
    }

    pub fn matches_path(&self, path: &Path) -> bool {
        Self::from_path(path).is_ok_and(|current| &current == self)
    }
}

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
    /// Hex SHA-256 of the source bytes the run consumed, absent on lines
    /// written before hashes existed. Identical bytes still match; any edit
    /// mismatches even when the filesystem reports the same timestamp.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub source_hash: Option<String>,
    /// Relative to the output root.
    pub output: PathBuf,
    /// What this run installed, so an undo can tell its own file from one
    /// somebody has edited since.
    pub output_bytes: u64,
    pub output_modified: Option<u64>,
    /// Hex SHA-256 of the output bytes at install time. Absent on older lines,
    /// which retain their size and timestamp restore check.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub output_hash: Option<String>,
    pub format: String,
    pub quality: String,
    pub max_edge: Option<u32>,
    /// The libaom speed the AVIF output was encoded at, when it was not the
    /// default. Another speed writes other bytes, so a record without it only
    /// matches a run that also used the default. Absent on lines written
    /// before the field existed, which therefore match default-speed runs.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub avif_speed: Option<u8>,
    /// Hex fingerprint of the normalized effective settings that produced
    /// this output. Absent on lines written before fingerprints existed.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub recipe: Option<String>,
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
            let stats = metadata.is_file()
                && metadata.len() == self.output_bytes
                && modified(&metadata) == self.output_modified;
            match self.output_hash.as_deref() {
                Some(expected) => stats && hash_file(path).is_ok_and(|actual| actual == expected),
                None => stats,
            }
        })
    }
    /// Whether a fingerprinted record still names the bytes at `path`.
    /// Unreadable sources are mismatches, so verified reuse fails closed.
    pub fn source_matches(&self, path: &Path) -> Option<bool> {
        let recorded = self.source_hash.as_deref()?;
        Some(
            SourceIdentity::from_path(path).is_ok_and(|identity| {
                identity.bytes == self.source_bytes && identity.hash == recorded
            }),
        )
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
    /// Non-blank lines that were not valid UTF-8 or not a `Record` at all, kept
    /// verbatim. They are evidence — the only trace of whatever a torn write or a
    /// hand edit left behind — so a rewrite puts them back rather than dropping
    /// them.
    pub unparsed: Vec<Vec<u8>>,
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
    recipe: String,
    written: u64,
}

impl Stamp {
    pub fn new(format: Format, quality: Quality, max_edge: MaxEdge) -> Self {
        Self::with_speed(format, quality, max_edge, crate::avif::configured_speed())
    }

    /// The same stamp with an explicit speed, so tests pin identity without
    /// touching the process-wide dial that parallel tests share.
    pub fn with_speed(
        format: Format,
        quality: Quality,
        max_edge: MaxEdge,
        avif_speed: Option<u8>,
    ) -> Self {
        let avif_speed = crate::recipe::canonical_avif_speed(avif_speed);
        Self {
            format: format.label().to_string(),
            quality: quality.label(),
            max_edge: max_edge.0,
            avif_speed,
            recipe: crate::recipe::fingerprint_settings(format, quality, max_edge, avif_speed),
            written: now(),
        }
    }

    /// The effective-settings fingerprint this stamp writes with each record.
    /// One authority: callers compare against it rather than recomputing it.
    pub(crate) fn recipe(&self) -> &str {
        &self.recipe
    }

    /// The AVIF speed this stamp claims, as the encoder wants it. `Stamp::new`
    /// reads the process-wide dial once at the start of a run; every file that run
    /// writes then encodes at this value rather than reading the dial again, so the
    /// bytes and the line describing them always agree.
    pub fn avif_speed(&self) -> u8 {
        self.avif_speed.unwrap_or(crate::avif::DEFAULT_SPEED)
    }

    /// One output as a record, or `None` when its paths do not sit under the roots
    /// they were planned against — which would make the record a lie.
    ///
    /// Written from the staged file, before anything moves: `staged` carries the
    /// bytes and the timestamp the installed file will have, and the source is
    /// still under its own name.
    #[cfg(test)]
    pub fn record(
        &self,
        roots: (&Path, &Path),
        source: &Path,
        output: &Path,
        staged: &Path,
        backup: Option<&Path>,
    ) -> Option<Record> {
        self.record_inner(roots, source, output, staged, backup, None, None)
    }

    /// Record the source identity captured from the decoder's input buffer and
    /// hash the exact encoded bytes that will be installed.
    #[allow(clippy::too_many_arguments)]
    pub fn record_with_identity(
        &self,
        roots: (&Path, &Path),
        source: &Path,
        output: &Path,
        staged: &Path,
        backup: Option<&Path>,
        identity: &SourceIdentity,
        encoded: &[u8],
    ) -> Option<Record> {
        self.record_inner(
            roots,
            source,
            output,
            staged,
            backup,
            Some(identity),
            Some(hash_bytes(encoded)),
        )
    }

    #[allow(clippy::too_many_arguments)]
    fn record_inner(
        &self,
        roots: (&Path, &Path),
        source: &Path,
        output: &Path,
        staged: &Path,
        backup: Option<&Path>,
        identity: Option<&SourceIdentity>,
        output_hash: Option<String>,
    ) -> Option<Record> {
        let (root, out_dir) = roots;
        let relative_source = source.strip_prefix(root).ok()?;
        let relative_output = output.strip_prefix(out_dir).ok()?;
        // The mirror hangs off the established output boundary, which is where
        // the caller built the backup path from. Stripping the audited root
        // instead would agree only by accident: a canonicalised output root and
        // the walk's own spelling of the same folder differ on Windows, where
        // one wears a verbatim prefix, and on macOS across `/var`.
        let backup = match backup {
            Some(backup) => Some(
                backup
                    .strip_prefix(backup_root(out_dir))
                    .ok()?
                    .to_path_buf(),
            ),
            None => None,
        };
        let original = std::fs::metadata(source).ok();
        let installed = std::fs::metadata(staged).ok();
        Some(Record {
            source: relative_source.to_path_buf(),
            source_bytes: identity.map_or_else(
                || original.as_ref().map_or(0, |metadata| metadata.len()),
                |identity| identity.bytes,
            ),
            source_modified: original.as_ref().and_then(modified),
            // New runs use the exact bytes decoded. Older callers retain the
            // record-time hash for manifest compatibility.
            source_hash: identity.map_or_else(
                || original.as_ref().and_then(|_| hash_file(source).ok()),
                |identity| Some(identity.hash.clone()),
            ),
            output: relative_output.to_path_buf(),
            output_bytes: installed.as_ref().map_or(0, |metadata| metadata.len()),
            output_modified: installed.as_ref().and_then(modified),
            output_hash: output_hash
                .or_else(|| installed.as_ref().and_then(|_| hash_file(staged).ok())),
            format: self.format.clone(),
            quality: self.quality.clone(),
            max_edge: self.max_edge,
            avif_speed: self.avif_speed,
            recipe: Some(self.recipe.clone()),
            written: self.written,
            backup,
            void: false,
        })
    }
}

/// Replace mode moves every original into one mirror of the audited tree, so a
/// record stores the path under it and stays portable.
///
/// The argument is the established output root, not whatever spelling the walk
/// used: every caller that names, records or restores a backup has to arrive at
/// the same directory, and only one of the two spellings is canonical.
pub fn backup_root(output_root: &Path) -> PathBuf {
    output_root.join(crate::scan::BACKUP_DIR)
}

pub fn path(output_root: &Path) -> PathBuf {
    output_root.join(NAME)
}

/// A missing manifest reads as an empty one. Used by planning code (`convert.rs`,
/// `saved_plan.rs`) that wants best-effort history and would rather under-report
/// than fail a run over a manifest it merely could not finish reading.
///
/// Anything that *acts* on what it reads — deletes, moves, or otherwise treats
/// the manifest as authoritative — must call `read` instead: this function turns
/// a read error into the same empty result as "never converted here", and
/// `restore` deleting an empty-looking manifest it actually failed to read is
/// the bug this module exists to not repeat.
pub fn load(output_root: &Path) -> Manifest {
    read(output_root).unwrap_or_default()
}

/// Read the manifest, telling "nothing here yet" apart from "something is here
/// and could not be read." The two look the same to an eye that only counts
/// records, but they are not the same fact: an empty result must be safe to
/// delete over, and an unreadable file must not be.
///
/// The file is read whole as bytes, then split into lines and decoded one at a
/// time, so neither a read error nor a byte torn mid-character anywhere in the
/// file can cost more than the line it sits in.
pub fn read(output_root: &Path) -> Result<Manifest, String> {
    let bytes = match std::fs::read(path(output_root)) {
        Ok(bytes) => bytes,
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => {
            return Ok(Manifest::default());
        }
        Err(error) => return Err(format!("{NAME} could not be read: {error}")),
    };
    let mut manifest = Manifest::default();
    let mut voided = std::collections::HashSet::new();
    for (index, line) in bytes.split(|byte| *byte == b'\n').enumerate() {
        if line.iter().all(u8::is_ascii_whitespace) {
            continue;
        }
        let Ok(text) = std::str::from_utf8(line) else {
            manifest.unparsed.push(line.to_vec());
            continue;
        };
        let Ok(record) = serde_json::from_str::<Record>(text) else {
            manifest.unparsed.push(line.to_vec());
            continue;
        };
        if record.void {
            voided.insert(record.identity());
            continue;
        }
        match untrusted(&record) {
            Some(reason) => manifest.rejected.push(Rejected {
                line: text.to_string(),
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
    Ok(manifest)
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
/// refused to act on or could not even parse: those lines are the only evidence
/// of what was claimed here, and dropping them would quietly erase the thing
/// being reported. Only `restore` rewrites, and it goes through the same
/// stage-and-rename as any output.
fn save(
    root: &Path,
    records: &[Record],
    rejected: &[Rejected],
    unparsed: &[Vec<u8>],
) -> Result<(), String> {
    let mut encoded = Vec::new();
    for record in records {
        serde_json::to_writer(&mut encoded, record).map_err(|error| error.to_string())?;
        encoded.push(b'\n');
    }
    for line in rejected {
        encoded.extend_from_slice(line.line.as_bytes());
        encoded.push(b'\n');
    }
    for line in unparsed {
        encoded.extend_from_slice(line);
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
    // Computed once: every backup-bearing record shares the same mirror, so a
    // symlinked or missing-as-a-directory mirror fails all of them the same way
    // rather than being re-stated (and re-raced) per record.
    let backups_unsafe = unsafe_backup_root(&backups);
    let loaded = match read(root) {
        Ok(loaded) => loaded,
        // An unreadable manifest is not an empty one. Acting on it — even just
        // deleting it as "nothing to restore" — would erase the only map from
        // the backups on disk to the files they belong under.
        Err(message) => {
            return Restore {
                restored: Vec::new(),
                failures: vec![message],
            };
        }
    };
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
        if let Some(reason) = &backups_unsafe {
            failures.push(format!("{} ({reason})", record.output.display()));
            kept.push(record.clone());
            continue;
        }
        // The folder a manifest line names came with the audited tree, which is
        // untrusted below the root the user chose: a symlink standing in for any
        // folder on these paths could send the move somewhere else entirely.
        if let Err(reason) = plain_folders(&backups, &from)
            .and_then(|()| plain_folders(root, &original))
            .and_then(|()| plain_folders(root, &output))
        {
            failures.push(format!("{} ({reason})", record.output.display()));
            kept.push(record.clone());
            continue;
        }
        match restore_one(root, &from, &output, &original, record, &backups) {
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
    let saved = if kept.is_empty() && loaded.rejected.is_empty() && loaded.unparsed.is_empty() {
        remove_if_present(&path(root))
    } else {
        save(root, &kept, &loaded.rejected, &loaded.unparsed)
    };
    if let Err(message) = saved {
        failures.push(format!("{NAME} ({message})"));
    }
    // Only ever succeeds once the tree it mirrored is empty again, which is the
    // one case where leaving it behind would be litter.
    let _ = std::fs::remove_dir(&backups);
    Restore { restored, failures }
}

/// Whether a folder on disk is safe to walk through: a plain directory, not a
/// symlink or (on Windows) a junction standing in for one.
fn unsafe_folder(metadata: &std::fs::Metadata) -> bool {
    if metadata.file_type().is_symlink() || !metadata.is_dir() {
        return true;
    }
    #[cfg(windows)]
    {
        use std::os::windows::fs::MetadataExt;
        if metadata.file_attributes() & 0x400 != 0 {
            return true;
        }
    }
    false
}

/// Whether the backup mirror itself is safe to act through, checked directly
/// because `plain_folders` only walks the levels *below* the base it is given
/// and this is that base. The write side creates this folder fresh under the
/// audited root; a symlink or junction standing in for it did not come from a
/// run this app made — it came with the folder.
fn unsafe_backup_root(backups: &Path) -> Option<String> {
    let metadata = backups.symlink_metadata().ok()?;
    unsafe_folder(&metadata).then(|| format!("{} is not a plain folder", backups.display()))
}

/// Every folder from `base` down to `path`'s parent must be a plain directory.
/// `base` is trusted (the folder the user chose); everything under it came with
/// the folder, so a symlink or junction there could point anywhere.
/// Missing levels are fine here — only existing ones are checked, and the walk
/// stops as soon as one is missing, because nothing below a folder that does
/// not exist yet can already be a symlink.
fn plain_folders(base: &Path, path: &Path) -> Result<(), String> {
    let Ok(relative) = path.strip_prefix(base) else {
        return Err(format!(
            "{} is not under {}",
            path.display(),
            base.display()
        ));
    };
    let Some(parent) = relative.parent() else {
        return Ok(());
    };
    let mut ancestor = base.to_path_buf();
    for component in parent.components() {
        let std::path::Component::Normal(part) = component else {
            return Err(format!(
                "{} is not a plain folder",
                ancestor.join(component.as_os_str()).display()
            ));
        };
        ancestor.push(part);
        match ancestor.symlink_metadata() {
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => return Ok(()),
            Err(_) => return Err(format!("{} is not a plain folder", ancestor.display())),
            Ok(metadata) if unsafe_folder(&metadata) => {
                return Err(format!("{} is not a plain folder", ancestor.display()));
            }
            Ok(_) => {}
        }
    }
    Ok(())
}

fn restore_one(
    root: &Path,
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
        ensure_plain_parents(root, parent)?;
    }
    std::fs::rename(backup, original)
        .map_err(|error| format!("could not move it back: {error}"))?;
    prune_empty(backup.parent(), backups);
    Ok(Some(original.to_path_buf()))
}

/// Create `parent` under `root` one level at a time, checking each level before
/// creating the next and again right after. `create_dir_all` would build a
/// missing chain through whatever its parents currently resolve to; this walk
/// refuses a symlink at any level instead of walking through it, the same shape
/// as `convert.rs::ensure_directory` on the write side.
fn ensure_plain_parents(root: &Path, parent: &Path) -> Result<(), String> {
    let Ok(relative) = parent.strip_prefix(root) else {
        return Err(format!(
            "{} is not under {}",
            parent.display(),
            root.display()
        ));
    };
    let mut ancestor = root.to_path_buf();
    for component in relative.components() {
        let std::path::Component::Normal(part) = component else {
            return Err(format!(
                "{} is not a plain folder",
                ancestor.join(component.as_os_str()).display()
            ));
        };
        ancestor.push(part);
        match ancestor.symlink_metadata() {
            Ok(metadata) if unsafe_folder(&metadata) => {
                return Err(format!("{} is not a plain folder", ancestor.display()));
            }
            Ok(_) => {}
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => {
                std::fs::create_dir(&ancestor).map_err(|error| {
                    format!("{} could not be recreated: {error}", ancestor.display())
                })?;
                let metadata = ancestor.symlink_metadata().map_err(|error| {
                    format!("{} could not be checked: {error}", ancestor.display())
                })?;
                if unsafe_folder(&metadata) {
                    return Err(format!("{} is not a plain folder", ancestor.display()));
                }
            }
            Err(error) => {
                return Err(format!(
                    "{} could not be checked: {error}",
                    ancestor.display()
                ));
            }
        }
    }
    Ok(())
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

/// Hex SHA-256 of a file's bytes, streamed so a large source never sits in
/// memory twice. Recording hashes the source the run already decoded; skipping
/// hashes only the outputs it would otherwise reuse.
pub(crate) fn hash_file(path: &Path) -> std::io::Result<String> {
    use sha2::Digest;
    let mut file = std::fs::File::open(path)?;
    let mut hash = sha2::Sha256::new();
    std::io::copy(&mut file, &mut hash)?;
    Ok(format!("{:x}", hash.finalize()))
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub(crate) struct FileIdentity {
    pub bytes: u64,
    pub modified: Option<u64>,
    pub hash: String,
}

pub(crate) fn file_identity(path: &Path) -> Option<FileIdentity> {
    let metadata = std::fs::symlink_metadata(path).ok()?;
    if !metadata.is_file() {
        return None;
    }
    Some(FileIdentity {
        bytes: metadata.len(),
        modified: modified(&metadata),
        hash: hash_file(path).ok()?,
    })
}

fn hash_bytes(bytes: &[u8]) -> String {
    use sha2::{Digest, Sha256};
    format!("{:x}", Sha256::digest(bytes))
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
            source_hash: None,
            output: PathBuf::from(output),
            output_bytes: 0,
            output_modified: None,
            output_hash: None,
            format: Format::WebP.label().to_string(),
            quality: Quality::lossy(80.).label(),
            max_edge: MaxEdge::FULL.0,
            avif_speed: None,
            recipe: None,
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
        let original_modified = record.output_modified;
        std::fs::write(&staged, vec![8u8; 64]).expect("the output opens");
        if let Some(seconds) = original_modified {
            std::fs::File::options()
                .write(true)
                .open(&staged)
                .expect("the edited output opens")
                .set_modified(UNIX_EPOCH + std::time::Duration::from_secs(seconds))
                .expect("the original timestamp is restored");
        }
        assert!(!record.installed(&staged));
        assert!(remove_output(&staged, &record).is_err());
        assert!(staged.is_file(), "a refused undo leaves the edit alone");
        let _ = std::fs::remove_dir_all(&dir);
    }
    #[test]
    fn a_rewritten_source_mismatches_its_recorded_hash() {
        let dir = test_dir("source-hash");
        let source = dir.join("source.png");
        std::fs::write(&source, vec![1u8; 16]).expect("the source is written");
        let record = stamp()
            .record(
                (&dir, &dir),
                &source,
                &dir.join("out.webp"),
                &dir.join("out.webp"),
                None,
            )
            .expect("plain relative paths record");
        assert!(
            record.source_hash.is_some(),
            "recording hashes what it consumed"
        );
        assert_eq!(record.source_matches(&source), Some(true));
        std::fs::write(&source, vec![2u8; 16]).expect("the edit lands");
        assert_eq!(record.source_matches(&source), Some(false));
        // Identical bytes rewritten are still the same content, whatever the
        // clock says: reuse stays valid.
        std::fs::write(&source, vec![1u8; 16]).expect("the rewrite lands");
        assert_eq!(record.source_matches(&source), Some(true));
        // Lines from before hashes answer nothing. A vanished modern source is
        // a mismatch, so verified reuse fails closed.
        let mut legacy = record.clone();
        legacy.source_hash = None;
        assert_eq!(legacy.source_matches(&source), None);
        assert_eq!(record.source_matches(&dir.join("gone.png")), Some(false));
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

    /// One replace-mode conversion, staged and installed the way `convert.rs`
    /// really does it: `name` moves into the backup mirror, `output_name` takes
    /// its place, and a record describing the move is appended. Returns the
    /// original's bytes so a test can tell a genuine restore from a lookalike.
    fn replace_one(dir: &Path, name: &str, output_name: &str) -> Vec<u8> {
        let backups = backup_root(dir);
        std::fs::create_dir_all(&backups).expect("the backup mirror is created");
        let original_bytes = vec![9u8; 24];
        std::fs::write(dir.join(name), &original_bytes).expect("the original is written");
        let staged = dir.join(format!("staged-{output_name}"));
        std::fs::write(&staged, vec![5u8; 40]).expect("the run stages its output");
        let record = stamp()
            .record(
                (dir, dir),
                &dir.join(name),
                &dir.join(output_name),
                &staged,
                Some(&backups.join(name)),
            )
            .expect("plain relative paths record");
        std::fs::rename(dir.join(name), backups.join(name)).expect("the original moves aside");
        std::fs::rename(&staged, dir.join(output_name)).expect("the output installs");
        append_record(dir, &record).expect("the record appends");
        original_bytes
    }

    #[test]
    fn a_restore_blocked_by_a_new_file_keeps_its_record() {
        let dir = test_dir("blocked");
        replace_one(&dir, "photo.png", "photo.webp");
        let blocker = vec![3u8; 5];
        std::fs::write(dir.join("photo.png"), &blocker).expect("a new file lands on the slot");
        let restored = restore(&dir);
        assert_eq!(restored.failures.len(), 1, "{:?}", restored.failures);
        assert!(
            restored.failures[0].contains("something else is already at"),
            "{}",
            restored.failures[0]
        );
        assert!(restored.restored.is_empty());
        assert_eq!(
            std::fs::read(dir.join("photo.png")).expect("the blocker reads back"),
            blocker,
            "the blocking file is untouched"
        );
        assert!(
            dir.join("photo.webp").is_file(),
            "the output stays until the slot is free"
        );
        assert_eq!(
            load(&dir).outputs.len(),
            1,
            "the record survives for a retry"
        );
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn a_missing_backup_with_the_original_present_is_already_restored() {
        let dir = test_dir("already-restored");
        let original_bytes = replace_one(&dir, "photo.png", "photo.webp");
        let backups = backup_root(&dir);
        // Somebody already put the original back by hand: the backup is gone
        // and the original is sitting under its own name again.
        std::fs::rename(backups.join("photo.png"), dir.join("photo.png"))
            .expect("the hand restore moves the original back");
        let restored = restore(&dir);
        assert!(restored.failures.is_empty(), "{:?}", restored.failures);
        assert_eq!(
            std::fs::read(dir.join("photo.png")).expect("the original is still there"),
            original_bytes
        );
        assert!(
            load(&dir).outputs.is_empty(),
            "the record has nothing left to say"
        );
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn a_missing_backup_and_missing_original_is_named() {
        let dir = test_dir("both-missing");
        replace_one(&dir, "photo.png", "photo.webp");
        let backups = backup_root(&dir);
        std::fs::remove_file(backups.join("photo.png")).expect("the backup is lost");
        let restored = restore(&dir);
        assert_eq!(restored.failures.len(), 1, "{:?}", restored.failures);
        assert!(
            restored.failures[0].contains("its original is no longer at"),
            "{}",
            restored.failures[0]
        );
        assert!(restored.restored.is_empty());
        assert_eq!(
            load(&dir).outputs.len(),
            1,
            "the record survives for a retry"
        );
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn a_partial_restore_leaves_only_the_failures_for_a_retry() {
        let dir = test_dir("partial");
        replace_one(&dir, "one.png", "one.webp");
        replace_one(&dir, "two.png", "two.webp");
        let blocker = vec![4u8; 6];
        std::fs::write(dir.join("one.png"), &blocker).expect("one slot is blocked");
        let first = restore(&dir);
        assert_eq!(first.failures.len(), 1, "{:?}", first.failures);
        assert_eq!(first.restored.len(), 1, "{:?}", first.restored);
        assert_eq!(
            load(&dir).outputs.len(),
            1,
            "only the blocked record remains"
        );
        std::fs::remove_file(dir.join("one.png")).expect("the blocker is cleared");
        let second = restore(&dir);
        assert!(second.failures.is_empty(), "{:?}", second.failures);
        assert_eq!(second.restored.len(), 1, "{:?}", second.restored);
        assert!(load(&dir).outputs.is_empty());
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn a_line_torn_mid_character_costs_only_itself() {
        let dir = test_dir("torn-utf8");
        append_record(&dir, &record("one.png", "one.webp", None, 1)).expect("append");
        append_record(&dir, &record("two.png", "two.webp", None, 2)).expect("append");
        std::fs::OpenOptions::new()
            .append(true)
            .open(path(&dir))
            .expect("the manifest opens")
            .write_all(b"{\"source\":\"caf\xC3")
            .expect("the torn line is written");
        // A fresh append always starts its own line, so the torn line above
        // cannot swallow this one even though it never got a trailing newline.
        append_record(&dir, &record("three.png", "three.webp", None, 3)).expect("append");
        let loaded = load(&dir);
        assert_eq!(loaded.outputs.len(), 3, "{:?}", loaded.outputs);
        let read = read(&dir).expect("a readable file with one bad line is still Ok");
        assert_eq!(read.unparsed.len(), 1, "{:?}", read.unparsed);
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn restore_keeps_a_manifest_it_could_not_parse() {
        let dir = test_dir("invalid-utf8-only");
        let bytes: &[u8] = b"\xFF\xFE\n";
        std::fs::write(path(&dir), bytes).expect("the hostile manifest is written");
        let _ = restore(&dir);
        assert_eq!(
            std::fs::read(path(&dir)).expect("the manifest is still there"),
            bytes,
            "a line that never parsed is put back exactly as it was, not dropped"
        );
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[cfg(unix)]
    #[test]
    fn restore_reports_a_manifest_it_could_not_read() {
        let dir = test_dir("manifest-is-a-directory");
        std::fs::create_dir(path(&dir)).expect("a directory sits where the manifest belongs");
        let restored = restore(&dir);
        assert_eq!(restored.failures.len(), 1, "{:?}", restored.failures);
        assert!(
            restored.failures[0].contains(NAME),
            "{}",
            restored.failures[0]
        );
        assert!(
            path(&dir).is_dir(),
            "a manifest restore could not even read is left exactly as it was"
        );
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[cfg(unix)]
    #[test]
    fn restore_refuses_a_symlinked_subfolder() {
        let dir = test_dir("symlinked-subfolder");
        let elsewhere = test_dir("symlinked-subfolder-elsewhere");
        let backups = backup_root(&dir);
        std::fs::create_dir_all(backups.join("sub")).expect("the mirror subfolder exists");
        std::os::unix::fs::symlink(&elsewhere, dir.join("sub"))
            .expect("the root gets a symlinked subfolder");
        let backup_bytes = vec![6u8; 12];
        std::fs::write(backups.join("sub/x.png"), &backup_bytes)
            .expect("a real backup sits in the mirror");
        append_record(&dir, &record("sub/x.png", "x.webp", Some("sub/x.png"), 1)).expect("append");
        let restored = restore(&dir);
        assert_eq!(restored.failures.len(), 1, "{:?}", restored.failures);
        assert!(
            restored.failures[0].contains("is not a plain folder"),
            "{}",
            restored.failures[0]
        );
        assert!(
            !elsewhere.join("x.png").exists(),
            "nothing lands through the symlinked subfolder"
        );
        assert!(
            backups.join("sub/x.png").is_file(),
            "the backup stays in the mirror"
        );
        assert_eq!(
            load(&dir).outputs.len(),
            1,
            "the record is kept for a retry"
        );
        let _ = std::fs::remove_dir_all(&dir);
        let _ = std::fs::remove_dir_all(&elsewhere);
    }

    #[cfg(unix)]
    #[test]
    fn restore_refuses_a_symlinked_backup_mirror() {
        let dir = test_dir("symlinked-mirror");
        let elsewhere = test_dir("symlinked-mirror-elsewhere");
        let secret_bytes = vec![7u8; 9];
        std::fs::write(elsewhere.join("secret.png"), &secret_bytes)
            .expect("the outside file exists");
        std::os::unix::fs::symlink(&elsewhere, backup_root(&dir))
            .expect("the mirror itself is a symlink");
        append_record(
            &dir,
            &record("secret.png", "secret.webp", Some("secret.png"), 1),
        )
        .expect("append");
        let restored = restore(&dir);
        assert_eq!(restored.failures.len(), 1, "{:?}", restored.failures);
        assert!(
            restored.failures[0].contains("is not a plain folder"),
            "{}",
            restored.failures[0]
        );
        assert_eq!(
            std::fs::read(elsewhere.join("secret.png")).expect("the outside file is untouched"),
            secret_bytes
        );
        assert!(
            !dir.join("secret.png").exists(),
            "nothing arrives through the symlinked mirror"
        );
        let _ = std::fs::remove_dir_all(&dir);
        let _ = std::fs::remove_dir_all(&elsewhere);
    }

    #[cfg(unix)]
    #[test]
    fn restore_refuses_to_delete_an_output_through_a_symlink() {
        let dir = test_dir("symlinked-output");
        let elsewhere = test_dir("symlinked-output-elsewhere");
        let backups = backup_root(&dir);
        std::fs::create_dir_all(&backups).expect("the backup mirror is created");
        std::os::unix::fs::symlink(&elsewhere, dir.join("sub"))
            .expect("the root gets a symlinked subfolder");
        let output_bytes = vec![8u8; 20];
        std::fs::write(elsewhere.join("out.webp"), &output_bytes)
            .expect("the real output sits outside the root");
        let backup_bytes = vec![2u8; 14];
        std::fs::write(backups.join("x.png"), &backup_bytes)
            .expect("a real backup sits in the mirror");
        let installed = stamp()
            .record(
                (&dir, &dir),
                &dir.join("x.png"),
                &dir.join("sub/out.webp"),
                &elsewhere.join("out.webp"),
                Some(&backups.join("x.png")),
            )
            .expect("plain relative paths record");
        append_record(&dir, &installed).expect("append");
        let restored = restore(&dir);
        assert_eq!(restored.failures.len(), 1, "{:?}", restored.failures);
        assert!(
            restored.failures[0].contains("is not a plain folder"),
            "{}",
            restored.failures[0]
        );
        assert_eq!(
            std::fs::read(elsewhere.join("out.webp")).expect("the outside output is untouched"),
            output_bytes
        );
        assert!(
            backups.join("x.png").is_file(),
            "the backup stays in the mirror"
        );
        let _ = std::fs::remove_dir_all(&dir);
        let _ = std::fs::remove_dir_all(&elsewhere);
    }
}
