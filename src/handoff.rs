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
