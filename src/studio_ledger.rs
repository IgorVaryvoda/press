//! Quoted hosted work: the client side of paid Studio operations.
//!
//! The server contract this mirrors does not exist yet, so every service
//! answer here comes from a fixture or the scripted [`FakeLedger`]. What IS
//! real is the client machinery: capability gating, quote pinning, attempt
//! persistence before transmit, idempotent acceptance, reconciliation without
//! blind redispatch, and settlement inspection. Legacy direct AI calls keep
//! their current disclosures; nothing here adds retry, sponsorship or
//! recoverable-billing claims to those paths.
//!
//! Money rules the code enforces locally: one explicit payer per job, a
//! bounded expiring quote approved before upload, the same client attempt id
//! on every retry, no new job after an ambiguous provider timeout, and no
//! spend or upload the user did not confirm. Prompts travel as hashes; raw
//! text stays out of logs, receipts and diagnostics.

use serde::{Deserialize, Serialize};
use std::collections::HashMap;
use std::fs::{self, File, OpenOptions};
use std::io::Read;
use std::path::{Path, PathBuf};
use std::time::{SystemTime, UNIX_EPOCH};

/// The only ledger schemas this Press reads.
pub const SCHEMA_VERSION: u32 = 1;

/// A hosted job file larger than this is not a job.
pub const MAX_FILE_BYTES: u64 = 64 * 1024;

/// An explicitly selected hosted input larger than this is refused before it
/// is read. The real service may choose a different limit later; this local
/// rehearsal has one bounded input path and never contacts that service.
pub const MAX_INPUT_BYTES: u64 = 20 * 1024 * 1024;

/// The one contextual operation this rehearsal exposes. It is a local client
/// handler, not a claim about a production Studio endpoint.
pub const HOSTED_OPERATION: &str = "upscale";
pub const HOSTED_OPERATION_VERSION: u32 = 1;
const QUOTE_TTL_SECS: u64 = 15 * 60;
const MAX_ID_LEN: usize = 64;

/// One operation the service says this client may call, with the input bound
/// it enforces. Unknown operations stay unavailable: the capability response
/// cannot deliver executable UI, scripts or URLs, and this client would not
/// run them if it could.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Capability {
    pub op: String,
    pub version: u32,
    pub max_input_bytes: u64,
}

/// The capability document a rehearsal script or server provides.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Capabilities {
    pub operations: Vec<Capability>,
}

/// A bounded, approved promise to do paid work: operation, exact input
/// identity, parameters, model revision, payer and maximum charge. Any change
/// to input, prompt, model, payer or price needs a new quote, never a silent
/// edit of this one.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Quote {
    pub id: String,
    pub op: String,
    pub op_version: u32,
    pub input_hash: String,
    pub input_bytes: u64,
    pub prompt_hash: Option<String>,
    pub model_revision: String,
    pub payer: String,
    pub max_credits: u64,
    pub cancellation: String,
    /// Unix seconds after which this quote cannot be accepted.
    pub expires_at: u64,
}

/// Where a hosted job stands. Set from service answers or explicit local
/// decisions only. `Unresolved` is a real state, not a missing one: the
/// provider's outcome could not be determined, and only a lookup may move it.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum HostedState {
    Quoted,
    /// Acceptance may already have reached the service. Retry the same client
    /// id or reconcile it; never mint a second job.
    Submitting,
    Accepted,
    Processing,
    Complete,
    Failed,
    Cancelled,
    Unresolved,
}

impl HostedState {
    pub fn terminal(self) -> bool {
        matches!(self, Self::Complete | Self::Failed | Self::Cancelled)
    }

    fn can_follow(self, next: Self) -> bool {
        if self == next {
            return true;
        }
        if self == Self::Unresolved {
            return matches!(
                next,
                Self::Accepted | Self::Processing | Self::Complete | Self::Failed | Self::Cancelled
            );
        }
        if self.terminal() {
            return false;
        }
        matches!(
            (self, next),
            (Self::Quoted, Self::Submitting | Self::Cancelled)
                | (Self::Submitting, Self::Accepted | Self::Unresolved)
                | (
                    Self::Accepted,
                    Self::Processing
                        | Self::Complete
                        | Self::Failed
                        | Self::Cancelled
                        | Self::Unresolved
                )
                | (
                    Self::Processing,
                    Self::Complete | Self::Failed | Self::Cancelled | Self::Unresolved
                )
                | (
                    Self::Unresolved,
                    Self::Accepted
                        | Self::Processing
                        | Self::Complete
                        | Self::Failed
                        | Self::Cancelled
                )
        )
    }
}

/// What the money did, in the server's own words.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Settlement {
    pub reserved: u64,
    pub settled: u64,
    pub refunded: u64,
}

impl Settlement {
    fn validate(&self, maximum: u64) -> Result<(), LedgerError> {
        if self.reserved > maximum
            || self.settled > self.reserved
            || self.refunded > self.reserved
            || self.settled.saturating_add(self.refunded) > self.reserved
        {
            return Err(LedgerError::Transport(
                "the service returned an over-cap settlement".into(),
            ));
        }
        Ok(())
    }
}

/// One hosted job: its pinned quote, its server identity once accepted, its
/// state and its settlement. The receipt is the server's answer verbatim.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct HostedJob {
    pub schema: u32,
    pub id: String,
    pub payer: String,
    pub quote: Quote,
    pub state: HostedState,
    pub server_job: Option<String>,
    pub settlement: Option<Settlement>,
    pub receipt: Option<String>,
}

/// Why quoted work did not proceed, in the caller's words.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum LedgerError {
    /// The operation, input or payer is not covered: fix the request.
    Refused(String),
    /// The bytes did not arrive or the answer did not come back.
    Transport(String),
    /// The provider cannot say what happened: reconcile, never redispatch.
    Unknown(String),
}

/// The service side of quoted work. Implementations repeat service answers;
/// rehearsal scripts declare them up front.
pub trait Ledger {
    fn quote(
        &mut self,
        op: &str,
        version: u32,
        input_hash: &str,
        input_bytes: u64,
        prompt_hash: Option<&str>,
        payer: &str,
    ) -> Result<Quote, LedgerError>;
    fn accept(&mut self, job: &HostedJob) -> Result<(String, HostedState), LedgerError>;
    /// Find an acceptance by its stable client id without transmitting again.
    fn lookup(
        &mut self,
        client_job: &str,
    ) -> Result<Option<(String, HostedState, Option<Settlement>)>, LedgerError>;
    fn status(
        &mut self,
        server_job: &str,
    ) -> Result<(HostedState, Option<Settlement>), LedgerError>;
    fn cancel(&mut self, server_job: &str) -> Result<Settlement, LedgerError>;
    fn retrieve(&mut self, server_job: &str) -> Result<Vec<u8>, LedgerError>;
}

/// Open a quoted job. The id comes from a persisted counter at the call
/// site; the quote pins everything the money depends on.
pub fn open_job(id: &str, payer: &str, quote: Quote) -> HostedJob {
    HostedJob {
        schema: SCHEMA_VERSION,
        id: id.into(),
        payer: payer.into(),
        quote,
        state: HostedState::Quoted,
        server_job: None,
        settlement: None,
        receipt: None,
    }
}

/// Ask for a quote the capability document actually covers: known operation
/// and version, input within bounds, non-empty payer. Anything else refuses
/// before a single byte is prepared for upload.
#[allow(clippy::too_many_arguments)]
pub fn request_quote<L: Ledger>(
    ledger: &mut L,
    capabilities: &Capabilities,
    op: &str,
    version: u32,
    input_hash: &str,
    input_bytes: u64,
    prompt_hash: Option<&str>,
    payer: &str,
) -> Result<Quote, LedgerError> {
    if payer.trim().is_empty() {
        return Err(LedgerError::Refused(
            "a quoted job needs an explicit payer".into(),
        ));
    }
    if op != HOSTED_OPERATION || version != HOSTED_OPERATION_VERSION {
        return Err(LedgerError::Refused(format!(
            "operation {op:?} version {version} is unavailable in this build"
        )));
    }
    let Some(capability) = capabilities
        .operations
        .iter()
        .find(|known| known.op == op && known.version == version)
    else {
        return Err(LedgerError::Refused(format!(
            "operation {op:?} version {version} is not offered"
        )));
    };
    if input_bytes > capability.max_input_bytes {
        return Err(LedgerError::Refused(format!(
            "input {input_bytes} bytes exceeds the {op} limit of {}",
            capability.max_input_bytes
        )));
    }
    let quote = ledger.quote(op, version, input_hash, input_bytes, prompt_hash, payer)?;
    if quote.op != op
        || quote.op_version != version
        || quote.input_hash != input_hash
        || quote.input_bytes != input_bytes
        || quote.prompt_hash.as_deref() != prompt_hash
        || quote.payer != payer
        || quote.max_credits == 0
        || quote.expires_at <= unix_seconds()
    {
        return Err(LedgerError::Transport(
            "the service returned a quote that does not match the request".into(),
        ));
    }
    Ok(quote)
}

/// Mark a quoted job before an acceptance request leaves the process.
pub fn begin_accept(job: &mut HostedJob) -> Result<(), LedgerError> {
    if job.state != HostedState::Quoted {
        return Err(LedgerError::Refused(format!(
            "job {:?} is {:?}, not quoted",
            job.id, job.state
        )));
    }
    if job.quote.expires_at <= unix_seconds() {
        return Err(LedgerError::Refused(format!(
            "quote {:?} expired before acceptance",
            job.quote.id
        )));
    }
    job.state = HostedState::Submitting;
    Ok(())
}

/// Accept a job under its stable client id. Callers persist `Submitting` with
/// [`save_job`] before calling this function; a transport error intentionally
/// leaves that state for a later idempotent retry or reconciliation.
pub fn accept_job<L: Ledger>(ledger: &mut L, job: &mut HostedJob) -> Result<(), LedgerError> {
    if job.state == HostedState::Quoted {
        begin_accept(job)?;
    }
    if job.state != HostedState::Submitting {
        return Err(LedgerError::Refused(format!(
            "job {:?} is {:?}, not awaiting acceptance",
            job.id, job.state
        )));
    }
    match ledger.accept(job) {
        Ok((server_job, state)) => {
            if server_job.trim().is_empty()
                || !matches!(state, HostedState::Accepted | HostedState::Processing)
            {
                return Err(LedgerError::Transport(
                    "the service returned an invalid acceptance".into(),
                ));
            }
            job.server_job = Some(server_job);
            job.state = state;
            Ok(())
        }
        Err(LedgerError::Unknown(reason)) => {
            job.state = HostedState::Unresolved;
            job.receipt = Some(reason.clone());
            Err(LedgerError::Unknown(reason))
        }
        Err(error) => Err(error),
    }
}

/// Advance a live job from the service's status answer. Terminal answers
/// settle the job; anything else just moves it along.
pub fn poll_job<L: Ledger>(ledger: &mut L, job: &mut HostedJob) -> Result<(), LedgerError> {
    if job.state.terminal() {
        return Ok(());
    }
    let Some(server_job) = job.server_job.clone() else {
        return Err(LedgerError::Refused(format!(
            "job {:?} was never accepted",
            job.id
        )));
    };
    match ledger.status(&server_job) {
        Ok((state, settlement)) => {
            if !job.state.can_follow(state) {
                return Err(LedgerError::Transport(format!(
                    "the service moved job {:?} from {:?} back to {:?}",
                    job.id, job.state, state
                )));
            }
            if state.terminal() && settlement.is_none() {
                return Err(LedgerError::Transport(
                    "the service returned a terminal state without settlement".into(),
                ));
            }
            if let Some(settlement) = settlement.as_ref() {
                settlement.validate(job.quote.max_credits)?;
            }
            job.state = state;
            if settlement.is_some() {
                job.settlement = settlement;
            }
            Ok(())
        }
        Err(LedgerError::Unknown(reason)) => {
            job.state = HostedState::Unresolved;
            job.receipt = Some(reason);
            Ok(())
        }
        Err(error) => Err(error),
    }
}

/// Look up a job whose outcome went ambiguous: adopt the server's answer, or
/// stay explicitly unresolved. Never dispatch again on an unknown outcome.
pub fn reconcile_job<L: Ledger>(ledger: &mut L, job: &mut HostedJob) -> Result<(), LedgerError> {
    if job.state.terminal() {
        return Ok(());
    }
    // A pending accept has no server id by design. Look it up by the stable
    // client id; this is also the only useful recovery for an unknown accept
    // that did not return a server id. Neither branch transmits again.
    if job.state == HostedState::Submitting
        || (job.state == HostedState::Unresolved && job.server_job.is_none())
    {
        let Some((server_job, state, settlement)) = ledger.lookup(&job.id)? else {
            return Ok(());
        };
        if server_job.trim().is_empty()
            || !matches!(
                state,
                HostedState::Accepted
                    | HostedState::Processing
                    | HostedState::Complete
                    | HostedState::Failed
                    | HostedState::Cancelled
            )
            || (state.terminal() && settlement.is_none())
        {
            return Err(LedgerError::Transport(
                "the service returned an invalid acceptance lookup".into(),
            ));
        }
        if let Some(settlement) = settlement.as_ref() {
            settlement.validate(job.quote.max_credits)?;
        }
        job.server_job = Some(server_job);
        job.state = state;
        job.settlement = settlement;
        return Ok(());
    }
    poll_job(ledger, job)
}

/// Cancel under the quoted policy: unaccepted jobs simply stop, accepted
/// ones settle whatever the policy says and keep their receipts.
pub fn cancel_job<L: Ledger>(ledger: &mut L, job: &mut HostedJob) -> Result<(), LedgerError> {
    match job.state {
        HostedState::Quoted => {
            job.state = HostedState::Cancelled;
            Ok(())
        }
        HostedState::Submitting => Err(LedgerError::Refused(
            "acceptance is still unresolved; reconcile it before cancelling".into(),
        )),
        HostedState::Accepted | HostedState::Processing | HostedState::Unresolved => {
            let Some(server_job) = job.server_job.clone() else {
                return Err(LedgerError::Refused(
                    "the accepted job has no server id; it remains unresolved".into(),
                ));
            };
            match ledger.cancel(&server_job) {
                Ok(settlement) => {
                    settlement.validate(job.quote.max_credits)?;
                    job.settlement = Some(settlement);
                    job.state = HostedState::Cancelled;
                    Ok(())
                }
                Err(error) => Err(error),
            }
        }
        terminal => Err(LedgerError::Refused(format!(
            "job {:?} is already {terminal:?}",
            job.id
        ))),
    }
}

fn validate_capabilities(capabilities: &Capabilities) -> Result<(), String> {
    if capabilities.operations.is_empty() {
        return Err("a capability file with no operations offers nothing".into());
    }
    let mut seen = false;
    for capability in &capabilities.operations {
        if capability.op != HOSTED_OPERATION || capability.version != HOSTED_OPERATION_VERSION {
            return Err(format!(
                "capability {:?} version {} is not supported by this build",
                capability.op, capability.version
            ));
        }
        if capability.max_input_bytes == 0 || capability.max_input_bytes > MAX_INPUT_BYTES {
            return Err(format!(
                "capability {:?} has an invalid input bound",
                capability.op
            ));
        }
        if seen {
            return Err(format!("duplicate capability {:?}", capability.op));
        }
        seen = true;
    }
    Ok(())
}

/// One scripted ledger answer, in call order per job id. The last answer
/// repeats: polling a settled job keeps telling the truth.
#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
#[serde(deny_unknown_fields)]
pub enum LedgerAnswer {
    Quote {
        max_credits: u64,
        model: String,
    },
    Accept {
        server_job: String,
    },
    Progress {
        state: HostedState,
        settled: Option<u64>,
        refunded: Option<u64>,
    },
    Cancelled {
        settled: u64,
        refunded: u64,
    },
    RefuseQuote(String),
    Transport,
    UnknownOutcome,
}

/// A rehearsal ledger: declared answers per client job id, plus canned
/// result bytes for retrieval. Explicitly a rehearsal tool, never a server.
#[derive(Default)]
pub struct FakeLedger {
    answers: HashMap<String, Vec<LedgerAnswer>>,
    cursors: HashMap<String, usize>,
    accepted: HashMap<String, AcceptedJob>,
    results: HashMap<String, Vec<u8>>,
    pricing: HashMap<String, FakePricing>,
    journal: Option<PathBuf>,
}

#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
struct AcceptedJob {
    server_job: String,
    max_credits: u64,
    state: HostedState,
    settlement: Option<Settlement>,
}

#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
struct FakeLedgerState {
    schema: u32,
    cursors: HashMap<String, usize>,
    accepted: HashMap<String, AcceptedJob>,
}

/// A rehearsal script file: capability document plus scripted answers.
#[derive(Clone, Debug, Default, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct FakeLedgerScript {
    #[serde(default)]
    pub capabilities: Vec<Capability>,
    #[serde(default)]
    pub answers: std::collections::HashMap<String, Vec<LedgerAnswer>>,
    #[serde(default)]
    pub results: std::collections::HashMap<String, Vec<u8>>,
    #[serde(default)]
    pub pricing: std::collections::HashMap<String, FakePricing>,
}

/// Parse a rehearsal script with the same size bound as every other import.
pub fn parse_script(bytes: &[u8]) -> Result<FakeLedgerScript, String> {
    if bytes.len() as u64 > MAX_FILE_BYTES {
        return Err(format!(
            "rehearsal scripts larger than {MAX_FILE_BYTES} bytes are refused"
        ));
    }
    let script: FakeLedgerScript = serde_json::from_slice(bytes)
        .map_err(|error| format!("rehearsal script does not parse: {error}"))?;
    validate_capabilities(&Capabilities {
        operations: script.capabilities.clone(),
    })?;
    for pricing in script.pricing.values() {
        if pricing.max_credits == 0 || pricing.model.trim().is_empty() {
            return Err("rehearsal pricing needs a positive charge and model".into());
        }
    }
    Ok(script)
}

impl FakeLedger {
    /// Rehearse a script file. Unscripted calls fail transport like an
    /// unknown server; unscripted operations were never offered.
    pub fn rehearsing(script: &FakeLedgerScript) -> Self {
        Self {
            answers: script.answers.clone(),
            cursors: HashMap::new(),
            accepted: HashMap::new(),
            results: script.results.clone(),
            pricing: script.pricing.clone(),
            journal: None,
        }
    }

    /// Open the same scripted service state after a process restart. The
    /// journal records accepted client ids and answer cursors; script bytes
    /// remain the caller's explicit fixture input.
    pub fn rehearsing_journaled(script: &FakeLedgerScript, journal: &Path) -> Result<Self, String> {
        let mut ledger = Self::rehearsing(script);
        ledger.journal = Some(journal.to_path_buf());
        match fs::symlink_metadata(journal) {
            Ok(_) => {
                let bytes = read_bounded(journal, MAX_FILE_BYTES)?;
                let state: FakeLedgerState = serde_json::from_slice(&bytes)
                    .map_err(|error| format!("rehearsal journal does not parse: {error}"))?;
                if state.schema != SCHEMA_VERSION {
                    return Err(format!(
                        "unsupported rehearsal journal schema {} (this Press reads schema {SCHEMA_VERSION})",
                        state.schema
                    ));
                }
                ledger.cursors = state.cursors;
                ledger.accepted = state.accepted;
            }
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => {}
            Err(error) => return Err(format!("rehearsal journal cannot be inspected: {error}")),
        }
        ledger.persist()?;
        Ok(ledger)
    }

    fn next(&mut self, job_id: &str) -> Result<Option<LedgerAnswer>, LedgerError> {
        let answer = self.answers.get(job_id).and_then(|queue| {
            let cursor = *self.cursors.get(job_id).unwrap_or(&0);
            queue
                .get(cursor.min(queue.len().saturating_sub(1)))
                .cloned()
        });
        if let Some(queue) = self.answers.get(job_id)
            && !queue.is_empty()
        {
            let cursor = self.cursors.entry(job_id.to_string()).or_default();
            *cursor = (*cursor + 1).min(queue.len() - 1);
        }
        Ok(answer)
    }

    fn persist(&self) -> Result<(), String> {
        let Some(path) = self.journal.as_ref() else {
            return Ok(());
        };
        let state = FakeLedgerState {
            schema: SCHEMA_VERSION,
            cursors: self.cursors.clone(),
            accepted: self.accepted.clone(),
        };
        let bytes = serde_json::to_vec(&state)
            .map_err(|error| format!("rehearsal journal does not serialize: {error}"))?;
        if bytes.len() as u64 > MAX_FILE_BYTES {
            return Err("the rehearsal journal outgrew its bound".into());
        }
        let Some(parent) = path.parent() else {
            return Err("rehearsal journal has no parent folder".into());
        };
        fs::create_dir_all(parent)
            .map_err(|error| format!("rehearsal journal folder failed: {error}"))?;
        let tmp = path.with_extension(format!("tmp-{}", std::process::id()));
        fs::write(&tmp, bytes)
            .map_err(|error| format!("rehearsal journal write failed: {error}"))?;
        if let Err(error) = crate::settings::replace_file(&tmp, path) {
            let _ = fs::remove_file(&tmp);
            return Err(format!("rehearsal journal replace failed: {error}"));
        }
        Ok(())
    }
}

impl Ledger for FakeLedger {
    fn quote(
        &mut self,
        op: &str,
        version: u32,
        input_hash: &str,
        input_bytes: u64,
        prompt_hash: Option<&str>,
        payer: &str,
    ) -> Result<Quote, LedgerError> {
        let Some(pricing) = self.pricing.get(op) else {
            return Err(LedgerError::Transport(format!(
                "no scripted price for {op:?}"
            )));
        };
        // Quote ids stay unique per request: two quotes for one input are
        // two promises, and the job file tells them apart.
        let nonce = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .map(|age| age.as_nanos())
            .unwrap_or(0);
        Ok(Quote {
            id: format!("q-{op}-{nonce}-{}", std::process::id()),
            op: op.into(),
            op_version: version,
            input_hash: input_hash.into(),
            input_bytes,
            prompt_hash: prompt_hash.map(str::to_string),
            model_revision: pricing.model.clone(),
            payer: payer.into(),
            max_credits: pricing.max_credits,
            cancellation: "cancel before dispatch frees the reservation".into(),
            expires_at: unix_seconds().saturating_add(QUOTE_TTL_SECS),
        })
    }

    fn accept(&mut self, job: &HostedJob) -> Result<(String, HostedState), LedgerError> {
        if let Some(accepted) = self.accepted.get(&job.id) {
            return Ok((accepted.server_job.clone(), accepted.state));
        }
        // A retry can arrive after the client was restarted. An unaccepted
        // quote must not consume a fixture answer once its promise expired;
        // an already recorded acceptance remains idempotently discoverable.
        if job.quote.expires_at <= unix_seconds() {
            return Err(LedgerError::Refused(format!(
                "quote {:?} expired before acceptance",
                job.quote.id
            )));
        }
        match self.next(&job.id)? {
            Some(LedgerAnswer::Accept { server_job }) => {
                if server_job.trim().is_empty() {
                    return Err(LedgerError::Transport(
                        "the scripted acceptance has no server id".into(),
                    ));
                }
                self.accepted.insert(
                    job.id.clone(),
                    AcceptedJob {
                        server_job: server_job.clone(),
                        max_credits: job.quote.max_credits,
                        state: HostedState::Accepted,
                        settlement: None,
                    },
                );
                self.persist().map_err(LedgerError::Transport)?;
                Ok((server_job, HostedState::Accepted))
            }
            Some(LedgerAnswer::Transport) | None => {
                self.persist().map_err(LedgerError::Transport)?;
                Err(LedgerError::Transport(
                    "the acceptance did not arrive".into(),
                ))
            }
            Some(unexpected) => {
                self.persist().map_err(LedgerError::Transport)?;
                Err(LedgerError::Transport(format!(
                    "scripted {unexpected:?} answers no acceptance"
                )))
            }
        }
    }

    fn lookup(
        &mut self,
        client_job: &str,
    ) -> Result<Option<(String, HostedState, Option<Settlement>)>, LedgerError> {
        Ok(self.accepted.get(client_job).map(|accepted| {
            (
                accepted.server_job.clone(),
                accepted.state,
                accepted.settlement.clone(),
            )
        }))
    }

    fn status(
        &mut self,
        server_job: &str,
    ) -> Result<(HostedState, Option<Settlement>), LedgerError> {
        let Some((job_id, max_credits)) = self.accepted.iter().find_map(|(id, known)| {
            (known.server_job == server_job).then_some((id.clone(), known.max_credits))
        }) else {
            return Err(LedgerError::Transport(format!(
                "the service holds no job named {server_job:?}"
            )));
        };
        let result = match self.next(&job_id)? {
            Some(LedgerAnswer::Progress {
                state,
                settled,
                refunded,
            }) => {
                let settlement = match (settled, refunded) {
                    (None, None) => None,
                    (settled, refunded) => Some(Settlement {
                        reserved: max_credits,
                        settled: settled.unwrap_or(0),
                        refunded: refunded.unwrap_or(0),
                    }),
                };
                if !self
                    .accepted
                    .get(&job_id)
                    .is_some_and(|accepted| accepted.state.can_follow(state))
                {
                    Err(LedgerError::Transport(
                        "the scripted service moved a job backwards".into(),
                    ))
                } else {
                    Ok((state, settlement))
                }
            }
            Some(LedgerAnswer::UnknownOutcome) | None => Err(LedgerError::Unknown(format!(
                "the provider cannot say what happened to {server_job:?}"
            ))),
            Some(LedgerAnswer::Transport) => {
                Err(LedgerError::Transport("the status did not arrive".into()))
            }
            Some(unexpected) => Err(LedgerError::Transport(format!(
                "scripted {unexpected:?} answers no status"
            ))),
        };
        if let Ok((state, settlement)) = &result
            && let Some(accepted) = self.accepted.get_mut(&job_id)
        {
            accepted.state = *state;
            if settlement.is_some() {
                accepted.settlement = settlement.clone();
            }
        }
        self.persist().map_err(LedgerError::Transport)?;
        result
    }

    fn cancel(&mut self, server_job: &str) -> Result<Settlement, LedgerError> {
        let Some((job_id, max_credits)) = self.accepted.iter().find_map(|(id, known)| {
            (known.server_job == server_job).then_some((id.clone(), known.max_credits))
        }) else {
            return Err(LedgerError::Transport(format!(
                "the service holds no job named {server_job:?}"
            )));
        };
        let result = match self.next(&job_id)? {
            Some(LedgerAnswer::Cancelled { settled, refunded }) => Ok(Settlement {
                reserved: max_credits,
                settled,
                refunded,
            }),
            _ => Err(LedgerError::Transport(
                "the cancellation did not arrive".into(),
            )),
        };
        if let Ok(settlement) = &result
            && let Some(accepted) = self.accepted.get_mut(&job_id)
        {
            accepted.state = HostedState::Cancelled;
            accepted.settlement = Some(settlement.clone());
        }
        self.persist().map_err(LedgerError::Transport)?;
        result
    }

    fn retrieve(&mut self, server_job: &str) -> Result<Vec<u8>, LedgerError> {
        let Some(job_id) = self
            .accepted
            .iter()
            .find_map(|(id, known)| (known.server_job == server_job).then_some(id.clone()))
        else {
            return Err(LedgerError::Transport(format!(
                "the service holds no job named {server_job:?}"
            )));
        };
        self.results
            .get(&job_id)
            .cloned()
            .ok_or_else(|| LedgerError::Transport(format!("no result is ready for {server_job:?}")))
    }
}

/// Rehearsal pricing for one operation: the script names the charge so
/// quote tests pin gating, ids and payer handling without a server.
#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct FakePricing {
    pub max_credits: u64,
    pub model: String,
}
/// Unix seconds used for quote expiry without introducing a clock dependency.
fn unix_seconds() -> u64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map_or(0, |duration| duration.as_secs())
}

fn valid_id(id: &str) -> bool {
    !id.is_empty()
        && id.len() <= MAX_ID_LEN
        && id.bytes().all(|byte| {
            byte.is_ascii_lowercase() || byte.is_ascii_digit() || byte == b'-' || byte == b'_'
        })
}

fn file_for(dir: &Path, id: &str) -> Result<PathBuf, String> {
    if !valid_id(id) {
        return Err(format!("hosted job id {id:?} is not a safe local id"));
    }
    Ok(dir.join(format!("{id}.hosted.json")))
}

/// Resolve the machine-local hosted job folder.
pub fn studio_dir() -> Option<PathBuf> {
    crate::settings::path().and_then(|path| {
        path.parent()
            .map(|parent| parent.join("studio").join("jobs"))
    })
}

fn validate_job(job: &HostedJob) -> Result<(), String> {
    if job.schema != SCHEMA_VERSION {
        return Err(format!(
            "unsupported hosted schema {} (this Press reads schema {SCHEMA_VERSION})",
            job.schema
        ));
    }
    if !valid_id(&job.id) || job.payer.trim().is_empty() || job.quote.payer != job.payer {
        return Err("hosted job has an invalid id or payer".into());
    }
    if job.quote.op != HOSTED_OPERATION
        || job.quote.op_version != HOSTED_OPERATION_VERSION
        || job.quote.input_hash.is_empty()
        || job.quote.input_bytes > MAX_INPUT_BYTES
        || job.quote.max_credits == 0
    {
        return Err("hosted job quote is outside this build's contract".into());
    }
    if let Some(settlement) = job.settlement.as_ref() {
        settlement
            .validate(job.quote.max_credits)
            .map_err(|error| match error {
                LedgerError::Transport(message) => message,
                LedgerError::Refused(message) | LedgerError::Unknown(message) => message,
            })?;
    }
    match job.state {
        HostedState::Quoted | HostedState::Submitting => {
            if job.server_job.is_some() {
                return Err("unaccepted hosted job has a server id".into());
            }
        }
        HostedState::Unresolved => {
            if job.server_job.as_deref().is_some_and(str::is_empty) {
                return Err("unresolved hosted job has an empty server id".into());
            }
        }
        HostedState::Accepted
        | HostedState::Processing
        | HostedState::Complete
        | HostedState::Failed => {
            if job.server_job.as_deref().is_none_or(str::is_empty) {
                return Err("accepted hosted job has no server id".into());
            }
        }
        HostedState::Cancelled => {
            if job.server_job.as_deref().is_none_or(str::is_empty) && job.settlement.is_some() {
                return Err("cancelled hosted job settlement has no server id".into());
            }
        }
    }
    if job.state.terminal()
        && job.settlement.is_none()
        && !(job.state == HostedState::Cancelled
            && job.server_job.is_none()
            && job.settlement.is_none())
    {
        return Err("terminal hosted job has no settlement".into());
    }
    Ok(())
}

/// Save one hosted job atomically. Callers that mutate an existing job should
/// hold [`lock_job`] across load, service calls and this write.
pub fn save_job(dir: &Path, job: &HostedJob) -> Result<(), String> {
    validate_job(job)?;
    static COUNTER: std::sync::atomic::AtomicU64 = std::sync::atomic::AtomicU64::new(0);
    let pretty = serde_json::to_string_pretty(job)
        .map_err(|error| format!("hosted job does not serialize: {error}"))?;
    if pretty.len() as u64 > MAX_FILE_BYTES {
        return Err("the hosted job outgrew its bound".into());
    }
    std::fs::create_dir_all(dir).map_err(|error| format!("hosted library failed: {error}"))?;
    let path = file_for(dir, &job.id)?;
    let tmp = path.with_extension(format!(
        "tmp-{}-{}",
        std::process::id(),
        COUNTER.fetch_add(1, std::sync::atomic::Ordering::Relaxed)
    ));
    std::fs::write(&tmp, pretty.as_bytes())
        .map_err(|error| format!("hosted write failed: {error}"))?;
    crate::settings::replace_file(&tmp, &path).map_err(|error| {
        let _ = std::fs::remove_file(&tmp);
        format!("hosted write failed: {error}")
    })
}

/// Load one bounded, schema-validated hosted job.
pub fn load_job(dir: &Path, id: &str) -> Result<HostedJob, String> {
    let path = file_for(dir, id)?;
    let bytes = read_bounded(&path, MAX_FILE_BYTES)
        .map_err(|message| format!("no hosted job named {id:?} exists: {message}"))?;
    let job: HostedJob = serde_json::from_slice(&bytes)
        .map_err(|error| format!("hosted job does not parse: {error}"))?;
    if job.id != id {
        return Err("the hosted job names a different id".into());
    }
    validate_job(&job)?;
    Ok(job)
}

/// Read an explicitly selected file only after checking its size on disk.
pub fn read_bounded(path: &Path, maximum: u64) -> Result<Vec<u8>, String> {
    let file =
        File::open(path).map_err(|error| format!("{} cannot be read: {error}", path.display()))?;
    let capacity = usize::try_from(maximum.saturating_add(1)).unwrap_or(usize::MAX);
    let mut bytes = Vec::with_capacity(capacity.min(1024 * 1024));
    file.take(maximum.saturating_add(1))
        .read_to_end(&mut bytes)
        .map_err(|error| format!("{} cannot be read: {error}", path.display()))?;
    if bytes.len() as u64 > maximum {
        return Err(format!(
            "{} is larger than the {maximum}-byte hosted input bound",
            path.display()
        ));
    }
    Ok(bytes)
}

/// Retrieve a completed result. Retrieval has no inference or billing side
/// effect and therefore may be retried using the same server job id.
pub fn retrieve_job<L: Ledger>(ledger: &mut L, job: &HostedJob) -> Result<Vec<u8>, LedgerError> {
    if job.state != HostedState::Complete {
        return Err(LedgerError::Refused(format!(
            "job {:?} is {:?}, not complete",
            job.id, job.state
        )));
    }
    let Some(server_job) = job.server_job.as_deref() else {
        return Err(LedgerError::Refused(
            "completed job has no server id".into(),
        ));
    };
    ledger.retrieve(server_job)
}

/// A process-local, recoverable exclusive lock for one hosted job. The standard
/// library advisory lock releases automatically when a process dies.
pub struct JobLock {
    _file: File,
}

/// Lock one job while loading, mutating and saving it.
pub fn lock_job(dir: &Path, id: &str) -> Result<JobLock, String> {
    let path = file_for(dir, id)?.with_extension("lock");
    fs::create_dir_all(dir).map_err(|error| format!("hosted library failed: {error}"))?;
    let file = OpenOptions::new()
        .read(true)
        .write(true)
        .create(true)
        .truncate(false)
        .open(&path)
        .map_err(|error| format!("hosted lock failed: {error}"))?;
    match file.try_lock() {
        Ok(()) => Ok(JobLock { _file: file }),
        Err(std::fs::TryLockError::WouldBlock) => Err(format!(
            "hosted job {id:?} is being changed by another process"
        )),
        Err(std::fs::TryLockError::Error(error)) => Err(format!("hosted lock failed: {error}")),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn capabilities() -> Capabilities {
        Capabilities {
            operations: vec![Capability {
                op: "upscale".into(),
                version: HOSTED_OPERATION_VERSION,
                max_input_bytes: MAX_INPUT_BYTES,
            }],
        }
    }

    fn quoted() -> (FakeLedger, HostedJob) {
        let mut ledger = FakeLedger::default();
        ledger.answers.insert(
            "h1".into(),
            vec![
                LedgerAnswer::Accept {
                    server_job: "srv-1".into(),
                },
                LedgerAnswer::Progress {
                    state: HostedState::Processing,
                    settled: None,
                    refunded: None,
                },
                LedgerAnswer::Progress {
                    state: HostedState::Complete,
                    settled: Some(40),
                    refunded: Some(10),
                },
            ],
        );
        let job = HostedJob {
            schema: SCHEMA_VERSION,
            id: "h1".into(),
            payer: "studio-9".into(),
            quote: Quote {
                id: "q-1".into(),
                op: "upscale".into(),
                op_version: HOSTED_OPERATION_VERSION,
                input_hash: "hash-1".into(),
                input_bytes: 1024,
                prompt_hash: None,
                model_revision: "upscaler-3".into(),
                payer: "studio-9".into(),
                max_credits: 50,
                cancellation: "cancel before dispatch frees the reservation".into(),
                expires_at: unix_seconds().saturating_add(QUOTE_TTL_SECS),
            },
            state: HostedState::Quoted,
            server_job: None,
            settlement: None,
            receipt: None,
        };
        (ledger, job)
    }

    #[test]
    fn quoting_gates_on_capability_and_bounds() {
        let capabilities = capabilities();
        // Unknown operations and versions refuse before anything prepares.
        assert!(
            request_quote(
                &mut FakeLedger::default(),
                &capabilities,
                "teleport",
                1,
                "h",
                10,
                None,
                "studio-9"
            )
            .is_err()
        );
        assert!(
            request_quote(
                &mut FakeLedger::default(),
                &capabilities,
                HOSTED_OPERATION,
                HOSTED_OPERATION_VERSION + 1,
                "h",
                10,
                None,
                "studio-9"
            )
            .is_err()
        );
        assert!(
            request_quote(
                &mut FakeLedger::default(),
                &capabilities,
                HOSTED_OPERATION,
                HOSTED_OPERATION_VERSION,
                "h",
                10,
                None,
                "  "
            )
            .is_err()
        );
        assert!(
            request_quote(
                &mut FakeLedger::default(),
                &capabilities,
                HOSTED_OPERATION,
                HOSTED_OPERATION_VERSION,
                "h",
                21 * 1024 * 1024,
                None,
                "studio-9"
            )
            .is_err()
        );
        let mut ledger = FakeLedger::rehearsing(&FakeLedgerScript {
            capabilities: capabilities.operations.clone(),
            pricing: HashMap::from([(
                HOSTED_OPERATION.into(),
                FakePricing {
                    max_credits: 2,
                    model: "rehearsal-1".into(),
                },
            )]),
            ..FakeLedgerScript::default()
        });
        let quote = request_quote(
            &mut ledger,
            &capabilities,
            HOSTED_OPERATION,
            HOSTED_OPERATION_VERSION,
            "hash-1",
            2048,
            None,
            "studio-9",
        )
        .unwrap();
        assert_eq!(quote.max_credits, 2);
        assert_eq!(quote.payer, "studio-9");
    }

    #[test]
    fn acceptance_is_idempotent_and_settles() {
        let (mut ledger, mut job) = quoted();
        accept_job(&mut ledger, &mut job).unwrap();
        assert_eq!(job.state, HostedState::Accepted);
        assert_eq!(job.server_job.as_deref(), Some("srv-1"));
        // Accepting twice answers the same server job, never a second one.
        accept_job(&mut ledger, &mut job).unwrap_err();
        poll_job(&mut ledger, &mut job).unwrap();
        assert_eq!(job.state, HostedState::Processing);
        poll_job(&mut ledger, &mut job).unwrap();
        assert_eq!(job.state, HostedState::Complete);
        let settlement = job.settlement.clone().unwrap();
        assert_eq!((settlement.settled, settlement.refunded), (40, 10));
    }

    #[test]
    fn an_ambiguous_provider_outcome_stays_unresolved() {
        let (mut ledger, mut job) = quoted();
        ledger.answers.insert(
            "h1".into(),
            vec![
                LedgerAnswer::Accept {
                    server_job: "srv-9".into(),
                },
                LedgerAnswer::UnknownOutcome,
            ],
        );
        accept_job(&mut ledger, &mut job).unwrap();
        poll_job(&mut ledger, &mut job).unwrap();
        assert_eq!(job.state, HostedState::Unresolved);
        // Reconciling without a new answer keeps it unresolved: the client
        // never redispatches on an unknown outcome.
        reconcile_job(&mut ledger, &mut job).unwrap();
        assert_eq!(job.state, HostedState::Unresolved);
    }

    #[test]
    fn cancellation_follows_the_quoted_policy() {
        let (mut ledger, mut job) = quoted();
        // Cancelling before acceptance simply stops.
        cancel_job(&mut ledger, &mut job).unwrap();
        assert_eq!(job.state, HostedState::Cancelled);
        let (mut ledger, mut job) = quoted();
        ledger.answers.insert(
            "h1".into(),
            vec![
                LedgerAnswer::Accept {
                    server_job: "srv-1".into(),
                },
                LedgerAnswer::Cancelled {
                    settled: 5,
                    refunded: 45,
                },
            ],
        );
        accept_job(&mut ledger, &mut job).unwrap();
        cancel_job(&mut ledger, &mut job).unwrap();
        assert_eq!(job.state, HostedState::Cancelled);
        assert_eq!(job.settlement.clone().unwrap().settled, 5);
        // Terminal jobs refuse further transitions by name.
        assert!(cancel_job(&mut ledger, &mut job).is_err());
    }

    #[test]
    fn quoted_jobs_persist_across_restarts() {
        let dir = std::env::temp_dir().join(format!("press-studio-ledger-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        let (_, job) = quoted();
        save_job(&dir, &job).unwrap();
        let reloaded = load_job(&dir, "h1").unwrap();
        assert_eq!(reloaded, job);
        assert!(load_job(&dir, "nope").is_err());
        std::fs::remove_dir_all(&dir).unwrap();
    }

    #[test]
    fn submitting_state_survives_before_acceptance_and_retries_same_answer() {
        let dir =
            std::env::temp_dir().join(format!("press-studio-ledger-submit-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        let (mut ledger, mut job) = quoted();
        begin_accept(&mut job).unwrap();
        save_job(&dir, &job).unwrap();
        assert_eq!(load_job(&dir, "h1").unwrap().state, HostedState::Submitting);
        accept_job(&mut ledger, &mut job).unwrap();
        assert_eq!(job.server_job.as_deref(), Some("srv-1"));
        std::fs::remove_dir_all(&dir).unwrap();
    }

    #[test]
    fn cancelled_quote_and_terminal_job_round_trip() {
        let dir = std::env::temp_dir().join(format!(
            "press-studio-ledger-terminal-{}",
            std::process::id()
        ));
        let _ = std::fs::remove_dir_all(&dir);
        let (mut ledger, mut cancelled) = quoted();
        cancel_job(&mut ledger, &mut cancelled).unwrap();
        save_job(&dir, &cancelled).unwrap();
        assert_eq!(load_job(&dir, "h1").unwrap().state, HostedState::Cancelled);

        let (mut ledger, mut complete) = quoted();
        accept_job(&mut ledger, &mut complete).unwrap();
        poll_job(&mut ledger, &mut complete).unwrap();
        poll_job(&mut ledger, &mut complete).unwrap();
        save_job(&dir, &complete).unwrap();
        assert_eq!(load_job(&dir, "h1").unwrap().state, HostedState::Complete);
        std::fs::remove_dir_all(&dir).unwrap();
    }

    #[test]
    fn unsafe_job_ids_and_oversized_reads_are_refused() {
        let dir =
            std::env::temp_dir().join(format!("press-studio-ledger-bound-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        assert!(lock_job(&dir, "../escape").is_err());
        let path = dir.join("input");
        std::fs::create_dir_all(&dir).unwrap();
        std::fs::write(&path, b"12345").unwrap();
        assert!(read_bounded(&path, 4).is_err());
        std::fs::remove_dir_all(&dir).unwrap();
    }

    #[test]
    fn reconciliation_adopts_a_completed_acceptance_after_restart() {
        let dir = std::env::temp_dir().join(format!(
            "press-studio-ledger-reconcile-{}",
            std::process::id()
        ));
        let _ = std::fs::remove_dir_all(&dir);
        let journal = dir.join("h1.rehearsal.json");
        let script = FakeLedgerScript {
            capabilities: capabilities().operations,
            answers: HashMap::from([(
                "h1".into(),
                vec![
                    LedgerAnswer::Accept {
                        server_job: "srv-1".into(),
                    },
                    LedgerAnswer::Progress {
                        state: HostedState::Processing,
                        settled: None,
                        refunded: None,
                    },
                    LedgerAnswer::Progress {
                        state: HostedState::Complete,
                        settled: Some(40),
                        refunded: Some(10),
                    },
                ],
            )]),
            ..FakeLedgerScript::default()
        };
        let (_, mut client_job) = quoted();
        let mut ledger = FakeLedger::rehearsing_journaled(&script, &journal).unwrap();
        begin_accept(&mut client_job).unwrap();
        accept_job(&mut ledger, &mut client_job).unwrap();
        let server_job = client_job.server_job.clone().unwrap();
        ledger.status(&server_job).unwrap();
        ledger.status(&server_job).unwrap();

        // The process can die after the service reaches Complete but before
        // the client writes its response. The durable fake state must let a
        // restarted Submitting job discover that terminal result.
        let (_, mut restarted) = quoted();
        restarted.state = HostedState::Submitting;
        let mut after_restart = FakeLedger::rehearsing_journaled(&script, &journal).unwrap();
        reconcile_job(&mut after_restart, &mut restarted).unwrap();
        assert_eq!(restarted.state, HostedState::Complete);
        assert_eq!(restarted.server_job.as_deref(), Some("srv-1"));
        assert_eq!(restarted.settlement.unwrap().refunded, 10);
        std::fs::remove_dir_all(&dir).unwrap();
    }

    #[test]
    fn an_expired_pending_acceptance_does_not_consume_a_script_answer() {
        let (mut ledger, mut job) = quoted();
        job.state = HostedState::Submitting;
        job.quote.expires_at = unix_seconds().saturating_sub(1);
        let error = accept_job(&mut ledger, &mut job).unwrap_err();
        assert!(matches!(error, LedgerError::Refused(_)));
        assert_eq!(job.state, HostedState::Submitting);
        assert_eq!(ledger.cursors.get("h1"), None);
        assert!(ledger.accepted.is_empty());
    }
}
