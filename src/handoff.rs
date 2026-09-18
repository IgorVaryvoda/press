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

/// How one mapped resource fares against a second local folder. Every
/// variant is evidence about filenames, formats and pixel dimensions on
/// this machine; none of them says a website serves those bytes.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum LocalCheckStatus {
    /// Exactly one local file answers to the mapping's stem and meets its
    /// format and size constraints.
    LocalMatch,
    /// Files answer to the name but violate a constraint.
    Differs,
    /// Nothing under the checked folder answers to the mapping.
    Missing,
    /// Several local files meet the constraints: a choice, not a badge.
    Ambiguous,
    /// The mapping pinned down no single local file, so nothing was looked
    /// for. An unmatched, ambiguous or out-of-scope resource leaves a hole in
    /// the checklist, and a hole is never a pass.
    NotChecked,
}

/// One resource's local-file verdict. Findings ride along as open items:
/// a file sitting on disk under the right name closes neither the page's
/// markup work nor any review concern.
#[derive(Clone, Debug, PartialEq, Serialize)]
pub struct LocalCheck {
    pub id: String,
    pub status: LocalCheckStatus,
    pub paths: Vec<String>,
    pub notes: Vec<String>,
    pub findings: Vec<String>,
}

/// What the local check looked at, carried beside the results so a JSON
/// reader never has to infer the boundary. It is repeated verbatim in the
/// text output.
pub const LOCAL_EVIDENCE_SCOPE: &str = "filenames, formats and pixel dimensions of files in the given local folder; \
not a live site, image identity, markup or a re-audit";

/// A scanned entry's format in the vocabulary constraints use. Content,
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

/// What one local candidate violates, if anything. An empty list passes.
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

/// Why one resource was not checked, named rather than counted. The wording
/// stays in the mapping's own vocabulary so a reader can trace the hole back
/// to the verdict that caused it.
fn unchecked_reason(mapping: Option<&Mapping>) -> String {
    let Some(mapping) = mapping else {
        return "no mapping under the source root: nothing was looked for".to_string();
    };
    match mapping.verdict {
        Verdict::PathMatch | Verdict::Candidate => {
            if mapping.paths.len() == 1 {
                "the mapped path has no filename to look for".to_string()
            } else {
                format!(
                    "the {} mapping names {} files, not one",
                    verdict_word(mapping.verdict),
                    mapping.paths.len()
                )
            }
        }
        verdict => format!(
            "{} under the source root: no single local file to look for",
            verdict_word(verdict)
        ),
    }
}

/// Check the report against a scan of a second local folder, one row per
/// resource in report order. Only path-match and candidate mappings carry
/// exactly one local file, so only they are looked for; ambiguous mappings
/// name several and the rest name none, and those resources come back
/// `NotChecked` rather than absent, so the caller can see the checklist is
/// short. Association is by file stem, exactly or with a `-suffix`/`_suffix`
/// derivative name — never by bytes, which re-encoding changes, and never by
/// resemblance.
///
/// This reads a directory on this machine. It cannot see what a website
/// serves, so a `LocalMatch` says a suitably named, suitably shaped file
/// exists locally and nothing more: not that those bytes were published,
/// not that the file is the reported image, not that the page's markup,
/// viewport or saving model changed, and not that anything was re-audited.
/// The mapping itself is still automatic name evidence, not a person's
/// confirmation, and this check does not upgrade it.
pub fn check_local_files(
    handoff: &Handoff,
    mappings: &[Mapping],
    local: &[crate::scan::Entry],
) -> Vec<LocalCheck> {
    let mut checks = Vec::new();
    for resource in &handoff.resources {
        let mapping = mappings.iter().find(|mapping| mapping.id == resource.id);
        // Only a mapping that pinned down exactly one local file names a stem
        // to look for. Every other resource still gets a row, because the
        // caller asked about the whole report and a silent omission would read
        // as a clean checklist.
        let stem = match mapping {
            Some(mapping)
                if matches!(mapping.verdict, Verdict::PathMatch | Verdict::Candidate)
                    && mapping.paths.len() == 1 =>
            {
                stem_of(std::path::Path::new(&mapping.paths[0]))
            }
            _ => String::new(),
        };
        if stem.is_empty() {
            checks.push(LocalCheck {
                id: resource.id.clone(),
                status: LocalCheckStatus::NotChecked,
                paths: Vec::new(),
                notes: vec![unchecked_reason(mapping)],
                findings: resource.findings.clone(),
            });
            continue;
        }
        let mut exact = Vec::new();
        let mut suffixed = Vec::new();
        for entry in local {
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
            checks.push(LocalCheck {
                id: resource.id.clone(),
                status: LocalCheckStatus::Missing,
                paths: Vec::new(),
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
        let (status, paths, mut notes) = match passing.len() {
            1 => (
                LocalCheckStatus::LocalMatch,
                vec![display(passing[0].path.clone())],
                Vec::new(),
            ),
            0 => (
                LocalCheckStatus::Differs,
                pool.iter().map(|entry| display(&entry.path)).collect(),
                violations,
            ),
            _ => (
                LocalCheckStatus::Ambiguous,
                passing.iter().map(|entry| display(&entry.path)).collect(),
                vec![format!(
                    "{} local files meet the constraints",
                    passing.len()
                )],
            ),
        };
        if notes.len() > 4 {
            let hidden = notes.len() - 3;
            notes.truncate(3);
            notes.push(format!("…and {hidden} more"));
        }
        checks.push(LocalCheck {
            id: resource.id.clone(),
            status,
            paths,
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

    /// The producer's own fixture, byte for byte. It is checked in here with
    /// the SHA-256 the extension landed at `2e56550`, so a producer change
    /// that alters the shared contract fails on this side too instead of
    /// being discovered by a user with a downloaded file.
    const PRODUCER_FIXTURE: &[u8] = include_bytes!("../tests/fixtures/press-handoff.json");
    const PRODUCER_FIXTURE_SHA256: &str =
        "5883f4f7d8f80baa3e97f9bbfadd2cc79e386f6626a59ea9ef171f4f6745cc2d";

    #[test]
    fn the_landed_producer_fixture_is_the_bytes_it_was_landed_as() {
        assert_eq!(
            crate::manifest::SourceIdentity::from_bytes(PRODUCER_FIXTURE).hash,
            PRODUCER_FIXTURE_SHA256,
            "the copied fixture is the producer's file, not a reformatted one"
        );
        assert!(
            !PRODUCER_FIXTURE.contains(&b'\r'),
            "the shared fixture stays LF, or its hash changes on a Windows checkout"
        );
    }

    /// The real export imports, and its advisory metadata survives as
    /// advisory: nothing in it becomes a format, a size or an instruction.
    #[test]
    fn the_landed_producer_fixture_imports_with_its_advisory_data_unread() {
        let pending = parse_bytes(PRODUCER_FIXTURE).expect("the producer fixture imports");
        let handoff = &pending.handoff;
        assert_eq!(handoff.producer, "imageguide-extension");
        assert_eq!(handoff.producer_revision, "press-export-1");
        assert_eq!(handoff.task, "full-audit");
        assert_eq!(handoff.resources.len(), 2);
        assert!(
            handoff
                .redactions
                .contains(&"page-url-and-title-omitted".to_string()),
            "the export's own redactions travel with it"
        );
        // Scope, report schema and model revision are the producer's, not this
        // schema's. They are kept verbatim and each one is named as unread.
        for key in ["scope", "report_schema", "model_revision"] {
            assert!(handoff.unknown.contains_key(key), "{key} is preserved");
            assert!(
                pending
                    .warnings
                    .iter()
                    .any(|warning| warning.contains(&format!("{key:?}"))),
                "{key} is named as kept without being read"
            );
        }
        let hero = &handoff.resources[0];
        assert_eq!(hero.path_hints, vec!["hero.jpg".to_string()]);
        assert!(hero.bytes_measured, "the producer measured this one");
        assert!(
            hero.formats.is_empty() && hero.max_edge.is_none(),
            "an observation never arrives as an output constraint"
        );
        assert_eq!(
            hero.unknown.get("format_recommendations"),
            Some(&serde_json::json!(["avif"])),
            "a recommendation stays a preserved unknown, not a chosen format"
        );
        let icon = &handoff.resources[1];
        assert!(
            !icon.bytes_measured && icon.bytes == Some(3200),
            "an estimate stays an estimate with its number intact"
        );
    }

    fn scratch(tag: &str) -> std::path::PathBuf {
        let nonce = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .expect("the clock is after the Unix epoch")
            .as_nanos();
        let dir = std::env::temp_dir().join(format!("press-handoff-{tag}-{nonce}"));
        std::fs::create_dir_all(&dir).expect("the fixture folder is created");
        std::fs::canonicalize(&dir).expect("the fixture folder has one identity")
    }

    fn image(path: &std::path::Path, colour: [u8; 3]) {
        if let Some(parent) = path.parent() {
            std::fs::create_dir_all(parent).expect("the fixture folder is created");
        }
        image::RgbImage::from_pixel(8, 8, image::Rgb(colour))
            .save(path)
            .expect("the fixture image is written");
    }

    fn hinted(id: &str, hints: &[&str]) -> HandoffResource {
        HandoffResource {
            path_hints: hints.iter().map(|hint| hint.to_string()).collect(),
            urls: Vec::new(),
            width: None,
            height: None,
            bytes: None,
            bytes_measured: false,
            findings: Vec::new(),
            max_edge: None,
            formats: Vec::new(),
            unknown: serde_json::Map::new(),
            id: id.into(),
        }
    }

    fn report(resources: Vec<HandoffResource>) -> Handoff {
        Handoff {
            schema: SCHEMA_VERSION,
            producer: "imageguide-extension".into(),
            producer_revision: "press-export-1".into(),
            task: "task".into(),
            observed: "2026-09-08T00:00:00.000Z".into(),
            resources,
            redactions: Vec::new(),
            unknown: serde_json::Map::new(),
        }
    }

    fn scanned(root: &std::path::Path) -> Vec<crate::scan::Entry> {
        crate::scan::scan(root, &root.join(crate::scan::OUTPUT_DIR)).entries
    }

    /// An exact hint is the strongest evidence matching can produce and is
    /// still not a confirmation: it names one file and says nothing about
    /// whether the person agrees that file is the reported image.
    #[test]
    fn an_exact_path_hint_is_a_path_match_and_serializes_as_one() {
        let root = scratch("exact");
        image(&root.join("hero.jpg"), [10, 20, 30]);
        let handoff = report(vec![hinted("r1", &["hero.jpg"])]);
        let resolved = resolve_to_root(&handoff, &root, &scanned(&root));
        assert_eq!(resolved[0].verdict, Verdict::PathMatch);
        assert_eq!(resolved[0].paths, vec![root.join("hero.jpg")]);
        assert_eq!(
            serde_json::to_value(displayed(&resolved[0])).unwrap()["verdict"],
            serde_json::json!("path_match"),
            "no automatic verdict is published as a confirmation"
        );
        assert_eq!(verdict_word(Verdict::PathMatch), "path match");
        let _ = std::fs::remove_dir_all(&root);
    }

    /// Two folders holding the same filename are a choice between two real
    /// paths. Each one keeps its own directory, so confirming the second
    /// cannot open the first.
    #[test]
    fn a_duplicate_basename_keeps_every_full_path() {
        let root = scratch("duplicate");
        image(&root.join("a").join("hero.jpg"), [10, 20, 30]);
        image(&root.join("b").join("hero.jpg"), [90, 80, 70]);
        let handoff = report(vec![hinted("r1", &["hero.jpg"])]);
        let resolved = resolve_to_root(&handoff, &root, &scanned(&root));
        assert_eq!(resolved[0].verdict, Verdict::Ambiguous);
        let mut paths = resolved[0].paths.clone();
        paths.sort();
        assert_eq!(
            paths,
            vec![root.join("a/hero.jpg"), root.join("b/hero.jpg")]
        );
        let chosen = &resolved[0].paths[1];
        let confirmed = confirm_source(&root, chosen).expect("the chosen duplicate confirms");
        assert_eq!(
            &confirmed.source.path, chosen,
            "the chosen path is the one read"
        );
        assert_eq!(
            confirmed.identity,
            crate::manifest::SourceIdentity::from_bytes(&std::fs::read(chosen).unwrap()),
            "the bytes read are that file's, not its namesake's"
        );
        let _ = std::fs::remove_dir_all(&root);
    }

    /// Past the shown limit the count travels beside the paths, never inside
    /// them: only the printed report ever renders it, where nothing opens it.
    #[test]
    fn hidden_choices_are_counted_beside_the_paths_not_among_them() {
        let root = scratch("many");
        for index in 0..MAX_SHOWN_CHOICES + 3 {
            image(&root.join(format!("d{index}")).join("hero.jpg"), [1, 2, 3]);
        }
        let handoff = report(vec![hinted("r1", &["hero.jpg"])]);
        let resolved = resolve_to_root(&handoff, &root, &scanned(&root));
        assert_eq!(resolved[0].paths.len(), MAX_SHOWN_CHOICES);
        assert_eq!(resolved[0].hidden, 3);
        assert!(
            resolved[0].paths.iter().all(|path| path.is_file()),
            "every offered choice is a real file"
        );
        let printed = displayed(&resolved[0]);
        assert_eq!(printed.paths.len(), MAX_SHOWN_CHOICES + 1);
        assert_eq!(printed.paths[MAX_SHOWN_CHOICES], "…and 3 more");
        let _ = std::fs::remove_dir_all(&root);
    }

    /// A confirmation reads the file the caller named, inside the root the
    /// user chose, and keeps the bytes as its authority.
    #[test]
    fn confirming_reads_the_bytes_and_records_them_beside_the_metadata() {
        let root = scratch("confirm");
        let hero = root.join("hero.png");
        image(&hero, [10, 20, 30]);
        let confirmed = confirm_source(&root, &hero).expect("the file confirms");
        let bytes = std::fs::read(&hero).unwrap();
        assert_eq!(
            confirmed.identity,
            crate::manifest::SourceIdentity::from_bytes(&bytes)
        );
        assert_eq!(confirmed.source.bytes, bytes.len() as u64);
        assert!(confirmed.source.modified.is_some());
        assert_eq!(recheck_source(&root, &confirmed), Recheck::Same);
        let _ = std::fs::remove_dir_all(&root);
    }

    /// Same size, same timestamp, other bytes. The filesystem metadata a
    /// mapping keeps cannot see this; the hash is what revokes it.
    #[test]
    fn a_same_size_same_timestamp_replacement_revokes_the_confirmation() {
        let root = scratch("replaced");
        let hero = root.join("hero.bin");
        std::fs::write(&hero, b"original-bytes").unwrap();
        let confirmed = confirm_source(&root, &hero).expect("the file confirms");
        let stamp = std::fs::metadata(&hero).unwrap().modified().unwrap();
        // The same number of other bytes, with the timestamp put back exactly
        // where it was: everything a mapping records still agrees.
        std::fs::write(&hero, b"replaced-bytes").unwrap();
        std::fs::File::options()
            .write(true)
            .open(&hero)
            .unwrap()
            .set_modified(stamp)
            .unwrap();
        let current = confirm_source(&root, &hero).expect("the replacement is readable");
        assert_eq!(
            current.source.bytes, confirmed.source.bytes,
            "size alone still agrees"
        );
        assert_eq!(
            current.source.modified, confirmed.source.modified,
            "the timestamp alone still agrees"
        );
        assert_eq!(recheck_source(&root, &confirmed), Recheck::Changed);
        let _ = std::fs::remove_dir_all(&root);
    }

    /// A file that is gone cannot be rechecked into a confirmation.
    #[test]
    fn a_deleted_source_fails_its_recheck_closed() {
        let root = scratch("deleted");
        let hero = root.join("hero.png");
        image(&hero, [10, 20, 30]);
        let confirmed = confirm_source(&root, &hero).expect("the file confirms");
        std::fs::remove_file(&hero).unwrap();
        assert!(matches!(
            recheck_source(&root, &confirmed),
            Recheck::Refused(_)
        ));
        let _ = std::fs::remove_dir_all(&root);
    }

    /// The read bound is the conversion bound. A source past it is refused
    /// rather than allocated.
    #[test]
    fn an_oversized_source_is_refused_before_it_is_read() {
        let root = scratch("oversized");
        let big = root.join("big.bin");
        std::fs::write(&big, vec![7u8; 64]).unwrap();
        let error = confirm_source_bounded(&root, &big, 32).expect_err("64 bytes exceed 32");
        assert!(error.contains("larger than 32 bytes"), "{error}");
        assert!(
            confirm_source_bounded(&root, &big, 64).is_ok(),
            "the bound itself still reads"
        );
        let _ = std::fs::remove_dir_all(&root);
    }

    /// A file the user picks outside the chosen root is refused when it is
    /// picked, not after its bytes have been read.
    #[test]
    fn a_manual_choice_outside_the_root_is_refused() {
        let base = scratch("outside");
        let root = base.join("root");
        std::fs::create_dir_all(&root).unwrap();
        let outside = base.join("hero.png");
        image(&outside, [10, 20, 30]);
        let error = resolve_choice(&root, &outside).expect_err("the pick is outside the root");
        assert!(error.contains("outside the selected folder"), "{error}");
        assert!(confirm_source(&root, &outside).is_err());
        let _ = std::fs::remove_dir_all(&base);
    }

    /// A link inside the root that leads out of it is a way out of the root,
    /// and so is typing a parent through one. `..` is resolved by the kernel
    /// after the link, not cancelled against the text before it.
    #[cfg(unix)]
    #[test]
    fn a_link_out_of_the_root_and_a_typed_parent_through_it_are_both_refused() {
        let base = scratch("escape");
        let root = base.join("root");
        std::fs::create_dir_all(&root).unwrap();
        let outside = base.join("outside");
        std::fs::create_dir_all(&outside).unwrap();
        let secret = outside.join("secret.png");
        image(&secret, [1, 2, 3]);
        std::os::unix::fs::symlink(&outside, root.join("link")).unwrap();

        let through_link = root.join("link").join("secret.png");
        let error = confirm_source(&root, &through_link).expect_err("a link out is a way out");
        assert!(error.contains("outside the selected folder"), "{error}");

        // `<root>/link/../outside/secret.png`. Collapsing `link/..` as text
        // would call this the root; the kernel resolves the link first and
        // lands in the parent of `outside`.
        let typed = root
            .join("link")
            .join("..")
            .join("outside")
            .join("secret.png");
        let error =
            confirm_source(&root, &typed).expect_err("the parent of a link is not the root");
        assert!(error.contains("outside the selected folder"), "{error}");
        assert!(
            crate::scan::canonical_boundary(&typed)
                .expect("the lexical helper answers")
                .starts_with(&root),
            "the lexical helper is the one that would have been fooled"
        );

        // A link inside the root that stays inside it is still a file in the
        // root, and confirming it reads the file it names.
        image(&root.join("images").join("photo.png"), [4, 5, 6]);
        std::os::unix::fs::symlink(root.join("images"), root.join("alias")).unwrap();
        let aliased = root.join("alias").join("photo.png");
        let confirmed = confirm_source(&root, &aliased).expect("an in-root alias confirms");
        assert_eq!(
            confirmed.source.path,
            root.join("images").join("photo.png"),
            "the confirmation names the file the kernel opened, not the alias"
        );
        let _ = std::fs::remove_dir_all(&base);
    }

    /// The confinement is against the folder that was selected. Renaming that
    /// folder away and putting a link to somewhere else in its place must not
    /// silently move the boundary to the new destination.
    #[cfg(unix)]
    #[test]
    fn a_root_replaced_by_a_link_elsewhere_confirms_nothing() {
        let base = scratch("swapped-root");
        let root = base.join("root");
        std::fs::create_dir_all(&root).unwrap();
        image(&root.join("hero.png"), [1, 2, 3]);
        let elsewhere = base.join("elsewhere");
        std::fs::create_dir_all(&elsewhere).unwrap();
        let planted = elsewhere.join("hero.png");
        image(&planted, [9, 9, 9]);

        assert!(confirm_source(&root, &root.join("hero.png")).is_ok());
        std::fs::rename(&root, base.join("moved")).unwrap();
        std::os::unix::fs::symlink(&elsewhere, &root).unwrap();

        let error = confirm_source(&root, &root.join("hero.png"))
            .expect_err("the selected folder is gone, whatever its name now points at");
        assert!(
            error.contains("no longer the folder that was selected"),
            "{error}"
        );
        assert!(
            confirm_source(&root, &planted).is_err(),
            "and nothing under the replacement is reachable through it"
        );
        let _ = std::fs::remove_dir_all(&base);
    }

    /// A name that is not UTF-8 is still a file. It keeps its identity as a
    /// path; only its label is lossy. macOS is excluded because APFS and HFS+
    /// refuse such a name outright (EILSEQ), so the fixture cannot exist
    /// there; the label-versus-path rule is checked in memory below instead.
    #[cfg(all(unix, not(target_os = "macos")))]
    #[test]
    fn a_non_utf8_name_keeps_its_identity() {
        use std::os::unix::ffi::OsStrExt;
        let root = scratch("non-utf8");
        let name = std::ffi::OsStr::from_bytes(b"hero-\xff.png");
        let hero = root.join(name);
        image(&hero, [10, 20, 30]);
        assert!(hero.to_str().is_none(), "the fixture name is not UTF-8");

        // The producer can only send text, so its hint carries the
        // replacement character. Name matching is text and resembles it; the
        // choice it offers is still the real path, byte for byte.
        let handoff = report(vec![hinted("r1", &["hero-\u{fffd}.png"])]);
        let resolved = resolve_to_root(&handoff, &root, &scanned(&root));
        assert_eq!(resolved[0].verdict, Verdict::Candidate);
        assert_eq!(resolved[0].paths, vec![hero.clone()]);

        let confirmed =
            confirm_source(&root, &resolved[0].paths[0]).expect("the offered choice confirms");
        assert_eq!(confirmed.source.path, hero);
        assert_eq!(
            confirmed.identity,
            crate::manifest::SourceIdentity::from_bytes(&std::fs::read(&hero).unwrap())
        );

        // Which is why the path is the authority: its label does not name it.
        let label = &displayed(&resolved[0]).paths[0];
        assert_ne!(
            std::path::Path::new(label),
            hero.as_path(),
            "the displayed name is lossy"
        );
        assert!(
            confirm_source(&root, std::path::Path::new(label)).is_err(),
            "a path rebuilt from a label opens nothing"
        );
        let _ = std::fs::remove_dir_all(&root);
    }

    /// The same rule with no filesystem in it, so every Unix keeps the
    /// coverage the fixture above can only have where such a name can be
    /// written: the resolution carries the bytes of the name, and the printed
    /// label is a lossy rendering nothing can open its way back through.
    #[cfg(unix)]
    #[test]
    fn a_non_utf8_path_outlives_its_lossy_label() {
        use std::os::unix::ffi::OsStrExt;
        let raw = std::path::PathBuf::from(std::ffi::OsStr::from_bytes(b"/root/hero-\xff.png"));
        let resolution = Resolution {
            id: "r1".into(),
            verdict: Verdict::Candidate,
            paths: vec![raw.clone()],
            hidden: 0,
            notes: Vec::new(),
        };
        assert_eq!(
            resolution.paths[0].as_os_str().as_bytes(),
            b"/root/hero-\xff.png",
            "the choice a confirmation reads is the name byte for byte"
        );

        let label = &displayed(&resolution).paths[0];
        assert!(label.contains('\u{fffd}'), "the label is lossy: {label}");
        assert_ne!(
            std::path::Path::new(label),
            raw.as_path(),
            "and a path rebuilt from that label is a different name"
        );
    }

    /// A Unicode name is not the lossy case: it is a real name on every
    /// platform, and it must scan, resolve and confirm as itself. The
    /// characters are ideographs, which no filesystem normalisation
    /// decomposes, so the name read back is the name written.
    #[test]
    fn a_unicode_filename_resolves_and_confirms_by_its_real_path() {
        let root = scratch("unicode");
        let hero = root.join("hero-日本語.png");
        image(&hero, [10, 20, 30]);
        let entries = scanned(&root);
        assert_eq!(
            entries.len(),
            1,
            "the scan sees the file under its own name"
        );

        let handoff = report(vec![hinted("r1", &["hero-日本語.png"])]);
        let resolved = resolve_to_root(&handoff, &root, &entries);
        assert_eq!(resolved[0].verdict, Verdict::PathMatch);
        assert_eq!(resolved[0].paths, vec![hero.clone()]);

        let confirmed =
            confirm_source(&root, &resolved[0].paths[0]).expect("the named file confirms");
        assert_eq!(confirmed.source.path, hero);
        assert_eq!(
            confirmed.identity,
            crate::manifest::SourceIdentity::from_bytes(&std::fs::read(&hero).unwrap())
        );

        // Nothing here is lossy, so the printed label names the same file the
        // confirmation read.
        let label = &displayed(&resolved[0]).paths[0];
        assert_eq!(std::path::Path::new(label), hero.as_path());
        assert!(
            confirm_source(&root, std::path::Path::new(label)).is_ok(),
            "a path rebuilt from this label opens the same file"
        );
        let _ = std::fs::remove_dir_all(&root);
    }

    /// A folder is not a source, and neither is a name nothing answers to.
    #[test]
    fn a_missing_file_and_a_folder_are_both_refused() {
        let root = scratch("missing");
        std::fs::create_dir_all(root.join("images")).unwrap();
        assert!(confirm_source(&root, &root.join("gone.png")).is_err());
        let error =
            confirm_source(&root, &root.join("images")).expect_err("a folder is not a file");
        assert!(error.contains("is not a file"), "{error}");
        let _ = std::fs::remove_dir_all(&root);
    }
}

/// How one reported resource resolved against a user-chosen root. Every
/// verdict here is automatic match evidence about names and layout. None of
/// them is a human confirmation: only a person looking at a chosen file
/// confirms that it is the source, which is why the strongest evidence is
/// named after what it actually observed.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum Verdict {
    /// A path hint named exactly one existing file under the root. The hint
    /// was exact; nobody has yet said the file is the reported image.
    PathMatch,
    /// One filename match: plausible, but only a human confirms it.
    Candidate,
    /// Several files match: a choice, never a guess.
    Ambiguous,
    /// Nothing under the root answers to this resource.
    Unmatched,
    /// The hints point at a different machine layout, not at this root.
    OutOfScope,
}

/// The verdict as one word, shared by the CLI and the review card so the two
/// cannot drift into describing the same evidence differently.
pub fn verdict_word(verdict: Verdict) -> &'static str {
    match verdict {
        Verdict::PathMatch => "path match",
        Verdict::Candidate => "candidate",
        Verdict::Ambiguous => "ambiguous",
        Verdict::Unmatched => "unmatched",
        Verdict::OutOfScope => "out of scope",
    }
}

/// One resource's resolution: its verdict, the local paths behind it, and
/// observations that do not change the verdict but travel with it. This is
/// the text shape the CLI prints and serializes; `Resolution` is the shape
/// a chooser must consume, because these strings are lossy.
#[derive(Clone, Debug, PartialEq, Serialize)]
pub struct Mapping {
    pub id: String,
    pub verdict: Verdict,
    pub paths: Vec<String>,
    pub notes: Vec<String>,
}

/// The most real choices one row offers before a report becomes a directory
/// listing. Past it the count travels instead of the paths, and the row still
/// reaches every other file through an explicit file picker.
pub const MAX_SHOWN_CHOICES: usize = 8;

/// One resource resolved against a user-chosen root, keeping its local files
/// as paths. A `PathBuf` is the authority a confirmation reads bytes from; the
/// display string beside it is for people. On Unix a name that is not UTF-8
/// survives here and would not survive a `String`, and no rendered summary
/// line — including the truncation count — can be mistaken for a file.
#[derive(Clone, Debug, PartialEq)]
pub struct Resolution {
    pub id: String,
    pub verdict: Verdict,
    /// Real, openable choices: at most `MAX_SHOWN_CHOICES` of them.
    pub paths: Vec<std::path::PathBuf>,
    /// How many further choices this row does not list.
    pub hidden: usize,
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
pub fn resolve_to_root(
    handoff: &Handoff,
    root: &std::path::Path,
    entries: &[crate::scan::Entry],
) -> Vec<Resolution> {
    handoff
        .resources
        .iter()
        .map(|resource| resolve_one(resource, root, entries))
        .collect()
}

/// The same resolution as text, for the CLI report. Nothing reads a path back
/// out of these strings: the resolution above keeps the paths themselves.
pub fn map_to_root(
    handoff: &Handoff,
    root: &std::path::Path,
    entries: &[crate::scan::Entry],
) -> Vec<Mapping> {
    resolve_to_root(handoff, root, entries)
        .iter()
        .map(displayed)
        .collect()
}

/// One resolution rendered for print. The truncation line lives only here,
/// where nothing will try to open it.
pub fn displayed(resolution: &Resolution) -> Mapping {
    let mut paths: Vec<String> = resolution.paths.iter().map(display).collect();
    if resolution.hidden > 0 {
        paths.push(format!("…and {} more", resolution.hidden));
    }
    Mapping {
        id: resolution.id.clone(),
        verdict: resolution.verdict,
        paths,
        notes: resolution.notes.clone(),
    }
}

fn resolve_one(
    resource: &HandoffResource,
    root: &std::path::Path,
    entries: &[crate::scan::Entry],
) -> Resolution {
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
        return Resolution {
            id: resource.id.clone(),
            verdict: Verdict::PathMatch,
            paths: vec![single],
            hidden: 0,
            notes,
        };
    }
    if hits.len() > 1 {
        // Several hints named several files: a choice between them, never a
        // silent merge into one resource.
        let (paths, hidden) = capped(hits);
        return Resolution {
            id: resource.id.clone(),
            verdict: Verdict::Ambiguous,
            paths,
            hidden,
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
        0 if elsewhere => Resolution {
            id: resource.id.clone(),
            verdict: Verdict::OutOfScope,
            paths: Vec::new(),
            hidden: 0,
            notes,
        },
        0 => Resolution {
            id: resource.id.clone(),
            verdict: Verdict::Unmatched,
            paths: Vec::new(),
            hidden: 0,
            notes,
        },
        1 => {
            let single = candidates.remove(0);
            corroborate(resource, entries, &single, &mut notes);
            Resolution {
                id: resource.id.clone(),
                verdict: Verdict::Candidate,
                paths: vec![single],
                hidden: 0,
                notes,
            }
        }
        _ => {
            let (paths, hidden) = capped(candidates);
            Resolution {
                id: resource.id.clone(),
                verdict: Verdict::Ambiguous,
                paths,
                hidden,
                notes,
            }
        }
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
/// report, not a directory listing. The count is returned beside the paths
/// rather than appended to them, so no caller can chose it as a file.
fn capped(mut paths: Vec<std::path::PathBuf>) -> (Vec<std::path::PathBuf>, usize) {
    let hidden = paths.len().saturating_sub(MAX_SHOWN_CHOICES);
    paths.truncate(MAX_SHOWN_CHOICES);
    (paths, hidden)
}

/// What a person confirmed about one local file: the bytes that were actually
/// read, plus the size and timestamp beside them. The identity is the
/// authority. `SourceRef` records what the filesystem said at the same moment
/// and is deliberately not proof: a replacement can keep both numbers, and
/// only the hash notices.
#[derive(Clone, Debug, PartialEq)]
pub struct ConfirmedSource {
    pub source: crate::job::SourceRef,
    pub identity: crate::manifest::SourceIdentity,
}

/// The one file `root` holds at `path`, or why it will not be read.
///
/// `root` is the canonical folder the user selected, and the confinement is
/// against *that* folder rather than against whatever its name points at now.
/// Re-resolving the name and trusting the new answer would let the tree be
/// renamed and replaced by a link to somewhere else between the selection and
/// the read, and every file under the replacement would then pass; so a root
/// whose name no longer resolves to itself is refused instead.
///
/// The file side goes through `std::fs::canonicalize`, which is the kernel's
/// own answer. A lexical collapse of `..` would accept `root/link/..` whenever
/// `link` points outside the tree, because it cancels the component before
/// anything resolves the link; that is why `scan::canonical_boundary` is the
/// wrong tool here, however convenient its handling of missing paths is.
fn resolved_within(
    root: &std::path::Path,
    path: &std::path::Path,
) -> Result<std::path::PathBuf, String> {
    let current = std::fs::canonicalize(root)
        .map_err(|error| format!("the selected folder cannot be resolved: {error}"))?;
    if current != root {
        return Err(format!(
            "{} is no longer the folder that was selected",
            root.display()
        ));
    }
    let root = current;
    let resolved = std::fs::canonicalize(path)
        .map_err(|error| format!("{} cannot be resolved: {error}", path.display()))?;
    if !resolved.starts_with(&root) {
        return Err(format!(
            "{} resolves outside the selected folder",
            path.display()
        ));
    }
    if !std::fs::metadata(&resolved).is_ok_and(|metadata| metadata.is_file()) {
        return Err(format!("{} is not a file", path.display()));
    }
    Ok(resolved)
}

/// Resolve a file somebody picked by hand against the canonical chosen root,
/// so a pick outside it is refused when it is made rather than after its bytes
/// are read.
pub fn resolve_choice(
    root: &std::path::Path,
    path: &std::path::Path,
) -> Result<std::path::PathBuf, String> {
    resolved_within(root, path)
}

fn modified_secs(path: &std::path::Path) -> Option<u64> {
    std::fs::metadata(path)
        .and_then(|metadata| metadata.modified())
        .ok()
        .and_then(|time| time.duration_since(std::time::UNIX_EPOCH).ok())
        .map(|age| age.as_secs())
}

/// Confirm one chosen file as a resource's local source: resolve it inside the
/// chosen root, then read the bytes it holds now. `root` must be the canonical
/// path resolved when the user selected the folder — the confinement is
/// against that folder, not against its name. Reading is bounded by the same
/// limit conversion applies, so an enormous file is refused here rather than
/// allocated. Nothing is written and nothing is converted.
pub fn confirm_source(
    root: &std::path::Path,
    path: &std::path::Path,
) -> Result<ConfirmedSource, String> {
    confirm_source_bounded(root, path, crate::convert::MAX_SOURCE_BYTES)
}

/// The same confirmation with the read bound named, so a test can prove the
/// refusal without writing a quarter of a gigabyte to disk.
fn confirm_source_bounded(
    root: &std::path::Path,
    path: &std::path::Path,
    max_bytes: u64,
) -> Result<ConfirmedSource, String> {
    let path = resolved_within(root, path)?;
    let bytes = crate::job::read_bounded(&path, max_bytes, "source")?;
    let identity = crate::manifest::SourceIdentity::from_bytes(&bytes);
    Ok(ConfirmedSource {
        source: crate::job::SourceRef {
            bytes: identity.bytes,
            modified: modified_secs(&path),
            path,
        },
        identity,
    })
}

/// Whether a confirmed source still holds the bytes somebody confirmed.
#[derive(Clone, Debug, PartialEq)]
pub enum Recheck {
    /// The same bytes: the confirmation stands.
    Same,
    /// Different bytes under the same name. Size and timestamp can both be
    /// unchanged and this still fires, which is the point of keeping a hash.
    Changed,
    /// The file could not be read as a source at all any more.
    Refused(String),
}

/// Read the confirmed file again and compare it byte for byte. A file that
/// will not resolve, escapes the root, or exceeds the read bound fails closed:
/// the caller revokes the confirmation either way.
pub fn recheck_source(root: &std::path::Path, confirmed: &ConfirmedSource) -> Recheck {
    match confirm_source(root, &confirmed.source.path) {
        Ok(current) if current.identity == confirmed.identity => Recheck::Same,
        Ok(_) => Recheck::Changed,
        Err(message) => Recheck::Refused(message),
    }
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
        assert_eq!(mappings[0].verdict, Verdict::PathMatch);
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
        assert_eq!(mappings[0].verdict, Verdict::PathMatch);
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

    fn local_entry(
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
            verdict: Verdict::PathMatch,
            paths: vec![local.into()],
            notes: Vec::new(),
        };
        (handoff, vec![mapping])
    }

    #[test]
    fn an_exact_local_derivative_matches() {
        let dir = workdir("local-check");
        let local = dir.join("photos").join("hero.jpg");
        let (handoff, mappings) =
            constrained("hero", &local.to_string_lossy(), &["avif"], Some(1600));
        let out = dir.join("out");
        let checks = check_local_files(
            &handoff,
            &mappings,
            &[local_entry(
                &out.join("hero.avif"),
                image::ImageFormat::Avif,
                1200,
                800,
            )],
        );
        assert_eq!(checks[0].status, LocalCheckStatus::LocalMatch);
        assert_eq!(checks[0].paths.len(), 1);
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
        let checks = check_local_files(
            &handoff,
            &mappings,
            &[local_entry(
                &out.join("hero.avif"),
                image::ImageFormat::Avif,
                1200,
                800,
            )],
        );
        assert_eq!(checks[0].status, LocalCheckStatus::Differs);
        assert!(checks[0].notes.iter().any(|note| note.contains("jpeg")));
        // Right format, over the size limit.
        let (handoff, mappings) = constrained("hero", &local.to_string_lossy(), &[], Some(1000));
        let checks = check_local_files(
            &handoff,
            &mappings,
            &[local_entry(
                &out.join("hero.avif"),
                image::ImageFormat::Avif,
                1200,
                800,
            )],
        );
        assert_eq!(checks[0].status, LocalCheckStatus::Differs);
        assert!(checks[0].notes.iter().any(|note| note.contains("1000px")));
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn suffixed_derivatives_match_and_exact_wins() {
        let dir = workdir("suffix");
        let local = dir.join("photos").join("hero.jpg");
        let out = dir.join("out");
        let (handoff, mappings) =
            constrained("hero", &local.to_string_lossy(), &["avif"], Some(1600));
        // No exact stem: the suffixed derivative matches.
        let checks = check_local_files(
            &handoff,
            &mappings,
            &[local_entry(
                &out.join("hero-1600.avif"),
                image::ImageFormat::Avif,
                1200,
                800,
            )],
        );
        assert_eq!(checks[0].status, LocalCheckStatus::LocalMatch);
        // An exact stem exists but violates: derivatives do not rescue it.
        let checks = check_local_files(
            &handoff,
            &mappings,
            &[
                local_entry(&out.join("hero.png"), image::ImageFormat::Png, 1200, 800),
                local_entry(
                    &out.join("hero-1600.avif"),
                    image::ImageFormat::Avif,
                    1200,
                    800,
                ),
            ],
        );
        assert_eq!(checks[0].status, LocalCheckStatus::Differs);
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn two_passing_derivatives_stay_ambiguous_and_absent_is_missing() {
        let dir = workdir("amb-local");
        let local = dir.join("photos").join("hero.jpg");
        let out = dir.join("out");
        let (handoff, mappings) = constrained("hero", &local.to_string_lossy(), &["avif"], None);
        let checks = check_local_files(
            &handoff,
            &mappings,
            &[
                local_entry(
                    &out.join("hero-1600.avif"),
                    image::ImageFormat::Avif,
                    1200,
                    800,
                ),
                local_entry(
                    &out.join("hero_800.avif"),
                    image::ImageFormat::Avif,
                    800,
                    600,
                ),
            ],
        );
        assert_eq!(checks[0].status, LocalCheckStatus::Ambiguous);
        let checks = check_local_files(&handoff, &mappings, &[]);
        assert_eq!(checks[0].status, LocalCheckStatus::Missing);
        // An unpinned mapping names no file to look for, and says so in its
        // own row: the resource keeps its findings and never falls out of the
        // checklist.
        let mut floating = mappings;
        floating[0].verdict = Verdict::Ambiguous;
        floating[0].paths = vec!["a".into(), "b".into()];
        let checks = check_local_files(&handoff, &floating, &[]);
        assert_eq!(checks.len(), 1);
        assert_eq!(checks[0].status, LocalCheckStatus::NotChecked);
        assert!(checks[0].paths.is_empty());
        assert!(checks[0].notes[0].contains("ambiguous"), "{:?}", checks[0]);
        assert_eq!(checks[0].findings, vec!["excess-dimensions".to_string()]);
        // A resource nothing mapped at all is a hole too, not an omission.
        let orphan = check_local_files(&handoff, &[], &[]);
        assert_eq!(orphan.len(), 1);
        assert_eq!(orphan[0].status, LocalCheckStatus::NotChecked);
        assert!(orphan[0].notes[0].contains("no mapping"), "{:?}", orphan[0]);
        let _ = std::fs::remove_dir_all(&dir);
    }
}
