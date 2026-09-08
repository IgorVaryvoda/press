//! ImageGuide report import: a validated pending task, nothing else.
//!
//! First delivery is an explicit report file, not native messaging: the
//! producer exports a sanitized, bounded handoff and Press validates it into
//! a pending local task. Import performs no network and writes nothing; the
//! user chooses each local source root later, which is H2's mapping work.
//! Imported page text is untrusted data and stays data: it never becomes
//! instructions, paths to open, or upload authorization.

use serde::{Deserialize, Serialize};

/// The only envelope this Press reads. Newer refuses named, like jobs.
pub const SCHEMA_VERSION: u32 = 1;

/// A report larger than this is not a findings handoff. Checked on the raw
/// bytes before parsing allocates the full document.
pub const MAX_FILE_BYTES: u64 = 1024 * 1024;

/// Counts that keep one file from standing in for a database.
pub const MAX_RESOURCES: usize = 512;
pub const MAX_LIST_ENTRIES: usize = 64;
pub const MAX_STRING_CHARS: usize = 4096;
pub const MAX_REDACTIONS: usize = 128;

/// One validated handoff: the task the producer described, with its unknown
/// fields preserved verbatim and its warnings listed separately.
#[derive(Clone, Debug, PartialEq, Serialize)]
pub struct PendingHandoff {
    pub handoff: Handoff,
    pub warnings: Vec<String>,
}

/// The producer envelope. Unknown fields are preserved, never interpreted:
/// a future producer may add advisory metadata, and dropping it silently
/// would change what a later reader sees. Anything the importer must
/// understand and does not is a refusal, not a guess.
#[derive(Clone, Debug, PartialEq, Deserialize, Serialize)]
pub struct Handoff {
    pub schema: u32,
    pub producer: String,
    pub producer_revision: String,
    pub task: String,
    pub observed: String,
    pub resources: Vec<HandoffResource>,
    #[serde(default)]
    pub redactions: Vec<String>,
    #[serde(flatten, default)]
    pub unknown: serde_json::Map<String, serde_json::Value>,
}

/// One reported image and what the producer observed. URLs and path hints
/// are matching hints for a user-chosen root, not locations to fetch: H2
/// never downloads a report URL on import.
#[derive(Clone, Debug, PartialEq, Deserialize, Serialize)]
pub struct HandoffResource {
    pub id: String,
    pub urls: Vec<String>,
    pub path_hints: Vec<String>,
    pub width: Option<u32>,
    pub height: Option<u32>,
    pub bytes: Option<u64>,
    /// False means the producer estimated: never treat it as measured.
    #[serde(default)]
    pub bytes_measured: bool,
    pub findings: Vec<String>,
    pub max_edge: Option<u32>,
    pub formats: Vec<String>,
    #[serde(flatten, default)]
    pub unknown: serde_json::Map<String, serde_json::Value>,
}

/// Validate report bytes into a pending task. Pure bytes in, data out: no
/// filesystem reads, no network, no writes.
pub fn parse_bytes(bytes: &[u8]) -> Result<PendingHandoff, String> {
    if bytes.len() as u64 > MAX_FILE_BYTES {
        return Err(format!(
            "handoff files larger than {MAX_FILE_BYTES} bytes are refused"
        ));
    }
    let handoff: Handoff = serde_json::from_slice(bytes)
        .map_err(|error| format!("handoff does not parse: {error}"))?;
    validate(&handoff)?;
    Ok(PendingHandoff {
        warnings: warn(&handoff),
        handoff,
    })
}

fn check_string(value: &str, what: &str) -> Result<(), String> {
    if value.chars().count() > MAX_STRING_CHARS {
        return Err(format!(
            "{what} is longer than {MAX_STRING_CHARS} characters"
        ));
    }
    Ok(())
}

fn check_list(values: &[String], what: &str) -> Result<(), String> {
    if values.len() > MAX_LIST_ENTRIES {
        return Err(format!("{what} lists more than {MAX_LIST_ENTRIES} entries"));
    }
    for value in values {
        check_string(value, what)?;
    }
    Ok(())
}

/// A URL carrying user-info leaks credentials into a matching hint. Only the
/// authority part counts: a `@` in a path is a legal, if odd, character.
fn has_user_info(url: &str) -> bool {
    let Some(after_scheme) = url.split_once("://").map(|(_, rest)| rest) else {
        return false;
    };
    let authority = after_scheme.split('/').next().unwrap_or(after_scheme);
    authority.contains('@')
}

fn validate(handoff: &Handoff) -> Result<(), String> {
    if handoff.schema != SCHEMA_VERSION {
        return Err(format!(
            "unsupported handoff schema {} (this Press reads schema {SCHEMA_VERSION})",
            handoff.schema
        ));
    }
    for (value, what) in [
        (&handoff.producer, "producer"),
        (&handoff.producer_revision, "producer revision"),
        (&handoff.task, "task"),
        (&handoff.observed, "observation time"),
    ] {
        if value.trim().is_empty() {
            return Err(format!("a handoff needs a {what}"));
        }
        check_string(value, what)?;
    }
    if handoff.resources.is_empty() {
        return Err("a handoff with no resources names no work".into());
    }
    if handoff.resources.len() > MAX_RESOURCES {
        return Err(format!("a handoff holds at most {MAX_RESOURCES} resources"));
    }
    if handoff.redactions.len() > MAX_REDACTIONS {
        return Err(format!(
            "a handoff lists at most {MAX_REDACTIONS} redactions"
        ));
    }
    for redaction in &handoff.redactions {
        check_string(redaction, "redactions")?;
    }
    let mut ids = std::collections::HashSet::new();
    for resource in &handoff.resources {
        if resource.id.trim().is_empty() {
            return Err("every resource needs an id".into());
        }
        check_string(&resource.id, "resource id")?;
        if !ids.insert(resource.id.as_str()) {
            return Err(format!("duplicate resource id {:?}", resource.id));
        }
        check_list(&resource.urls, "resource urls")?;
        check_list(&resource.path_hints, "resource path hints")?;
        check_list(&resource.findings, "resource findings")?;
        check_list(&resource.formats, "resource formats")?;
        for url in &resource.urls {
            if has_user_info(url) {
                return Err(format!(
                    "resource {:?} carries user-info in its URL",
                    resource.id
                ));
            }
        }
        if resource.width == Some(0) || resource.height == Some(0) {
            return Err(format!("resource {:?} has a zero dimension", resource.id));
        }
    }
    Ok(())
}

/// Visible inconvenience, not refusal: preserved unknowns, fragments kept as
/// hints, and constraint formats outside the converters' vocabulary.
fn warn(handoff: &Handoff) -> Vec<String> {
    let mut warnings = Vec::new();
    for key in handoff.unknown.keys() {
        warnings.push(format!(
            "kept unknown task field {key:?} without reading it"
        ));
    }
    for resource in &handoff.resources {
        for key in resource.unknown.keys() {
            warnings.push(format!(
                "kept unknown field {key:?} on resource {:?} without reading it",
                resource.id
            ));
        }
        if resource.urls.iter().any(|url| url.contains('#')) {
            warnings.push(format!(
                "resource {:?} keeps a URL fragment as a matching hint only",
                resource.id
            ));
        }
        for format in &resource.formats {
            if !matches!(format.as_str(), "webp" | "avif" | "jxl" | "jpeg" | "png") {
                warnings.push(format!(
                    "resource {:?} asks for unsupported format {format:?}",
                    resource.id
                ));
            }
        }
    }
    warnings
}

/// Whether one mapped resource verifies against the deployed tree.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum DeployStatus {
    /// Exactly one deployed file answers to the mapping and meets its
    /// format and size constraints.
    Deployed,
    /// Files answer to the name but violate a constraint.
    Differs,
    /// Nothing under the deployed root answers to the mapping.
    Missing,
    /// Several deployed files meet the constraints: a choice, not a badge.
    Ambiguous,
}

/// One resource's deployment verdict. Findings ride along as open items:
/// an arrived file proves delivery, never that markup or review concerns
/// are closed.
#[derive(Clone, Debug, PartialEq, Serialize)]
pub struct DeploymentCheck {
    pub id: String,
    pub status: DeployStatus,
    pub deployed: Vec<String>,
    pub notes: Vec<String>,
    pub findings: Vec<String>,
}

/// The deployed entry's format in the vocabulary constraints use. Content,
/// never the extension.
fn entry_format(entry: &crate::scan::Entry) -> &'static str {
    match entry.format {
        crate::scan::FileFormat::JpegXl => "jxl",
        crate::scan::FileFormat::Image(format) => {
            if format == image::ImageFormat::Png {
                "png"
            } else if format == image::ImageFormat::Jpeg {
                "jpeg"
            } else if format == image::ImageFormat::WebP {
                "webp"
            } else if format == image::ImageFormat::Avif {
                "avif"
            } else if format == image::ImageFormat::Gif {
                "gif"
            } else if format == image::ImageFormat::Bmp {
                "bmp"
            } else if format == image::ImageFormat::Tiff {
                "tiff"
            } else {
                "unknown"
            }
        }
    }
}

fn stem_of(path: &std::path::Path) -> String {
    path.file_stem()
        .map(|stem| stem.to_string_lossy().to_lowercase())
        .unwrap_or_default()
}

/// What one deployed candidate violates, if anything. An empty list verifies.
fn constraint_violations(resource: &HandoffResource, entry: &crate::scan::Entry) -> Vec<String> {
    let mut violations = Vec::new();
    if !resource.formats.is_empty()
        && !resource
            .formats
            .iter()
            .any(|format| format.eq_ignore_ascii_case(entry_format(entry)))
    {
        violations.push(format!(
            "{} is {}, expected {}",
            entry.name(),
            entry_format(entry),
            resource.formats.join("/")
        ));
    }
    if let Some(edge) = resource.max_edge {
        let longest = entry.width.max(entry.height);
        if longest > edge {
            violations.push(format!("{longest}px exceeds the {edge}px limit"));
        }
    }
    violations
}

/// Verify every pinned-down mapping against a scan of the deployed tree.
/// Only confirmed and candidate mappings carry exactly one local file, so
/// only they verify: ambiguous mappings need confirmation first, and the
/// rest name no file to look for. Association is by file stem, exactly or
/// with a `-suffix`/`_suffix` derivative name — never by bytes, which
/// re-encoding changes, and never by resemblance.
pub fn verify_deployment(
    handoff: &Handoff,
    mappings: &[Mapping],
    deployed: &[crate::scan::Entry],
) -> Vec<DeploymentCheck> {
    let mut checks = Vec::new();
    for mapping in mappings {
        if !matches!(mapping.verdict, Verdict::Confirmed | Verdict::Candidate)
            || mapping.paths.len() != 1
        {
            continue;
        }
        let Some(resource) = handoff
            .resources
            .iter()
            .find(|known| known.id == mapping.id)
        else {
            continue;
        };
        let stem = stem_of(std::path::Path::new(&mapping.paths[0]));
        if stem.is_empty() {
            continue;
        }
        let mut exact = Vec::new();
        let mut suffixed = Vec::new();
        for entry in deployed {
            let candidate = stem_of(&entry.path);
            if candidate == stem {
                exact.push(entry);
            } else if candidate.starts_with(&format!("{stem}-"))
                || candidate.starts_with(&format!("{stem}_"))
            {
                suffixed.push(entry);
            }
        }
        // An exact stem wins over derivatives: the derivative set only
        // matters when no file kept the name.
        let pool = if exact.is_empty() { suffixed } else { exact };
        if pool.is_empty() {
            checks.push(DeploymentCheck {
                id: mapping.id.clone(),
                status: DeployStatus::Missing,
                deployed: Vec::new(),
                notes: Vec::new(),
                findings: resource.findings.clone(),
            });
            continue;
        }
        let mut passing = Vec::new();
        let mut violations = Vec::new();
        for &entry in &pool {
            let mut entry_violations = constraint_violations(resource, entry);
            if entry_violations.is_empty() {
                passing.push(entry);
            } else {
                violations.append(&mut entry_violations);
            }
        }
        let (status, deployed, mut notes) = match passing.len() {
            1 => (
                DeployStatus::Deployed,
                vec![display(passing[0].path.clone())],
                Vec::new(),
            ),
            0 => (
                DeployStatus::Differs,
                pool.iter().map(|entry| display(&entry.path)).collect(),
                violations,
            ),
            _ => (
                DeployStatus::Ambiguous,
                passing.iter().map(|entry| display(&entry.path)).collect(),
                vec![format!(
                    "{} deployed files meet the constraints",
                    passing.len()
                )],
            ),
        };
        if notes.len() > 4 {
            let hidden = notes.len() - 3;
            notes.truncate(3);
            notes.push(format!("…and {hidden} more"));
        }
        checks.push(DeploymentCheck {
            id: mapping.id.clone(),
            status,
            deployed,
            notes,
            findings: resource.findings.clone(),
        });
    }
    checks
}

#[cfg(test)]
mod tests {
    use super::*;

    fn resource(id: &str) -> HandoffResource {
        HandoffResource {
            id: id.into(),
            urls: vec!["https://example.com/hero.jpg?w=1600".into()],
            path_hints: vec!["images/hero.jpg".into()],
            width: Some(1600),
            height: Some(1200),
            bytes: Some(240_000),
            bytes_measured: true,
            findings: vec!["excess-dimensions".into()],
            max_edge: Some(1600),
            formats: vec!["avif".into()],
            unknown: serde_json::Map::new(),
        }
    }

    fn envelope() -> serde_json::Value {
        serde_json::json!({
            "schema": SCHEMA_VERSION,
            "producer": "imageguide-extension",
            "producer_revision": "report-schema-4",
            "task": "task-1",
            "observed": "2026-09-08T12:00:00Z",
            "resources": [resource("hero")],
            "redactions": ["query values dropped"],
        })
    }

    fn pending(value: serde_json::Value) -> Result<PendingHandoff, String> {
        parse_bytes(&serde_json::to_vec(&value).unwrap())
    }

    #[test]
    fn a_sanitized_fixture_imports_to_a_pending_task() {
        let pending = pending(envelope()).unwrap();
        assert_eq!(pending.handoff.task, "task-1");
        assert_eq!(pending.handoff.resources.len(), 1);
        assert!(pending.warnings.is_empty(), "{:?}", pending.warnings);
        // The validated task re-serializes for whatever H2 stages next.
        let again = parse_bytes(&serde_json::to_vec(&pending.handoff).unwrap()).unwrap();
        assert_eq!(again.handoff, pending.handoff);
    }

    #[test]
    fn unknown_fields_ride_along_without_being_read() {
        let mut value = envelope();
        value["retention"] = serde_json::json!("ninety days");
        value["resources"][0]["density"] = serde_json::json!("2x");
        let pending = pending(value).unwrap();
        assert_eq!(pending.handoff.unknown.len(), 1);
        assert_eq!(pending.handoff.resources[0].unknown.len(), 1);
        assert_eq!(pending.warnings.len(), 2);
    }

    #[test]
    fn future_schemas_refuse_by_number() {
        let mut value = envelope();
        value["schema"] = serde_json::json!(SCHEMA_VERSION + 1);
        let refused = pending(value).unwrap_err();
        assert!(refused.contains("unsupported handoff schema"), "{refused}");
    }

    #[test]
    fn oversize_reports_refuse_before_they_matter() {
        let big = vec![7u8; MAX_FILE_BYTES as usize + 1];
        assert!(parse_bytes(&big).is_err());
        let mut value = envelope();
        value["resources"] = serde_json::Value::Array(
            (0..MAX_RESOURCES + 1)
                .map(|index| {
                    let mut resource = serde_json::to_value(resource("x")).unwrap();
                    resource["id"] = serde_json::json!(format!("r-{index}"));
                    resource
                })
                .collect(),
        );
        let refused = pending(value).unwrap_err();
        assert!(refused.contains(&MAX_RESOURCES.to_string()), "{refused}");
    }

    #[test]
    fn credentialed_urls_and_anonymous_resources_refuse() {
        let mut value = envelope();
        value["resources"][0]["urls"] = serde_json::json!(["https://user:pass@example.com/x.jpg"]);
        let refused = pending(value).unwrap_err();
        assert!(refused.contains("user-info"), "{refused}");
        let mut value = envelope();
        value["resources"][0]["id"] = serde_json::json!("  ");
        assert!(pending(value).is_err());
        let mut value = envelope();
        value["resources"] = serde_json::json!([]);
        assert!(pending(value).is_err());
        let mut twice = envelope();
        twice["resources"] = serde_json::json!([resource("same"), resource("same")]);
        let refused = pending(twice).unwrap_err();
        assert!(refused.contains("duplicate"), "{refused}");
    }

    #[test]
    fn fragments_warn_and_unknown_formats_warn() {
        let mut value = envelope();
        value["resources"][0]["urls"] = serde_json::json!(["https://example.com/p#hero"]);
        value["resources"][0]["formats"] = serde_json::json!(["bmp"]);
        let pending = pending(value).unwrap();
        assert_eq!(pending.warnings.len(), 2);
    }
}

/// How one reported resource resolved against a user-chosen root.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum Verdict {
    /// A path hint named exactly one existing file under the root.
    Confirmed,
    /// One filename match: plausible, but only a human confirms it.
    Candidate,
    /// Several files match: a choice, never a guess.
    Ambiguous,
    /// Nothing under the root answers to this resource.
    Unmatched,
    /// The hints point at a different machine layout, not at this root.
    OutOfScope,
}

/// One resource's resolution: its verdict, the local paths behind it, and
/// observations that do not change the verdict but travel with it.
#[derive(Clone, Debug, PartialEq, Serialize)]
pub struct Mapping {
    pub id: String,
    pub verdict: Verdict,
    pub paths: Vec<String>,
    pub notes: Vec<String>,
}

/// Join a hint onto the root without leaving it lexically: absolute, rooted
/// and parent-escaping hints refuse before touching the filesystem.
fn confined_join(root: &std::path::Path, hint: &str) -> Option<std::path::PathBuf> {
    use std::path::Component;
    let hint = std::path::Path::new(hint);
    if hint.is_absolute() || hint.has_root() {
        return None;
    }
    let mut depth = 0u32;
    let mut joined = root.to_path_buf();
    for component in hint.components() {
        match component {
            Component::Normal(part) => {
                depth += 1;
                joined.push(part);
            }
            Component::CurDir => {}
            Component::ParentDir => {
                depth = depth.checked_sub(1)?;
                joined.pop();
            }
            _ => return None,
        }
    }
    Some(joined)
}

/// Whether `path` resolves to a real file still under `root`. Symlinks
/// resolve before the comparison, so a link pointing out reads as absent.
/// Anything unresolvable reads as absent too: mapping advises on files it
/// can actually open, and the row stays available to manual work regardless.
fn resolves_within(root: &std::path::Path, path: &std::path::Path) -> bool {
    if !path
        .symlink_metadata()
        .is_ok_and(|metadata| metadata.is_file())
    {
        return false;
    }
    match (std::fs::canonicalize(path), std::fs::canonicalize(root)) {
        (Ok(resolved), Ok(confined)) => resolved.starts_with(&confined),
        _ => false,
    }
}

/// The joined hint as a real file still under the root.
fn existing_file(root: &std::path::Path, hint: &str) -> Option<std::path::PathBuf> {
    let candidate = confined_join(root, hint)?;
    resolves_within(root, &candidate).then_some(candidate)
}

fn basename(hint: &str) -> &str {
    hint.rsplit(['/', '\\']).next().unwrap_or(hint)
}

/// Resolve every resource against scanned entries under `root`. Reads
/// metadata, never image bytes and never the network. Mapping never converts:
/// candidates wait for explicit confirmation, which is later work consuming
/// this report.
pub fn map_to_root(
    handoff: &Handoff,
    root: &std::path::Path,
    entries: &[crate::scan::Entry],
) -> Vec<Mapping> {
    handoff
        .resources
        .iter()
        .map(|resource| map_one(resource, root, entries))
        .collect()
}

fn map_one(
    resource: &HandoffResource,
    root: &std::path::Path,
    entries: &[crate::scan::Entry],
) -> Mapping {
    let mut notes = Vec::new();
    let mut hits: Vec<std::path::PathBuf> = Vec::new();
    let mut elsewhere = false;
    for hint in resource.path_hints.iter().chain(resource.urls.iter()) {
        let path = std::path::Path::new(hint);
        if path.is_absolute() || path.has_root() {
            // An absolute hint under this root is still usable; anywhere else
            // it describes a different machine layout, not this folder.
            if let Ok(relative) = path.strip_prefix(root)
                && let Some(relative) = relative.to_str()
                && let Some(hit) = existing_file(root, relative)
            {
                push_hit(&mut hits, hit);
                continue;
            }
            elsewhere = true;
            continue;
        }
        if let Some(hit) = existing_file(root, hint) {
            push_hit(&mut hits, hit);
        }
    }
    if hits.len() == 1 {
        let single = hits.remove(0);
        corroborate(resource, entries, &single, &mut notes);
        return Mapping {
            id: resource.id.clone(),
            verdict: Verdict::Confirmed,
            paths: vec![display(&single)],
            notes,
        };
    }
    if hits.len() > 1 {
        // Several hints named several files: a choice between them, never a
        // silent merge into one resource.
        return Mapping {
            id: resource.id.clone(),
            verdict: Verdict::Ambiguous,
            paths: capped_paths(hits),
            notes,
        };
    }
    // No hint named a file: fall back to filename matching over the scan.
    let mut candidates = Vec::new();
    for hint in resource.path_hints.iter().chain(resource.urls.iter()) {
        match crate::job::match_filename(basename(hint), entries) {
            crate::job::Match::Exact(path) => {
                if !candidates.contains(&path) {
                    candidates.push(path);
                }
            }
            crate::job::Match::Ambiguous(mut paths) => {
                for path in paths.drain(..) {
                    if !candidates.contains(&path) {
                        candidates.push(path);
                    }
                }
            }
            crate::job::Match::Missing => {}
        }
    }
    // A name match outside the root is not a candidate: following it would
    // read where the user did not point.
    candidates.retain(|path| resolves_within(root, path));
    match candidates.len() {
        0 if elsewhere => Mapping {
            id: resource.id.clone(),
            verdict: Verdict::OutOfScope,
            paths: Vec::new(),
            notes,
        },
        0 => Mapping {
            id: resource.id.clone(),
            verdict: Verdict::Unmatched,
            paths: Vec::new(),
            notes,
        },
        1 => {
            let single = candidates.remove(0);
            corroborate(resource, entries, &single, &mut notes);
            Mapping {
                id: resource.id.clone(),
                verdict: Verdict::Candidate,
                paths: vec![display(&single)],
                notes,
            }
        }
        _ => Mapping {
            id: resource.id.clone(),
            verdict: Verdict::Ambiguous,
            paths: capped(candidates.into_iter().map(display).collect()),
            notes,
        },
    }
}

fn push_hit(hits: &mut Vec<std::path::PathBuf>, hit: std::path::PathBuf) {
    if !hits.contains(&hit) {
        hits.push(hit);
    }
}

fn display(path: impl AsRef<std::path::Path>) -> String {
    path.as_ref().display().to_string()
}

/// Long candidate lists truncate with their count kept: the report stays a
/// report, not a directory listing.
fn capped(mut paths: Vec<String>) -> Vec<String> {
    const SHOWN: usize = 8;
    if paths.len() > SHOWN {
        let hidden = paths.len() - SHOWN;
        paths.truncate(SHOWN);
        paths.push(format!("…and {hidden} more"));
    }
    paths
}

fn capped_paths(paths: Vec<std::path::PathBuf>) -> Vec<String> {
    capped(paths.into_iter().map(display).collect())
}

/// Compare the report's measurements against the scanned entry, when both
/// exist. A mismatch is a note, never a verdict change: the local file is
/// what it is, and H3 decides what a changed source means.
fn corroborate(
    resource: &HandoffResource,
    entries: &[crate::scan::Entry],
    path: &std::path::Path,
    notes: &mut Vec<String>,
) {
    let Some(entry) = entries.iter().find(|entry| entry.path == path) else {
        return;
    };
    if let (Some(width), Some(height)) = (resource.width, resource.height)
        && (entry.width != width || entry.height != height)
    {
        notes.push(format!(
            "local {}x{} differs from reported {width}x{height}",
            entry.width, entry.height
        ));
    }
    if resource.bytes_measured
        && let Some(bytes) = resource.bytes
        && entry.bytes != bytes
    {
        notes.push(format!(
            "local {} bytes differ from measured {bytes}",
            entry.bytes
        ));
    }
}

#[cfg(test)]
mod mapping_tests {
    use super::*;

    fn workdir(tag: &str) -> std::path::PathBuf {
        let dir =
            std::env::temp_dir().join(format!("press-handoff-map-{tag}-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).expect("the fixture dir is created");
        dir
    }

    fn write(dir: &std::path::Path, name: &str) -> std::path::PathBuf {
        let path = dir.join(name);
        if let Some(parent) = path.parent() {
            std::fs::create_dir_all(parent).expect("the fixture parent is created");
        }
        std::fs::write(&path, b"fixture-bytes").expect("the fixture file is written");
        path
    }

    fn entry(path: &std::path::Path, width: u32, height: u32) -> crate::scan::Entry {
        crate::scan::Entry {
            path: path.to_path_buf(),
            format: image::ImageFormat::Png.into(),
            width,
            height,
            bytes: std::fs::metadata(path)
                .map(|metadata| metadata.len())
                .unwrap_or(0),
        }
    }

    fn named(id: &str, hints: &[&str]) -> HandoffResource {
        HandoffResource {
            id: id.into(),
            urls: Vec::new(),
            path_hints: hints.iter().map(|hint| hint.to_string()).collect(),
            width: None,
            height: None,
            bytes: None,
            bytes_measured: false,
            findings: Vec::new(),
            max_edge: None,
            formats: Vec::new(),
            unknown: serde_json::Map::new(),
        }
    }

    fn handoff_with(resources: Vec<HandoffResource>) -> Handoff {
        Handoff {
            schema: SCHEMA_VERSION,
            producer: "imageguide-extension".into(),
            producer_revision: "report-schema-4".into(),
            task: "task-1".into(),
            observed: "2026-09-08T12:00:00Z".into(),
            resources,
            redactions: Vec::new(),
            unknown: serde_json::Map::new(),
        }
    }

    fn map(
        handoff: &Handoff,
        root: &std::path::Path,
        files: &[std::path::PathBuf],
    ) -> Vec<Mapping> {
        let entries: Vec<crate::scan::Entry> =
            files.iter().map(|path| entry(path, 800, 600)).collect();
        map_to_root(handoff, root, &entries)
    }

    #[test]
    fn an_exact_hint_confirms_its_file() {
        let dir = workdir("exact");
        let hero = write(&dir, "hero.jpg");
        let mappings = map(
            &handoff_with(vec![named("hero", &["hero.jpg"])]),
            &dir,
            &[hero],
        );
        assert_eq!(mappings[0].verdict, Verdict::Confirmed);
        assert_eq!(mappings[0].paths.len(), 1);
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn a_filename_match_is_a_candidate_until_confirmed() {
        let dir = workdir("candidate");
        let hero = write(&dir, "shoots/hero-final.jpg");
        // The hint names a file that is not there; its basename matches one
        // file that is. Plausible, not proven.
        let mappings = map(
            &handoff_with(vec![named("hero", &["images/hero.jpg"])]),
            &dir,
            &[hero],
        );
        assert_eq!(mappings[0].verdict, Verdict::Candidate);
        assert_eq!(mappings[0].paths.len(), 1);
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn duplicate_basenames_stay_ambiguous() {
        let dir = workdir("ambiguous");
        let one = write(&dir, "a/hero.jpg");
        let two = write(&dir, "b/hero.jpg");
        let mappings = map(
            &handoff_with(vec![named("hero", &["hero.jpg"])]),
            &dir,
            &[one, two],
        );
        // Both hints are absent, and the basename matches twice: no guess.
        assert_eq!(mappings[0].verdict, Verdict::Ambiguous);
        assert_eq!(mappings[0].paths.len(), 2);
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn nothing_matching_is_unmatched_not_missing() {
        let dir = workdir("unmatched");
        let other = write(&dir, "other.jpg");
        let mappings = map(
            &handoff_with(vec![named("hero", &["hero.jpg"])]),
            &dir,
            &[other],
        );
        assert_eq!(mappings[0].verdict, Verdict::Unmatched);
        assert!(mappings[0].paths.is_empty());
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn hints_at_another_machine_are_out_of_scope() {
        let dir = workdir("scope");
        let other = write(&dir, "other.jpg");
        let mappings = map(
            &handoff_with(vec![named("hero", &["/elsewhere/hero.jpg"])]),
            &dir,
            &[other],
        );
        assert_eq!(mappings[0].verdict, Verdict::OutOfScope);
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn escaping_hints_never_leave_the_root() {
        let dir = workdir("escape");
        // The file exists outside the root: only lexical refusal keeps the
        // verdict unmatched instead of confirming through the climb.
        write(&dir, "outside.jpg");
        let mappings = map(
            &handoff_with(vec![named("hero", &["../outside.jpg"])]),
            &dir.join("sub"),
            &[],
        );
        assert_eq!(mappings[0].verdict, Verdict::Unmatched);
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn measurements_that_disagree_become_notes() {
        let dir = workdir("notes");
        let hero = write(&dir, "hero.jpg");
        let mut resource = named("hero", &["hero.jpg"]);
        resource.width = Some(1600);
        resource.height = Some(1200);
        resource.bytes = Some(999_999);
        resource.bytes_measured = true;
        // The entry is 800x600 and far smaller: the hint still identifies
        // the file, but the disagreement travels with the mapping.
        let mappings = map(&handoff_with(vec![resource]), &dir, &[hero]);
        assert_eq!(mappings[0].verdict, Verdict::Confirmed);
        assert_eq!(mappings[0].notes.len(), 2);
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[cfg(unix)]
    #[test]
    fn a_link_pointing_out_reads_as_absent() {
        let dir = workdir("link");
        let outside = write(&dir, "outside.jpg");
        let root = dir.join("root");
        std::fs::create_dir_all(&root).expect("the root is created");
        std::os::unix::fs::symlink(&outside, root.join("link.jpg")).unwrap();
        let mappings = map(
            &handoff_with(vec![named("hero", &["link.jpg"])]),
            &root,
            &[root.join("link.jpg")],
        );
        assert_eq!(mappings[0].verdict, Verdict::Unmatched);
        let _ = std::fs::remove_dir_all(&dir);
    }

    fn deployed(
        path: &std::path::Path,
        format: image::ImageFormat,
        width: u32,
        height: u32,
    ) -> crate::scan::Entry {
        crate::scan::Entry {
            path: path.to_path_buf(),
            format: format.into(),
            width,
            height,
            bytes: 1000,
        }
    }

    fn constrained(
        id: &str,
        local: &str,
        formats: &[&str],
        max_edge: Option<u32>,
    ) -> (Handoff, Vec<Mapping>) {
        let mut resource = named(id, &[]);
        resource.formats = formats.iter().map(|format| format.to_string()).collect();
        resource.max_edge = max_edge;
        resource.findings = vec!["excess-dimensions".into()];
        let handoff = handoff_with(vec![resource]);
        let mapping = Mapping {
            id: id.into(),
            verdict: Verdict::Confirmed,
            paths: vec![local.into()],
            notes: Vec::new(),
        };
        (handoff, vec![mapping])
    }

    #[test]
    fn an_exact_deployed_derivative_verifies() {
        let dir = workdir("deployed");
        let local = dir.join("photos").join("hero.jpg");
        let (handoff, mappings) =
            constrained("hero", &local.to_string_lossy(), &["avif"], Some(1600));
        let out = dir.join("out");
        let checks = verify_deployment(
            &handoff,
            &mappings,
            &[deployed(
                &out.join("hero.avif"),
                image::ImageFormat::Avif,
                1200,
                800,
            )],
        );
        assert_eq!(checks[0].status, DeployStatus::Deployed);
        assert_eq!(checks[0].deployed.len(), 1);
        assert_eq!(checks[0].findings, vec!["excess-dimensions".to_string()]);
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn a_wrong_format_or_size_differs() {
        let dir = workdir("differs");
        let local = dir.join("photos").join("hero.jpg");
        let (handoff, mappings) =
            constrained("hero", &local.to_string_lossy(), &["jpeg"], Some(1600));
        let out = dir.join("out");
        // Avif where JPEG was asked: found, but violating.
        let checks = verify_deployment(
            &handoff,
            &mappings,
            &[deployed(
                &out.join("hero.avif"),
                image::ImageFormat::Avif,
                1200,
                800,
            )],
        );
        assert_eq!(checks[0].status, DeployStatus::Differs);
        assert!(checks[0].notes.iter().any(|note| note.contains("jpeg")));
        // Right format, over the size limit.
        let (handoff, mappings) = constrained("hero", &local.to_string_lossy(), &[], Some(1000));
        let checks = verify_deployment(
            &handoff,
            &mappings,
            &[deployed(
                &out.join("hero.avif"),
                image::ImageFormat::Avif,
                1200,
                800,
            )],
        );
        assert_eq!(checks[0].status, DeployStatus::Differs);
        assert!(checks[0].notes.iter().any(|note| note.contains("1000px")));
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn suffixed_derivatives_verify_and_exact_wins() {
        let dir = workdir("suffix");
        let local = dir.join("photos").join("hero.jpg");
        let out = dir.join("out");
        let (handoff, mappings) =
            constrained("hero", &local.to_string_lossy(), &["avif"], Some(1600));
        // No exact stem: the suffixed derivative verifies.
        let checks = verify_deployment(
            &handoff,
            &mappings,
            &[deployed(
                &out.join("hero-1600.avif"),
                image::ImageFormat::Avif,
                1200,
                800,
            )],
        );
        assert_eq!(checks[0].status, DeployStatus::Deployed);
        // An exact stem exists but violates: derivatives do not rescue it.
        let checks = verify_deployment(
            &handoff,
            &mappings,
            &[
                deployed(&out.join("hero.png"), image::ImageFormat::Png, 1200, 800),
                deployed(
                    &out.join("hero-1600.avif"),
                    image::ImageFormat::Avif,
                    1200,
                    800,
                ),
            ],
        );
        assert_eq!(checks[0].status, DeployStatus::Differs);
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn two_passing_derivatives_stay_ambiguous_and_absent_is_missing() {
        let dir = workdir("amb-deploy");
        let local = dir.join("photos").join("hero.jpg");
        let out = dir.join("out");
        let (handoff, mappings) = constrained("hero", &local.to_string_lossy(), &["avif"], None);
        let checks = verify_deployment(
            &handoff,
            &mappings,
            &[
                deployed(
                    &out.join("hero-1600.avif"),
                    image::ImageFormat::Avif,
                    1200,
                    800,
                ),
                deployed(
                    &out.join("hero_800.avif"),
                    image::ImageFormat::Avif,
                    800,
                    600,
                ),
            ],
        );
        assert_eq!(checks[0].status, DeployStatus::Ambiguous);
        let checks = verify_deployment(&handoff, &mappings, &[]);
        assert_eq!(checks[0].status, DeployStatus::Missing);
        // Unpinned mappings verify nothing at all.
        let mut floating = mappings;
        floating[0].verdict = Verdict::Ambiguous;
        floating[0].paths = vec!["a".into(), "b".into()];
        assert!(verify_deployment(&handoff, &floating, &[]).is_empty());
        let _ = std::fs::remove_dir_all(&dir);
    }
}
