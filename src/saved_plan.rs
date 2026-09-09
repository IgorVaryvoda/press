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
/// A portable path long enough for any real tree and short enough that a plan
/// cannot carry a path bomb into a join.
const MAX_PATH_CHARS: usize = 1024;
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
    /// What stood at that destination when this plan was reviewed. It is part
    /// of the sealed digest, so the state a person consented to travels with
    /// the plan and the writer can refuse anything else that has arrived since.
    pub destination: Option<crate::convert::ReviewedDestination>,
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
    /// The optional requirements snapshot this plan was reviewed against. It is
    /// identity and applicability only: the rules stay in their own file, which
    /// execution must supply again and which must still be the same document.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub requirements: Option<PlanRequirements>,
}

/// Which requirements snapshot a plan was reviewed against, and when it applies.
/// A plan never carries the rules themselves, so a copied plan cannot smuggle a
/// weakened set of checks past the person who reviewed the original.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct PlanRequirements {
    pub id: String,
    pub revision: u32,
    pub provenance: crate::requirements::Provenance,
    pub last_verified: String,
    pub effective_from: Option<String>,
    pub effective_until: Option<String>,
    pub checker_version: u32,
    /// SHA-256 of the normalized parsed snapshot. Execution refuses a different
    /// document under the same id and revision.
    pub digest: String,
}

impl PlanRequirements {
    pub fn from_snapshot(snapshot: &crate::requirements::RequirementsSnapshot) -> Self {
        Self {
            id: snapshot.id.clone(),
            revision: snapshot.revision,
            provenance: snapshot.provenance.clone(),
            last_verified: snapshot.last_verified.clone(),
            effective_from: snapshot.effective_from.clone(),
            effective_until: snapshot.effective_until.clone(),
            checker_version: snapshot.checker_version,
            digest: snapshot_digest(snapshot),
        }
    }

    /// The supplied rules must be the reviewed document, not another one that
    /// happens to share an id.
    pub fn matches(
        &self,
        snapshot: &crate::requirements::RequirementsSnapshot,
    ) -> Result<(), String> {
        let supplied = Self::from_snapshot(snapshot);
        if supplied == *self {
            return Ok(());
        }
        Err(format!(
            "the supplied requirements are not the snapshot this plan was reviewed against ({} rev {})",
            self.id, self.revision
        ))
    }
}

fn snapshot_digest(snapshot: &crate::requirements::RequirementsSnapshot) -> String {
    serde_json::to_vec(snapshot)
        .map(|bytes| hex_digest(&bytes))
        .unwrap_or_default()
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
            portable_path(&source.source, "source")?;
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
                portable_path(&target.out, "target output")?;
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
                    mapping.destination.as_ref(),
                ) {
                    (MappingStatus::Planned, Some(output), None, Some(destination)) => {
                        portable_path(output, "planned output")?;
                        // A pinned snapshot is held to the same shape as a
                        // source identity: a real SHA-256 over a file that has
                        // bytes. Consent to replace somebody's file is not a
                        // place to start accepting data that cannot be true.
                        if let crate::convert::ReviewedDestination::Own { sha256, bytes } =
                            destination
                            && (!is_hex(sha256, 64) || *bytes == 0)
                        {
                            return Err(format!(
                                "target {:?} pins {output:?} to an invalid reviewed output \
                                 snapshot",
                                target.target_id
                            ));
                        }
                        // A target's namespace is its own. A mapping that names
                        // a file outside it would let one target write through
                        // another's folder.
                        if !target.out.is_empty()
                            && !output
                                .strip_prefix(target.out.as_str())
                                .is_some_and(|rest| rest.starts_with('/') && rest.len() > 1)
                        {
                            return Err(format!(
                                "target {:?} maps {output:?} outside its own namespace {:?}",
                                target.target_id, target.out
                            ));
                        }
                    }
                    (MappingStatus::Refused, None, Some(error), None) if !error.is_empty() => {}
                    _ => {
                        return Err(format!(
                            "target {:?} has an invalid mapping status, output or reviewed \
                             destination",
                            target.target_id
                        ));
                    }
                }
            }
        }
        self.validate_layout()?;
        if self.digest != self.compute_digest()? {
            return Err("saved plan digest does not match its normalized contents".into());
        }
        Ok(())
    }

    /// Namespace and destination conflicts, checked on the plan's own portable
    /// data before any root is bound and before any effect.
    ///
    /// Comparison folds case because two spellings of one name are one file
    /// wherever the filesystem aliases case, and `plan_outputs` already renames
    /// around a case-folded collision. Refusing both spellings is the answer
    /// that is safe on every supported filesystem.
    fn validate_layout(&self) -> Result<(), String> {
        let namespace = |out: &str| -> Vec<String> {
            if out.is_empty() {
                Vec::new()
            } else {
                out.split('/').map(str::to_lowercase).collect()
            }
        };
        for (index, target) in self.targets.iter().enumerate() {
            let mine = namespace(&target.out);
            for other in &self.targets[index + 1..] {
                let theirs = namespace(&other.out);
                let shared = mine.len().min(theirs.len());
                if mine[..shared] == theirs[..shared] {
                    return Err(format!(
                        "targets {:?} and {:?} share an output namespace; a target folder is                          not authority over another target's files",
                        target.target_id, other.target_id
                    ));
                }
            }
        }
        let mut taken: std::collections::HashMap<String, &str> = std::collections::HashMap::new();
        for target in &self.targets {
            for mapping in &target.mappings {
                let Some(output) = mapping.output.as_deref() else {
                    continue;
                };
                if let Some(owner) = taken.insert(output.to_lowercase(), &target.target_id) {
                    return Err(format!(
                        "targets {owner:?} and {:?} both write {output:?}",
                        target.target_id
                    ));
                }
            }
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
            #[serde(skip_serializing_if = "Option::is_none")]
            requirements: &'a Option<PlanRequirements>,
        }
        let view = DigestView {
            schema_version: self.schema_version,
            sources: &self.sources,
            targets: &self.targets,
            checks: &self.checks,
            write_scope: &self.write_scope,
            requirements: &self.requirements,
        };
        let bytes = serde_json::to_vec(&view).map_err(|error| error.to_string())?;
        Ok(hex_digest(&bytes))
    }
}

#[allow(clippy::too_many_arguments)]
pub fn build(
    root: &Path,
    output_root: &Path,
    entries: &[crate::scan::Entry],
    scan_errors: usize,
    mut targets: Vec<TargetInput>,
    requirements: Option<&crate::requirements::RequirementsSnapshot>,
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
        // Dimensions, container and encoded depth are measured from the bytes
        // that were just hashed. Reading the file again for its header would
        // let a file that changes in between describe itself as two different
        // images and record facts nothing ever checked together.
        let probed = crate::scan::probe_bytes(&bytes).map_err(|refusal| match refusal {
            crate::scan::ProbeRefusal::TooLarge => {
                format!("source {source:?} exceeds the bounded input size")
            }
            crate::scan::ProbeRefusal::UnsupportedPreparation => {
                format!("source {source:?} carries a preparation Press cannot apply to its pixels")
            }
            crate::scan::ProbeRefusal::Unreadable => {
                format!("source {source:?} could not be measured from its snapshot")
            }
        })?;
        if probed.width != entry.width
            || probed.height != entry.height
            || probed.format != entry.format
        {
            return Err(format!(
                "source {source:?} changed while the plan was being created; create a new plan"
            ));
        }
        let depth = match probed.depth_bits {
            Some(depth) => Check {
                name: "encoded_depth".into(),
                status: CheckStatus::Completed,
                reason: Some(format!("{depth}-bit samples in the hashed snapshot")),
            },
            None => Check {
                name: "encoded_depth".into(),
                status: CheckStatus::Deferred,
                reason: Some(
                    "this container does not state a sample depth before its pixels decode".into(),
                ),
            },
        };
        sources.push(PlanSource {
            item_id: hex_digest(format!("item|{source}").as_bytes()),
            source,
            bytes: identity.bytes,
            sha256: identity.hash,
            width: probed.width,
            height: probed.height,
            format: crate::scan::format_name(probed.format)
                .to_ascii_lowercase()
                .replace(' ', "_"),
            checks: vec![
                Check {
                    name: "source_snapshot".into(),
                    status: CheckStatus::Completed,
                    reason: None,
                },
                Check {
                    name: "encoded_metadata".into(),
                    status: CheckStatus::Completed,
                    reason: Some("read from the same snapshot the identity hashes".into()),
                },
                depth,
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
            portable_path(&target.out, "target output")?;
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
                    destination: Some(reviewed_destination(&recorded, &target_root, source, &path)),
                    status: MappingStatus::Planned,
                    error: None,
                }),
                Err(error) => Ok(PlanMapping {
                    item_id: source.item_id.clone(),
                    output: None,
                    destination: None,
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
    let mut checks = vec![
        Check {
            name: "source_scan".into(),
            status: CheckStatus::Completed,
            reason: None,
        },
        Check {
            name: "output_layout".into(),
            status: CheckStatus::Completed,
            reason: Some(
                "target namespaces and planned destinations are distinct and confined".into(),
            ),
        },
        Check {
            name: "output_ownership".into(),
            status: CheckStatus::Deferred,
            reason: Some("execution rechecks the explicit output boundary and ownership".into()),
        },
    ];
    checks.push(match requirements {
        Some(snapshot) => Check {
            name: "requirements".into(),
            status: CheckStatus::Deferred,
            reason: Some(format!(
                "{} rev {} is checked against each actual output at execution",
                snapshot.id, snapshot.revision
            )),
        },
        None => Check {
            name: "requirements".into(),
            status: CheckStatus::Deferred,
            reason: Some("no requirements snapshot was supplied for this plan".into()),
        },
    });
    let mut plan = Plan {
        schema_version: SCHEMA_VERSION,
        plan_id: String::new(),
        digest: String::new(),
        sources,
        targets: plan_targets,
        checks,
        write_scope: "outputs_only".into(),
        requirements: requirements.map(PlanRequirements::from_snapshot),
    };
    plan.digest = plan.compute_digest()?;
    plan.plan_id = format!("plan-{}", &plan.digest[..16]);
    plan.validate()?;
    Ok(plan)
}

/// What the destination held when this plan was reviewed.
///
/// Two states are consent and no others: a free name, and exactly this source's
/// own earlier output, still the bytes this folder's manifest credits to the
/// source bytes the plan just hashed. Another source's output, a file no
/// manifest accounts for, a folder and a link are all somebody else's, and the
/// plan records that rather than leave the writer to infer permission from a
/// timestamp or a matching recipe.
fn reviewed_destination(
    manifest: &crate::manifest::Manifest,
    target_root: &Path,
    source: &PlanSource,
    output: &Path,
) -> crate::convert::ReviewedDestination {
    use crate::convert::ReviewedDestination;
    match output.symlink_metadata() {
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => ReviewedDestination::Absent,
        Err(_) => ReviewedDestination::Foreign,
        Ok(metadata) if !metadata.is_file() => ReviewedDestination::Foreign,
        Ok(_) => {
            let (Some(identity), Ok(relative)) = (
                crate::manifest::file_identity(output),
                output.strip_prefix(target_root),
            ) else {
                return ReviewedDestination::Foreign;
            };
            match manifest.latest(Path::new(&source.source), relative) {
                Some(record)
                    if record.source_hash.as_deref() == Some(source.sha256.as_str())
                        && record.source_bytes == source.bytes
                        && record.output_hash.as_deref() == Some(identity.hash.as_str())
                        && record.output_bytes == identity.bytes =>
                {
                    ReviewedDestination::Own {
                        sha256: identity.hash,
                        bytes: identity.bytes,
                    }
                }
                _ => ReviewedDestination::Foreign,
            }
        }
    }
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
    let text = path
        .components()
        .map(|component| {
            component
                .as_os_str()
                .to_str()
                .map(str::to_owned)
                .ok_or_else(|| format!("{} contains a non-UTF-8 path component", path.display()))
        })
        .collect::<Result<Vec<_>, _>>()
        .map(|components| components.join("/"))?;
    portable_path(&text, "relative")?;
    Ok(text)
}

/// The portable spelling rule for every path a plan carries between machines.
///
/// It is the rule saved jobs already export by, reused rather than restated: a
/// literal backslash, a drive or alternate-stream marker, a reserved Windows
/// device, a control character, a trailing dot or space, and any non-UTF-8
/// component are refused here. A plan that names one of those is refused, not
/// silently retargeted somewhere else on the machine that opens it. Native
/// source and output bindings are separate and keep their own rules.
fn portable_path(text: &str, what: &str) -> Result<(), String> {
    if text.is_empty() {
        return Err(format!("{what} path is empty"));
    }
    if text.chars().count() > MAX_PATH_CHARS {
        return Err(format!(
            "{what} path is longer than {MAX_PATH_CHARS} characters"
        ));
    }
    for part in text.split('/') {
        if part.contains('\\') {
            return Err(format!(
                "{what} path {text:?} contains a literal backslash, which is a separator on Windows"
            ));
        }
        if !crate::job::portable_component(part) {
            return Err(format!(
                "{what} path {text:?} contains the unsafe portable component {part:?}"
            ));
        }
    }
    ensure_relative(Path::new(text), what)
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
    /// The requirement outcome for this actual output, when the plan was
    /// reviewed against a snapshot. Inapplicable and unsupported checks stay
    /// visible here; neither can become an approval.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub requirements: Option<ItemRequirements>,
    pub error: Option<String>,
}

/// One output's requirement outcome, kept as the checker produced it.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ItemRequirements {
    pub id: String,
    pub revision: u32,
    pub checked_at: String,
    /// Whether the snapshot applies on `checked_at`. A snapshot outside its
    /// interval leaves every check NotApplicable and cannot pass.
    pub effective: bool,
    pub all_required_pass: bool,
    pub checks: Vec<crate::requirements::CheckResult>,
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
    #[serde(skip_serializing_if = "Option::is_none")]
    pub requirements: Option<PlanRequirements>,
    pub items: Vec<RunItem>,
}

#[derive(Clone, Debug, Serialize)]
pub struct RunCounts {
    pub total: usize,
    pub written: usize,
    pub failed: usize,
    pub unstarted: usize,
    pub cancelled: usize,
    /// Written outputs whose required checks did not pass. A file that exists
    /// is not a file that meets the requirements it was reviewed against.
    pub requirements_failed: usize,
}

pub struct RunResult {
    pub report: RunReport,
    pub exit_code: i32,
}

/// Everything one command binds before it may cause an effect: the plan, the
/// two explicit roots, the requirement rules it was reviewed against, the
/// run lock and the validated run state.
///
/// Load, validation, effects and persistence all happen inside one lock, held
/// for the whole command. Splitting them let a second process read a state this
/// one was still writing and repeat work it had already done.
struct Bound<'a> {
    plan: Plan,
    requirements: Option<&'a crate::requirements::RequirementsSnapshot>,
    state_path: PathBuf,
    state: RunState,
    _lock: RunLock,
}

fn bind<'a>(
    plan_path: &Path,
    source_root: &Path,
    output_root: &Path,
    requirements: Option<&'a crate::requirements::RequirementsSnapshot>,
    state_must_exist: bool,
) -> Result<Bound<'a>, String> {
    let plan = load(plan_path)?;
    match (plan.requirements.as_ref(), requirements) {
        (Some(recorded), Some(snapshot)) => recorded.matches(snapshot)?,
        (Some(recorded), None) => {
            return Err(format!(
                "this plan was reviewed against requirements {} rev {}; supply that file again \
                 with --requirements-file",
                recorded.id, recorded.revision
            ));
        }
        (None, Some(_)) => {
            return Err(
                "this plan records no requirements snapshot; create a new plan to review one"
                    .into(),
            );
        }
        (None, None) => {}
    }
    validate_bindings(source_root, output_root)?;
    let state_path = state_path(plan_path, source_root, output_root)?;
    let lock = RunLock::acquire(&state_path)?;
    // Only inside the lock: the collision mapping and the layout are facts about
    // the folders, and another run could be changing them until this point.
    validate_mappings(&plan, source_root, output_root)?;
    validate_layout(&plan, source_root, output_root)?;
    let exists = match state_path.symlink_metadata() {
        Ok(metadata) if metadata.is_file() => true,
        Ok(_) => {
            return Err(format!(
                "saved run state {} is not a regular file",
                state_path.display()
            ));
        }
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => false,
        Err(error) => {
            return Err(format!(
                "saved run state {} cannot be inspected: {error}",
                state_path.display()
            ));
        }
    };
    if state_must_exist && !exists {
        return Err("no saved run state exists for this plan and explicit root binding".into());
    }
    let state = if exists {
        load_state(&state_path, &plan, source_root, output_root)?
    } else {
        new_state(&plan, source_root, output_root)
    };
    Ok(Bound {
        plan,
        requirements,
        state_path,
        state,
        _lock: lock,
    })
}

pub fn execute(
    plan_path: &Path,
    source_root: &Path,
    output_root: &Path,
    mode: ExecutionMode,
    requirements: Option<&crate::requirements::RequirementsSnapshot>,
) -> Result<RunResult, String> {
    let Bound {
        plan,
        requirements,
        state_path,
        mut state,
        _lock,
    } = bind(plan_path, source_root, output_root, requirements, false)?;
    // Whatever the last run recorded, believe the folder. A receipt is repaired
    // from manifest and hash evidence before any item is chosen for work, so an
    // invented success cannot skip an encode and a lost receipt cannot repeat one.
    reconcile_items(&plan, source_root, output_root, requirements, &mut state)?;
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
                clear_receipt(&mut state.items[index]);
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
                clear_receipt(&mut state.items[index]);
                save_state(&state_path, &state)?;
                continue;
            }
            // This is the state a killed run leaves behind, so it is written as
            // what is true at that moment: work in progress, and no receipt.
            state.items[index].status = ItemStatus::Running;
            state.items[index].error = None;
            clear_receipt(&mut state.items[index]);
            save_state(&state_path, &state)?;
            match execute_item(
                &plan,
                target,
                source,
                mapping,
                source_root,
                output_root,
                requirements,
            ) {
                Ok(receipt) => state.items[index] = receipt,
                Err(error) => {
                    state.items[index].status = ItemStatus::Failed;
                    state.items[index].error = Some(error);
                    clear_receipt(&mut state.items[index]);
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
    requirements: Option<&crate::requirements::RequirementsSnapshot>,
) -> Result<RunResult, String> {
    let Bound {
        plan,
        requirements,
        state_path,
        mut state,
        _lock,
    } = bind(plan_path, source_root, output_root, requirements, true)?;
    reconcile_items(&plan, source_root, output_root, requirements, &mut state)?;
    save_state(&state_path, &state)?;
    Ok(result(&plan, &state, "reconcile"))
}

/// One advisory lock covering a plan bound to one pair of explicit roots.
///
/// `File::try_lock` releases when the process exits, however it exits, so a
/// killed run leaves its plan restartable instead of stranded. The lock file
/// stays behind as a stable empty inode, which is harmless.
struct RunLock {
    _file: std::fs::File,
}

impl RunLock {
    fn acquire(state_path: &Path) -> Result<Self, String> {
        let mut path = state_path.as_os_str().to_owned();
        path.push(".lock");
        let path = PathBuf::from(path);
        // A symlink here would redirect the lock and leave two runs believing
        // they each held it, or point the open at something else entirely.
        if path
            .symlink_metadata()
            .is_ok_and(|metadata| !metadata.is_file())
        {
            return Err(format!(
                "saved run lock {} is not a regular file",
                path.display()
            ));
        }
        let file = std::fs::OpenOptions::new()
            .read(true)
            .write(true)
            .create(true)
            .truncate(false)
            .open(&path)
            .map_err(|error| format!("saved run lock {} failed: {error}", path.display()))?;
        match file.try_lock() {
            Ok(()) => Ok(Self { _file: file }),
            Err(std::fs::TryLockError::WouldBlock) => {
                Err("another Press command is running this plan against the same roots".into())
            }
            Err(std::fs::TryLockError::Error(error)) => {
                Err(format!("saved run lock failed: {error}"))
            }
        }
    }
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

/// Where a destination really lands: its parent resolved through whatever
/// aliases lead there, with the final name left unresolved.
///
/// Resolving the parent is what makes two target folders that are links to one
/// directory compare equal. Leaving the last component alone keeps a symlink
/// sitting on a destination an item-level refusal at the writer boundary rather
/// than a whole-run refusal here.
fn resolved_destination(path: &Path) -> Result<PathBuf, String> {
    let parent = path
        .parent()
        .ok_or_else(|| format!("{} has no parent folder", path.display()))?;
    let name = path
        .file_name()
        .ok_or_else(|| format!("{} has no file name", path.display()))?;
    let parent = crate::scan::canonical_boundary(parent)
        .map_err(|error| format!("{} cannot be resolved: {error}", parent.display()))?;
    Ok(parent.join(name))
}

/// One case-folded spelling of a resolved path, matching the rule conversion
/// already uses to decide that two names are one file.
fn destination_key(path: &Path) -> String {
    crate::convert::path_key(path)
}

/// Namespace, destination and source conflicts against the two bound roots,
/// checked before any effect.
///
/// The lexical half is in `Plan::validate_layout`; this half is what only the
/// filesystem can answer: two target folders that are aliases of one directory,
/// a target folder that a link carries outside the proven output root, and a
/// planned destination that is really one of the plan's own sources. A target
/// folder is a namespace, never authority over anything else.
fn validate_layout(plan: &Plan, source_root: &Path, output_root: &Path) -> Result<(), String> {
    let output_boundary = crate::scan::canonical_boundary(output_root)
        .map_err(|error| format!("output root cannot be resolved: {error}"))?;
    let mut roots: Vec<(&str, PathBuf)> = Vec::with_capacity(plan.targets.len());
    for target in &plan.targets {
        let target_root = target_root(target, output_root);
        let resolved = crate::scan::canonical_boundary(&target_root).map_err(|error| {
            format!(
                "target {:?} folder is unavailable: {error}",
                target.target_id
            )
        })?;
        if !resolved.starts_with(&output_boundary) {
            return Err(format!(
                "target {:?} resolves outside the explicit output root",
                target.target_id
            ));
        }
        for (other, existing) in &roots {
            if resolved.starts_with(existing) || existing.starts_with(&resolved) {
                return Err(format!(
                    "targets {other:?} and {:?} resolve to the same output folder",
                    target.target_id
                ));
            }
        }
        roots.push((&target.target_id, resolved));
    }

    let mut sources: std::collections::HashMap<String, &str> =
        std::collections::HashMap::with_capacity(plan.sources.len());
    for source in &plan.sources {
        let path = resolved_destination(&source_root.join(Path::new(&source.source)))?;
        sources.insert(destination_key(&path), &source.source);
    }
    let mut taken: std::collections::HashMap<String, (&str, &str)> =
        std::collections::HashMap::new();
    for target in &plan.targets {
        for mapping in &target.mappings {
            let Some(output) = mapping.output.as_deref() else {
                continue;
            };
            let path = output_root.join(Path::new(output));
            let resolved = resolved_destination(&path)?;
            let key = destination_key(&resolved);
            if let Some(source) = sources.get(key.as_str()) {
                return Err(format!(
                    "target {:?} would write {output:?} onto the source {source:?}",
                    target.target_id
                ));
            }
            if let Some((owner, other)) = taken.insert(key, (&target.target_id, output)) {
                return Err(format!(
                    "targets {owner:?} and {:?} both write the same file ({other:?} and {output:?})",
                    target.target_id
                ));
            }
        }
    }
    Ok(())
}

fn target_root(target: &PlanTarget, output_root: &Path) -> PathBuf {
    if target.out.is_empty() {
        output_root.to_path_buf()
    } else {
        output_root.join(Path::new(&target.out))
    }
}

/// A source path is only this plan's source when it resolves inside the root
/// the person explicitly bound. A link with the planned bytes behind it is a
/// different file, and matching a hash is not being the file that was reviewed.
fn confined_source(source_root: &Path, relative: &str) -> Result<PathBuf, String> {
    let path = source_root.join(Path::new(relative));
    let boundary = crate::scan::canonical_boundary(source_root)
        .map_err(|error| format!("source root cannot be resolved: {error}"))?;
    let resolved = crate::scan::canonical_boundary(&path)
        .map_err(|_| format!("source {relative:?} is missing or unreadable"))?;
    if !resolved.starts_with(&boundary) {
        return Err(format!(
            "source {relative:?} resolves outside the explicit source root"
        ));
    }
    Ok(path)
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
                requirements: None,
                error,
            });
        }
    }
    RunState {
        schema_version: SCHEMA_VERSION,
        plan_digest: plan.digest.clone(),
        run_id: run_id(source_root, output_root),
        source_root: root_text(source_root),
        output_root: root_text(output_root),
        items,
    }
}

/// Drop a receipt that no longer describes an installed output.
///
/// A hash, a size, dimensions, the checks that measured them and the
/// requirement outcome are all statements about a file this state can no longer
/// vouch for. Carrying them into an unstarted, running, cancelled or failed
/// item leaves a state that describes two different things at once — and one
/// that `load_state` refuses, so an interrupted retry could never be resumed.
/// The named failure is the item's own field and stays.
fn clear_receipt(item: &mut RunItem) {
    item.output_hash = None;
    item.output_bytes = None;
    item.width = None;
    item.height = None;
    item.checks = Vec::new();
    item.requirements = None;
}

fn run_id(source_root: &Path, output_root: &Path) -> String {
    format!(
        "run-{}",
        &hex_digest(format!("{}|{}", root_text(source_root), root_text(output_root)).as_bytes())
            [..16]
    )
}

#[allow(clippy::too_many_arguments)]
fn execute_item(
    plan: &Plan,
    target: &PlanTarget,
    source: &PlanSource,
    mapping: &PlanMapping,
    source_root: &Path,
    output_root: &Path,
    requirements: Option<&crate::requirements::RequirementsSnapshot>,
) -> Result<RunItem, String> {
    let Some(output) = mapping.output.as_deref() else {
        return Err("saved plan mapping has no output".into());
    };
    let source_path = confined_source(source_root, &source.source)?;
    let written = output_root.join(Path::new(output));
    let target_root = target_root(target, output_root);
    let (format, quality, max_edge) = target.recipe.settings()?;
    crate::avif::set_speed(target.recipe.avif_speed);
    let stamp = crate::manifest::Stamp::with_speed(
        format,
        quality,
        max_edge,
        Some(target.recipe.avif_speed),
    );
    // The planned ownership contract, applied by the writer itself. A saved plan
    // pinned this destination when it was reviewed, so it does not inherit
    // ordinary conversion's permission to overwrite an unrecorded file merely
    // because that file is older than the source.
    let Some(reviewed) = mapping.destination.as_ref() else {
        return Err("saved plan mapping pins no reviewed destination".into());
    };
    let recording =
        crate::convert::Recording::for_planned(source_root, &target_root, &stamp, reviewed);
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
    let manifest = crate::manifest::load(&target_root);
    verified_item(
        plan,
        target,
        source,
        mapping,
        source_root,
        output_root,
        &manifest,
        requirements,
    )
}

fn reconcile_items(
    plan: &Plan,
    source_root: &Path,
    output_root: &Path,
    requirements: Option<&crate::requirements::RequirementsSnapshot>,
    state: &mut RunState,
) -> Result<(), String> {
    for target in &plan.targets {
        // One manifest read per target, not one per item: the file answers the
        // same question for every mapping under it.
        let manifest = crate::manifest::load(&target_root(target, output_root));
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
            match verified_item(
                plan,
                target,
                source,
                mapping,
                source_root,
                output_root,
                &manifest,
                requirements,
            ) {
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
                    clear_receipt(&mut state.items[index]);
                }
                Err(error) if state.items[index].status == ItemStatus::Written => {
                    state.items[index].status = ItemStatus::Failed;
                    state.items[index].error = Some(error);
                    clear_receipt(&mut state.items[index]);
                }
                Err(_) => {}
            }
        }
    }
    Ok(())
}

#[allow(clippy::too_many_arguments)]
fn verified_item(
    _plan: &Plan,
    target: &PlanTarget,
    source: &PlanSource,
    mapping: &PlanMapping,
    source_root: &Path,
    output_root: &Path,
    manifest: &crate::manifest::Manifest,
    requirements: Option<&crate::requirements::RequirementsSnapshot>,
) -> Result<RunItem, String> {
    let Some(output) = mapping.output.as_deref() else {
        return Err(mapping
            .error
            .clone()
            .unwrap_or_else(|| "saved plan mapping is refused".into()));
    };
    let output_path = output_root.join(Path::new(output));
    // One bounded read of the installed output answers every question about it.
    // Hashing it once and measuring it again later could combine an old hash
    // with new dimensions and report a file that never existed.
    let installed = output_snapshot(&output_path)?;
    let identity = crate::manifest::SourceIdentity::from_bytes(&installed);
    let probed = crate::scan::probe_bytes(&installed)
        .map_err(|_| "installed output cannot be measured".to_string())?;
    let source_path = confined_source(source_root, &source.source)?;
    let current = crate::manifest::SourceIdentity::from_path(&source_path)
        .map_err(|_| "the source changed or is unreadable since planning".to_string())?;
    if current.bytes != source.bytes || current.hash != source.sha256 {
        return Err("the source changed since planning".into());
    }
    let target_root = target_root(target, output_root);
    let output_relative = output_path
        .strip_prefix(&target_root)
        .map_err(|_| "planned output leaves its target namespace".to_string())?;
    let record = manifest
        .latest(Path::new(&source.source), output_relative)
        .ok_or_else(|| "installed output has no matching manifest receipt".to_string())?;
    // `installed` re-reads and re-hashes the file on disk. It has to agree with
    // the snapshot measured above, so a file that changes between the two reads
    // is refused rather than described by a mixture of both.
    if record.output_hash.as_deref() != Some(identity.hash.as_str())
        || record.output_bytes != identity.bytes
        || record.source_hash.as_deref() != Some(source.sha256.as_str())
        || record.recipe.as_deref() != Some(target.recipe.fingerprint.as_str())
        || !record.installed(&output_path)
    {
        return Err("installed output or manifest receipt changed".into());
    }
    let (width, height) = (probed.width, probed.height);
    let mut checks = vec![
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
    ];
    let requirements = requirements
        .map(|snapshot| check_requirements(output_root, output, target, snapshot, &identity))
        .transpose()?;
    checks.push(match &requirements {
        Some(outcome) => Check {
            name: "requirements".into(),
            status: if outcome.all_required_pass {
                CheckStatus::Completed
            } else {
                CheckStatus::Refused
            },
            reason: Some(format!(
                "{} rev {} checked on {}{}",
                outcome.id,
                outcome.revision,
                outcome.checked_at,
                if outcome.effective {
                    ""
                } else {
                    " (outside its effective interval)"
                }
            )),
        },
        None => Check {
            name: "requirements".into(),
            status: CheckStatus::Deferred,
            reason: Some("no requirements snapshot was supplied for this plan".into()),
        },
    });
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
        checks,
        requirements,
        error: None,
    })
}

/// Check one actual output through the shared requirements checker. The plan's
/// own recipe identity travels with the reference so a receipt says which
/// preparation produced the file it measured.
fn check_requirements(
    output_root: &Path,
    output: &str,
    target: &PlanTarget,
    snapshot: &crate::requirements::RequirementsSnapshot,
    installed: &crate::manifest::SourceIdentity,
) -> Result<ItemRequirements, String> {
    let reference = crate::requirements::OutputRef {
        source: None,
        output: PathBuf::from(output),
        processing: Some(crate::requirements::ProcessingIdentity {
            recipe_id: Some(target.target_id.clone()),
            recipe_revision: None,
            recipe_fingerprint: Some(target.recipe.fingerprint.clone()),
            processing_revision: crate::recipe::FINGERPRINT_REVISION,
        }),
    };
    let receipt = crate::requirements::inspect_outputs(output_root, &[reference], snapshot)?;
    let report = receipt
        .outputs
        .into_iter()
        .next()
        .ok_or_else(|| "the requirements checker reported no output".to_string())?;
    // The checker reads the output itself. Its findings are only this run's
    // evidence while they describe the same bytes this receipt identifies.
    if report.output_hash.as_deref() != Some(installed.hash.as_str()) {
        return Err("the installed output changed while it was being checked".into());
    }
    Ok(ItemRequirements {
        id: receipt.requirements.id,
        revision: receipt.requirements.revision,
        checked_at: receipt.checked_at,
        effective: receipt.effective,
        all_required_pass: receipt.all_required_pass,
        checks: report.checks,
    })
}

/// One bounded snapshot of an installed output, through the shared reader.
fn output_snapshot(path: &Path) -> Result<Vec<u8>, String> {
    let metadata = std::fs::symlink_metadata(path)
        .map_err(|_| "installed output is missing or unreadable".to_string())?;
    if !metadata.is_file() {
        return Err("installed output is not a regular file".into());
    }
    crate::scan::read_source_bytes(path)
        .map_err(|_| "installed output is missing or unreadable".to_string())
}

fn validate_mappings(plan: &Plan, source_root: &Path, output_root: &Path) -> Result<(), String> {
    let source_paths: Vec<_> = plan
        .sources
        .iter()
        .map(|source| source_root.join(Path::new(&source.source)))
        .collect();
    for target in &plan.targets {
        let target_root = target_root(target, output_root);
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
                    // The pinned destination is the plan's own reviewed fact and
                    // is rechecked at the writer boundary against the folder as
                    // it is when the bytes are about to move. Recomputing it
                    // here would only compare this moment with itself.
                    destination: expected.destination.clone(),
                    status: MappingStatus::Planned,
                    error: None,
                },
                Err(error) => PlanMapping {
                    item_id: source.item_id.clone(),
                    output: None,
                    destination: None,
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
        requirements_failed: state
            .items
            .iter()
            .filter(|item| {
                item.requirements
                    .as_ref()
                    .is_some_and(|outcome| !outcome.all_required_pass)
            })
            .count(),
    };
    // A requirement that did not pass is not a complete run. An inapplicable or
    // unsupported required check leaves `all_required_pass` false, so neither
    // can be reported as approval.
    let incomplete =
        counts.failed + counts.unstarted + counts.cancelled + counts.requirements_failed;
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
            requirements: plan.requirements.clone(),
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

/// Read a run state and prove it describes exactly this plan and this binding.
///
/// A schema check only proves the file is JSON of the right shape. Everything a
/// later command trusts — which items exist, which source and destination each
/// one names, which recipe produced it, and what a receipt has to carry to be a
/// receipt at all — is checked against the plan here. `reconcile_items` then
/// re-establishes each written item from manifest and hash evidence, so a
/// well-formed invented success still cannot skip an encode.
fn load_state(
    path: &Path,
    plan: &Plan,
    source_root: &Path,
    output_root: &Path,
) -> Result<RunState, String> {
    let bytes = read_bounded(path, MAX_STATE_BYTES, "saved run state")?;
    let state: RunState = serde_json::from_slice(&bytes)
        .map_err(|error| format!("saved run state does not parse: {error}"))?;
    if state.schema_version != SCHEMA_VERSION {
        return Err(format!(
            "unsupported saved run state schema {}",
            state.schema_version
        ));
    }
    if state.plan_digest != plan.digest
        || state.source_root != root_text(source_root)
        || state.output_root != root_text(output_root)
    {
        return Err(format!(
            "saved run state {} belongs to another plan or explicit root binding",
            path.display()
        ));
    }
    if state.run_id != run_id(source_root, output_root) {
        return Err("saved run state carries another run identity".into());
    }
    let expected = plan.sources.len() * plan.targets.len();
    if state.items.len() != expected {
        return Err(format!(
            "saved run state has {} items for a plan of {expected}",
            state.items.len()
        ));
    }
    let mut seen = std::collections::HashSet::with_capacity(expected);
    for item in &state.items {
        if !seen.insert((item.item_id.as_str(), item.target_id.as_str())) {
            return Err(format!(
                "saved run state repeats item {:?} for target {:?}",
                item.item_id, item.target_id
            ));
        }
    }
    for target in &plan.targets {
        for source in &plan.sources {
            let Some(item) = state
                .items
                .iter()
                .find(|item| item.item_id == source.item_id && item.target_id == target.target_id)
            else {
                return Err(format!(
                    "saved run state is missing source {:?} for target {:?}",
                    source.source, target.target_id
                ));
            };
            validate_item(item, plan, target, source)?;
        }
    }
    Ok(state)
}

/// One item's immutable identity and its receipt shape. Immutable fields come
/// from the plan, so a state file cannot retarget a source, a destination or a
/// recipe after the plan was reviewed.
fn validate_item(
    item: &RunItem,
    _plan: &Plan,
    target: &PlanTarget,
    source: &PlanSource,
) -> Result<(), String> {
    let named = format!("{:?} for target {:?}", source.source, target.target_id);
    let mapping = target
        .mappings
        .iter()
        .find(|mapping| mapping.item_id == source.item_id)
        .ok_or_else(|| "saved plan is missing a target mapping".to_string())?;
    if item.source != source.source
        || item.output.as_deref() != mapping.output.as_deref()
        || item.recipe != target.recipe.fingerprint
        || item.engine_revision != target.recipe.engine_revision
    {
        return Err(format!(
            "saved run state changed the fixed fields of {named}"
        ));
    }
    if mapping.status == MappingStatus::Refused
        && !matches!(item.status, ItemStatus::Failed | ItemStatus::Cancelled)
    {
        return Err(format!(
            "saved run state claims progress on the refused mapping {named}"
        ));
    }
    match item.status {
        ItemStatus::Written => {
            let complete = item.output.is_some()
                && item
                    .output_hash
                    .as_deref()
                    .is_some_and(|hash| is_hex(hash, 64))
                && item.output_bytes.is_some()
                && item.width.is_some_and(|width| width > 0)
                && item.height.is_some_and(|height| height > 0)
                && item.error.is_none();
            if !complete {
                return Err(format!(
                    "saved run state has an incomplete receipt for {named}"
                ));
            }
        }
        ItemStatus::Failed => {
            if !item.error.as_deref().is_some_and(|error| !error.is_empty()) {
                return Err(format!(
                    "saved run state records {named} as failed without naming the failure"
                ));
            }
        }
        ItemStatus::Unstarted | ItemStatus::Running | ItemStatus::Cancelled => {
            if item.output_hash.is_some()
                || item.output_bytes.is_some()
                || item.width.is_some()
                || item.height.is_some()
                || item.requirements.is_some()
            {
                return Err(format!(
                    "saved run state carries a receipt for {named}, which it reports as not written"
                ));
            }
        }
    }
    Ok(())
}

/// Stage beside the state file and rename onto it.
///
/// The staging name is created with `create_new`, never truncated open: a
/// predictable `.PID.part` that a `File::create` would happily reopen is a name
/// somebody else can put a file or a link on, and this process would then write
/// through it. An occupied name is stepped over rather than removed, because it
/// is not this run's file to delete. Every failure is returned, and the previous
/// complete state stays on disk until the rename succeeds.
fn save_state(path: &Path, state: &RunState) -> Result<(), String> {
    let bytes = serde_json::to_vec_pretty(state).map_err(|error| error.to_string())?;
    if bytes.len() as u64 > MAX_STATE_BYTES {
        return Err(format!(
            "saved run state exceeds the {MAX_STATE_BYTES}-byte limit"
        ));
    }
    if path
        .symlink_metadata()
        .is_ok_and(|metadata| !metadata.is_file())
    {
        return Err(format!(
            "saved run state {} is not a regular file",
            path.display()
        ));
    }
    static COUNTER: std::sync::atomic::AtomicU64 = std::sync::atomic::AtomicU64::new(0);
    let mut occupied = 0usize;
    for _ in 0..8 {
        let mut staged = path.as_os_str().to_owned();
        staged.push(format!(
            ".{}-{}.part",
            std::process::id(),
            COUNTER.fetch_add(1, std::sync::atomic::Ordering::Relaxed)
        ));
        let staged = PathBuf::from(staged);
        let mut file = match std::fs::OpenOptions::new()
            .write(true)
            .create_new(true)
            .open(&staged)
        {
            Ok(file) => file,
            Err(error) if error.kind() == std::io::ErrorKind::AlreadyExists => {
                occupied += 1;
                continue;
            }
            Err(error) => {
                return Err(format!(
                    "could not stage saved run state beside {}: {error}",
                    path.display()
                ));
            }
        };
        let written = file.write_all(&bytes).and_then(|()| file.sync_all());
        drop(file);
        if let Err(error) = written {
            let _ = std::fs::remove_file(&staged);
            return Err(format!("could not write saved run state: {error}"));
        }
        if let Err(error) = crate::settings::replace_file(&staged, path) {
            let _ = std::fs::remove_file(&staged);
            return Err(format!(
                "could not install saved run state {}: {error}",
                path.display()
            ));
        }
        return Ok(());
    }
    Err(format!(
        "could not stage saved run state beside {}: {occupied} staging names are occupied",
        path.display()
    ))
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

#[cfg(test)]
mod tests {
    use super::*;

    fn recipe() -> EffectiveRecipe {
        EffectiveRecipe::from_settings(Format::WebP, Quality::lossy(80.), MaxEdge::FULL, None)
    }

    fn item_id(name: &str) -> String {
        hex_digest(format!("item|{name}").as_bytes())
    }

    fn source(name: &str) -> PlanSource {
        PlanSource {
            item_id: item_id(name),
            source: name.into(),
            bytes: 12,
            sha256: hex_digest(name.as_bytes()),
            width: 8,
            height: 8,
            format: "png".into(),
            checks: Vec::new(),
        }
    }

    fn mapping(name: &str, output: &str) -> PlanMapping {
        PlanMapping {
            item_id: item_id(name),
            output: Some(output.into()),
            destination: Some(crate::convert::ReviewedDestination::Absent),
            status: MappingStatus::Planned,
            error: None,
        }
    }

    /// A plan with a correct digest, so validation fails on what the test
    /// changed rather than on the seal.
    fn sealed(sources: Vec<PlanSource>, targets: Vec<PlanTarget>) -> Plan {
        let mut plan = Plan {
            schema_version: SCHEMA_VERSION,
            plan_id: String::new(),
            digest: String::new(),
            sources,
            targets,
            checks: Vec::new(),
            write_scope: "outputs_only".into(),
            requirements: None,
        };
        plan.digest = plan.compute_digest().expect("the digest computes");
        plan.plan_id = format!("plan-{}", &plan.digest[..16]);
        plan
    }

    fn one_source_plan() -> Plan {
        sealed(
            vec![source("shot.png")],
            vec![PlanTarget {
                target_id: "default".into(),
                out: String::new(),
                recipe: recipe(),
                mappings: vec![mapping("shot.png", "shot.webp")],
            }],
        )
    }

    #[test]
    fn a_target_folder_is_not_authority_over_another_targets_files() {
        let plan = sealed(
            vec![source("shot.png")],
            vec![
                PlanTarget {
                    target_id: "web".into(),
                    out: "web".into(),
                    recipe: recipe(),
                    mappings: vec![mapping("shot.png", "web/shot.webp")],
                },
                PlanTarget {
                    target_id: "hero".into(),
                    out: "web/hero".into(),
                    recipe: recipe(),
                    mappings: vec![mapping("shot.png", "web/hero/shot.webp")],
                },
            ],
        );
        let error = plan.validate().expect_err("a nested namespace is refused");
        assert!(error.contains("share an output namespace"), "{error}");
    }

    #[test]
    fn a_default_namespace_still_overlaps_a_named_one() {
        let plan = sealed(
            vec![source("shot.png")],
            vec![
                PlanTarget {
                    target_id: "root".into(),
                    out: String::new(),
                    recipe: recipe(),
                    mappings: vec![mapping("shot.png", "shot.webp")],
                },
                PlanTarget {
                    target_id: "web".into(),
                    out: "web".into(),
                    recipe: recipe(),
                    mappings: vec![mapping("shot.png", "web/shot.webp")],
                },
            ],
        );
        let error = plan.validate().expect_err("the output root contains web");
        assert!(error.contains("share an output namespace"), "{error}");
    }

    #[test]
    fn a_namespace_that_differs_only_in_case_is_still_the_same_folder() {
        let plan = sealed(
            vec![source("shot.png")],
            vec![
                PlanTarget {
                    target_id: "lower".into(),
                    out: "web".into(),
                    recipe: recipe(),
                    mappings: vec![mapping("shot.png", "web/shot.webp")],
                },
                PlanTarget {
                    target_id: "upper".into(),
                    out: "WEB".into(),
                    recipe: recipe(),
                    mappings: vec![mapping("shot.png", "WEB/shot.webp")],
                },
            ],
        );
        let error = plan.validate().expect_err("case aliases are one folder");
        assert!(error.contains("share an output namespace"), "{error}");
    }

    #[test]
    fn two_sources_may_not_be_planned_onto_one_file() {
        let plan = sealed(
            vec![source("a.png"), source("b.png")],
            vec![PlanTarget {
                target_id: "default".into(),
                out: String::new(),
                recipe: recipe(),
                mappings: vec![mapping("a.png", "shot.webp"), mapping("b.png", "shot.webp")],
            }],
        );
        let error = plan
            .validate()
            .expect_err("one destination cannot hold two sources");
        assert!(error.contains("both write"), "{error}");
    }

    #[test]
    fn a_mapping_may_not_name_a_file_outside_its_own_target() {
        let plan = sealed(
            vec![source("shot.png")],
            vec![PlanTarget {
                target_id: "web".into(),
                out: "web".into(),
                recipe: recipe(),
                mappings: vec![mapping("shot.png", "elsewhere/shot.webp")],
            }],
        );
        let error = plan.validate().expect_err("the mapping escapes its target");
        assert!(error.contains("outside its own namespace"), "{error}");
    }

    #[test]
    fn portable_paths_refuse_what_another_platform_would_retarget() {
        for name in [
            "back\\slash.png",
            "com1.png",
            "stream:name.png",
            "trailing.png ",
            "trailing.png.",
            "nul.png",
            "control\u{7}.png",
            "..",
        ] {
            let plan = sealed(
                vec![source(name)],
                vec![PlanTarget {
                    target_id: "default".into(),
                    out: String::new(),
                    recipe: recipe(),
                    mappings: vec![mapping(name, "shot.webp")],
                }],
            );
            let error = plan
                .validate()
                .expect_err("a name another platform would retarget is refused");
            assert!(
                error.contains("unsafe portable component") || error.contains("literal backslash"),
                "{name}: {error}"
            );
        }
    }

    #[test]
    fn an_ordinary_relative_name_still_passes_the_portable_rule() {
        portable_path("album/one.two.png", "source").expect("a plain nested name is portable");
        let error = portable_path("", "source").expect_err("an empty path is not a name");
        assert!(error.contains("empty"), "{error}");
    }

    #[test]
    fn an_effective_recipe_pins_the_current_processing_revision_and_speed() {
        assert_eq!(
            crate::recipe::FINGERPRINT_REVISION,
            2,
            "bumping the revision revisits saved plans"
        );
        let ambient =
            EffectiveRecipe::from_settings(Format::Avif, Quality::lossy(60.), MaxEdge::FULL, None);
        assert_eq!(ambient.avif_speed, crate::avif::DEFAULT_SPEED);
        let explicit = EffectiveRecipe::from_settings(
            Format::Avif,
            Quality::lossy(60.),
            MaxEdge::FULL,
            Some(2),
        );
        assert_eq!(explicit.avif_speed, 2);
        assert_ne!(
            ambient.fingerprint, explicit.fingerprint,
            "an explicit speed changes what the encoder writes"
        );
        assert_eq!(
            explicit.fingerprint,
            crate::recipe::fingerprint_settings(
                Format::Avif,
                Quality::lossy(60.),
                MaxEdge::FULL,
                Some(2)
            ),
            "one authority computes the effective recipe identity"
        );
        explicit.settings().expect("the recorded recipe resolves");
    }

    fn state_dir(tag: &str) -> PathBuf {
        let dir = std::env::temp_dir().join(format!(
            "press-saved-plan-{tag}-{}-{:?}",
            std::process::id(),
            std::thread::current().id()
        ));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).expect("the state fixture dir is created");
        dir
    }

    fn roots() -> (PathBuf, PathBuf) {
        (PathBuf::from("/source"), PathBuf::from("/output"))
    }

    fn stored(dir: &Path, state: &RunState) -> PathBuf {
        let path = dir.join("state.json");
        std::fs::write(
            &path,
            serde_json::to_vec_pretty(state).expect("the state encodes"),
        )
        .expect("the state is written");
        path
    }

    fn loaded(dir: &Path, plan: &Plan, state: &RunState) -> Result<RunState, String> {
        let (source_root, output_root) = roots();
        load_state(&stored(dir, state), plan, &source_root, &output_root)
    }

    #[test]
    fn a_run_state_must_be_exactly_this_plans_items() {
        let dir = state_dir("state-items");
        let plan = sealed(
            vec![source("a.png"), source("b.png")],
            vec![PlanTarget {
                target_id: "default".into(),
                out: String::new(),
                recipe: recipe(),
                mappings: vec![mapping("a.png", "a.webp"), mapping("b.png", "b.webp")],
            }],
        );
        let (source_root, output_root) = roots();
        let base = new_state(&plan, &source_root, &output_root);
        loaded(&dir, &plan, &base).expect("the state this run wrote loads");

        let mut short = base.clone();
        short.items.pop();
        let error = loaded(&dir, &plan, &short).expect_err("a missing item is refused");
        assert!(error.contains("1 items for a plan of 2"), "{error}");

        let mut foreign = base.clone();
        foreign.items[1].item_id = item_id("never-planned.png");
        let error = loaded(&dir, &plan, &foreign).expect_err("an unplanned item is refused");
        assert!(error.contains("is missing source"), "{error}");

        let mut repeated = base.clone();
        repeated.items[1] = repeated.items[0].clone();
        let error = loaded(&dir, &plan, &repeated).expect_err("a repeated item is refused");
        assert!(error.contains("repeats item"), "{error}");

        let mut extra = base.clone();
        extra.items.push(extra.items[0].clone());
        let error = loaded(&dir, &plan, &extra).expect_err("an extra item is refused");
        assert!(error.contains("3 items for a plan of 2"), "{error}");

        let mut renamed = base.clone();
        renamed.items[0].source = "elsewhere.png".into();
        let error = loaded(&dir, &plan, &renamed).expect_err("a retargeted source is refused");
        assert!(error.contains("fixed fields"), "{error}");

        let mut retargeted = base.clone();
        retargeted.items[0].output = Some("somewhere-else.webp".into());
        let error = loaded(&dir, &plan, &retargeted).expect_err("a retargeted output is refused");
        assert!(error.contains("fixed fields"), "{error}");

        let mut rerecipe = base.clone();
        rerecipe.items[0].recipe = hex_digest(b"another recipe");
        let error = loaded(&dir, &plan, &rerecipe).expect_err("a swapped recipe is refused");
        assert!(error.contains("fixed fields"), "{error}");

        let mut renamed_run = base;
        renamed_run.run_id = "run-0000000000000000".into();
        let error = loaded(&dir, &plan, &renamed_run).expect_err("another run id is refused");
        assert!(error.contains("run identity"), "{error}");

        let _ = std::fs::remove_dir_all(dir);
    }

    #[test]
    fn a_run_state_may_not_invent_a_success_or_hide_a_failure() {
        let dir = state_dir("state-receipts");
        let plan = one_source_plan();
        let (source_root, output_root) = roots();
        let base = new_state(&plan, &source_root, &output_root);

        let mut hollow = base.clone();
        hollow.items[0].status = ItemStatus::Written;
        let error = loaded(&dir, &plan, &hollow).expect_err("a written item owes a receipt");
        assert!(error.contains("incomplete receipt"), "{error}");

        let mut forged = base.clone();
        forged.items[0].status = ItemStatus::Written;
        forged.items[0].output_hash = Some("not a sha".into());
        forged.items[0].output_bytes = Some(1);
        forged.items[0].width = Some(8);
        forged.items[0].height = Some(8);
        let error = loaded(&dir, &plan, &forged).expect_err("a receipt needs a real hash");
        assert!(error.contains("incomplete receipt"), "{error}");

        let mut unnamed = base.clone();
        unnamed.items[0].status = ItemStatus::Failed;
        let error = loaded(&dir, &plan, &unnamed).expect_err("failures are named, not counted");
        assert!(error.contains("naming the failure"), "{error}");

        let mut premature = base;
        premature.items[0].output_hash = Some(hex_digest(b"anything"));
        let error =
            loaded(&dir, &plan, &premature).expect_err("an unstarted item cannot carry a receipt");
        assert!(error.contains("not written"), "{error}");

        let _ = std::fs::remove_dir_all(dir);
    }

    #[test]
    fn a_planned_mapping_carries_the_destination_it_was_reviewed_against() {
        let mut plan = one_source_plan();
        plan.targets[0].mappings[0].destination = None;
        plan.digest = plan.compute_digest().expect("the digest computes");
        plan.plan_id = format!("plan-{}", &plan.digest[..16]);
        let error = plan
            .validate()
            .expect_err("a mapping without a pinned destination is not a reviewed plan");
        assert!(error.contains("reviewed destination"), "{error}");

        // The pin is sealed with everything else, so swapping consent for an
        // absence into consent for somebody else's file breaks the digest.
        let mut plan = one_source_plan();
        plan.targets[0].mappings[0].destination = Some(crate::convert::ReviewedDestination::Own {
            sha256: hex_digest(b"another output"),
            bytes: 12,
        });
        let error = plan
            .validate()
            .expect_err("the reviewed destination is part of the seal");
        assert!(error.contains("digest does not match"), "{error}");

        // Sealed or not, a hash that is not a hash is not a snapshot of
        // anything, so it never becomes consent to replace a file.
        let mut plan = one_source_plan();
        plan.targets[0].mappings[0].destination = Some(crate::convert::ReviewedDestination::Own {
            sha256: "not a sha".into(),
            bytes: 12,
        });
        plan.digest = plan.compute_digest().expect("the digest computes");
        plan.plan_id = format!("plan-{}", &plan.digest[..16]);
        let error = plan
            .validate()
            .expect_err("a malformed reviewed hash is refused");
        assert!(
            error.contains("invalid reviewed output snapshot"),
            "{error}"
        );

        let mut plan = one_source_plan();
        plan.targets[0].mappings[0].destination = Some(crate::convert::ReviewedDestination::Own {
            sha256: hex_digest(b"an output"),
            bytes: 0,
        });
        plan.digest = plan.compute_digest().expect("the digest computes");
        plan.plan_id = format!("plan-{}", &plan.digest[..16]);
        let error = plan
            .validate()
            .expect_err("a file with no bytes is not an installed output");
        assert!(
            error.contains("invalid reviewed output snapshot"),
            "{error}"
        );
    }

    #[test]
    fn a_receipt_does_not_outlive_the_output_it_described() {
        let dir = state_dir("state-cleared");
        let plan = one_source_plan();
        let (source_root, output_root) = roots();
        let mut state = new_state(&plan, &source_root, &output_root);
        state.items[0].status = ItemStatus::Written;
        state.items[0].output_hash = Some(hex_digest(b"installed"));
        state.items[0].output_bytes = Some(64);
        state.items[0].width = Some(8);
        state.items[0].height = Some(8);
        state.items[0].checks = vec![Check {
            name: "output_hash".into(),
            status: CheckStatus::Completed,
            reason: None,
        }];
        loaded(&dir, &plan, &state).expect("a complete receipt loads");

        // What a reconcile records when the output it vouched for is gone.
        state.items[0].status = ItemStatus::Failed;
        state.items[0].error = Some("installed output is missing or unreadable".into());
        clear_receipt(&mut state.items[0]);
        let failed = loaded(&dir, &plan, &state).expect("a named failure loads");
        assert_eq!(failed.items[0].output_hash, None);
        assert_eq!(failed.items[0].output_bytes, None);
        assert!(failed.items[0].checks.is_empty());
        assert_eq!(
            failed.items[0].error.as_deref(),
            Some("installed output is missing or unreadable"),
            "the failure is still named"
        );

        // And what the retry after it leaves on disk if it is killed mid-encode.
        state.items[0].status = ItemStatus::Running;
        state.items[0].error = None;
        clear_receipt(&mut state.items[0]);
        loaded(&dir, &plan, &state).expect("an interrupted retry state loads again");
        let _ = std::fs::remove_dir_all(dir);
    }

    #[test]
    fn a_refused_mapping_cannot_be_recorded_as_progress() {
        let dir = state_dir("state-refused");
        let plan = sealed(
            vec![source("shot.png")],
            vec![PlanTarget {
                target_id: "default".into(),
                out: String::new(),
                recipe: recipe(),
                mappings: vec![PlanMapping {
                    item_id: item_id("shot.png"),
                    output: None,
                    destination: None,
                    status: MappingStatus::Refused,
                    error: Some("the plan refused this source".into()),
                }],
            }],
        );
        let (source_root, output_root) = roots();
        let mut state = new_state(&plan, &source_root, &output_root);
        assert_eq!(state.items[0].status, ItemStatus::Failed);
        loaded(&dir, &plan, &state).expect("the refusal this run recorded loads");
        state.items[0].status = ItemStatus::Unstarted;
        state.items[0].error = None;
        let error = loaded(&dir, &plan, &state).expect_err("a refusal is not work to start");
        assert!(error.contains("refused mapping"), "{error}");
        let _ = std::fs::remove_dir_all(dir);
    }

    #[test]
    fn one_run_lock_covers_a_plan_bound_to_one_pair_of_roots() {
        let dir = state_dir("run-lock");
        let path = dir.join("plan.json.run-abc.json");
        let held = RunLock::acquire(&path).expect("the first command takes the lock");
        // A second lock in this process would succeed on some platforms, so the
        // observable contract tested here is that the lock file is an ordinary
        // file the next process can open, and that an impersonated one is not.
        let lock_path = dir.join("plan.json.run-abc.json.lock");
        assert!(lock_path.is_file(), "the lock is a plain file");
        drop(held);
        // Impersonating the lock needs a link, which is a Unix fixture here.
        // The plain-file half above is the part every platform runs.
        #[cfg(unix)]
        {
            std::fs::remove_file(&lock_path).expect("the lock file is removed");
            std::os::unix::fs::symlink(dir.join("elsewhere"), &lock_path)
                .expect("the impersonating link is created");
            let error = match RunLock::acquire(&path) {
                Ok(_) => unreachable!("a link is not this run's lock"),
                Err(error) => error,
            };
            assert!(error.contains("not a regular file"), "{error}");
        }
        let _ = std::fs::remove_dir_all(dir);
    }

    /// A folder that refuses a new file is a Unix mode here. The portable half
    /// of saving — staging, installing and refusing an impersonated path — is
    /// covered by the tests around this one on every platform.
    #[cfg(unix)]
    #[test]
    fn a_failed_save_returns_its_error_and_keeps_the_previous_state() {
        use std::os::unix::fs::PermissionsExt as _;

        let dir = state_dir("state-preserved");
        let plan = one_source_plan();
        let (source_root, output_root) = roots();
        let state = new_state(&plan, &source_root, &output_root);
        let path = dir.join("state.json");
        save_state(&path, &state).expect("the first state is written");
        let before = std::fs::read(&path).expect("the first state is readable");

        let mut progressed = state;
        progressed.items[0].status = ItemStatus::Cancelled;
        std::fs::set_permissions(&dir, std::fs::Permissions::from_mode(0o500))
            .expect("the folder is made read-only");
        let error = save_state(&path, &progressed).expect_err("a folder that refuses a new file");
        std::fs::set_permissions(&dir, std::fs::Permissions::from_mode(0o700))
            .expect("the folder is made writable again");

        assert!(error.contains("stage saved run state"), "{error}");
        assert_eq!(
            std::fs::read(&path).expect("the old state is still readable"),
            before,
            "a failed save leaves the last complete state"
        );
        let _ = std::fs::remove_dir_all(dir);
    }

    #[cfg(unix)]
    #[test]
    fn a_run_state_is_never_written_through_an_impersonated_name() {
        let dir = state_dir("state-impersonated");
        let path = dir.join("state.json");
        std::os::unix::fs::symlink(dir.join("elsewhere.json"), &path)
            .expect("the impersonating link is created");
        let plan = one_source_plan();
        let (source_root, output_root) = roots();
        let state = new_state(&plan, &source_root, &output_root);
        let error = save_state(&path, &state).expect_err("a link is not the run state");
        assert!(error.contains("not a regular file"), "{error}");
        assert!(
            !dir.join("elsewhere.json").exists(),
            "nothing was written through the link"
        );
        let _ = std::fs::remove_dir_all(dir);
    }
}
