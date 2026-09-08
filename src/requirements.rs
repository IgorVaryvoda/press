//! Bounded, local requirement snapshots and actual-output receipts.
//!
//! This is an intentionally small contract for one user-authored destination. It
//! describes checks Press can make from the bytes it has, never an executable
//! policy language. A future server-resolved snapshot needs a separate provenance
//! contract; this version refuses to pretend a local file is retailer policy.

use std::collections::HashSet;
use std::fmt::Write as _;
use std::io::Read as _;
use std::path::{Path, PathBuf};

use image::{ImageDecoder as _, ImageFormat, ImageReader, metadata::Orientation};
use serde::{Deserialize, Serialize};

/// The only requirements schema this build reads.
pub const SCHEMA_VERSION: u32 = 1;
pub const SUPPORTED_ENGINE: &str = "press";
pub const SUPPORTED_ENGINE_VERSION: u32 = 1;
pub const SUPPORTED_CHECKER_VERSION: u32 = 1;

/// Requirements are typed data, so a larger file is refused before JSON parsing
/// can allocate a collection that this checker does not need.
pub const MAX_FILE_BYTES: u64 = 64 * 1024;
pub const MAX_RULES: usize = 64;
pub const MAX_OUTPUTS: usize = 1_024;
const MAX_ID_CHARS: usize = 64;
const MAX_TEXT_CHARS: usize = 128;
const MAX_FORMATS: usize = 8;
const MAX_OUTPUT_SNAPSHOT_BYTES: u64 = crate::convert::MAX_SOURCE_BYTES;

/// The first local contract is deliberately explicit about its authority. A
/// retailer or service snapshot must arrive through a separately agreed contract.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
#[serde(rename_all = "snake_case")]
pub enum Provenance {
    LocalAuthor { author: String },
}

/// A stable local requirements snapshot. The effective interval is inclusive at
/// `effective_from` and exclusive at `effective_until`, both in UTC calendar dates.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct RequirementsSnapshot {
    pub schema: u32,
    pub id: String,
    pub name: String,
    pub revision: u32,
    pub provenance: Provenance,
    pub target: String,
    pub role: String,
    pub category: Option<String>,
    pub region: Option<String>,
    pub last_verified: String,
    pub effective_from: Option<String>,
    pub effective_until: Option<String>,
    pub engine: String,
    pub engine_version: u32,
    pub checker_version: u32,
    pub rules: Vec<Rule>,
}

/// One mandatory or advisory check. `required: false` is advisory and never
/// turns a failed advisory check into a failed technical requirement set.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Rule {
    pub id: String,
    pub label: String,
    pub required: bool,
    pub constraint: Constraint,
}

/// Fixed semantics are safer than importing expressions, regular expressions, or
/// code. `Unsupported` is an explicit server/user marker: it remains NotChecked.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
#[serde(tag = "kind", rename_all = "snake_case")]
pub enum Constraint {
    Format {
        allowed: Vec<ContentFormat>,
    },
    Dimensions {
        min_width: Option<u32>,
        max_width: Option<u32>,
        min_height: Option<u32>,
        max_height: Option<u32>,
    },
    Bytes {
        min: Option<u64>,
        max: Option<u64>,
    },
    Alpha {
        allowed: AlphaPolicy,
    },
    Name {
        allowed_extensions: Vec<String>,
    },
    Profile {
        present: bool,
    },
    Depth {
        min_bits: Option<u8>,
        max_bits: Option<u8>,
    },
    Unsupported {
        label: String,
    },
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum AlphaPolicy {
    Required,
    Forbidden,
}

/// Formats Press can identify from the encoded bytes. `Other` keeps an observed
/// format named without allowing a requirement to invent an unchecked format.
#[derive(Clone, Copy, Debug, Hash, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ContentFormat {
    Jpeg,
    Png,
    Webp,
    Avif,
    JpegXl,
    Gif,
    Tiff,
    Bmp,
    Other,
}

impl ContentFormat {
    fn label(self) -> &'static str {
        match self {
            Self::Jpeg => "jpeg",
            Self::Png => "png",
            Self::Webp => "webp",
            Self::Avif => "avif",
            Self::JpegXl => "jpeg xl",
            Self::Gif => "gif",
            Self::Tiff => "tiff",
            Self::Bmp => "bmp",
            Self::Other => "other",
        }
    }
}

/// The processing identity that prepared an output. All fields are optional
/// except the processing revision because a caller may only have a manifest
/// fingerprint while a saved plan can provide the full recipe identity.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ProcessingIdentity {
    pub recipe_id: Option<String>,
    pub recipe_revision: Option<u32>,
    pub recipe_fingerprint: Option<String>,
    pub processing_revision: u32,
}

/// One source/output pairing to inspect. Paths are accepted for local use, but
/// receipt fields are always relative to `root`.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct OutputRef {
    pub source: Option<PathBuf>,
    pub output: PathBuf,
    pub processing: Option<ProcessingIdentity>,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum CheckStatus {
    Pass,
    Fail,
    NotChecked,
    NotApplicable,
    NeedsReview,
}

/// Observed values stay typed so a JSON receipt cannot confuse a display string
/// with measured bytes or dimensions.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "kind", content = "value", rename_all = "snake_case")]
pub enum ObservedValue {
    Format(ContentFormat),
    Dimensions { width: u32, height: u32 },
    Bytes(u64),
    Alpha(bool),
    Name(String),
    Profile(bool),
    Depth(u8),
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct CheckResult {
    pub id: String,
    pub label: String,
    pub required: bool,
    pub status: CheckStatus,
    pub observed: Option<ObservedValue>,
    pub evidence: String,
}

/// Facts read from the output bytes. Missing facts remain `None` and are not
/// turned into a guessed pass.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ActualOutput {
    pub format: Option<ContentFormat>,
    pub width: Option<u32>,
    pub height: Option<u32>,
    pub bytes: Option<u64>,
    pub alpha: Option<bool>,
    pub profile: Option<bool>,
    pub depth_bits: Option<u8>,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct OutputReport {
    pub source: Option<String>,
    pub output: String,
    pub source_hash: Option<String>,
    pub output_hash: Option<String>,
    pub processing: Option<ProcessingIdentity>,
    pub actual: ActualOutput,
    pub inspection_error: Option<String>,
    pub checks: Vec<CheckResult>,
    pub all_required_pass: bool,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct RequirementsIdentity {
    pub id: String,
    pub revision: u32,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Receipt {
    pub schema: u32,
    pub checked_at: String,
    pub requirements: RequirementsIdentity,
    pub effective: bool,
    pub outputs: Vec<OutputReport>,
    pub all_required_pass: bool,
}

/// Parse a requirements snapshot with its raw-byte bound applied first.
pub fn parse_bytes(bytes: &[u8]) -> Result<RequirementsSnapshot, String> {
    if bytes.len() as u64 > MAX_FILE_BYTES {
        return Err(format!(
            "requirements files larger than {MAX_FILE_BYTES} bytes are refused"
        ));
    }
    let snapshot: RequirementsSnapshot = serde_json::from_slice(bytes)
        .map_err(|error| format!("requirements do not parse: {error}"))?;
    validate(&snapshot)?;
    Ok(snapshot)
}

/// Read a requirements file with a second bounded read, covering a file that
/// grows after the initial metadata check.
pub fn parse_file(path: &Path) -> Result<RequirementsSnapshot, String> {
    let file =
        std::fs::File::open(path).map_err(|error| format!("requirements unreadable: {error}"))?;
    if file
        .metadata()
        .map_err(|error| format!("requirements metadata unavailable: {error}"))?
        .len()
        > MAX_FILE_BYTES
    {
        return Err(format!(
            "requirements files larger than {MAX_FILE_BYTES} bytes are refused"
        ));
    }
    let mut bytes = Vec::new();
    file.take(MAX_FILE_BYTES + 1)
        .read_to_end(&mut bytes)
        .map_err(|error| format!("requirements unreadable: {error}"))?;
    parse_bytes(&bytes)
}

/// Inspect outputs at a deterministic UTC date. The caller can use this for a
/// saved plan or a reproducible receipt; no current-time policy decision is hidden.
pub fn inspect_outputs_at(
    root: &Path,
    refs: &[OutputRef],
    snapshot: &RequirementsSnapshot,
    as_of: &str,
) -> Result<Receipt, String> {
    validate(snapshot)?;
    if !valid_date(as_of) {
        return Err(format!("inspection date {as_of:?} is not YYYY-MM-DD"));
    }
    if refs.is_empty() {
        return Err("at least one output is required".into());
    }
    if refs.len() > MAX_OUTPUTS {
        return Err(format!("inspection lists more than {MAX_OUTPUTS} outputs"));
    }
    let root =
        std::fs::canonicalize(root).map_err(|error| format!("root is unavailable: {error}"))?;
    let effective = snapshot.is_effective(as_of);
    let mut outputs = Vec::with_capacity(refs.len());
    for reference in refs {
        outputs.push(inspect_one(&root, reference, snapshot, effective)?);
    }
    let all_required_pass = effective
        && outputs.iter().all(|output| output.all_required_pass)
        && snapshot.rules.iter().any(|rule| rule.required);
    Ok(Receipt {
        schema: SCHEMA_VERSION,
        checked_at: as_of.to_string(),
        requirements: RequirementsIdentity {
            id: snapshot.id.clone(),
            revision: snapshot.revision,
        },
        effective,
        outputs,
        all_required_pass,
    })
}

/// Inspect using today's UTC date. Reproducible callers should use
/// `inspect_outputs_at` instead.
pub fn inspect_outputs(
    root: &Path,
    refs: &[OutputRef],
    snapshot: &RequirementsSnapshot,
) -> Result<Receipt, String> {
    inspect_outputs_at(root, refs, snapshot, &utc_date())
}

pub fn render_json(receipt: &Receipt) -> Result<String, String> {
    serde_json::to_string_pretty(receipt)
        .map_err(|error| format!("receipt does not encode: {error}"))
}

/// Human-readable, shareable output. It intentionally prints only root-relative
/// names and hashes; the local root and any signed URL never enter the receipt.
pub fn render_report(receipt: &Receipt) -> String {
    let mut report = String::new();
    let _ = writeln!(
        report,
        "Requirements {} rev {} · checked {} · {}",
        receipt.requirements.id,
        receipt.requirements.revision,
        receipt.checked_at,
        if receipt.effective {
            "effective"
        } else {
            "outside effective interval"
        }
    );
    let _ = writeln!(
        report,
        "Technical requirements: {}",
        if receipt.all_required_pass {
            "PASS"
        } else {
            "NOT PASS"
        }
    );
    for output in &receipt.outputs {
        let _ = writeln!(report, "\nOutput: {}", output.output);
        if let Some(source) = &output.source {
            let _ = writeln!(report, "Source: {source}");
        }
        if let Some(hash) = &output.source_hash {
            let _ = writeln!(report, "Source SHA-256: {hash}");
        }
        if let Some(hash) = &output.output_hash {
            let _ = writeln!(report, "Output SHA-256: {hash}");
        }
        if let Some(error) = &output.inspection_error {
            let _ = writeln!(report, "Inspection: {error}");
        }
        for check in &output.checks {
            let requirement = if check.required {
                "required"
            } else {
                "advisory"
            };
            let observed = check
                .observed
                .as_ref()
                .map(display_observed)
                .unwrap_or_else(|| "not observed".into());
            let _ = writeln!(
                report,
                "- {} [{} / {}] {} ({observed}): {}",
                check.label,
                requirement,
                display_status(&check.status),
                check.evidence,
                check.id
            );
        }
    }
    report
}

impl RequirementsSnapshot {
    pub fn is_effective(&self, date: &str) -> bool {
        self.effective_from
            .as_deref()
            .is_none_or(|from| date >= from)
            && self
                .effective_until
                .as_deref()
                .is_none_or(|until| date < until)
    }
}

fn inspect_one(
    root: &Path,
    reference: &OutputRef,
    snapshot: &RequirementsSnapshot,
    effective: bool,
) -> Result<OutputReport, String> {
    let output_name = relative_name(root, &reference.output)?;
    let source_name = reference
        .source
        .as_deref()
        .map(|source| relative_name(root, source))
        .transpose()?;
    let output_path = resolve_under_root(root, &output_name)?;
    let source_path = source_name
        .as_deref()
        .map(|source| resolve_under_root(root, source))
        .transpose()?;
    let source_hash = source_path
        .as_deref()
        .and_then(|path| read_snapshot(path).ok())
        .map(|bytes| hash_bytes(&bytes));
    let output_snapshot = read_snapshot(&output_path);
    let output_hash = output_snapshot.as_ref().ok().map(|bytes| hash_bytes(bytes));
    let output_metadata = std::fs::symlink_metadata(&output_path).ok();
    let mut actual = ActualOutput {
        format: None,
        width: None,
        height: None,
        bytes: output_metadata
            .as_ref()
            .filter(|metadata| metadata.is_file())
            .map(std::fs::Metadata::len),
        alpha: None,
        profile: None,
        depth_bits: None,
    };
    let mut decode_evidence = None;
    match output_snapshot {
        Ok(bytes) => {
            actual.bytes = Some(bytes.len() as u64);
            if let Some(header) = header_from_bytes(&bytes) {
                actual.format = Some(header.format);
                actual.width = Some(header.width);
                actual.height = Some(header.height);
                actual.profile = header.profile;
                actual.depth_bits = header.depth_bits;
                if crate::convert::check_budget_bytes(crate::convert::decode_budget_estimate(
                    header.width,
                    header.height,
                ))
                .is_err()
                {
                    decode_evidence = Some("decoder budget refused this output".to_string());
                } else {
                    match crate::scan::decode_for_conversion_from_bytes(
                        &bytes,
                        crate::convert::MaxEdge::FULL,
                    ) {
                        Ok(decoded) => actual.alpha = Some(decoded.image.has_alpha()),
                        Err(error) => decode_evidence = Some(conversion_decode_evidence(error)),
                    }
                }
            } else {
                decode_evidence = Some("output is not a supported image".into());
            }
        }
        Err(SnapshotFailure::NotFound) => {
            decode_evidence = Some("output was not found".into());
        }
        Err(SnapshotFailure::TooLarge) => {
            decode_evidence = Some(format!(
                "output exceeds the {MAX_OUTPUT_SNAPSHOT_BYTES}-byte inspection bound"
            ));
        }
        Err(SnapshotFailure::NotFile) => {
            decode_evidence = Some("output is not a regular file".into());
        }
        Err(SnapshotFailure::Unreadable) => {
            decode_evidence = Some("output could not be read".into());
        }
    }
    let checks = snapshot
        .rules
        .iter()
        .map(|rule| {
            check_rule(
                rule,
                &actual,
                &output_name,
                effective,
                decode_evidence.as_deref(),
            )
        })
        .collect::<Vec<_>>();
    let inspection_error = decode_evidence;
    let all_required_pass = effective
        && inspection_error.is_none()
        && output_hash.is_some()
        && actual.format.is_some()
        && checks
            .iter()
            .filter(|check| check.required)
            .all(|check| check.status == CheckStatus::Pass);
    Ok(OutputReport {
        source: source_name,
        output: output_name,
        source_hash,
        output_hash,
        processing: reference.processing.clone(),
        actual,
        inspection_error,
        checks,
        all_required_pass,
    })
}

fn check_rule(
    rule: &Rule,
    actual: &ActualOutput,
    output_name: &str,
    effective: bool,
    decode_evidence: Option<&str>,
) -> CheckResult {
    let mut result = CheckResult {
        id: rule.id.clone(),
        label: rule.label.clone(),
        required: rule.required,
        status: CheckStatus::NotChecked,
        observed: None,
        evidence: String::new(),
    };
    if !effective {
        result.status = CheckStatus::NotApplicable;
        result.evidence = "snapshot is outside its effective interval".into();
        return result;
    }
    match &rule.constraint {
        Constraint::Format { allowed } => {
            result.observed = actual.format.map(ObservedValue::Format);
            result.status = actual.format.map_or(CheckStatus::NotChecked, |format| {
                if allowed.contains(&format) {
                    CheckStatus::Pass
                } else {
                    CheckStatus::Fail
                }
            });
            result.evidence = actual.format.map_or_else(
                || "encoded format was not observed".into(),
                |format| format!("encoded bytes are {}", format.label()),
            );
        }
        Constraint::Dimensions {
            min_width,
            max_width,
            min_height,
            max_height,
        } => {
            result.observed = actual
                .width
                .zip(actual.height)
                .map(|(width, height)| ObservedValue::Dimensions { width, height });
            result.status = actual.width.zip(actual.height).map_or(
                CheckStatus::NotChecked,
                |(width, height)| {
                    let ok = min_width.is_none_or(|min| width >= min)
                        && max_width.is_none_or(|max| width <= max)
                        && min_height.is_none_or(|min| height >= min)
                        && max_height.is_none_or(|max| height <= max);
                    if ok {
                        CheckStatus::Pass
                    } else {
                        CheckStatus::Fail
                    }
                },
            );
            result.evidence = actual.width.zip(actual.height).map_or_else(
                || "dimensions were not observed".into(),
                |(width, height)| format!("encoded dimensions are {width}x{height}"),
            );
        }
        Constraint::Bytes { min, max } => {
            result.observed = actual.bytes.map(ObservedValue::Bytes);
            result.status = actual.bytes.map_or(CheckStatus::NotChecked, |bytes| {
                let ok = min.is_none_or(|minimum| bytes >= minimum)
                    && max.is_none_or(|maximum| bytes <= maximum);
                if ok {
                    CheckStatus::Pass
                } else {
                    CheckStatus::Fail
                }
            });
            result.evidence = actual.bytes.map_or_else(
                || "output bytes were not observed".into(),
                |bytes| format!("output contains {bytes} bytes"),
            );
        }
        Constraint::Alpha { allowed } => {
            result.observed = actual.alpha.map(ObservedValue::Alpha);
            result.status = actual.alpha.map_or(CheckStatus::NotChecked, |alpha| {
                let ok = matches!(
                    (allowed, alpha),
                    (AlphaPolicy::Required, true) | (AlphaPolicy::Forbidden, false)
                );
                if ok {
                    CheckStatus::Pass
                } else {
                    CheckStatus::Fail
                }
            });
            result.evidence = actual.alpha.map_or_else(
                || decode_evidence.unwrap_or("alpha was not observed").into(),
                |alpha| {
                    format!(
                        "decoded pixels {} an alpha channel",
                        if alpha { "have" } else { "do not have" }
                    )
                },
            );
        }
        Constraint::Name { allowed_extensions } => {
            let name = Path::new(output_name)
                .file_name()
                .map(|name| name.to_string_lossy().into_owned());
            result.observed = name.clone().map(ObservedValue::Name);
            result.status = name.map_or(CheckStatus::NotChecked, |name| {
                let extension = Path::new(&name)
                    .extension()
                    .and_then(|extension| extension.to_str())
                    .map(str::to_ascii_lowercase);
                if extension.is_some_and(|extension| allowed_extensions.contains(&extension)) {
                    CheckStatus::Pass
                } else {
                    CheckStatus::Fail
                }
            });
            result.evidence = format!(
                "output name is {}",
                Path::new(output_name)
                    .file_name()
                    .unwrap_or_default()
                    .to_string_lossy()
            );
        }
        Constraint::Profile { present } => {
            result.observed = actual.profile.map(ObservedValue::Profile);
            result.status = actual.profile.map_or(CheckStatus::NotChecked, |observed| {
                if observed == *present {
                    CheckStatus::Pass
                } else {
                    CheckStatus::Fail
                }
            });
            result.evidence = actual.profile.map_or_else(
                || {
                    decode_evidence
                        .unwrap_or("a supported colour profile was not observed")
                        .into()
                },
                |observed| {
                    format!(
                        "ICC profile {} present",
                        if observed { "is" } else { "is not" }
                    )
                },
            );
        }
        Constraint::Depth { min_bits, max_bits } => {
            result.observed = actual.depth_bits.map(ObservedValue::Depth);
            result.status = actual.depth_bits.map_or(CheckStatus::NotChecked, |bits| {
                let ok = min_bits.is_none_or(|min| bits >= min)
                    && max_bits.is_none_or(|max| bits <= max);
                if ok {
                    CheckStatus::Pass
                } else {
                    CheckStatus::Fail
                }
            });
            result.evidence = actual.depth_bits.map_or_else(
                || {
                    decode_evidence
                        .unwrap_or("sample depth was not observed")
                        .into()
                },
                |bits| format!("encoded sample depth is {bits} bits"),
            );
        }
        Constraint::Unsupported { label } => {
            result.evidence = format!("unsupported check: {label}");
        }
    }
    result
}

fn validate(snapshot: &RequirementsSnapshot) -> Result<(), String> {
    if snapshot.schema != SCHEMA_VERSION {
        return Err(format!(
            "unsupported requirements schema {} (this Press reads schema {SCHEMA_VERSION})",
            snapshot.schema
        ));
    }
    check_id(&snapshot.id, "requirements id")?;
    check_text(&snapshot.name, "requirements name")?;
    if snapshot.revision == 0 {
        return Err("requirements revision starts at 1".into());
    }
    let Provenance::LocalAuthor { author } = &snapshot.provenance;
    check_text(author, "local author")?;
    check_text(&snapshot.target, "target")?;
    check_text(&snapshot.role, "role")?;
    if let Some(category) = &snapshot.category {
        check_text(category, "category")?;
    }
    if let Some(region) = &snapshot.region {
        check_text(region, "region")?;
    }
    if !valid_date(&snapshot.last_verified) {
        return Err("last_verified must be YYYY-MM-DD".into());
    }
    for (label, date) in [
        ("effective_from", snapshot.effective_from.as_deref()),
        ("effective_until", snapshot.effective_until.as_deref()),
    ] {
        if let Some(date) = date
            && !valid_date(date)
        {
            return Err(format!("{label} must be YYYY-MM-DD"));
        }
    }
    if let (Some(from), Some(until)) = (&snapshot.effective_from, &snapshot.effective_until)
        && from >= until
    {
        return Err("effective_until must be after effective_from".into());
    }
    check_text(&snapshot.engine, "engine")?;
    if snapshot.engine != SUPPORTED_ENGINE
        || snapshot.engine_version != SUPPORTED_ENGINE_VERSION
        || snapshot.checker_version != SUPPORTED_CHECKER_VERSION
    {
        return Err(format!(
            "requirements support only engine {SUPPORTED_ENGINE} {SUPPORTED_ENGINE_VERSION} and checker {SUPPORTED_CHECKER_VERSION}"
        ));
    }
    if snapshot.rules.is_empty() || snapshot.rules.len() > MAX_RULES {
        return Err(format!("requirements must contain 1-{MAX_RULES} rules"));
    }
    let mut ids = HashSet::new();
    for rule in &snapshot.rules {
        check_id(&rule.id, "rule id")?;
        check_text(&rule.label, "rule label")?;
        if !ids.insert(rule.id.as_str()) {
            return Err(format!("duplicate rule id {:?}", rule.id));
        }
        validate_constraint(&rule.constraint)?;
    }
    Ok(())
}

fn validate_constraint(constraint: &Constraint) -> Result<(), String> {
    match constraint {
        Constraint::Format { allowed } => {
            if allowed.is_empty() || allowed.len() > MAX_FORMATS {
                return Err(format!(
                    "format constraint must list 1-{MAX_FORMATS} formats"
                ));
            }
            let mut seen = HashSet::new();
            for format in allowed {
                if !seen.insert(*format) {
                    return Err("format constraint repeats a format".into());
                }
            }
        }
        Constraint::Dimensions {
            min_width,
            max_width,
            min_height,
            max_height,
        } => {
            if min_width.is_none()
                && max_width.is_none()
                && min_height.is_none()
                && max_height.is_none()
            {
                return Err("dimensions constraint needs at least one bound".into());
            }
            if min_width.is_some_and(|value| value == 0)
                || max_width.is_some_and(|value| value == 0)
                || min_height.is_some_and(|value| value == 0)
                || max_height.is_some_and(|value| value == 0)
            {
                return Err("dimension bounds must be greater than zero".into());
            }
            if min_width
                .zip(*max_width)
                .is_some_and(|(min, max)| min > max)
                || min_height
                    .zip(*max_height)
                    .is_some_and(|(min, max)| min > max)
            {
                return Err("dimension minimum cannot exceed maximum".into());
            }
        }
        Constraint::Bytes { min, max } => {
            if min.is_none() && max.is_none() {
                return Err("bytes constraint needs a bound".into());
            }
            if min.zip(*max).is_some_and(|(min, max)| min > max) {
                return Err("byte minimum cannot exceed maximum".into());
            }
        }
        Constraint::Name { allowed_extensions } => {
            if allowed_extensions.is_empty() || allowed_extensions.len() > MAX_FORMATS {
                return Err(format!(
                    "name constraint must list 1-{MAX_FORMATS} extensions"
                ));
            }
            for extension in allowed_extensions {
                if extension.is_empty()
                    || extension != &extension.to_ascii_lowercase()
                    || !extension
                        .bytes()
                        .all(|byte| byte.is_ascii_alphanumeric() || byte == b'-' || byte == b'_')
                {
                    return Err(format!("invalid output extension {:?}", extension));
                }
            }
        }
        Constraint::Depth { min_bits, max_bits } => {
            if min_bits.is_none() && max_bits.is_none() {
                return Err("depth constraint needs a bound".into());
            }
            if min_bits.is_some_and(|value| !(1..=32).contains(&value))
                || max_bits.is_some_and(|value| !(1..=32).contains(&value))
            {
                return Err("depth bounds must be 1-32 bits".into());
            }
            if min_bits.zip(*max_bits).is_some_and(|(min, max)| min > max) {
                return Err("depth minimum cannot exceed maximum".into());
            }
        }
        Constraint::Unsupported { label } => check_text(label, "unsupported check label")?,
        Constraint::Alpha { .. } | Constraint::Profile { .. } => {}
    }
    Ok(())
}

fn check_id(value: &str, label: &str) -> Result<(), String> {
    if value.is_empty()
        || value.chars().count() > MAX_ID_CHARS
        || !value.bytes().all(|byte| {
            byte.is_ascii_lowercase() || byte.is_ascii_digit() || byte == b'-' || byte == b'_'
        })
    {
        return Err(format!(
            "{label} must be 1-{MAX_ID_CHARS} lowercase letters, digits, dashes or underscores"
        ));
    }
    Ok(())
}

fn check_text(value: &str, label: &str) -> Result<(), String> {
    if value.is_empty()
        || value.chars().count() > MAX_TEXT_CHARS
        || value.chars().any(char::is_control)
    {
        return Err(format!(
            "{label} must be 1-{MAX_TEXT_CHARS} printable characters"
        ));
    }
    Ok(())
}

fn valid_date(value: &str) -> bool {
    let bytes = value.as_bytes();
    if bytes.len() != 10 || bytes[4] != b'-' || bytes[7] != b'-' {
        return false;
    }
    if !bytes
        .iter()
        .enumerate()
        .all(|(index, byte)| matches!(index, 4 | 7) || byte.is_ascii_digit())
    {
        return false;
    }
    let year = value[0..4].parse::<u32>().ok();
    let month = value[5..7].parse::<u32>().ok();
    let day = value[8..10].parse::<u32>().ok();
    let (Some(year), Some(month), Some(day)) = (year, month, day) else {
        return false;
    };
    if year == 0 || !(1..=12).contains(&month) {
        return false;
    }
    let leap = year.is_multiple_of(4) && (!year.is_multiple_of(100) || year.is_multiple_of(400));
    let days = match month {
        2 if leap => 29,
        2 => 28,
        4 | 6 | 9 | 11 => 30,
        _ => 31,
    };
    (1..=days).contains(&day)
}

fn relative_name(root: &Path, path: &Path) -> Result<String, String> {
    let canonical;
    let relative = if path.is_absolute() {
        canonical = canonicalize_allow_missing(path)?;
        canonical
            .strip_prefix(root)
            .map_err(|_| "a receipt path is outside its root".to_string())?
    } else {
        path
    };
    if relative.to_string_lossy().contains('\\') {
        return Err("receipt paths containing backslashes are unsupported".into());
    }
    crate::output::normal_relative(relative)
        .map_err(|error| format!("unsafe receipt path: {error}"))?;
    Ok(relative.to_string_lossy().replace('\\', "/"))
}

fn resolve_under_root(root: &Path, relative: &str) -> Result<PathBuf, String> {
    let root =
        std::fs::canonicalize(root).map_err(|error| format!("root is unavailable: {error}"))?;
    let canonical = canonicalize_allow_missing(&root.join(relative))?;
    if canonical.starts_with(&root) {
        Ok(canonical)
    } else {
        Err("a receipt path resolves outside its root".into())
    }
}

fn canonicalize_allow_missing(path: &Path) -> Result<PathBuf, String> {
    let mut current = path.to_path_buf();
    let mut missing = Vec::new();
    loop {
        match std::fs::canonicalize(&current) {
            Ok(mut canonical) => {
                for component in missing.iter().rev() {
                    canonical.push(component);
                }
                return Ok(canonical);
            }
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => {
                let Some(name) = current.file_name() else {
                    return Err("receipt path is unavailable: missing ancestor".into());
                };
                missing.push(name.to_owned());
                current.pop();
            }
            Err(error) => return Err(format!("receipt path is unavailable: {error}")),
        }
    }
}

struct HeaderFacts {
    format: ContentFormat,
    width: u32,
    height: u32,
    profile: Option<bool>,
    depth_bits: Option<u8>,
}

fn header_from_bytes(bytes: &[u8]) -> Option<HeaderFacts> {
    if let Some(reader) = ImageReader::new(std::io::Cursor::new(bytes))
        .with_guessed_format()
        .ok()
        && let Some(image_format) = reader.format()
    {
        let mut decoder = reader.into_decoder().ok()?;
        let (mut width, mut height) = decoder.dimensions();
        if orientation_swaps_dimensions(decoder.orientation().unwrap_or(Orientation::NoTransforms))
        {
            std::mem::swap(&mut width, &mut height);
        }
        let format = content_format_from_image(image_format);
        let profile = decoder.icc_profile().ok().map(|profile| profile.is_some());
        let depth_bits = encoded_depth_bits(&mut decoder, format);
        return Some(HeaderFacts {
            format,
            width,
            height,
            profile,
            depth_bits,
        });
    }

    let info = crate::jxl::probe_bytes(bytes)?;
    Some(HeaderFacts {
        format: ContentFormat::JpegXl,
        width: info.width,
        height: info.height,
        profile: None,
        depth_bits: None,
    })
}

fn content_format_from_image(format: ImageFormat) -> ContentFormat {
    match format {
        ImageFormat::Jpeg => ContentFormat::Jpeg,
        ImageFormat::Png => ContentFormat::Png,
        ImageFormat::WebP => ContentFormat::Webp,
        ImageFormat::Avif => ContentFormat::Avif,
        ImageFormat::Gif => ContentFormat::Gif,
        ImageFormat::Tiff => ContentFormat::Tiff,
        ImageFormat::Bmp => ContentFormat::Bmp,
        _ => ContentFormat::Other,
    }
}

fn encoded_depth_bits<D: image::ImageDecoder>(
    decoder: &mut D,
    format: ContentFormat,
) -> Option<u8> {
    use image::ExtendedColorType;
    // These two decoders expose the source precision without expanding a packed
    // sample or converting a high-depth frame to the DynamicImage representation.
    if !matches!(format, ContentFormat::Jpeg | ContentFormat::Webp) {
        return None;
    }
    let original = decoder.original_color_type();
    let channels = u16::from(original.channel_count());
    let bits = original.bits_per_pixel();
    if channels == 0 || !bits.is_multiple_of(channels) {
        return None;
    }
    let bits = bits / channels;
    matches!(
        original,
        ExtendedColorType::L8
            | ExtendedColorType::La8
            | ExtendedColorType::Rgb8
            | ExtendedColorType::Rgba8
    )
    .then_some(bits as u8)
}

fn orientation_swaps_dimensions(orientation: Orientation) -> bool {
    matches!(
        orientation,
        Orientation::Rotate90
            | Orientation::Rotate270
            | Orientation::Rotate90FlipH
            | Orientation::Rotate270FlipH
    )
}

enum SnapshotFailure {
    NotFound,
    TooLarge,
    NotFile,
    Unreadable,
}

fn read_snapshot(path: &Path) -> Result<Vec<u8>, SnapshotFailure> {
    let metadata = match std::fs::metadata(path) {
        Ok(metadata) => metadata,
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => {
            return Err(SnapshotFailure::NotFound);
        }
        Err(_) => return Err(SnapshotFailure::Unreadable),
    };
    if !metadata.is_file() {
        return Err(SnapshotFailure::NotFile);
    }
    if metadata.len() > MAX_OUTPUT_SNAPSHOT_BYTES {
        return Err(SnapshotFailure::TooLarge);
    }
    crate::scan::read_source_bytes(path).map_err(|error| match error {
        crate::scan::ConversionDecodeError::TooLarge => SnapshotFailure::TooLarge,
        _ => SnapshotFailure::Unreadable,
    })
}

fn hash_bytes(bytes: &[u8]) -> String {
    crate::manifest::SourceIdentity::from_bytes(bytes).hash
}

fn conversion_decode_evidence(error: crate::scan::ConversionDecodeError) -> String {
    match error {
        crate::scan::ConversionDecodeError::Failed => {
            "output header was readable but pixels were not decoded".into()
        }
        crate::scan::ConversionDecodeError::TooLarge => {
            format!("output exceeds the {MAX_OUTPUT_SNAPSHOT_BYTES}-byte inspection bound")
        }
        crate::scan::ConversionDecodeError::SourceChanged => {
            "output changed during inspection".into()
        }
        crate::scan::ConversionDecodeError::AnimatedGif
        | crate::scan::ConversionDecodeError::AnimatedPng
        | crate::scan::ConversionDecodeError::AnimatedWebP
        | crate::scan::ConversionDecodeError::AnimatedJpegXl => {
            "animated output is not a still image".into()
        }
    }
}

fn display_status(status: &CheckStatus) -> &'static str {
    match status {
        CheckStatus::Pass => "pass",
        CheckStatus::Fail => "fail",
        CheckStatus::NotChecked => "not checked",
        CheckStatus::NotApplicable => "not applicable",
        CheckStatus::NeedsReview => "needs review",
    }
}

fn display_observed(observed: &ObservedValue) -> String {
    match observed {
        ObservedValue::Format(format) => format.label().into(),
        ObservedValue::Dimensions { width, height } => format!("{width}x{height}"),
        ObservedValue::Bytes(bytes) => format!("{bytes} bytes"),
        ObservedValue::Alpha(value) => value.to_string(),
        ObservedValue::Name(name) => name.clone(),
        ObservedValue::Profile(value) => value.to_string(),
        ObservedValue::Depth(bits) => format!("{bits} bits"),
    }
}

fn utc_date() -> String {
    use std::time::{SystemTime, UNIX_EPOCH};
    let days = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map_or(0, |duration| duration.as_secs() / 86_400) as i64;
    let z = days + 719_468;
    let era = if z >= 0 { z } else { z - 146_096 } / 146_097;
    let doe = z - era * 146_097;
    let yoe = (doe - doe / 1_460 + doe / 36_524 - doe / 146_096) / 365;
    let mut year = yoe + era * 400;
    let doy = doe - (365 * yoe + yoe / 4 - yoe / 100);
    let month_part = (5 * doy + 2) / 153;
    let day = doy - (153 * month_part + 2) / 5 + 1;
    let month = month_part + if month_part < 10 { 3 } else { -9 };
    year += i64::from(month <= 2);
    format!("{year:04}-{month:02}-{day:02}")
}

#[cfg(test)]
mod tests {
    use super::*;
    use image::{ImageBuffer, ImageFormat, Rgb};

    fn snapshot(rules: Vec<Rule>) -> RequirementsSnapshot {
        RequirementsSnapshot {
            schema: SCHEMA_VERSION,
            id: "local-web".into(),
            name: "Local web output".into(),
            revision: 1,
            provenance: Provenance::LocalAuthor {
                author: "Igor".into(),
            },
            target: "website".into(),
            role: "hero".into(),
            category: None,
            region: None,
            last_verified: "2026-09-08".into(),
            effective_from: Some("2026-09-01".into()),
            effective_until: Some("2026-10-01".into()),
            engine: "press".into(),
            engine_version: 1,
            checker_version: 1,
            rules,
        }
    }

    fn rule(id: &str, required: bool, constraint: Constraint) -> Rule {
        Rule {
            id: id.into(),
            label: id.into(),
            required,
            constraint,
        }
    }

    #[test]
    fn parses_strictly_and_bounds_files_before_deserialize() {
        let mut value = serde_json::to_value(snapshot(vec![rule(
            "format",
            true,
            Constraint::Format {
                allowed: vec![ContentFormat::Png],
            },
        )]))
        .unwrap();
        value["unknown"] = serde_json::json!(true);
        assert!(parse_bytes(&serde_json::to_vec(&value).unwrap()).is_err());
        let mut nested = serde_json::to_value(snapshot(vec![rule(
            "format",
            true,
            Constraint::Format {
                allowed: vec![ContentFormat::Png],
            },
        )]))
        .unwrap();
        nested["rules"][0]["constraint"]["unknown"] = serde_json::json!(true);
        assert!(parse_bytes(&serde_json::to_vec(&nested).unwrap()).is_err());
        assert!(parse_bytes(&vec![b'x'; MAX_FILE_BYTES as usize + 1]).is_err());
    }

    #[test]
    fn parses_the_user_authored_reference_fixture() {
        let pack = parse_bytes(include_bytes!("../fixtures/requirements/local-web.json"))
            .expect("the local fixture follows the supported snapshot schema");
        assert_eq!(pack.id, "local-web-hero");
        assert_eq!(pack.rules.len(), 5);
    }

    #[test]
    fn rejects_an_engine_or_checker_version_this_build_cannot_interpret() {
        let mut pack = snapshot(vec![rule(
            "bytes",
            true,
            Constraint::Bytes {
                min: Some(1),
                max: None,
            },
        )]);
        pack.engine_version += 1;
        assert!(validate(&pack).is_err());
    }

    #[test]
    fn effective_interval_is_inclusive_then_exclusive() {
        let pack = snapshot(vec![rule(
            "bytes",
            true,
            Constraint::Bytes {
                min: Some(1),
                max: None,
            },
        )]);
        assert!(pack.is_effective("2026-09-01"));
        assert!(pack.is_effective("2026-09-30"));
        assert!(!pack.is_effective("2026-10-01"));
        assert!(!pack.is_effective("2026-08-31"));
    }

    #[test]
    fn content_format_is_checked_from_bytes_not_extension() {
        let dir = tempfile::tempdir().unwrap();
        let output = dir.path().join("optimized/result.webp");
        std::fs::create_dir_all(output.parent().unwrap()).unwrap();
        let image = ImageBuffer::<Rgb<u8>, _>::from_pixel(3, 2, Rgb([80, 90, 100]));
        image::DynamicImage::ImageRgb8(image)
            .save_with_format(&output, ImageFormat::Png)
            .unwrap();
        let pack = snapshot(vec![
            rule(
                "format",
                true,
                Constraint::Format {
                    allowed: vec![ContentFormat::Png],
                },
            ),
            rule(
                "name",
                true,
                Constraint::Name {
                    allowed_extensions: vec!["png".into()],
                },
            ),
        ]);
        let receipt = inspect_outputs_at(
            dir.path(),
            &[OutputRef {
                source: None,
                output: PathBuf::from("optimized/result.webp"),
                processing: None,
            }],
            &pack,
            "2026-09-08",
        )
        .unwrap();
        assert_eq!(receipt.outputs[0].actual.format, Some(ContentFormat::Png));
        assert_eq!(receipt.outputs[0].checks[0].status, CheckStatus::Pass);
        assert_eq!(receipt.outputs[0].checks[1].status, CheckStatus::Fail);
    }

    #[test]
    fn rejects_impossible_bounds() {
        let pack = snapshot(vec![rule(
            "dimensions",
            true,
            Constraint::Dimensions {
                min_width: Some(100),
                max_width: Some(10),
                min_height: None,
                max_height: None,
            },
        )]);
        assert!(validate(&pack).is_err());
    }

    #[test]
    fn required_unsupported_check_is_not_checked_and_cannot_pass() {
        let dir = tempfile::tempdir().unwrap();
        let output = dir.path().join("result.png");
        ImageBuffer::<Rgb<u8>, _>::from_pixel(2, 2, Rgb([1, 2, 3]))
            .save(&output)
            .unwrap();
        let pack = snapshot(vec![rule(
            "occupancy",
            true,
            Constraint::Unsupported {
                label: "subject occupancy".into(),
            },
        )]);
        let receipt = inspect_outputs_at(
            dir.path(),
            &[OutputRef {
                source: None,
                output: PathBuf::from("result.png"),
                processing: None,
            }],
            &pack,
            "2026-09-08",
        )
        .unwrap();
        assert_eq!(receipt.outputs[0].checks[0].status, CheckStatus::NotChecked);
        assert!(!receipt.all_required_pass);
    }

    #[test]
    fn a_missing_output_cannot_pass_bytes_or_name_rules() {
        let dir = tempfile::tempdir().unwrap();
        let pack = snapshot(vec![
            rule(
                "bytes",
                true,
                Constraint::Bytes {
                    min: Some(1),
                    max: None,
                },
            ),
            rule(
                "name",
                true,
                Constraint::Name {
                    allowed_extensions: vec!["png".into()],
                },
            ),
        ]);
        let receipt = inspect_outputs_at(
            dir.path(),
            &[OutputRef {
                source: None,
                output: PathBuf::from("missing.png"),
                processing: None,
            }],
            &pack,
            "2026-09-08",
        )
        .unwrap();
        assert_eq!(receipt.outputs[0].checks[0].status, CheckStatus::NotChecked);
        assert_eq!(receipt.outputs[0].checks[1].status, CheckStatus::Pass);
        assert!(!receipt.outputs[0].all_required_pass);
        assert!(!receipt.all_required_pass);
    }

    #[cfg(unix)]
    #[test]
    fn rejects_a_missing_output_below_a_symlinked_ancestor() {
        let root = tempfile::tempdir().unwrap();
        let outside = tempfile::tempdir().unwrap();
        std::os::unix::fs::symlink(outside.path(), root.path().join("linked")).unwrap();
        let pack = snapshot(vec![rule(
            "bytes",
            true,
            Constraint::Bytes {
                min: Some(1),
                max: None,
            },
        )]);
        let error = inspect_outputs_at(
            root.path(),
            &[OutputRef {
                source: None,
                output: PathBuf::from("linked/missing.png"),
                processing: None,
            }],
            &pack,
            "2026-09-08",
        )
        .expect_err("a symlinked parent must not become an external read");
        assert!(error.contains("outside"), "{error}");
    }

    #[test]
    fn rejects_a_literal_backslash_in_a_receipt_path() {
        let root = tempfile::tempdir().unwrap();
        let pack = snapshot(vec![rule(
            "bytes",
            true,
            Constraint::Bytes {
                min: Some(1),
                max: None,
            },
        )]);
        let error = inspect_outputs_at(
            root.path(),
            &[OutputRef {
                source: None,
                output: PathBuf::from(r"a\b.png"),
                processing: None,
            }],
            &pack,
            "2026-09-08",
        )
        .expect_err("portable receipt paths must not retarget a literal filename");
        assert!(error.contains("backslashes"), "{error}");
    }

    #[test]
    fn receipt_report_contains_relative_names_and_actual_hashes_only() {
        let dir = tempfile::tempdir().unwrap();
        let source = dir.path().join("source.png");
        let output = dir.path().join("optimized/output.png");
        std::fs::create_dir_all(output.parent().unwrap()).unwrap();
        ImageBuffer::<Rgb<u8>, _>::from_pixel(4, 3, Rgb([1, 2, 3]))
            .save(&source)
            .unwrap();
        ImageBuffer::<Rgb<u8>, _>::from_pixel(2, 2, Rgb([4, 5, 6]))
            .save(&output)
            .unwrap();
        let pack = snapshot(vec![
            rule(
                "format",
                true,
                Constraint::Format {
                    allowed: vec![ContentFormat::Png],
                },
            ),
            rule(
                "dimensions",
                true,
                Constraint::Dimensions {
                    min_width: Some(2),
                    max_width: None,
                    min_height: Some(2),
                    max_height: None,
                },
            ),
        ]);
        let receipt = inspect_outputs_at(
            dir.path(),
            &[OutputRef {
                source: Some(source),
                output,
                processing: Some(ProcessingIdentity {
                    recipe_id: Some("local".into()),
                    recipe_revision: Some(1),
                    recipe_fingerprint: Some("abc".into()),
                    processing_revision: 1,
                }),
            }],
            &pack,
            "2026-09-08",
        )
        .unwrap();
        let report = render_report(&receipt);
        assert!(report.contains("optimized/output.png"));
        assert!(report.contains("source.png"));
        assert!(!report.contains(dir.path().to_string_lossy().as_ref()));
        assert!(receipt.outputs[0].source_hash.is_some());
        assert!(receipt.outputs[0].output_hash.is_some());
        assert_eq!(receipt.outputs[0].actual.width, Some(2));
        assert_eq!(receipt.outputs[0].actual.height, Some(2));
    }
}
