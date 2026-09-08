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

/// The only assignment and log schemas this Press reads.
pub const SCHEMA_VERSION: u32 = 1;

/// An attempt log larger than this is not a job queue.
pub const MAX_FILE_BYTES: u64 = 64 * 1024;

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
        let mut slots = std::collections::HashSet::new();
        for product in &self.products {
            if product.id.trim().is_empty() {
                return Err("every assigned product needs an id".into());
            }
            for slot in &product.slots {
                if slot.id.trim().is_empty() || slot.policy_revision.trim().is_empty() {
                    return Err(format!(
                        "slot {:?} needs an id and a policy revision",
                        slot.id
                    ));
                }
                if !slots.insert(slot.id.as_str()) {
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
            Self::Approved
                | Self::Rejected
                | Self::Delivered
                | Self::DeliveryFailed
                | Self::Cancelled
        )
    }
}

/// One file's submission identity: which mapping, which exact bytes, which
/// slot and policy revision, and where the server says it stands.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Attempt {
    pub id: String,
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

    /// Prepare one file for one slot. Retrying identical bytes reuses the
    /// sender's identity; changed content, target or recipe mints a distinct
    /// attempt so the server never confuses the two submissions.
    pub fn prepare(
        &mut self,
        mapping_id: &str,
        source_hash: &str,
        source_bytes: u64,
        slot: &str,
        recipe_fingerprint: &str,
        policy_revision: &str,
    ) -> String {
        if let Some(known) = self.attempts.iter().find(|attempt| {
            attempt.mapping_id == mapping_id
                && attempt.source_hash == source_hash
                && attempt.slot == slot
                && attempt.recipe_fingerprint == recipe_fingerprint
                && !attempt.state.terminal()
        }) {
            return known.id.clone();
        }
        let id = format!("a{}", self.next_attempt);
        self.next_attempt += 1;
        self.attempts.push(Attempt {
            id: id.clone(),
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
        match attempt.state {
            AttemptState::Prepared | AttemptState::Transferred => {
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
        let (mapping_id, slot, policy_revision) = (
            rejected.mapping_id.clone(),
            rejected.slot.clone(),
            rejected.policy_revision.clone(),
        );
        let id = format!("a{}", self.next_attempt);
        self.next_attempt += 1;
        self.attempts.push(Attempt {
            id: id.clone(),
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
    Duplicate { receipt: String },
    /// The bytes did not arrive; the attempt stays unconfirmed.
    Transport(String),
    /// Review refused the asset.
    Rejected { reason: String },
}

/// Canonical intake behind submissions. Implementations never invent server
/// state: receipts and errors repeat what the service answered.
pub trait Intake {
    fn submit(&mut self, attempt: &Attempt, bytes: &[u8]) -> Result<Receipt, IntakeError>;
    fn status(&mut self, attempt_id: &str) -> Result<Receipt, IntakeError>;
}

/// File holding one job's attempt log beside the job library.
fn file_for(dir: &std::path::Path, job_id: &str) -> std::path::PathBuf {
    dir.join(format!("{job_id}.attempts.json"))
}

/// Persist the queue atomically through the shared commit: attempt IDs are
/// allocated before sending, so the log on disk must already name an attempt
/// its bytes may be in flight for.
pub fn save_log(dir: &std::path::Path, log: &AttemptLog) -> Result<(), String> {
    static COUNTER: std::sync::atomic::AtomicU64 = std::sync::atomic::AtomicU64::new(0);
    let pretty = serde_json::to_string_pretty(log)
        .map_err(|error| format!("attempt log does not serialize: {error}"))?;
    if pretty.len() as u64 > MAX_FILE_BYTES {
        return Err("the attempt log outgrew its bound".into());
    }
    std::fs::create_dir_all(dir).map_err(|error| format!("attempt library failed: {error}"))?;
    let path = file_for(dir, &log.job_id);
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
    let bytes = std::fs::read(file_for(dir, job_id))
        .map_err(|_| format!("no attempt log for job {job_id:?} exists"))?;
    if bytes.len() as u64 > MAX_FILE_BYTES {
        return Err("the attempt log outgrew its bound".into());
    }
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
    Ok(log)
}

/// The saved job covering a folder, if the library holds one. Exact roots
/// win over canonical-only matches, mirroring the window's loader tiers.
pub fn open_job_for(root: &std::path::Path) -> Option<crate::job::Job> {
    let dir = crate::job::dir()?;
    let (jobs, _) = crate::job::list(&dir);
    jobs.iter()
        .find(|job| job.source_roots.iter().any(|known| known == root))
        .or_else(|| {
            jobs.iter().find(|job| {
                job.source_roots.iter().any(|known| {
                    known == root
                        || matches!(
                            (std::fs::canonicalize(known), std::fs::canonicalize(root)),
                            (Ok(left), Ok(right)) if left == right
                        )
                })
            })
        })
        .cloned()
}

/// What preparing one mapping produced: a sender identity, or why the
/// mapping sits this run out. Skips are information for the report, never
/// silent drops.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum PrepareOutcome {
    Ready(String),
    Skipped(String),
}

/// Bind one mapped file to one assignment slot: the slot follows the
/// mapping's role, the hash follows the bytes on disk right now, and the
/// fingerprint follows the resolved target recipe. Retrying identical bytes
/// reuses the attempt; anything changed mints a new one.
pub fn prepare_mapping(
    log: &mut AttemptLog,
    assignment: &Assignment,
    fingerprint: &str,
    mapping: &crate::job::Mapping,
) -> PrepareOutcome {
    let Some(slot) = assignment
        .products
        .iter()
        .flat_map(|product| product.slots.iter())
        .find(|slot| slot.id == mapping.role_id)
    else {
        return PrepareOutcome::Skipped(format!(
            "mapping {:?} has no assignment slot {:?}",
            mapping.id, mapping.role_id
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
    PrepareOutcome::Ready(log.prepare(
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
    save_log(dir, log).map_err(IntakeError::Transport)?;
    let attempt = log.attempts[index].clone();
    match intake.submit(&attempt, bytes) {
        Ok(receipt) => {
            apply_receipt(log, &receipt);
            let _ = save_log(dir, log);
            Ok(receipt)
        }
        Err(IntakeError::Duplicate { receipt }) => {
            // The bytes already have a server identity: adopt it instead of
            // minting a second submission for the same content.
            let receipt = Receipt {
                attempt_id: attempt_id.into(),
                receipt,
                state: AttemptState::Accepted,
            };
            apply_receipt(log, &receipt);
            let _ = save_log(dir, log);
            Ok(receipt)
        }
        Err(error) => Err(error),
    }
}

/// Look up every unconfirmed attempt after an interruption: an accepted
/// attempt adopts the server's answer, and anything still unknown stays
/// unconfirmed for a later lookup instead of being resent blindly.
pub fn reconcile<I: Intake>(dir: &std::path::Path, log: &mut AttemptLog, intake: &mut I) {
    let unconfirmed: Vec<String> = log
        .attempts
        .iter()
        .filter(|attempt| attempt.state == AttemptState::Transferred)
        .map(|attempt| attempt.id.clone())
        .collect();
    for attempt_id in unconfirmed {
        if let Ok(receipt) = intake.status(&attempt_id) {
            apply_receipt(log, &receipt);
        }
    }
    let _ = save_log(dir, log);
}

fn apply_receipt(log: &mut AttemptLog, receipt: &Receipt) {
    if let Some(attempt) = log
        .attempts
        .iter_mut()
        .find(|attempt| attempt.id == receipt.attempt_id)
    {
        attempt.state = receipt.state;
        attempt.receipt = Some(receipt.receipt.clone());
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
    pub fn rehearsing_journaled(script: &FakeScript, journal: &std::path::Path) -> Self {
        let mut intake = Self::rehearsing(script);
        intake.journal = Some(journal.to_path_buf());
        if let Ok(bytes) = std::fs::read(journal)
            && let Ok(accepted) = serde_json::from_slice(&bytes)
        {
            intake.accepted = accepted;
        }
        intake
    }

    /// Persist accepted receipts for the next process. Best effort, like all
    /// rehearsal bookkeeping: the attempt log stays the durable queue.
    pub fn save_journal(&self) {
        if let Some(path) = &self.journal
            && let Ok(bytes) = serde_json::to_vec_pretty(&self.accepted)
        {
            let _ = std::fs::write(path, bytes);
        }
    }
}

impl Intake for FakeIntake {
    fn submit(&mut self, attempt: &Attempt, _bytes: &[u8]) -> Result<Receipt, IntakeError> {
        if let Some(known) = self.accepted.get(&attempt.id) {
            return Err(IntakeError::Duplicate {
                receipt: known.receipt.clone(),
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
        log.prepare("m1", "hash-1", 16, "main", "fp-1", "policy-7")
    }

    #[test]
    fn identical_bytes_reuse_the_sender_identity() {
        let mut log = queue();
        let first = prepared(&mut log);
        assert_eq!(prepared(&mut log), first, "a retry resends, not re-mints");
        // Changed bytes mint a distinct attempt for a distinct submission.
        let second = log.prepare("m1", "hash-2", 18, "main", "fp-1", "policy-7");
        assert_ne!(second, first);
        assert_eq!(log.attempts.len(), 2);
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
        let receipt = submit_attempt(&dir, &mut log, &mut intake, &id, b"bytes").unwrap();
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
        let refused = submit_attempt(&dir, &mut log, &mut intake, &id, b"bytes").unwrap_err();
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
            ],
        );
        submit_attempt(&dir, &mut log, &mut intake, &id, b"bytes").unwrap();
        // The reply never arrived, but the server took it: lookup adopts the
        // accepted answer instead of minting a duplicate submission.
        reconcile(&dir, &mut log, &mut intake);
        assert_eq!(log.attempts[0].state, AttemptState::Accepted);
        assert_eq!(log.attempts[0].receipt.as_deref(), Some("r-1"));
        // A retry of the same bytes meets the duplicate answer, not a second job.
        let again = submit_attempt(&dir, &mut log, &mut intake, &id, b"bytes").unwrap();
        assert_eq!(again.receipt, "r-1");
        std::fs::remove_dir_all(&dir).unwrap();
    }

    #[test]
    fn partial_batches_and_restarts_keep_successful_siblings() {
        let dir = store("partial");
        let mut log = queue();
        let first = log.prepare("m1", "hash-1", 16, "main", "fp-1", "policy-7");
        let second = log.prepare("m2", "hash-2", 16, "detail", "fp-1", "policy-7");
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
        let refused = submit_attempt(&dir, &mut log, &mut intake, &id, b"bytes").unwrap_err();
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
        assert!(cloned.validate().is_err(), "slot ids stay unique");
        assert!(parse_assignment(&vec![7u8; MAX_FILE_BYTES as usize + 1]).is_err());
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
        let PrepareOutcome::Ready(id) = prepare_mapping(&mut log, &assignment, "fp-1", &mapping)
        else {
            panic!("the mapped file prepares");
        };
        // Same bytes again reuses the identity.
        let PrepareOutcome::Ready(same) = prepare_mapping(&mut log, &assignment, "fp-1", &mapping)
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
            prepare_mapping(&mut log, &assignment, "fp-1", &stray),
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
            prepare_mapping(&mut log, &assignment, "fp-1", &gone),
            PrepareOutcome::Skipped(_)
        ));
        std::fs::remove_dir_all(&dir).unwrap();
    }
}
