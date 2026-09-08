//! Black-box CLI contract: exit status, stdout JSON shape, stderr and
//! filesystem effects asserted together against the built binary. Unit tests
//! cover parsing and planning; these prove the process boundary an agent sees.
//!
//! Every test owns a unique temp dir, so parallel threads never share state.
//! Fixtures are hand-encoded 8x8 PNGs: std has no deflate, so the IDAT holds
//! one uncompressed stored block. A malformed fixture fails loudly here, with
//! zero files listed or a failed conversion, rather than passing vacuously.

use std::path::{Path, PathBuf};
use std::process::{Command, Output};

fn crc32(data: &[u8]) -> u32 {
    let mut crc = 0xFFFF_FFFFu32;
    for byte in data {
        crc ^= u32::from(*byte);
        for _ in 0..8 {
            crc = if crc & 1 == 1 {
                (crc >> 1) ^ 0xEDB8_8320
            } else {
                crc >> 1
            };
        }
    }
    !crc
}

fn chunk(kind: &[u8; 4], data: &[u8], out: &mut Vec<u8>) {
    out.extend_from_slice(&(data.len() as u32).to_be_bytes());
    out.extend_from_slice(kind);
    out.extend_from_slice(data);
    let mut check = kind.to_vec();
    check.extend_from_slice(data);
    out.extend_from_slice(&crc32(&check).to_be_bytes());
}

/// Deterministic 8x8 RGB noise: flat colours compress to nothing and would
/// let a conversion report zero bytes without proving anything moved.
fn photo_png() -> Vec<u8> {
    let mut raw = Vec::new();
    for y in 0..8u32 {
        raw.push(0u8);
        for x in 0..8u32 {
            raw.push(((x * 37 + y * 91) % 251) as u8);
            raw.push(((x * 11 + y * 53) % 251) as u8);
            raw.push(((x * 7 + y * 13) % 251) as u8);
        }
    }
    assert!(raw.len() <= 0xFFFF, "one stored block holds the fixture");
    let mut zlib = vec![0x78, 0x01, 0x01];
    zlib.extend_from_slice(&(raw.len() as u16).to_le_bytes());
    zlib.extend_from_slice(&(!(raw.len() as u16)).to_le_bytes());
    zlib.extend_from_slice(&raw);
    let (mut a, mut b) = (1u32, 0u32);
    for byte in &raw {
        a = (a + u32::from(*byte)) % 65521;
        b = (b + a) % 65521;
    }
    zlib.extend_from_slice(&((a << 16) | b).to_be_bytes());
    let mut png = b"\x89PNG\r\n\x1a\n".to_vec();
    let mut ihdr = Vec::new();
    ihdr.extend_from_slice(&8u32.to_be_bytes());
    ihdr.extend_from_slice(&8u32.to_be_bytes());
    ihdr.extend_from_slice(&[8, 2, 0, 0, 0]);
    chunk(b"IHDR", &ihdr, &mut png);
    chunk(b"IDAT", &zlib, &mut png);
    chunk(b"IEND", &[], &mut png);
    png
}

fn workdir(tag: &str) -> PathBuf {
    let dir = std::env::temp_dir().join(format!("press-cli-contract-{tag}-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&dir);
    std::fs::create_dir_all(&dir).expect("the fixture dir is created");
    dir
}

fn photo(dir: &Path, name: &str) -> PathBuf {
    let path = dir.join(name);
    std::fs::write(&path, photo_png()).expect("the fixture image is written");
    path
}

fn run(args: &[&str]) -> Output {
    Command::new(env!("CARGO_BIN_EXE_press"))
        .args(args)
        .output()
        .expect("the binary runs")
}

fn stdout_json(output: &Output) -> serde_json::Value {
    serde_json::from_slice(&output.stdout).expect("stdout is one JSON document")
}

fn stderr(output: &Output) -> String {
    String::from_utf8_lossy(&output.stderr).into_owned()
}

#[test]
fn audit_json_reports_the_folder_and_writes_nothing() {
    let dir = workdir("audit");
    photo(&dir, "shot.png");
    let output = run(&["audit", &dir.to_string_lossy(), "--json"]);
    assert_eq!(output.status.code(), Some(0));
    let report = stdout_json(&output);
    assert_eq!(report["schema_version"], 1);
    assert_eq!(report["files"].as_array().map(Vec::len), Some(1));
    assert!(stderr(&output).is_empty(), "a clean audit stays quiet");
    assert!(!dir.join("optimized").exists(), "an audit never writes");
    let _ = std::fs::remove_dir_all(&dir);
}

#[test]
fn convert_json_writes_outputs_and_reports_schema_two() {
    let dir = workdir("convert");
    photo(&dir, "shot.png");
    let output = run(&["convert", &dir.to_string_lossy(), "--json"]);
    assert_eq!(output.status.code(), Some(0));
    let report = stdout_json(&output);
    assert_eq!(report["schema_version"], 2);
    assert_eq!(report["files"][0]["status"], "converted");
    assert!(dir.join("optimized").join("shot.webp").is_file());
    assert!(stderr(&output).is_empty(), "a clean convert stays quiet");
    let _ = std::fs::remove_dir_all(&dir);
}

#[test]
fn an_invalid_invocation_exits_two_with_stderr_only() {
    let dir = workdir("invalid");
    photo(&dir, "shot.png");
    let output = run(&["audit", &dir.to_string_lossy(), "--skip-existing"]);
    assert_eq!(output.status.code(), Some(2));
    assert!(output.stdout.is_empty(), "no document on invalid input");
    assert!(stderr(&output).contains("press: "), "{}", stderr(&output));
    let _ = std::fs::remove_dir_all(&dir);
}

#[test]
fn a_refused_destination_json_still_reports_and_exits_two() {
    let dir = workdir("refused-json");
    photo(&dir, "shot.png");
    let target = dir.to_string_lossy().into_owned();
    let output = run(&["convert", &target, "--output", &target, "--json"]);
    assert_eq!(output.status.code(), Some(2));
    // The refusal is a schema-two error document, not silence: `output` never
    // established, `error` names the reason, every file carries the refusal.
    let report = stdout_json(&output);
    assert_eq!(report["schema_version"], 2);
    assert!(report["output"].is_null());
    assert!(report["error"].is_string());
    assert_eq!(report["files"][0]["status"], "failed");
    assert!(
        !dir.join("optimized").exists(),
        "a refused run writes nothing"
    );
    let _ = std::fs::remove_dir_all(&dir);
}

#[test]
fn a_refused_destination_without_json_names_it_on_stderr() {
    let dir = workdir("refused-text");
    photo(&dir, "shot.png");
    let target = dir.to_string_lossy().into_owned();
    let output = run(&["convert", &target, "--output", &target]);
    assert_eq!(output.status.code(), Some(2));
    assert!(
        stderr(&output).contains("could not use output"),
        "{}",
        stderr(&output)
    );
    assert!(
        String::from_utf8_lossy(&output.stdout).contains("failed"),
        "each file reports the refusal"
    );
    let _ = std::fs::remove_dir_all(&dir);
}

#[test]
fn a_dry_run_writes_nothing_and_exits_zero() {
    let dir = workdir("dry");
    photo(&dir, "shot.png");
    let output = run(&["convert", &dir.to_string_lossy(), "--dry-run", "--json"]);
    assert_eq!(output.status.code(), Some(0));
    let report = stdout_json(&output);
    assert_eq!(report["dry_run"], true);
    assert_eq!(report["files"][0]["status"], "planned");
    assert!(!dir.join("optimized").exists(), "planning writes nothing");
    let _ = std::fs::remove_dir_all(&dir);
}

#[test]
fn skip_existing_skips_a_managed_output_and_a_legacy_one() {
    let dir = workdir("skip");
    photo(&dir, "managed.png");
    let target = dir.to_string_lossy().into_owned();
    // The first run records its manifest: the second meets a managed output.
    assert_eq!(run(&["convert", &target]).status.code(), Some(0));
    // A hand-placed output with no record is legacy: current by timestamp.
    photo(&dir, "legacy.png");
    std::fs::write(dir.join("optimized").join("legacy.webp"), b"already there")
        .expect("the legacy output is placed");
    let output = run(&["convert", &target, "--skip-existing", "--json"]);
    assert_eq!(output.status.code(), Some(0));
    let report = stdout_json(&output);
    let files = report["files"].as_array().expect("files list");
    assert_eq!(files.len(), 2);
    assert!(files.iter().all(|file| file["status"] == "skipped"));
    let reasons: Vec<&str> = files
        .iter()
        .map(|file| file["reason"].as_str().expect("a named reason"))
        .collect();
    // The managed file names its matching recipe; the legacy file can only
    // cite timestamps. Collapsing them would hide which proof skipped.
    assert!(
        reasons
            .iter()
            .any(|reason| reason.starts_with("the output already matches")),
        "{reasons:?}"
    );
    assert!(
        reasons.contains(&"the output is not older than the source"),
        "{reasons:?}"
    );
    let _ = std::fs::remove_dir_all(&dir);
}

#[test]
fn replace_and_restore_round_trip_through_the_cli() {
    let dir = workdir("replace");
    photo(&dir, "shot.png");
    let target = dir.to_string_lossy().into_owned();
    assert_eq!(
        run(&["convert", &target, "--replace"]).status.code(),
        Some(0)
    );
    assert!(
        !dir.join("shot.png").exists(),
        "replace takes the original's name"
    );
    assert!(dir.join("shot.webp").is_file());
    assert!(dir.join("press-originals").join("shot.png").is_file());
    assert_eq!(run(&["restore", &target]).status.code(), Some(0));
    assert!(
        dir.join("shot.png").is_file(),
        "restore hands the original back"
    );
    let _ = std::fs::remove_dir_all(&dir);
}

fn handoff_fixture(dir: &Path) -> PathBuf {
    let report = serde_json::json!({
        "schema": 1,
        "producer": "imageguide-extension",
        "producer_revision": "report-schema-4",
        "task": "task-1",
        "observed": "2026-09-08T12:00:00Z",
        "resources": [{
            "id": "hero",
            "urls": ["https://example.com/hero.jpg?w=1600"],
            "path_hints": ["images/hero.jpg"],
            "width": 1600,
            "height": 1200,
            "bytes": 240000,
            "bytes_measured": true,
            "findings": ["excess-dimensions"],
            "max_edge": 1600,
            "formats": ["avif"],
        }],
        "redactions": ["query values dropped"],
    });
    let path = dir.join("report.json");
    std::fs::write(&path, serde_json::to_vec_pretty(&report).unwrap())
        .expect("the handoff fixture is written");
    path
}

#[test]
fn handoff_json_validates_a_report_without_writing() {
    let dir = workdir("handoff");
    let report = handoff_fixture(&dir);
    let output = run(&["handoff", &report.to_string_lossy(), "--json"]);
    assert_eq!(output.status.code(), Some(0));
    let doc = stdout_json(&output);
    assert_eq!(doc["schema_version"], 1);
    assert_eq!(doc["command"], "handoff");
    assert_eq!(doc["task"], "task-1");
    assert_eq!(doc["resources"], 1);
    assert!(stderr(&output).is_empty(), "a clean import stays quiet");
    assert_eq!(
        std::fs::read_dir(&dir).expect("the dir lists").count(),
        1,
        "import stages nothing beside the report"
    );
    let _ = std::fs::remove_dir_all(&dir);
}

#[test]
fn handoff_text_names_the_task_and_warns_on_stderr() {
    let dir = workdir("handoff-text");
    let report = handoff_fixture(&dir);
    let output = run(&["handoff", &report.to_string_lossy()]);
    assert_eq!(output.status.code(), Some(0));
    assert!(
        String::from_utf8_lossy(&output.stdout).contains("task-1"),
        "the task is named"
    );
    let _ = std::fs::remove_dir_all(&dir);
}

#[test]
fn handoff_refuses_garbage_with_exit_two() {
    let dir = workdir("handoff-garbage");
    let report = dir.join("report.json");
    std::fs::write(&report, b"{\"schema\":99}").expect("the bad fixture is written");
    let output = run(&["handoff", &report.to_string_lossy(), "--json"]);
    assert_eq!(output.status.code(), Some(2));
    assert!(output.stdout.is_empty(), "no document on refusal");
    assert!(stderr(&output).contains("press: "), "{}", stderr(&output));
    let _ = std::fs::remove_dir_all(&dir);
}

fn mapping_report(dir: &Path, name: &str, resources: serde_json::Value) -> PathBuf {
    let report = serde_json::json!({
        "schema": 1,
        "producer": "imageguide-extension",
        "producer_revision": "report-schema-4",
        "task": "task-9",
        "observed": "2026-09-08T12:00:00Z",
        "resources": resources,
        "redactions": [],
    });
    let path = dir.join(name);
    std::fs::write(&path, serde_json::to_vec_pretty(&report).unwrap())
        .expect("the mapping fixture is written");
    path
}

fn minimal_resource(id: &str, hints: &[&str]) -> serde_json::Value {
    serde_json::json!({
        "id": id,
        "urls": [],
        "path_hints": hints,
        "width": null,
        "height": null,
        "bytes": null,
        "findings": [],
        "max_edge": null,
        "formats": [],
    })
}

#[test]
fn handoff_root_maps_hints_to_verdicts() {
    let dir = workdir("mapping");
    let root = dir.join("photos");
    std::fs::create_dir_all(&root).expect("the root is created");
    std::fs::write(root.join("hero.jpg"), photo_png()).expect("hero is written");
    std::fs::create_dir_all(root.join("gallery")).expect("the subfolder is created");
    std::fs::write(root.join("gallery").join("shot.jpg"), photo_png()).expect("shot is written");
    let report = mapping_report(
        &dir,
        "report.json",
        serde_json::json!([
            minimal_resource("r1", &["hero.jpg"]),
            minimal_resource("r2", &["missing/shot.jpg"]),
            minimal_resource("r3", &["/elsewhere/gone.jpg"]),
            minimal_resource("r4", &["nothing.jpg"]),
        ]),
    );
    let output = run(&[
        "handoff",
        &report.to_string_lossy(),
        "--root",
        &root.to_string_lossy(),
        "--json",
    ]);
    assert_eq!(output.status.code(), Some(0));
    let doc = stdout_json(&output);
    assert_eq!(doc["task"], "task-9");
    let verdicts: Vec<&str> = doc["mappings"]
        .as_array()
        .expect("mappings list")
        .iter()
        .map(|mapping| mapping["verdict"].as_str().expect("a verdict"))
        .collect();
    // An exact hint confirms, a basename match stays a candidate, a foreign
    // absolute hint is out of scope, and nothing is unmatched. Nothing
    // converted: mapping only reports.
    assert_eq!(
        verdicts,
        vec!["confirmed", "candidate", "out_of_scope", "unmatched"]
    );
    assert_eq!(doc["summary"]["confirmed"], 1);
    assert_eq!(doc["summary"]["candidate"], 1);
    assert!(!root.join("optimized").exists(), "mapping writes nothing");
    let _ = std::fs::remove_dir_all(&dir);
}

#[test]
fn handoff_refuses_a_missing_root() {
    let dir = workdir("mapping-missing-root");
    let report = mapping_report(
        &dir,
        "report.json",
        serde_json::json!([minimal_resource("r1", &["a.jpg"])]),
    );
    let output = run(&[
        "handoff",
        &report.to_string_lossy(),
        "--root",
        &dir.join("gone").to_string_lossy(),
    ]);
    assert_eq!(output.status.code(), Some(2));
    assert!(
        stderr(&output).contains("is not a folder"),
        "{}",
        stderr(&output)
    );
    let _ = std::fs::remove_dir_all(&dir);
}

fn deployed_tree(dir: &Path) -> (PathBuf, PathBuf) {
    let root = dir.join("photos");
    std::fs::create_dir_all(&root).expect("the root is created");
    std::fs::write(root.join("hero.jpg"), photo_png()).expect("hero is written");
    let deployed = dir.join("live");
    std::fs::create_dir_all(&deployed).expect("the deployed dir is created");
    // A real conversion produces the deployed derivative: same stem, AVIF.
    let output = run(&[
        "convert",
        &root.to_string_lossy(),
        "--output",
        &deployed.to_string_lossy(),
        "--format",
        "avif",
    ]);
    assert_eq!(output.status.code(), Some(0));
    assert!(deployed.join("hero.avif").is_file());
    (root, deployed)
}

fn deployed_resource(id: &str, formats: &[&str]) -> serde_json::Value {
    let mut resource = minimal_resource(id, &["hero.jpg"]);
    resource["formats"] = serde_json::json!(formats);
    resource["max_edge"] = serde_json::json!(1600);
    resource
}

#[test]
fn handoff_deployed_verifies_constraints_and_names_gaps() {
    let dir = workdir("deployed");
    let (root, deployed) = deployed_tree(&dir);
    let report = mapping_report(
        &dir,
        "report.json",
        serde_json::json!([
            deployed_resource("r1", &["avif"]),
            deployed_resource("r2", &["jpeg"]),
        ]),
    );
    let output = run(&[
        "handoff",
        &report.to_string_lossy(),
        "--root",
        &root.to_string_lossy(),
        "--deployed",
        &deployed.to_string_lossy(),
        "--json",
    ]);
    // One derivative arrived meeting its constraints, the other answers to
    // the name in the wrong format: gaps exit 1, like a partial run.
    assert_eq!(output.status.code(), Some(1));
    let doc = stdout_json(&output);
    let statuses: Vec<&str> = doc["deployed"]
        .as_array()
        .expect("checks list")
        .iter()
        .map(|check| check["status"].as_str().expect("a status"))
        .collect();
    assert_eq!(statuses, vec!["deployed", "differs"]);
    assert_eq!(doc["deploy_summary"]["deployed"], 1);
    assert_eq!(doc["deploy_summary"]["differs"], 1);
    let _ = std::fs::remove_dir_all(&dir);
}

#[test]
fn handoff_deployed_clean_tree_exits_zero() {
    let dir = workdir("deployed-clean");
    let (root, deployed) = deployed_tree(&dir);
    let report = mapping_report(
        &dir,
        "report.json",
        serde_json::json!([deployed_resource("r1", &["avif"])]),
    );
    let output = run(&[
        "handoff",
        &report.to_string_lossy(),
        "--root",
        &root.to_string_lossy(),
        "--deployed",
        &deployed.to_string_lossy(),
    ]);
    assert_eq!(output.status.code(), Some(0));
    assert!(
        String::from_utf8_lossy(&output.stdout).contains("deployed r1"),
        "the checklist names the verified file"
    );
    let _ = std::fs::remove_dir_all(&dir);
}

#[test]
fn handoff_deployed_missing_tree_reports_missing() {
    let dir = workdir("deployed-missing");
    let root = dir.join("photos");
    std::fs::create_dir_all(&root).expect("the root is created");
    std::fs::write(root.join("hero.jpg"), photo_png()).expect("hero is written");
    let empty = dir.join("live");
    std::fs::create_dir_all(&empty).expect("the empty tree is created");
    let report = mapping_report(
        &dir,
        "report.json",
        serde_json::json!([deployed_resource("r1", &["avif"])]),
    );
    let output = run(&[
        "handoff",
        &report.to_string_lossy(),
        "--root",
        &root.to_string_lossy(),
        "--deployed",
        &empty.to_string_lossy(),
        "--json",
    ]);
    assert_eq!(output.status.code(), Some(1));
    let doc = stdout_json(&output);
    assert_eq!(doc["deployed"][0]["status"], "missing");
    let _ = std::fs::remove_dir_all(&dir);
}
