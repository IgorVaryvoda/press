//! Supplier submission state: durable attempts against a resolved assignment.
//!
//! The desktop never invents authority. An assignment snapshot names the
//! workspace, supplier, products, slots and requirement revisions a retailer
//! authorized; every submission carries those pins plus the identity of the
//! exact bytes it sends. Uploads go through a canonical intake behind the
//! [`Intake`] trait. Production intake is Studio's confirmed contract, which
//! does not exist yet; tests and drills run the scripted [`FakeIntake`].
//! Nothing here performs network, billing or review: the server owns those,
//! and every displayed state below only ever repeats a server answer.
//!
//! Attempt IDs are allocated from a persisted counter before anything sends,
//! so a retry reuses the sender's identity and a restart recovers the same
//! queue instead of minting duplicates.

use serde::{Deserialize, Serialize};
use std::io::Read;

/// The only assignment and log schemas this Press reads.
pub const SCHEMA_VERSION: u32 = 1;

/// An attempt log larger than this is not a job queue.
pub const MAX_FILE_BYTES: u64 = 64 * 1024;

fn valid_file_id(id: &str) -> bool {
    !id.is_empty()
        && id.len() <= crate::job::MAX_ID_LEN
        && id.bytes().all(|byte| {
            byte.is_ascii_lowercase() || byte.is_ascii_digit() || byte == b'-' || byte == b'_'
        })
}

fn missing(path: &std::path::Path) -> bool {
    matches!(
        std::fs::symlink_metadata(path),
        Err(error) if error.kind() == std::io::ErrorKind::NotFound
    )
}

/// Read a small control file after checking its size, so malformed input cannot
/// make the rehearsal allocate an unbounded buffer.
pub fn read_bounded(path: &std::path::Path, label: &str) -> Result<Vec<u8>, String> {
    if !std::fs::metadata(path)
        .map_err(|error| format!("{} cannot be read: {error}", path.display()))?
        .is_file()
    {
        return Err(format!("{} is not a file", path.display()));
    }
    let file = std::fs::File::open(path)
        .map_err(|error| format!("{} cannot be read: {error}", path.display()))?;
    if file
        .metadata()
        .map_err(|error| format!("{} cannot be read: {error}", path.display()))?
        .len()
        > MAX_FILE_BYTES
    {
        return Err(format!(
            "{label}s larger than {MAX_FILE_BYTES} bytes are refused"
        ));
    }
    let mut bytes = Vec::new();
    file.take(MAX_FILE_BYTES + 1)
        .read_to_end(&mut bytes)
        .map_err(|error| format!("{} cannot be read: {error}", path.display()))?;
    if bytes.len() as u64 > MAX_FILE_BYTES {
        return Err(format!(
            "{label}s larger than {MAX_FILE_BYTES} bytes are refused"
        ));
    }
    Ok(bytes)
}

/// One retailer's authorized ask: who may submit what, under which policy
/// revisions, as resolved at one moment. Cached display, not authority: the
/// server rechecks everything at finalization.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Assignment {
    pub schema: u32,
    pub id: String,
    pub workspace: String,
    pub supplier: String,
    pub products: Vec<AssignedProduct>,
    pub retrieved_at: u64,
}

/// One assigned product with the slots the supplier must fill.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct AssignedProduct {
    pub id: String,
    pub slots: Vec<AssignedSlot>,
}

/// One image slot with the requirement revisions that govern it.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct AssignedSlot {
    pub id: String,
    pub policy_revision: String,
    pub requirements: Vec<String>,
}

impl Assignment {
    /// Refuse an assignment that names no work, no one, or two slots alike:
    /// mapping matches roles to slot ids, so ambiguity there would guess.
    pub fn validate(&self) -> Result<(), String> {
        if self.schema != SCHEMA_VERSION {
            return Err(format!(
                "unsupported assignment schema {} (this Press reads schema {SCHEMA_VERSION})",
                self.schema
            ));
        }
        for (value, what) in [
            (&self.id, "assignment"),
            (&self.workspace, "workspace"),
            (&self.supplier, "supplier"),
        ] {
            if value.trim().is_empty() {
                return Err(format!("an assignment needs a {what}"));
            }
        }
        if self.products.is_empty() {
            return Err("an assignment with no products names no work".into());
        }
        let mut products = std::collections::HashSet::new();
        let mut slots = std::collections::HashSet::new();
        for product in &self.products {
            if product.id.trim().is_empty() {
                return Err("every assigned product needs an id".into());
            }
            if !products.insert(product.id.as_str()) {
                return Err(format!("duplicate product id {:?}", product.id));
            }
            for slot in &product.slots {
                if slot.id.trim().is_empty() || slot.policy_revision.trim().is_empty() {
                    return Err(format!(
                        "slot {:?} needs an id and a policy revision",
                        slot.id
                    ));
                }
                if !slots.insert((product.id.as_str(), slot.id.as_str())) {
                    return Err(format!("duplicate slot id {:?}", slot.id));
                }
            }
        }
        Ok(())
    }
}

/// Parse assignment bytes with the same size bound as every other import.
pub fn parse_assignment(bytes: &[u8]) -> Result<Assignment, String> {
    if bytes.len() as u64 > MAX_FILE_BYTES {
        return Err(format!(
            "assignments larger than {MAX_FILE_BYTES} bytes are refused"
        ));
    }
    let assignment: Assignment = serde_json::from_slice(bytes)
        .map_err(|error| format!("assignment does not parse: {error}"))?;
    assignment.validate()?;
    Ok(assignment)
}

/// Where one submitted file stands. Only ever set from a server answer or
/// from an explicit local decision (prepare, cancel); never guessed from a
/// transport event. An upload finishing is not an approval.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum AttemptState {
    /// Mapped and hashed locally, never sent.
    Prepared,
    /// Bytes accepted for upload, outcome not yet known.
    Transferred,
    /// The server took the submission attempt.
    Accepted,
    /// Accepted and waiting for human review.
    AwaitingReview,
    /// Review approved the asset.
    Approved,
    /// Review rejected the asset, with a reason in `receipt`.
    Rejected,
    /// Approved and published downstream.
    Delivered,
    /// Approved but downstream delivery failed.
    DeliveryFailed,
    /// Withdrawn before server acceptance.
    Cancelled,
}

impl AttemptState {
    /// Nothing further will change a terminal attempt; recovery restores the
    /// rest by name.
    pub fn terminal(self) -> bool {
        matches!(
            self,
            Self::Rejected | Self::Delivered | Self::DeliveryFailed | Self::Cancelled
        )
    }
}

/// One file's submission identity: which mapping, which exact bytes, which
/// slot and policy revision, and where the server says it stands.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Attempt {
    pub id: String,
    /// These fields are optional only for reading pre-rehearsal logs. Such
    /// legacy attempts are retained but refused before any new effect.
    #[serde(default)]
    pub assignment_id: String,
    #[serde(default)]
    pub workspace: String,
    #[serde(default)]
    pub supplier: String,
    #[serde(default)]
    pub product: String,
    pub mapping_id: String,
    pub source_hash: String,
    pub source_bytes: u64,
    pub slot: String,
    pub recipe_fingerprint: String,
    pub policy_revision: String,
    pub state: AttemptState,
    /// Server receipt or rejection reason, verbatim.
    pub receipt: Option<String>,
    /// The rejected attempt this one corrects, if any.
    pub correction_of: Option<String>,
}

impl Attempt {
    pub fn context_complete(&self) -> bool {
        !self.assignment_id.is_empty()
            && !self.workspace.is_empty()
            && !self.supplier.is_empty()
            && !self.product.is_empty()
    }
}

/// The durable queue for one job: every attempt plus the counter the next
/// id comes from, so restarts never reuse an identity.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct AttemptLog {
    pub schema: u32,
    pub job_id: String,
    pub client_job_id: String,
    pub next_attempt: u32,
    pub attempts: Vec<Attempt>,
}

impl AttemptLog {
    /// Open a queue for a job. The client job id pins this process's queue;
    /// changing accounts cannot redirect it, which S2 checks at the call site.
    pub fn open(job_id: &str) -> Self {
        let nanos = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .map(|age| age.as_nanos())
            .unwrap_or(0);
        Self {
            schema: SCHEMA_VERSION,
            job_id: job_id.into(),
            client_job_id: format!("cj-{}-{nanos}", std::process::id()),
            next_attempt: 1,
            attempts: Vec::new(),
        }
    }

    /// Items still needing work or answers, in queue order.
    pub fn pending(&self) -> Vec<&Attempt> {
        self.attempts
            .iter()
            .filter(|attempt| !attempt.state.terminal())
            .collect()
    }

    /// Prepare with the complete assignment context. The context is part of
    /// retry identity: an identical file for another product or policy is a
    /// different submission.
    #[allow(clippy::too_many_arguments)]
    pub fn prepare_for_context(
        &mut self,
        assignment_id: &str,
        workspace: &str,
        supplier: &str,
        product: &str,
        mapping_id: &str,
        source_hash: &str,
        source_bytes: u64,
        slot: &str,
        recipe_fingerprint: &str,
        policy_revision: &str,
    ) -> String {
        if let Some(known) = self.attempts.iter().find(|attempt| {
            attempt.assignment_id == assignment_id
                && attempt.workspace == workspace
                && attempt.supplier == supplier
                && attempt.product == product
                && attempt.mapping_id == mapping_id
                && attempt.source_hash == source_hash
                && attempt.source_bytes == source_bytes
                && attempt.slot == slot
                && attempt.recipe_fingerprint == recipe_fingerprint
                && attempt.policy_revision == policy_revision
                && !attempt.state.terminal()
        }) {
            return known.id.clone();
        }
        let id = format!("a{}", self.next_attempt);
        self.next_attempt += 1;
        self.attempts.push(Attempt {
            id: id.clone(),
            assignment_id: assignment_id.into(),
            workspace: workspace.into(),
            supplier: supplier.into(),
            product: product.into(),
            mapping_id: mapping_id.into(),
            source_hash: source_hash.into(),
            source_bytes,
            slot: slot.into(),
            recipe_fingerprint: recipe_fingerprint.into(),
            policy_revision: policy_revision.into(),
            state: AttemptState::Prepared,
            receipt: None,
            correction_of: None,
        });
        id
    }

    /// Existing queue entries must all belong to the same assignment. Empty
    /// context identifies an older format that cannot be safely redirected.
    pub fn validate_context(&self, assignment: &Assignment) -> Result<(), String> {
        for attempt in &self.attempts {
            if !attempt.context_complete() {
                return Err(format!(
                    "attempt {:?} predates supplier context; preserved, but cannot be reused",
                    attempt.id
                ));
            }
            if attempt.assignment_id != assignment.id
                || attempt.workspace != assignment.workspace
                || attempt.supplier != assignment.supplier
            {
                return Err(format!(
                    "attempt {:?} belongs to another assignment context; preserved, but cannot be redirected",
                    attempt.id
                ));
            }
        }
        Ok(())
    }

    /// Withdraw an unsent attempt. Server-accepted work is not un-sent by a
    /// client-side decision; cancel reports what actually happened instead.
    pub fn cancel(&mut self, attempt_id: &str) -> Result<(), String> {
        let Some(attempt) = self
            .attempts
            .iter_mut()
            .find(|attempt| attempt.id == attempt_id)
        else {
            return Err(format!("no attempt named {attempt_id:?} exists"));
        };
        if !attempt.context_complete() {
            return Err(format!(
                "attempt {attempt_id:?} predates supplier context; preserved, but cannot be changed"
            ));
        }
        match attempt.state {
            AttemptState::Prepared => {
                attempt.state = AttemptState::Cancelled;
                Ok(())
            }
            state => Err(format!(
                "attempt {attempt_id:?} is already {state:?}; reconcile it instead"
            )),
        }
    }

    /// Open a correction against a rejected attempt. The new attempt carries
    /// fresh bytes under a fresh identity, linked to what it corrects.
    pub fn correct(
        &mut self,
        rejected_id: &str,
        source_hash: &str,
        source_bytes: u64,
        recipe_fingerprint: &str,
    ) -> Result<String, String> {
        let Some(rejected) = self
            .attempts
            .iter()
            .find(|attempt| attempt.id == rejected_id)
        else {
            return Err(format!("no attempt named {rejected_id:?} exists"));
        };
        if rejected.state != AttemptState::Rejected {
            return Err(format!(
                "attempt {rejected_id:?} is not rejected; only rejections take corrections"
            ));
        }
        if !rejected.context_complete() {
            return Err(format!(
                "attempt {rejected_id:?} predates supplier context; preserved, but cannot be corrected"
            ));
        }
        let (mapping_id, slot, policy_revision) = (
            rejected.mapping_id.clone(),
            rejected.slot.clone(),
            rejected.policy_revision.clone(),
        );
        let id = format!("a{}", self.next_attempt);
        self.next_attempt += 1;
        self.attempts.push(Attempt {
            id: id.clone(),
            assignment_id: rejected.assignment_id.clone(),
            workspace: rejected.workspace.clone(),
            supplier: rejected.supplier.clone(),
            product: rejected.product.clone(),
            mapping_id,
            source_hash: source_hash.into(),
            source_bytes,
            slot,
            recipe_fingerprint: recipe_fingerprint.into(),
            policy_revision,
            state: AttemptState::Prepared,
            receipt: None,
            correction_of: Some(rejected_id.into()),
        });
        Ok(id)
    }
}

/// What the intake answered for one attempt.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct Receipt {
    pub attempt_id: String,
    pub receipt: String,
    pub state: AttemptState,
}

/// Why a submission did not proceed. Every variant names the actual problem:
/// a refusal never masquerades as a prompt to buy an unrelated plan.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum IntakeError {
    /// The assignment no longer authorizes this supplier.
    Revoked(String),
    /// The policy moved under a prepared attempt; refresh and revalidate.
    PolicyChanged { current: String },
    /// These exact bytes already have a receipt: reuse it, do not resend.
    Duplicate {
        receipt: String,
        state: AttemptState,
    },
    /// The bytes did not arrive; the attempt stays unconfirmed.
    Transport(String),
    /// Review refused the asset.
    Rejected { reason: String },
    /// The caller supplied bytes different from the prepared identity.
    PayloadMismatch {
        expected_hash: String,
        actual_hash: String,
        expected_bytes: u64,
        actual_bytes: u64,
    },
}

/// Canonical intake behind submissions. Implementations never invent server
/// state: receipts and errors repeat what the service answered.
pub trait Intake {
    fn submit(&mut self, attempt: &Attempt, bytes: &[u8]) -> Result<Receipt, IntakeError>;
    fn status(&mut self, attempt_id: &str) -> Result<Receipt, IntakeError>;
}

/// File holding one job's attempt log beside the job library.
pub fn attempt_log_path(dir: &std::path::Path, job_id: &str) -> Result<std::path::PathBuf, String> {
    if !valid_file_id(job_id) {
        return Err(format!("job id {job_id:?} is not safe for a filename"));
    }
    Ok(dir.join(format!("{job_id}.attempts.json")))
}

/// A process-wide queue lock. `File::try_lock` releases the lock on process
/// exit, while the lock file itself remains as a harmless stable inode.
pub struct MutationLock {
    _file: std::fs::File,
}

impl MutationLock {
    pub fn acquire(dir: &std::path::Path, job_id: &str) -> Result<Self, String> {
        if !valid_file_id(job_id) {
            return Err(format!("job id {job_id:?} is not safe for a filename"));
        }
        std::fs::create_dir_all(dir).map_err(|error| format!("attempt library failed: {error}"))?;
        let path = dir.join(format!(".{job_id}.supplier.lock"));
        let file = std::fs::OpenOptions::new()
            .create(true)
            .truncate(false)
            .read(true)
            .write(true)
            .open(&path)
            .map_err(|error| format!("supplier queue lock failed: {error}"))?;
        file.try_lock().map_err(|error| match error {
            std::fs::TryLockError::WouldBlock => {
                "another supplier command is using this job".into()
            }
            std::fs::TryLockError::Error(error) => format!("supplier queue lock failed: {error}"),
        })?;
        Ok(Self { _file: file })
    }
}

/// Persist the queue atomically through the shared commit: attempt IDs are
/// allocated before sending, so the log on disk must already name an attempt
/// its bytes may be in flight for.
pub fn save_log(dir: &std::path::Path, log: &AttemptLog) -> Result<(), String> {
    validate_log(log)?;
    static COUNTER: std::sync::atomic::AtomicU64 = std::sync::atomic::AtomicU64::new(0);
    let pretty = serde_json::to_string_pretty(log)
        .map_err(|error| format!("attempt log does not serialize: {error}"))?;
    if pretty.len() as u64 > MAX_FILE_BYTES {
        return Err("the attempt log outgrew its bound".into());
    }
    std::fs::create_dir_all(dir).map_err(|error| format!("attempt library failed: {error}"))?;
    let path = attempt_log_path(dir, &log.job_id)?;
    let tmp = path.with_extension(format!(
        "tmp-{}-{}",
        std::process::id(),
        COUNTER.fetch_add(1, std::sync::atomic::Ordering::Relaxed)
    ));
    std::fs::write(&tmp, pretty.as_bytes())
        .map_err(|error| format!("attempt write failed: {error}"))?;
    crate::settings::replace_file(&tmp, &path).map_err(|error| {
        let _ = std::fs::remove_file(&tmp);
        format!("attempt write failed: {error}")
    })
}

/// Reload a persisted queue: restart recovery restores named pending items,
/// and the saved counter keeps new attempts distinct from old ones.
pub fn load_log(dir: &std::path::Path, job_id: &str) -> Result<AttemptLog, String> {
    let path = attempt_log_path(dir, job_id)?;
    let bytes = read_bounded(&path, "attempt log").map_err(|error| {
        if missing(&path) {
            format!("no attempt log for job {job_id:?} exists")
        } else {
            error
        }
    })?;
    let log: AttemptLog = serde_json::from_slice(&bytes)
        .map_err(|error| format!("attempt log does not parse: {error}"))?;
    if log.schema != SCHEMA_VERSION {
        return Err(format!(
            "unsupported attempt schema {} (this Press reads schema {SCHEMA_VERSION})",
            log.schema
        ));
    }
    if log.job_id != job_id {
        return Err("the attempt log names a different job".into());
    }
    validate_log(&log)?;
    Ok(log)
}

fn validate_log(log: &AttemptLog) -> Result<(), String> {
    if log.schema != SCHEMA_VERSION {
        return Err(format!(
            "unsupported attempt schema {} (this Press reads schema {SCHEMA_VERSION})",
            log.schema
        ));
    }
    if !valid_file_id(&log.job_id) {
        return Err(format!(
            "job id {:?} is not safe for a filename",
            log.job_id
        ));
    }
    if log.next_attempt == 0 {
        return Err("the next attempt id must be positive".into());
    }
    let mut ids = std::collections::HashSet::new();
    let mut highest = 0u32;
    for attempt in &log.attempts {
        if !valid_file_id(&attempt.id) || !ids.insert(&attempt.id) {
            return Err(format!(
                "attempt id {:?} is invalid or duplicated",
                attempt.id
            ));
        }
        if let Some(number) = attempt.id.strip_prefix('a').and_then(|id| id.parse().ok()) {
            highest = highest.max(number);
        }
    }
    if log.next_attempt <= highest {
        return Err(format!(
            "next attempt {} would reuse existing attempt a{highest}",
            log.next_attempt
        ));
    }
    Ok(())
}

/// The saved job covering a folder, if the library holds one. Multiple jobs
/// for one root are an explicit choice the CLI cannot safely make for you.
pub fn open_job_for_checked(root: &std::path::Path) -> Result<Option<crate::job::Job>, String> {
    let Some(dir) = crate::job::dir() else {
        return Ok(None);
    };
    let (jobs, _) = crate::job::list(&dir);
    let matches: Vec<&crate::job::Job> = jobs
        .iter()
        .filter(|job| {
            job.source_roots.iter().any(|known| {
                known == root
                    || matches!(
                        (std::fs::canonicalize(known), std::fs::canonicalize(root)),
                        (Ok(left), Ok(right)) if left == right
                    )
            })
        })
        .collect();
    match matches.as_slice() {
        [] => Ok(None),
        [job] => Ok(Some((*job).clone())),
        _ => Err(format!(
            "multiple saved jobs cover {}; choose one before running supplier",
            root.display()
        )),
    }
}

/// What preparing one mapping produced: a sender identity, or why the
/// mapping sits this run out. Skips are information for the report, never
/// silent drops.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum PrepareOutcome {
    Ready(String),
    Skipped(String),
}

/// Bind a mapping to the slot owned by its product. The product id is required
/// because slot ids are scoped to products and may repeat across an assignment.
pub fn prepare_mapping_for_product(
    log: &mut AttemptLog,
    assignment: &Assignment,
    fingerprint: &str,
    product_id: &str,
    mapping: &crate::job::Mapping,
) -> PrepareOutcome {
    let Some(product) = assignment
        .products
        .iter()
        .find(|product| product.id == product_id)
    else {
        return PrepareOutcome::Skipped(format!(
            "mapping {:?} has no assigned product {:?}",
            mapping.id, product_id
        ));
    };
    let Some(slot) = product.slots.iter().find(|slot| slot.id == mapping.role_id) else {
        return PrepareOutcome::Skipped(format!(
            "mapping {:?} has no assignment slot {:?} for product {:?}",
            mapping.id, mapping.role_id, product_id
        ));
    };
    let metadata = match std::fs::metadata(&mapping.source.path) {
        Ok(metadata) if metadata.is_file() => metadata,
        _ => {
            return PrepareOutcome::Skipped(format!("mapping {:?} is gone from disk", mapping.id));
        }
    };
    let hash = match crate::manifest::hash_file(&mapping.source.path) {
        Ok(hash) => hash,
        Err(_) => {
            return PrepareOutcome::Skipped(format!("mapping {:?} cannot be read", mapping.id));
        }
    };
    PrepareOutcome::Ready(log.prepare_for_context(
        &assignment.id,
        &assignment.workspace,
        &assignment.supplier,
        product_id,
        &mapping.id,
        &hash,
        metadata.len(),
        &slot.id,
        fingerprint,
        &slot.policy_revision,
    ))
}

pub fn submit_attempt<I: Intake>(
    dir: &std::path::Path,
    log: &mut AttemptLog,
    intake: &mut I,
    attempt_id: &str,
    bytes: &[u8],
) -> Result<Receipt, IntakeError> {
    let Some(index) = log
        .attempts
        .iter()
        .position(|attempt| attempt.id == attempt_id)
    else {
        return Err(IntakeError::Transport(format!(
            "no attempt named {attempt_id:?} exists"
        )));
    };
    if log.attempts[index].state.terminal() {
        return Err(IntakeError::Transport(format!(
            "attempt {attempt_id:?} is already finished"
        )));
    }
    if log.attempts[index].state != AttemptState::Prepared {
        return Err(IntakeError::Transport(format!(
            "attempt {attempt_id:?} is {:?}; reconcile before retrying",
            log.attempts[index].state
        )));
    }
    let attempt = log.attempts[index].clone();
    if !attempt.context_complete() {
        return Err(IntakeError::Transport(format!(
            "attempt {attempt_id:?} predates supplier context; preserved, but cannot be reused"
        )));
    }
    let actual_hash = hash_bytes(bytes);
    let actual_bytes = bytes.len() as u64;
    if attempt.source_bytes != actual_bytes || attempt.source_hash != actual_hash {
        return Err(IntakeError::PayloadMismatch {
            expected_hash: attempt.source_hash,
            actual_hash,
            expected_bytes: attempt.source_bytes,
            actual_bytes,
        });
    }
    // A response can be lost after the intake accepts these bytes. Mark the
    // operation in flight and durably save it before crossing that boundary.
    log.attempts[index].state = AttemptState::Transferred;
    save_log(dir, log).map_err(IntakeError::Transport)?;
    match intake.submit(&attempt, bytes) {
        Ok(receipt) => {
            apply_receipt(log, &receipt).map_err(IntakeError::Transport)?;
            save_log(dir, log).map_err(IntakeError::Transport)?;
            Ok(receipt)
        }
        Err(IntakeError::Duplicate { receipt, state }) => {
            // The bytes already have a server identity: adopt it instead of
            // minting a second submission for the same content.
            let receipt = Receipt {
                attempt_id: attempt_id.into(),
                receipt,
                state,
            };
            apply_receipt(log, &receipt).map_err(IntakeError::Transport)?;
            save_log(dir, log).map_err(IntakeError::Transport)?;
            let attempt = &log.attempts[index];
            Ok(Receipt {
                attempt_id: attempt.id.clone(),
                receipt: attempt.receipt.clone().unwrap_or(receipt.receipt),
                state: attempt.state,
            })
        }
        Err(
            error @ (IntakeError::Revoked(_)
            | IntakeError::PolicyChanged { .. }
            | IntakeError::Rejected { .. }),
        ) => {
            // These answers are definitive refusals, so the prepared item can
            // be retried after the assignment or bytes change. Transport
            // errors stay Transferred because their server side is unknown.
            log.attempts[index].state = AttemptState::Prepared;
            save_log(dir, log).map_err(IntakeError::Transport)?;
            Err(error)
        }
        Err(error) => Err(error),
    }
}

fn hash_bytes(bytes: &[u8]) -> String {
    use sha2::Digest;
    format!("{:x}", sha2::Sha256::digest(bytes))
}

/// Look up every unconfirmed attempt after an interruption: an accepted
/// attempt adopts the server's answer, and anything still unknown stays
/// unconfirmed for a later lookup instead of being resent blindly.
pub fn reconcile<I: Intake>(
    dir: &std::path::Path,
    log: &mut AttemptLog,
    intake: &mut I,
) -> Result<Vec<String>, String> {
    let ongoing: Vec<String> = log
        .attempts
        .iter()
        .filter(|attempt| {
            matches!(
                attempt.state,
                AttemptState::Transferred
                    | AttemptState::Accepted
                    | AttemptState::AwaitingReview
                    | AttemptState::Approved
            )
        })
        .map(|attempt| attempt.id.clone())
        .collect();
    let mut failures = Vec::new();
    for attempt_id in ongoing {
        match intake.status(&attempt_id) {
            Ok(receipt) => {
                if let Err(error) = apply_receipt(log, &receipt) {
                    failures.push(format!("{attempt_id}: {error}"));
                }
            }
            Err(error) => failures.push(format!("{attempt_id}: {error:?}")),
        }
    }
    save_log(dir, log)?;
    Ok(failures)
}

fn apply_receipt(log: &mut AttemptLog, receipt: &Receipt) -> Result<(), String> {
    if let Some(attempt) = log
        .attempts
        .iter_mut()
        .find(|attempt| attempt.id == receipt.attempt_id)
    {
        if attempt.state.terminal() {
            return Ok(());
        }
        // A receipt may refresh the details for the same state or advance it;
        // equal-rank states such as Approved and Rejected are not interchangeable.
        if receipt.state == attempt.state || state_rank(receipt.state) > state_rank(attempt.state) {
            attempt.state = receipt.state;
            attempt.receipt = Some(receipt.receipt.clone());
        }
        Ok(())
    } else {
        Err(format!(
            "receipt names unknown attempt {:?}",
            receipt.attempt_id
        ))
    }
}

fn state_rank(state: AttemptState) -> u8 {
    match state {
        AttemptState::Prepared => 0,
        AttemptState::Transferred => 1,
        AttemptState::Accepted => 2,
        AttemptState::AwaitingReview => 3,
        AttemptState::Approved | AttemptState::Rejected => 4,
        AttemptState::Delivered | AttemptState::DeliveryFailed => 5,
        AttemptState::Cancelled => 6,
    }
}

/// A scripted intake for drills and tests: each attempt id maps to a queue
/// of answers, and accepted attempts stay answerable so reconciliation and
/// duplicate retries meet the same server they left.
#[derive(Default)]
pub struct FakeIntake {
    answers: std::collections::HashMap<String, Vec<FakeAnswer>>,
    accepted: std::collections::HashMap<String, Receipt>,
    journal: Option<std::path::PathBuf>,
}

/// One scripted answer: the next submit or status call for an attempt.
/// Serializable so rehearsal scripts live in files, not in test code.
#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum FakeAnswer {
    Accept {
        receipt: String,
    },
    Advance {
        receipt: String,
        state: AttemptState,
    },
    Revoked,
    PolicyChanged {
        current: String,
    },
    Rejected {
        reason: String,
    },
    Transport,
}

impl FakeIntake {
    /// Script answers programmatically. Tests use this; rehearsal files use
    /// [`FakeIntake::rehearsing`] instead.
    #[cfg(test)]
    pub fn script(&mut self, attempt_id: &str, answers: Vec<FakeAnswer>) {
        self.answers.insert(attempt_id.into(), answers);
    }

    fn next(&mut self, attempt_id: &str) -> Option<FakeAnswer> {
        self.answers.get_mut(attempt_id).and_then(|queue| {
            if queue.len() > 1 {
                Some(queue.remove(0))
            } else {
                queue.first().cloned()
            }
        })
    }
}

/// A rehearsal script: scripted answers per attempt id, loaded from a file
/// the pilot author can read. Explicitly a rehearsal tool, never a server.
#[derive(Clone, Debug, Default, Serialize, Deserialize)]
pub struct FakeScript {
    #[serde(default)]
    pub attempts: std::collections::HashMap<String, Vec<FakeAnswer>>,
}

/// Parse rehearsal bytes with the same size bound as every other import.
pub fn parse_script(bytes: &[u8]) -> Result<FakeScript, String> {
    if bytes.len() as u64 > MAX_FILE_BYTES {
        return Err(format!(
            "rehearsal scripts larger than {MAX_FILE_BYTES} bytes are refused"
        ));
    }
    serde_json::from_slice(bytes)
        .map_err(|error| format!("rehearsal script does not parse: {error}"))
}

impl FakeIntake {
    /// An intake rehearsing a script file. Every answer is declared up
    /// front; anything unscripted fails transport like an unknown server.
    pub fn rehearsing(script: &FakeScript) -> Self {
        Self {
            answers: script.attempts.clone(),
            accepted: std::collections::HashMap::new(),
            journal: None,
        }
    }

    /// Rehearse with the accepted receipts journaled beside the script, so a
    /// later process reconciles what this one sent. Deleting the journal
    /// replays the script from a server that remembers nothing.
    pub fn rehearsing_journaled(
        script: &FakeScript,
        journal: &std::path::Path,
    ) -> Result<Self, String> {
        let mut intake = Self::rehearsing(script);
        intake.journal = Some(journal.to_path_buf());
        match read_bounded(journal, "rehearsal journal") {
            Ok(bytes) => {
                intake.accepted = serde_json::from_slice(&bytes)
                    .map_err(|error| format!("rehearsal journal does not parse: {error}"))?;
            }
            Err(_error) if missing(journal) => {}
            Err(error) => return Err(error),
        }
        Ok(intake)
    }

    /// Persist accepted receipts for the next process through the same atomic
    /// replacement used by the other local ledgers. A failed journal save is a
    /// failed command, never a reason to forget what the fake accepted.
    pub fn save_journal(&self) -> Result<(), String> {
        let Some(path) = &self.journal else {
            return Ok(());
        };
        static COUNTER: std::sync::atomic::AtomicU64 = std::sync::atomic::AtomicU64::new(0);
        let bytes = serde_json::to_vec_pretty(&self.accepted)
            .map_err(|error| format!("rehearsal journal does not serialize: {error}"))?;
        if bytes.len() as u64 > MAX_FILE_BYTES {
            return Err("the rehearsal journal outgrew its bound".into());
        }
        if let Some(parent) = path.parent() {
            std::fs::create_dir_all(parent)
                .map_err(|error| format!("rehearsal journal directory failed: {error}"))?;
        }
        let tmp = path.with_extension(format!(
            "tmp-{}-{}",
            std::process::id(),
            COUNTER.fetch_add(1, std::sync::atomic::Ordering::Relaxed)
        ));
        std::fs::write(&tmp, bytes)
            .map_err(|error| format!("rehearsal journal write failed: {error}"))?;
        crate::settings::replace_file(&tmp, path).map_err(|error| {
            let _ = std::fs::remove_file(&tmp);
            format!("rehearsal journal write failed: {error}")
        })
    }
}

impl Intake for FakeIntake {
    fn submit(&mut self, attempt: &Attempt, _bytes: &[u8]) -> Result<Receipt, IntakeError> {
        if let Some(known) = self.accepted.get(&attempt.id) {
            return Err(IntakeError::Duplicate {
                receipt: known.receipt.clone(),
                state: known.state,
            });
        }
        match self.next(&attempt.id) {
            Some(FakeAnswer::Accept { receipt }) | Some(FakeAnswer::Advance { receipt, .. }) => {
                let receipt = Receipt {
                    attempt_id: attempt.id.clone(),
                    receipt,
                    state: AttemptState::Transferred,
                };
                self.accepted.insert(attempt.id.clone(), receipt.clone());
                self.save_journal().map_err(IntakeError::Transport)?;
                Ok(receipt)
            }
            Some(FakeAnswer::Revoked) => Err(IntakeError::Revoked(format!(
                "assignment no longer authorizes {}",
                attempt.slot
            ))),
            Some(FakeAnswer::PolicyChanged { current }) => {
                Err(IntakeError::PolicyChanged { current })
            }
            Some(FakeAnswer::Rejected { reason }) => Err(IntakeError::Rejected { reason }),
            Some(FakeAnswer::Transport) | None => {
                Err(IntakeError::Transport("the bytes did not arrive".into()))
            }
        }
    }

    fn status(&mut self, attempt_id: &str) -> Result<Receipt, IntakeError> {
        let Some(known) = self.accepted.get(attempt_id).cloned() else {
            return Err(IntakeError::Transport(format!(
                "the server holds no attempt named {attempt_id:?}"
            )));
        };
        match self.next(attempt_id) {
            Some(FakeAnswer::Advance { receipt, state }) => {
                let receipt = Receipt {
                    attempt_id: attempt_id.into(),
                    receipt,
                    state,
                };
                self.accepted.insert(attempt_id.into(), receipt.clone());
                self.save_journal().map_err(IntakeError::Transport)?;
                Ok(receipt)
            }
            _ => Ok(known),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn store(tag: &str) -> std::path::PathBuf {
        let dir = std::env::temp_dir().join(format!("press-supplier-{tag}-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).expect("the fixture dir is created");
        dir
    }

    fn queue() -> AttemptLog {
        AttemptLog::open("job-1")
    }

    fn prepared(log: &mut AttemptLog) -> String {
        let bytes = prepared_bytes();
        log.prepare_for_context(
            "assign-1",
            "retailer",
            "studio-9",
            "hero",
            "m1",
            &hash_bytes(bytes),
            bytes.len() as u64,
            "main",
            "fp-1",
            "policy-7",
        )
    }

    fn prepared_bytes() -> &'static [u8] {
        b"0123456789abcdef"
    }

    #[test]
    fn identical_bytes_reuse_the_sender_identity() {
        let mut log = queue();
        let first = prepared(&mut log);
        assert_eq!(prepared(&mut log), first, "a retry resends, not re-mints");
        let policy_change = log.prepare_for_context(
            "assign-1",
            "retailer",
            "studio-9",
            "hero",
            "m1",
            &hash_bytes(prepared_bytes()),
            prepared_bytes().len() as u64,
            "main",
            "fp-1",
            "policy-8",
        );
        assert_ne!(
            policy_change, first,
            "a policy change creates a new attempt"
        );
        // Changed bytes mint a distinct attempt for a distinct submission.
        let second = log.prepare_for_context(
            "assign-1", "retailer", "studio-9", "hero", "m1", "hash-2", 18, "main", "fp-1",
            "policy-7",
        );
        assert_ne!(second, first);
        assert_eq!(log.attempts.len(), 3);
    }

    #[test]
    fn an_existing_attempt_cannot_be_redirected_to_another_assignment() {
        let mut log = queue();
        prepared(&mut log);
        let mut changed = assignment();
        changed.id = "assign-2".into();
        assert!(log.validate_context(&changed).is_err());
    }

    #[test]
    fn a_happy_path_reaches_a_receipt() {
        let dir = store("happy");
        let mut log = queue();
        let id = prepared(&mut log);
        let mut intake = FakeIntake::default();
        intake.script(
            &id,
            vec![FakeAnswer::Accept {
                receipt: "r-1".into(),
            }],
        );
        let receipt = submit_attempt(&dir, &mut log, &mut intake, &id, prepared_bytes()).unwrap();
        assert_eq!(receipt.state, AttemptState::Transferred);
        assert_eq!(log.attempts[0].state, AttemptState::Transferred);
        // Restart recovery restores the named attempt with its counter.
        let reloaded = load_log(&dir, "job-1").unwrap();
        assert_eq!(reloaded.attempts[0].id, id);
        assert_eq!(reloaded.next_attempt, log.next_attempt);
        assert_eq!(reloaded.pending().len(), 1);
        std::fs::remove_dir_all(&dir).unwrap();
    }

    #[test]
    fn a_revoked_assignment_keeps_its_name_and_its_bytes() {
        let dir = store("revoked");
        let mut log = queue();
        let id = prepared(&mut log);
        let mut intake = FakeIntake::default();
        intake.script(&id, vec![FakeAnswer::Revoked]);
        let refused =
            submit_attempt(&dir, &mut log, &mut intake, &id, prepared_bytes()).unwrap_err();
        assert!(matches!(refused, IntakeError::Revoked(_)));
        assert_eq!(log.attempts[0].state, AttemptState::Prepared);
        std::fs::remove_dir_all(&dir).unwrap();
    }

    #[test]
    fn a_lost_response_reconciles_instead_of_resending() {
        let dir = store("reconcile");
        let mut log = queue();
        let id = prepared(&mut log);
        let mut intake = FakeIntake::default();
        intake.script(
            &id,
            vec![
                FakeAnswer::Accept {
                    receipt: "r-1".into(),
                },
                FakeAnswer::Advance {
                    receipt: "r-1".into(),
                    state: AttemptState::Accepted,
                },
                FakeAnswer::Advance {
                    receipt: "r-1".into(),
                    state: AttemptState::AwaitingReview,
                },
            ],
        );
        submit_attempt(&dir, &mut log, &mut intake, &id, prepared_bytes()).unwrap();
        // The reply never arrived, but the server took it: lookup adopts the
        // accepted answer instead of minting a duplicate submission.
        reconcile(&dir, &mut log, &mut intake).unwrap();
        assert_eq!(log.attempts[0].state, AttemptState::Accepted);
        assert_eq!(log.attempts[0].receipt.as_deref(), Some("r-1"));
        reconcile(&dir, &mut log, &mut intake).unwrap();
        assert_eq!(log.attempts[0].state, AttemptState::AwaitingReview);
        // An ambiguous retry must reconcile first, not create another job.
        let again = submit_attempt(&dir, &mut log, &mut intake, &id, prepared_bytes()).unwrap_err();
        assert!(matches!(again, IntakeError::Transport(reason) if reason.contains("reconcile")));
        std::fs::remove_dir_all(&dir).unwrap();
    }

    #[test]
    fn partial_batches_and_restarts_keep_successful_siblings() {
        let dir = store("partial");
        let mut log = queue();
        let first = log.prepare_for_context(
            "assign-1",
            "retailer",
            "studio-9",
            "hero",
            "m1",
            &hash_bytes(b"one"),
            3,
            "main",
            "fp-1",
            "policy-7",
        );
        let second = log.prepare_for_context(
            "assign-1",
            "retailer",
            "studio-9",
            "hero",
            "m2",
            &hash_bytes(b"two"),
            3,
            "detail",
            "fp-1",
            "policy-7",
        );
        let mut intake = FakeIntake::default();
        intake.script(
            &first,
            vec![FakeAnswer::Accept {
                receipt: "r-1".into(),
            }],
        );
        intake.script(&second, vec![FakeAnswer::Transport]);
        assert!(submit_attempt(&dir, &mut log, &mut intake, &first, b"one").is_ok());
        assert!(submit_attempt(&dir, &mut log, &mut intake, &second, b"two").is_err());
        // Only the failed sibling needs action after a restart.
        let reloaded = load_log(&dir, "job-1").unwrap();
        let pending: Vec<&str> = reloaded
            .pending()
            .iter()
            .map(|attempt| attempt.id.as_str())
            .collect();
        assert!(pending.contains(&first.as_str()));
        assert!(pending.contains(&second.as_str()));
        assert_eq!(reloaded.attempts.len(), 2, "no duplicate was minted");
        std::fs::remove_dir_all(&dir).unwrap();
    }

    #[test]
    fn cancellation_stops_before_acceptance_not_after() {
        let mut log = queue();
        let id = prepared(&mut log);
        log.cancel(&id).unwrap();
        assert_eq!(log.attempts[0].state, AttemptState::Cancelled);
        assert!(log.cancel(&id).is_err(), "withdrawing twice fails");
        assert!(log.cancel("nope").is_err());
        let id = log.prepare_for_context(
            "assign-1",
            "retailer",
            "studio-9",
            "hero",
            "m2",
            &hash_bytes(b"two"),
            3,
            "main",
            "fp-1",
            "policy-7",
        );
        log.attempts
            .iter_mut()
            .find(|attempt| attempt.id == id)
            .unwrap()
            .state = AttemptState::Transferred;
        assert!(
            log.cancel(&id).is_err(),
            "in-flight work cannot be cancelled locally"
        );
    }

    #[test]
    fn a_payload_change_is_refused_before_the_intake_is_called() {
        let dir = store("payload");
        let mut log = queue();
        let id = prepared(&mut log);
        let mut intake = FakeIntake::default();
        intake.script(
            &id,
            vec![FakeAnswer::Accept {
                receipt: "r-1".into(),
            }],
        );
        let refused = submit_attempt(&dir, &mut log, &mut intake, &id, b"changed").unwrap_err();
        assert!(matches!(refused, IntakeError::PayloadMismatch { .. }));
        assert_eq!(log.attempts[0].state, AttemptState::Prepared);
        std::fs::remove_dir_all(&dir).unwrap();
    }

    #[test]
    fn approved_attempts_reconcile_until_delivery_is_known() {
        let dir = store("approved");
        let mut log = queue();
        let id = prepared(&mut log);
        log.attempts[0].state = AttemptState::Approved;
        log.attempts[0].receipt = Some("approved".into());
        let mut intake = FakeIntake::default();
        intake.accepted.insert(
            id.clone(),
            Receipt {
                attempt_id: id.clone(),
                receipt: "approved".into(),
                state: AttemptState::Approved,
            },
        );
        intake.script(
            &id,
            vec![FakeAnswer::Advance {
                receipt: "delivered".into(),
                state: AttemptState::Delivered,
            }],
        );
        reconcile(&dir, &mut log, &mut intake).unwrap();
        assert_eq!(log.attempts[0].state, AttemptState::Delivered);
        assert_eq!(log.attempts[0].receipt.as_deref(), Some("delivered"));
        std::fs::remove_dir_all(&dir).unwrap();
    }

    #[test]
    fn duplicate_retry_keeps_the_newest_canonical_state() {
        let dir = store("duplicate-state");
        let mut log = queue();
        let id = prepared(&mut log);
        let mut intake = FakeIntake::default();
        intake.accepted.insert(
            id.clone(),
            Receipt {
                attempt_id: id.clone(),
                receipt: "accepted".into(),
                state: AttemptState::Accepted,
            },
        );
        let receipt = submit_attempt(&dir, &mut log, &mut intake, &id, prepared_bytes()).unwrap();
        assert_eq!(receipt.state, AttemptState::Accepted);
        assert_eq!(receipt.receipt, "accepted");
        std::fs::remove_dir_all(&dir).unwrap();
    }

    #[test]
    fn rejection_links_its_correction() {
        let mut log = queue();
        let id = prepared(&mut log);
        assert!(log.correct(&id, "hash-2", 18, "fp-1").is_err());
        log.attempts[0].state = AttemptState::Rejected;
        log.attempts[0].receipt = Some("too dark".into());
        let correction = log.correct(&id, "hash-2", 18, "fp-1").unwrap();
        assert_ne!(correction, id);
        let fixed = log
            .attempts
            .iter()
            .find(|attempt| attempt.id == correction)
            .unwrap();
        assert_eq!(fixed.correction_of.as_deref(), Some(id.as_str()));
        assert_eq!(fixed.state, AttemptState::Prepared);
    }

    #[test]
    fn a_changed_policy_refuses_until_requirements_refresh() {
        let dir = store("policy");
        let mut log = queue();
        let id = prepared(&mut log);
        let mut intake = FakeIntake::default();
        intake.script(
            &id,
            vec![FakeAnswer::PolicyChanged {
                current: "policy-8".into(),
            }],
        );
        let refused =
            submit_attempt(&dir, &mut log, &mut intake, &id, prepared_bytes()).unwrap_err();
        assert!(matches!(refused, IntakeError::PolicyChanged { .. }));
        assert_eq!(log.attempts[0].state, AttemptState::Prepared);
        std::fs::remove_dir_all(&dir).unwrap();
    }

    fn assignment() -> Assignment {
        Assignment {
            schema: SCHEMA_VERSION,
            id: "assign-1".into(),
            workspace: "retailer".into(),
            supplier: "studio-9".into(),
            products: vec![AssignedProduct {
                id: "hero".into(),
                slots: vec![AssignedSlot {
                    id: "main".into(),
                    policy_revision: "policy-7".into(),
                    requirements: vec!["1600px".into()],
                }],
            }],
            retrieved_at: 1,
        }
    }

    #[test]
    fn assignments_parse_strictly_and_validate() {
        assignment().validate().unwrap();
        let bytes = serde_json::to_vec(&assignment()).unwrap();
        assert_eq!(parse_assignment(&bytes).unwrap(), assignment());
        let mut future = assignment();
        future.schema = SCHEMA_VERSION + 1;
        assert!(parse_assignment(&serde_json::to_vec(&future).unwrap()).is_err());
        let mut anonymous = assignment();
        anonymous.supplier.clear();
        assert!(anonymous.validate().is_err());
        let mut empty = assignment();
        empty.products.clear();
        assert!(empty.validate().is_err());
        let mut cloned = assignment();
        cloned.products.push(AssignedProduct {
            id: "second".into(),
            slots: vec![AssignedSlot {
                id: "main".into(),
                policy_revision: "policy-7".into(),
                requirements: Vec::new(),
            }],
        });
        cloned.validate().unwrap();
        cloned.products[1].id = "hero".into();
        assert!(cloned.validate().is_err(), "product ids stay unique");
        assert!(parse_assignment(&vec![7u8; MAX_FILE_BYTES as usize + 1]).is_err());
    }

    #[test]
    fn same_slot_name_is_scoped_to_its_product() {
        let mut assignment = assignment();
        assignment.products.push(AssignedProduct {
            id: "detail-product".into(),
            slots: vec![AssignedSlot {
                id: "main".into(),
                policy_revision: "policy-9".into(),
                requirements: Vec::new(),
            }],
        });
        assignment.validate().unwrap();
        let dir = store("product-slot");
        let file = dir.join("detail.png");
        std::fs::write(&file, b"detail").unwrap();
        let mapping = crate::job::Mapping {
            id: "m-detail".into(),
            role_id: "main".into(),
            source: crate::job::SourceRef {
                path: file,
                bytes: 6,
                modified: None,
            },
        };
        let mut log = queue();
        let PrepareOutcome::Ready(id) =
            prepare_mapping_for_product(&mut log, &assignment, "fp-1", "detail-product", &mapping)
        else {
            panic!("the product-owned slot prepares");
        };
        assert_eq!(log.attempts[0].id, id);
        assert_eq!(log.attempts[0].product, "detail-product");
        assert_eq!(log.attempts[0].policy_revision, "policy-9");
        std::fs::remove_dir_all(&dir).unwrap();
    }

    #[test]
    fn damaged_ledgers_are_reported_without_erasing_the_last_bytes() {
        let dir = store("damaged");
        let path = attempt_log_path(&dir, "job-1").unwrap();
        let damaged = br"{broken";
        std::fs::write(&path, damaged).unwrap();
        assert!(load_log(&dir, "job-1").is_err());
        assert_eq!(std::fs::read(&path).unwrap(), damaged);

        let journal = dir.join("rehearsal.json");
        std::fs::write(&journal, damaged).unwrap();
        let script = FakeScript::default();
        assert!(FakeIntake::rehearsing_journaled(&script, &journal).is_err());
        assert_eq!(std::fs::read(&journal).unwrap(), damaged);
        std::fs::remove_dir_all(&dir).unwrap();
    }

    #[test]
    fn malformed_counters_and_duplicate_attempts_are_refused() {
        let dir = store("counter");
        let mut log = queue();
        let id = prepared(&mut log);
        let duplicate = log.attempts[0].clone();
        log.attempts.push(duplicate);
        assert!(save_log(&dir, &log).is_err());
        log.attempts.truncate(1);
        log.next_attempt = id[1..].parse::<u32>().unwrap();
        assert!(save_log(&dir, &log).is_err());
        std::fs::remove_dir_all(&dir).unwrap();
    }

    #[test]
    fn rehearsal_scripts_parse_and_drive_the_fake() {
        let script: FakeScript = serde_json::from_value(serde_json::json!({
            "attempts": {"a1": [{"accept": {"receipt": "r-1"}}]}
        }))
        .unwrap();
        let mut intake = FakeIntake::rehearsing(&script);
        let attempt = Attempt {
            id: "a1".into(),
            assignment_id: "assign-1".into(),
            workspace: "retailer".into(),
            supplier: "studio-9".into(),
            product: "hero".into(),
            mapping_id: "m1".into(),
            source_hash: "h".into(),
            source_bytes: 1,
            slot: "main".into(),
            recipe_fingerprint: "fp".into(),
            policy_revision: "policy-7".into(),
            state: AttemptState::Prepared,
            receipt: None,
            correction_of: None,
        };
        let receipt = intake.submit(&attempt, b"x").unwrap();
        assert_eq!(receipt.state, AttemptState::Transferred);
        assert!(parse_script(b"{\"attempts\":}").is_err());
    }

    #[test]
    fn preparing_binds_mappings_to_slots_or_names_why_not() {
        let dir = store("prepare");
        let file = dir.join("hero.png");
        std::fs::write(&file, b"sixteen-bytes!!").unwrap();
        let mapping = crate::job::Mapping {
            id: "m1".into(),
            role_id: "main".into(),
            source: crate::job::SourceRef {
                path: file,
                bytes: 16,
                modified: None,
            },
        };
        let assignment = assignment();
        let mut log = queue();
        let PrepareOutcome::Ready(id) =
            prepare_mapping_for_product(&mut log, &assignment, "fp-1", "hero", &mapping)
        else {
            panic!("the mapped file prepares");
        };
        // Same bytes again reuses the identity.
        let PrepareOutcome::Ready(same) =
            prepare_mapping_for_product(&mut log, &assignment, "fp-1", "hero", &mapping)
        else {
            panic!("identical bytes reuse the attempt");
        };
        assert_eq!(same, id);
        let stray = crate::job::Mapping {
            id: "m9".into(),
            role_id: "lifestyle".into(),
            source: mapping.source.clone(),
        };
        assert!(matches!(
            prepare_mapping_for_product(&mut log, &assignment, "fp-1", "hero", &stray),
            PrepareOutcome::Skipped(_)
        ));
        let gone = crate::job::Mapping {
            id: "m8".into(),
            role_id: "main".into(),
            source: crate::job::SourceRef {
                path: dir.join("gone.png"),
                bytes: 0,
                modified: None,
            },
        };
        assert!(matches!(
            prepare_mapping_for_product(&mut log, &assignment, "fp-1", "hero", &gone),
            PrepareOutcome::Skipped(_)
        ));
        std::fs::remove_dir_all(&dir).unwrap();
    }
}
