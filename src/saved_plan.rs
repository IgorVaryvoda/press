//! Portable, local conversion plans and their restartable receipts.
//!
//! A plan is a reviewed description of relative files and effective settings. It
//! carries no roots, credentials, commands, or permission. Execution binds it to
//! explicit roots again, checks the same source bytes and collision decisions, and
//! writes one local run state beside the plan for interruption recovery.

use std::io::{Read, Write};
use std::path::{Path, PathBuf};

use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};

use crate::convert::{Failure, Format, MaxEdge, Quality};

pub const SCHEMA_VERSION: u32 = 1;
pub const MAX_PLAN_BYTES: u64 = 8 * 1024 * 1024;
const MAX_STATE_BYTES: u64 = 64 * 1024 * 1024;
pub const MAX_SOURCES: usize = 16_384;
pub const MAX_TARGETS: usize = 16;
pub const ENGINE_REVISION: &str = "press-convert-engine-v1";

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum ExecutionMode {
    ContinueUnstarted,
    RetryFailed,
    Cancel,
}

#[derive(Clone, Debug, PartialEq)]
pub struct TargetInput {
    pub id: String,
    pub out: String,
    pub recipe: EffectiveRecipe,
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct EffectiveRecipe {
    pub format: String,
    pub quality: Option<f32>,
    pub max_edge: Option<u32>,
    /// The actual libaom setting, including the default. Plans are portable data;
    /// this value is not a machine preference and is applied only at execution.
    pub avif_speed: u8,
    pub fingerprint: String,
    pub engine_revision: String,
}

impl EffectiveRecipe {
    pub fn from_settings(
        format: Format,
        quality: Quality,
        max_edge: MaxEdge,
        avif_speed: Option<u8>,
    ) -> Self {
        let speed = avif_speed.unwrap_or(crate::avif::DEFAULT_SPEED);
        let fingerprint =
            crate::recipe::fingerprint_settings(format, quality, max_edge, Some(speed));
        Self {
            format: format.label().to_string(),
            quality: quality.0,
            max_edge: max_edge.0,
            avif_speed: speed,
            fingerprint,
            engine_revision: ENGINE_REVISION.to_string(),
        }
    }

    fn settings(&self) -> Result<(Format, Quality, MaxEdge), String> {
        let format = match self.format.as_str() {
            "webp" => Format::WebP,
            "avif" => Format::Avif,
            "jxl" => Format::JpegXl,
            "jpeg" => Format::Jpeg,
            "png" => Format::Png,
            "same" => Format::Same,
            _ => return Err(format!("plan names unsupported format {:?}", self.format)),
        };
        if self.avif_speed > 10 {
            return Err(format!(
                "plan AVIF speed {} is outside 0-10",
                self.avif_speed
            ));
        }
        if let Some(value) = self.quality
            && (!(1. ..=100.).contains(&value) || !value.is_finite())
        {
            return Err("plan quality must be finite and between 1 and 100".into());
        }
        if self.max_edge == Some(0) {
            return Err("plan max edge must be at least 1 pixel".into());
        }
        let quality = Quality(self.quality);
        let max_edge = MaxEdge(self.max_edge);
        let expected =
            crate::recipe::fingerprint_settings(format, quality, max_edge, Some(self.avif_speed));
        if self.engine_revision != ENGINE_REVISION {
            return Err(format!(
                "plan engine revision {:?} is not supported by this Press",
                self.engine_revision
            ));
        }
        if self.fingerprint != expected {
            return Err("plan recipe fingerprint does not match its effective settings".into());
        }
        if quality == Quality::LOSSLESS && !format.supports_lossless() {
            return Err(format!(
                "plan requests lossless output for {}",
                format.label()
            ));
        }
        Ok((format, quality, max_edge))
    }
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct PlanSource {
    pub item_id: String,
    pub source: String,
    pub bytes: u64,
    pub sha256: String,
    pub width: u32,
    pub height: u32,
    pub format: String,
    pub checks: Vec<Check>,
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct PlanTarget {
    pub target_id: String,
    /// Relative to the explicitly rebound output root. Empty is the default
    /// target namespace; every mapping still names a nonempty file below it.
    pub out: String,
    pub recipe: EffectiveRecipe,
    pub mappings: Vec<PlanMapping>,
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct PlanMapping {
    pub item_id: String,
    /// Relative to the explicitly rebound output root, including `target.out`.
    pub output: Option<String>,
    pub status: MappingStatus,
    pub error: Option<String>,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum MappingStatus {
    Planned,
    Refused,
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Check {
    pub name: String,
    pub status: CheckStatus,
    pub reason: Option<String>,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum CheckStatus {
    Completed,
    Deferred,
    Refused,
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Plan {
    pub schema_version: u32,
    pub plan_id: String,
    pub digest: String,
    pub sources: Vec<PlanSource>,
    pub targets: Vec<PlanTarget>,
    pub checks: Vec<Check>,
    pub write_scope: String,
}

impl Plan {
    pub fn validate(&self) -> Result<(), String> {
        if self.schema_version != SCHEMA_VERSION {
            return Err(format!(
                "unsupported saved plan schema {} (this Press reads schema {SCHEMA_VERSION})",
                self.schema_version
            ));
        }
        if !is_hex(&self.digest, 64) {
            return Err("saved plan digest must be a 64-character SHA-256 value".into());
        }
        if !self.plan_id.starts_with("plan-") || !is_hex(&self.plan_id[5..], 16) {
            return Err("saved plan id is not a stable plan identifier".into());
        }
        if self.plan_id != format!("plan-{}", &self.digest[..16]) {
            return Err("saved plan id does not match its digest".into());
        }
        if self.write_scope != "outputs_only" {
            return Err(format!(
                "saved plan write scope {:?} is not supported",
                self.write_scope
            ));
        }
        if self.sources.len() > MAX_SOURCES {
            return Err(format!("saved plan has more than {MAX_SOURCES} sources"));
        }
        if self.targets.is_empty() || self.targets.len() > MAX_TARGETS {
            return Err(format!("saved plan needs 1-{MAX_TARGETS} targets"));
        }
        let mut item_ids = std::collections::HashSet::new();
        let mut source_paths = std::collections::HashSet::new();
        for source in &self.sources {
            if !is_hex(&source.item_id, 64) {
                return Err(format!("source {:?} has an invalid item id", source.source));
            }
            if source.item_id != hex_digest(format!("item|{}", source.source).as_bytes()) {
                return Err(format!(
                    "source {:?} has an unstable item id",
                    source.source
                ));
            }
            if !item_ids.insert(source.item_id.as_str()) {
                return Err(format!("saved plan repeats item id {:?}", source.item_id));
            }
            let path = Path::new(&source.source);
            ensure_relative(path, "source")?;
            if !source_paths.insert(source.source.as_str()) {
                return Err(format!("saved plan repeats source {:?}", source.source));
            }
            if source.bytes > crate::convert::MAX_SOURCE_BYTES {
                return Err(format!(
                    "source {:?} exceeds the bounded input size",
                    source.source
                ));
            }
            if !is_hex(&source.sha256, 64) {
                return Err(format!("source {:?} has an invalid SHA-256", source.source));
            }
            if source.width == 0 || source.height == 0 {
                return Err(format!("source {:?} has invalid dimensions", source.source));
            }
        }
        let mut target_ids = std::collections::HashSet::new();
        for target in &self.targets {
            if !safe_id(&target.target_id) || !target_ids.insert(target.target_id.as_str()) {
                return Err(format!(
                    "saved plan has an invalid or repeated target id {:?}",
                    target.target_id
                ));
            }
            if !target.out.is_empty() {
                ensure_relative(Path::new(&target.out), "target output")?;
            }
            target.recipe.settings()?;
            if target.mappings.len() != self.sources.len() {
                return Err(format!(
                    "target {:?} has {} mappings for {} sources",
                    target.target_id,
                    target.mappings.len(),
                    self.sources.len()
                ));
            }
            let mut mapped = std::collections::HashSet::new();
            for mapping in &target.mappings {
                if !item_ids.contains(mapping.item_id.as_str())
                    || !mapped.insert(mapping.item_id.as_str())
                {
                    return Err(format!(
                        "target {:?} has an invalid mapping",
                        target.target_id
                    ));
                }
                match (
                    mapping.status,
                    mapping.output.as_deref(),
                    mapping.error.as_deref(),
                ) {
                    (MappingStatus::Planned, Some(output), None) => {
                        ensure_relative(Path::new(output), "planned output")?;
                    }
                    (MappingStatus::Refused, None, Some(error)) if !error.is_empty() => {}
                    _ => {
                        return Err(format!(
                            "target {:?} has an invalid mapping status",
                            target.target_id
                        ));
                    }
                }
            }
        }
        if self.digest != self.compute_digest()? {
            return Err("saved plan digest does not match its normalized contents".into());
        }
        Ok(())
    }

    fn compute_digest(&self) -> Result<String, String> {
        #[derive(Serialize)]
        struct DigestView<'a> {
            schema_version: u32,
            sources: &'a [PlanSource],
            targets: &'a [PlanTarget],
            checks: &'a [Check],
            write_scope: &'a str,
        }
        let view = DigestView {
            schema_version: self.schema_version,
            sources: &self.sources,
            targets: &self.targets,
            checks: &self.checks,
            write_scope: &self.write_scope,
        };
        let bytes = serde_json::to_vec(&view).map_err(|error| error.to_string())?;
        Ok(hex_digest(&bytes))
    }
}

pub fn build(
    root: &Path,
    output_root: &Path,
    entries: &[crate::scan::Entry],
    scan_errors: usize,
    mut targets: Vec<TargetInput>,
) -> Result<Plan, String> {
    if scan_errors != 0 {
        return Err(format!(
            "source scan has {scan_errors} unreadable paths; fix them and create a new plan"
        ));
    }
    if targets.is_empty() || targets.len() > MAX_TARGETS {
        return Err(format!("a saved plan needs 1-{MAX_TARGETS} targets"));
    }
    targets.sort_by(|left, right| left.id.cmp(&right.id));
    let mut sources_with_entries: Vec<_> = entries.iter().collect();
    sources_with_entries.sort_by_key(|entry| relative_text(root, &entry.path).unwrap_or_default());
    let mut sources = Vec::with_capacity(sources_with_entries.len());
    let mut source_paths = Vec::with_capacity(sources_with_entries.len());
    for entry in sources_with_entries {
        let source = relative_text(root, &entry.path)?;
        let bytes = crate::scan::read_source_bytes(&entry.path)
            .map_err(|error| format!("source {source:?} could not be snapshotted: {error:?}"))?;
        let identity = crate::manifest::SourceIdentity::from_bytes(&bytes);
        sources.push(PlanSource {
            item_id: hex_digest(format!("item|{source}").as_bytes()),
            source,
            bytes: identity.bytes,
            sha256: identity.hash,
            width: entry.width,
            height: entry.height,
            format: crate::scan::format_name(entry.format)
                .to_ascii_lowercase()
                .replace(' ', "_"),
            checks: vec![
                Check {
                    name: "source_snapshot".into(),
                    status: CheckStatus::Completed,
                    reason: None,
                },
                Check {
                    name: "pixel_decode".into(),
                    status: CheckStatus::Deferred,
                    reason: Some(
                        "execution consumes and verifies the bounded source snapshot".into(),
                    ),
                },
            ],
        });
        source_paths.push(entry.path.clone());
    }

    let mut plan_targets = Vec::with_capacity(targets.len());
    for target in targets {
        target.recipe.settings()?;
        let target_root = if target.out.is_empty() {
            output_root.to_path_buf()
        } else {
            ensure_relative(Path::new(&target.out), "target output")?;
            output_root.join(&target.out)
        };
        let recorded = crate::manifest::load(&target_root);
        let destination = crate::convert::Destination {
            out_dir: &target_root,
            backups: None,
            manifest: &recorded,
        };
        let (format, _, _) = target.recipe.settings()?;
        let planned =
            crate::convert::plan_outputs(root, &source_paths, &source_paths, &destination, format);
        let mappings = sources
            .iter()
            .zip(planned)
            .map(|(source, planned)| match planned {
                Ok(path) => Ok(PlanMapping {
                    item_id: source.item_id.clone(),
                    output: Some(relative_text(output_root, &path)?),
                    status: MappingStatus::Planned,
                    error: None,
                }),
                Err(error) => Ok(PlanMapping {
                    item_id: source.item_id.clone(),
                    output: None,
                    status: MappingStatus::Refused,
                    error: Some(failure_text(error)),
                }),
            })
            .collect::<Result<Vec<_>, String>>()?;
        plan_targets.push(PlanTarget {
            target_id: target.id,
            out: target.out,
            recipe: target.recipe,
            mappings,
        });
    }
    let mut plan = Plan {
        schema_version: SCHEMA_VERSION,
        plan_id: String::new(),
        digest: String::new(),
        sources,
        targets: plan_targets,
        checks: vec![
            Check {
                name: "source_scan".into(),
                status: CheckStatus::Completed,
                reason: None,
            },
            Check {
                name: "output_ownership".into(),
                status: CheckStatus::Deferred,
                reason: Some(
                    "execution rechecks the explicit output boundary and ownership".into(),
                ),
            },
        ],
        write_scope: "outputs_only".into(),
    };
    plan.digest = plan.compute_digest()?;
    plan.plan_id = format!("plan-{}", &plan.digest[..16]);
    plan.validate()?;
    Ok(plan)
}

pub fn save_new(path: &Path, plan: &Plan) -> Result<(), String> {
    plan.validate()?;
    if path.as_os_str().is_empty() {
        return Err("saved plan path is empty".into());
    }
    if path.symlink_metadata().is_ok() {
        return Err(format!("saved plan {} already exists", path.display()));
    }
    let bytes = serde_json::to_vec_pretty(plan).map_err(|error| error.to_string())?;
    if bytes.len() as u64 > MAX_PLAN_BYTES {
        return Err(format!(
            "saved plan exceeds the {MAX_PLAN_BYTES}-byte limit"
        ));
    }
    if let Some(parent) = path.parent()
        && !parent.as_os_str().is_empty()
        && !parent.is_dir()
    {
        return Err(format!(
            "saved plan parent {} is not a folder",
            parent.display()
        ));
    }
    let mut file = std::fs::OpenOptions::new()
        .write(true)
        .create_new(true)
        .open(path)
        .map_err(|error| format!("could not create saved plan {}: {error}", path.display()))?;
    if let Err(error) = std::io::Write::write_all(&mut file, &bytes)
        .and_then(|()| std::io::Write::flush(&mut file))
        .and_then(|()| file.sync_all())
    {
        drop(file);
        let _ = std::fs::remove_file(path);
        return Err(format!(
            "could not write saved plan {}: {error}",
            path.display()
        ));
    }
    Ok(())
}

pub fn load(path: &Path) -> Result<Plan, String> {
    let metadata = std::fs::symlink_metadata(path)
        .map_err(|error| format!("saved plan {} cannot be inspected: {error}", path.display()))?;
    if !metadata.is_file() {
        return Err(format!("saved plan {} is not a file", path.display()));
    }
    // The metadata check is only an early refusal. Read through the bounded
    // helper as well because a file can grow between stat and open.
    let bytes = read_bounded(path, MAX_PLAN_BYTES, "saved plan")?;
    let plan: Plan = serde_json::from_slice(&bytes)
        .map_err(|error| format!("saved plan does not parse: {error}"))?;
    plan.validate()?;
    Ok(plan)
}

fn relative_text(root: &Path, path: &Path) -> Result<String, String> {
    let relative = path
        .strip_prefix(root)
        .map_err(|_| format!("{} is outside {}", path.display(), root.display()))?;
    ensure_relative(relative, "source")?;
    portable_relative(relative)
}

pub fn portable_relative(path: &Path) -> Result<String, String> {
    if path.as_os_str().is_empty() {
        return Ok(String::new());
    }
    ensure_relative(path, "relative")?;
    path.components()
        .map(|component| {
            component
                .as_os_str()
                .to_str()
                .map(str::to_owned)
                .ok_or_else(|| format!("{} contains a non-UTF-8 path component", path.display()))
        })
        .collect::<Result<Vec<_>, _>>()
        .map(|components| components.join("/"))
}

fn ensure_relative(path: &Path, what: &str) -> Result<(), String> {
    crate::output::normal_relative(path)
        .map_err(|_| format!("{what} path {:?} is not plain relative data", path))
}

fn safe_id(value: &str) -> bool {
    !value.is_empty()
        && value.len() <= 64
        && value.bytes().all(|byte| {
            byte.is_ascii_lowercase() || byte.is_ascii_digit() || byte == b'-' || byte == b'_'
        })
}

fn is_hex(value: &str, length: usize) -> bool {
    value.len() == length && value.bytes().all(|byte| byte.is_ascii_hexdigit())
}

fn hex_digest(bytes: &[u8]) -> String {
    format!("{:x}", Sha256::digest(bytes))
}

fn failure_text(failure: Failure) -> String {
    failure
        .reason()
        .unwrap_or_else(|| format!("conversion refused: {failure:?}"))
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ItemStatus {
    Unstarted,
    Running,
    Written,
    Failed,
    Cancelled,
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct RunItem {
    pub item_id: String,
    pub target_id: String,
    pub source: String,
    pub status: ItemStatus,
    pub output: Option<String>,
    pub output_hash: Option<String>,
    pub output_bytes: Option<u64>,
    pub width: Option<u32>,
    pub height: Option<u32>,
    pub recipe: String,
    pub engine_revision: String,
    pub checks: Vec<Check>,
    pub error: Option<String>,
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
struct RunState {
    schema_version: u32,
    plan_digest: String,
    run_id: String,
    source_root: String,
    output_root: String,
    items: Vec<RunItem>,
}

#[derive(Clone, Debug, Serialize)]
pub struct RunReport {
    pub schema_version: u32,
    pub command: &'static str,
    pub status: &'static str,
    pub plan_id: String,
    pub digest: String,
    pub run_id: String,
    pub counts: RunCounts,
    pub items: Vec<RunItem>,
}

#[derive(Clone, Debug, Serialize)]
pub struct RunCounts {
    pub total: usize,
    pub written: usize,
    pub failed: usize,
    pub unstarted: usize,
    pub cancelled: usize,
}

pub struct RunResult {
    pub report: RunReport,
    pub exit_code: i32,
}

pub fn execute(
    plan_path: &Path,
    source_root: &Path,
    output_root: &Path,
    mode: ExecutionMode,
) -> Result<RunResult, String> {
    let plan = load(plan_path)?;
    validate_bindings(source_root, output_root)?;
    validate_mappings(&plan, source_root, output_root)?;
    let state_path = state_path(plan_path, source_root, output_root)?;
    let mut state = if state_path.exists() {
        load_state(&state_path)?
    } else {
        new_state(&plan, source_root, output_root)
    };
    if state.plan_digest != plan.digest
        || state.source_root != root_text(source_root)
        || state.output_root != root_text(output_root)
    {
        return Err(format!(
            "saved run state {} belongs to another plan or explicit root binding",
            state_path.display()
        ));
    }
    reconcile_items(&plan, source_root, output_root, &mut state)?;
    save_state(&state_path, &state)?;

    for target in &plan.targets {
        for source in &plan.sources {
            let Some(index) = state.items.iter().position(|item| {
                item.item_id == source.item_id && item.target_id == target.target_id
            }) else {
                return Err("saved run state is missing a plan item".into());
            };
            let eligible = match mode {
                ExecutionMode::ContinueUnstarted => {
                    state.items[index].status == ItemStatus::Unstarted
                }
                ExecutionMode::RetryFailed => state.items[index].status == ItemStatus::Failed,
                ExecutionMode::Cancel => state.items[index].status == ItemStatus::Unstarted,
            };
            if !eligible {
                continue;
            }
            if mode == ExecutionMode::Cancel {
                state.items[index].status = ItemStatus::Cancelled;
                state.items[index].error = None;
                save_state(&state_path, &state)?;
                continue;
            }
            let Some(mapping) = target
                .mappings
                .iter()
                .find(|mapping| mapping.item_id == source.item_id)
            else {
                return Err("saved plan is missing a target mapping".into());
            };
            if mapping.status != MappingStatus::Planned {
                state.items[index].status = ItemStatus::Failed;
                state.items[index].error = mapping.error.clone();
                save_state(&state_path, &state)?;
                continue;
            }
            state.items[index].status = ItemStatus::Running;
            state.items[index].error = None;
            save_state(&state_path, &state)?;
            match execute_item(&plan, target, source, mapping, source_root, output_root) {
                Ok(receipt) => state.items[index] = receipt,
                Err(error) => {
                    state.items[index].status = ItemStatus::Failed;
                    state.items[index].error = Some(error);
                }
            }
            save_state(&state_path, &state)?;
        }
    }
    Ok(result(&plan, &state, "execute"))
}

pub fn reconcile(
    plan_path: &Path,
    source_root: &Path,
    output_root: &Path,
) -> Result<RunResult, String> {
    let plan = load(plan_path)?;
    validate_bindings(source_root, output_root)?;
    let state_path = state_path(plan_path, source_root, output_root)?;
    if !state_path.exists() {
        return Err("no saved run state exists for this plan and explicit root binding".into());
    }
    let mut state = load_state(&state_path)?;
    if state.plan_digest != plan.digest
        || state.source_root != root_text(source_root)
        || state.output_root != root_text(output_root)
    {
        return Err("saved run state belongs to another plan or explicit root binding".into());
    }
    reconcile_items(&plan, source_root, output_root, &mut state)?;
    save_state(&state_path, &state)?;
    Ok(result(&plan, &state, "reconcile"))
}

fn validate_bindings(source_root: &Path, output_root: &Path) -> Result<(), String> {
    if !source_root.is_dir() {
        return Err(format!(
            "saved plan source root {} is not a folder",
            source_root.display()
        ));
    }
    let context = crate::output::Context::establish(source_root, output_root)
        .map_err(|error| format!("saved plan output refused: {error}"))?;
    if context.output_root() != output_root {
        return Err("saved plan output binding is not canonical".into());
    }
    Ok(())
}

fn new_state(plan: &Plan, source_root: &Path, output_root: &Path) -> RunState {
    let mut items = Vec::with_capacity(plan.sources.len() * plan.targets.len());
    for target in &plan.targets {
        for source in &plan.sources {
            let mapping = target
                .mappings
                .iter()
                .find(|mapping| mapping.item_id == source.item_id);
            let (status, error) = match mapping {
                Some(mapping) if mapping.status == MappingStatus::Refused => {
                    (ItemStatus::Failed, mapping.error.clone())
                }
                _ => (ItemStatus::Unstarted, None),
            };
            items.push(RunItem {
                item_id: source.item_id.clone(),
                target_id: target.target_id.clone(),
                source: source.source.clone(),
                status,
                output: mapping.and_then(|mapping| mapping.output.clone()),
                output_hash: None,
                output_bytes: None,
                width: None,
                height: None,
                recipe: target.recipe.fingerprint.clone(),
                engine_revision: target.recipe.engine_revision.clone(),
                checks: Vec::new(),
                error,
            });
        }
    }
    RunState {
        schema_version: SCHEMA_VERSION,
        plan_digest: plan.digest.clone(),
        run_id: format!(
            "run-{}",
            &hex_digest(
                format!("{}|{}", root_text(source_root), root_text(output_root)).as_bytes()
            )[..16]
        ),
        source_root: root_text(source_root),
        output_root: root_text(output_root),
        items,
    }
}

fn execute_item(
    plan: &Plan,
    target: &PlanTarget,
    source: &PlanSource,
    mapping: &PlanMapping,
    source_root: &Path,
    output_root: &Path,
) -> Result<RunItem, String> {
    let Some(output) = mapping.output.as_deref() else {
        return Err("saved plan mapping has no output".into());
    };
    let source_path = source_root.join(Path::new(&source.source));
    let written = output_root.join(Path::new(output));
    let target_root = if target.out.is_empty() {
        output_root.to_path_buf()
    } else {
        output_root.join(Path::new(&target.out))
    };
    let (format, quality, max_edge) = target.recipe.settings()?;
    crate::avif::set_speed(target.recipe.avif_speed);
    let stamp = crate::manifest::Stamp::with_speed(
        format,
        quality,
        max_edge,
        Some(target.recipe.avif_speed),
    );
    let recording = crate::convert::Recording::for_source(source_root, &target_root, &stamp, None);
    let expected = crate::manifest::SourceIdentity {
        bytes: source.bytes,
        hash: source.sha256.clone(),
    };
    crate::convert::convert_to_expected(
        &target_root,
        &source_path,
        &written,
        Some(&recording),
        format,
        quality,
        max_edge,
        &expected,
    )
    .map_err(failure_text)?;
    verified_item(plan, target, source, mapping, source_root, output_root)
}

fn reconcile_items(
    plan: &Plan,
    source_root: &Path,
    output_root: &Path,
    state: &mut RunState,
) -> Result<(), String> {
    for target in &plan.targets {
        for source in &plan.sources {
            let Some(index) = state.items.iter().position(|item| {
                item.item_id == source.item_id && item.target_id == target.target_id
            }) else {
                return Err("saved run state is missing a plan item".into());
            };
            if state.items[index].status == ItemStatus::Cancelled {
                continue;
            }
            let Some(mapping) = target
                .mappings
                .iter()
                .find(|mapping| mapping.item_id == source.item_id)
            else {
                return Err("saved plan is missing a target mapping".into());
            };
            match verified_item(plan, target, source, mapping, source_root, output_root) {
                Ok(receipt) => state.items[index] = receipt,
                Err(error) if state.items[index].status == ItemStatus::Running => {
                    let output_exists = mapping.output.as_deref().is_some_and(|output| {
                        output_root
                            .join(Path::new(output))
                            .symlink_metadata()
                            .is_ok()
                    });
                    if output_exists {
                        state.items[index].status = ItemStatus::Failed;
                        state.items[index].error = Some(error);
                    } else {
                        state.items[index].status = ItemStatus::Unstarted;
                        state.items[index].error = None;
                    }
                }
                Err(error) if state.items[index].status == ItemStatus::Written => {
                    state.items[index].status = ItemStatus::Failed;
                    state.items[index].error = Some(error);
                }
                Err(_) => {}
            }
        }
    }
    Ok(())
}

fn verified_item(
    _plan: &Plan,
    target: &PlanTarget,
    source: &PlanSource,
    mapping: &PlanMapping,
    source_root: &Path,
    output_root: &Path,
) -> Result<RunItem, String> {
    let Some(output) = mapping.output.as_deref() else {
        return Err(mapping
            .error
            .clone()
            .unwrap_or_else(|| "saved plan mapping is refused".into()));
    };
    let output_path = output_root.join(Path::new(output));
    let Some(identity) = crate::manifest::file_identity(&output_path) else {
        return Err("installed output is missing or unreadable".into());
    };
    let source_path = source_root.join(Path::new(&source.source));
    let current = crate::manifest::SourceIdentity::from_path(&source_path)
        .map_err(|_| "the source changed or is unreadable since planning".to_string())?;
    if current.bytes != source.bytes || current.hash != source.sha256 {
        return Err("the source changed since planning".into());
    }
    let target_root = if target.out.is_empty() {
        output_root.to_path_buf()
    } else {
        output_root.join(Path::new(&target.out))
    };
    let output_relative = output_path
        .strip_prefix(&target_root)
        .map_err(|_| "planned output leaves its target namespace".to_string())?;
    let manifest = crate::manifest::load(&target_root);
    let record = manifest
        .latest(Path::new(&source.source), output_relative)
        .ok_or_else(|| "installed output has no matching manifest receipt".to_string())?;
    if record.output_hash.as_deref() != Some(identity.hash.as_str())
        || record.source_hash.as_deref() != Some(source.sha256.as_str())
        || record.recipe.as_deref() != Some(target.recipe.fingerprint.as_str())
        || !record.installed(&output_path)
    {
        return Err("installed output or manifest receipt changed".into());
    }
    let (width, height) = scan_dimensions(&output_path)?;
    Ok(RunItem {
        item_id: source.item_id.clone(),
        target_id: target.target_id.clone(),
        source: source.source.clone(),
        status: ItemStatus::Written,
        output: Some(output.to_string()),
        output_hash: Some(identity.hash),
        output_bytes: Some(identity.bytes),
        width: Some(width),
        height: Some(height),
        recipe: target.recipe.fingerprint.clone(),
        engine_revision: target.recipe.engine_revision.clone(),
        checks: vec![
            Check {
                name: "manifest".into(),
                status: CheckStatus::Completed,
                reason: None,
            },
            Check {
                name: "output_hash".into(),
                status: CheckStatus::Completed,
                reason: None,
            },
        ],
        error: None,
    })
}

fn scan_dimensions(path: &Path) -> Result<(u32, u32), String> {
    crate::scan::probe(path)
        .map(|entry| (entry.width, entry.height))
        .ok_or_else(|| "installed output cannot be measured".into())
}

fn validate_mappings(plan: &Plan, source_root: &Path, output_root: &Path) -> Result<(), String> {
    let source_paths: Vec<_> = plan
        .sources
        .iter()
        .map(|source| source_root.join(Path::new(&source.source)))
        .collect();
    for target in &plan.targets {
        let target_root = if target.out.is_empty() {
            output_root.to_path_buf()
        } else {
            output_root.join(Path::new(&target.out))
        };
        let recorded = crate::manifest::load(&target_root);
        let destination = crate::convert::Destination {
            out_dir: &target_root,
            backups: None,
            manifest: &recorded,
        };
        let (format, _, _) = target.recipe.settings()?;
        let current = crate::convert::plan_outputs(
            source_root,
            &source_paths,
            &source_paths,
            &destination,
            format,
        );
        for (source, actual) in plan.sources.iter().zip(current) {
            let expected = target
                .mappings
                .iter()
                .find(|mapping| mapping.item_id == source.item_id)
                .ok_or_else(|| "saved plan is missing a target mapping".to_string())?;
            let actual = match actual {
                Ok(path) => PlanMapping {
                    item_id: source.item_id.clone(),
                    output: Some(relative_text(output_root, &path)?),
                    status: MappingStatus::Planned,
                    error: None,
                },
                Err(error) => PlanMapping {
                    item_id: source.item_id.clone(),
                    output: None,
                    status: MappingStatus::Refused,
                    error: Some(failure_text(error)),
                },
            };
            if actual.output != expected.output
                || actual.status != expected.status
                || actual.error != expected.error
            {
                return Err(format!(
                    "target {:?} collision mapping changed for source {:?}; create a new plan",
                    target.target_id, source.source
                ));
            }
        }
    }
    Ok(())
}

fn result(plan: &Plan, state: &RunState, command: &'static str) -> RunResult {
    let counts = RunCounts {
        total: state.items.len(),
        written: state
            .items
            .iter()
            .filter(|item| item.status == ItemStatus::Written)
            .count(),
        failed: state
            .items
            .iter()
            .filter(|item| item.status == ItemStatus::Failed)
            .count(),
        unstarted: state
            .items
            .iter()
            .filter(|item| {
                item.status == ItemStatus::Unstarted || item.status == ItemStatus::Running
            })
            .count(),
        cancelled: state
            .items
            .iter()
            .filter(|item| item.status == ItemStatus::Cancelled)
            .count(),
    };
    let incomplete = counts.failed + counts.unstarted + counts.cancelled;
    RunResult {
        report: RunReport {
            schema_version: SCHEMA_VERSION,
            command,
            status: if incomplete == 0 {
                "complete"
            } else {
                "partial"
            },
            plan_id: plan.plan_id.clone(),
            digest: plan.digest.clone(),
            run_id: state.run_id.clone(),
            counts,
            items: state.items.clone(),
        },
        exit_code: i32::from(incomplete != 0),
    }
}

fn state_path(plan_path: &Path, source_root: &Path, output_root: &Path) -> Result<PathBuf, String> {
    let name = plan_path
        .file_name()
        .ok_or_else(|| "saved plan path has no file name".to_string())?
        .to_string_lossy();
    let binding =
        hex_digest(format!("{}|{}", root_text(source_root), root_text(output_root)).as_bytes());
    Ok(plan_path.with_file_name(format!("{name}.run-{}.json", &binding[..16])))
}

fn root_text(path: &Path) -> String {
    path.to_string_lossy().into_owned()
}

fn load_state(path: &Path) -> Result<RunState, String> {
    let bytes = read_bounded(path, MAX_STATE_BYTES, "saved run state")?;
    let state: RunState = serde_json::from_slice(&bytes)
        .map_err(|error| format!("saved run state does not parse: {error}"))?;
    if state.schema_version != SCHEMA_VERSION {
        return Err(format!(
            "unsupported saved run state schema {}",
            state.schema_version
        ));
    }
    Ok(state)
}

fn save_state(path: &Path, state: &RunState) -> Result<(), String> {
    let bytes = serde_json::to_vec_pretty(state).map_err(|error| error.to_string())?;
    if bytes.len() as u64 > MAX_STATE_BYTES {
        return Err(format!(
            "saved run state exceeds the {MAX_STATE_BYTES}-byte limit"
        ));
    }
    let mut temporary = path.as_os_str().to_owned();
    temporary.push(format!(".{}.part", std::process::id()));
    let temporary = PathBuf::from(temporary);
    let mut file = std::fs::File::create(&temporary)
        .map_err(|error| format!("could not stage saved run state: {error}"))?;
    if let Err(error) = file
        .write_all(&bytes)
        .and_then(|()| file.sync_all())
        .and_then(|()| crate::settings::replace_file(&temporary, path))
    {
        let _ = std::fs::remove_file(&temporary);
        return Err(format!("could not save run state: {error}"));
    }
    Ok(())
}

fn read_bounded(path: &Path, limit: u64, what: &str) -> Result<Vec<u8>, String> {
    let metadata = std::fs::symlink_metadata(path)
        .map_err(|error| format!("{what} {} cannot be inspected: {error}", path.display()))?;
    if !metadata.is_file() {
        return Err(format!("{what} {} is not a file", path.display()));
    }
    if metadata.len() > limit {
        return Err(format!("{what} exceeds the {limit}-byte limit"));
    }
    let file = std::fs::File::open(path)
        .map_err(|error| format!("{what} {} cannot be read: {error}", path.display()))?;
    let mut bytes = Vec::new();
    file.take(limit + 1)
        .read_to_end(&mut bytes)
        .map_err(|error| format!("{what} {} cannot be read: {error}", path.display()))?;
    if bytes.len() as u64 > limit {
        return Err(format!("{what} exceeds the {limit}-byte limit"));
    }
    Ok(bytes)
}
