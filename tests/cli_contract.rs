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

fn run_owned(args: &[String]) -> Output {
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

#[test]
fn convert_targets_write_each_namespace_with_its_recipe() {
    let dir = workdir("targets");
    photo(&dir, "a.png");
    let target = dir.to_string_lossy().into_owned();
    let output = run(&[
        "convert",
        &target,
        "--target",
        "recommended=web",
        "--target",
        "small-files=small",
        "--json",
    ]);
    assert_eq!(output.status.code(), Some(0));
    assert!(dir.join("optimized").join("web").join("a.webp").is_file());
    assert!(dir.join("optimized").join("small").join("a.avif").is_file());
    let doc = stdout_json(&output);
    assert_eq!(doc["targets"].as_array().map(Vec::len), Some(2));
    assert_eq!(doc["summary"]["converted"], 2);
    // The second run reuses both namespaces without rewriting them.
    let again = run(&[
        "convert",
        &target,
        "--target",
        "recommended=web",
        "--target",
        "small-files=small",
        "--skip-existing",
        "--json",
    ]);
    assert_eq!(again.status.code(), Some(0));
    assert_eq!(stdout_json(&again)["summary"]["skipped"], 2);
    let _ = std::fs::remove_dir_all(&dir);
}

fn target_manifest_speed(path: &Path) -> Option<u8> {
    let manifest = std::fs::read_to_string(path).expect("the target manifest exists");
    let line = manifest
        .lines()
        .next()
        .expect("the manifest has one output");
    serde_json::from_str::<serde_json::Value>(line)
        .expect("the manifest line is JSON")["avif_speed"]
        .as_u64()
        .map(|speed| speed as u8)
}

fn write_target_recipe(dir: &Path, id: &str, speed: &str) {
    std::fs::write(
        dir.join(format!("{id}.json")),
        format!(
            r#"{{"schema":1,"id":"{id}","name":"{id}","revision":1,"provenance":"personal","format":"avif","quality":{{"lossy":60.0}},"max_edge":null,"avif_speed":{speed}}}"#
        ),
    )
    .expect("the recipe is written");
}

#[test]
fn target_recipes_record_normalized_avif_speed_and_skip_on_the_second_run() {
    let dir = workdir("target-speed");
    let root = dir.join("images");
    let recipes = config_root(&dir).join("imageguide/recipes");
    std::fs::create_dir_all(&root).expect("the image root is created");
    std::fs::create_dir_all(&recipes).expect("the recipe folder is created");
    photo(&root, "shot.png");
    write_target_recipe(&recipes, "fast", "10");
    write_target_recipe(&recipes, "explicit-default", "6");
    write_target_recipe(&recipes, "ambient-default", "null");
    let target = root.to_string_lossy().into_owned();
    let first = supplier_run(
        &dir,
        &[
            "convert",
            &target,
            "--target",
            "fast=fast",
            "--target",
            "explicit-default=explicit-default",
            "--target",
            "ambient-default=ambient-default",
            "--json",
        ],
    );
    assert_eq!(
        first.status.code(),
        Some(0),
        "stdout={} stderr={}",
        String::from_utf8_lossy(&first.stdout),
        String::from_utf8_lossy(&first.stderr)
    );
    assert_eq!(stdout_json(&first)["summary"]["converted"], 3);
    let output = root.join("optimized");
    assert_eq!(
        target_manifest_speed(&output.join("fast/.press-manifest.jsonl")),
        Some(10)
    );
    assert_eq!(
        target_manifest_speed(&output.join("explicit-default/.press-manifest.jsonl")),
        None
    );
    assert_eq!(
        target_manifest_speed(&output.join("ambient-default/.press-manifest.jsonl")),
        None
    );

    let second = supplier_run(
        &dir,
        &[
            "convert",
            &target,
            "--target",
            "fast=fast",
            "--target",
            "explicit-default=explicit-default",
            "--target",
            "ambient-default=ambient-default",
            "--skip-existing",
            "--json",
        ],
    );
    assert_eq!(
        second.status.code(),
        Some(0),
        "stdout={} stderr={}",
        String::from_utf8_lossy(&second.stdout),
        String::from_utf8_lossy(&second.stderr)
    );
    let report = stdout_json(&second);
    assert_eq!(report["summary"]["converted"], 0);
    assert_eq!(report["summary"]["skipped"], 3);
    let _ = std::fs::remove_dir_all(dir);
}

#[test]
fn convert_targets_refuse_before_writing_anything() {
    let dir = workdir("targets-refuse");
    photo(&dir, "a.png");
    let target = dir.to_string_lossy().into_owned();
    // Unknown recipes fail the run, not one sibling.
    let output = run(&["convert", &target, "--target", "nope=web"]);
    assert_eq!(output.status.code(), Some(2));
    assert!(
        !dir.join("optimized").exists(),
        "nothing converts on refusal"
    );
    // Overlapping namespaces and replace mode refuse at parse time.
    for args in [
        vec![
            "convert",
            &target,
            "--target",
            "recommended=a",
            "--target",
            "recommended=a/b",
        ],
        vec![
            "convert",
            &target,
            "--target",
            "recommended=web",
            "--replace",
        ],
    ] {
        assert_eq!(run(&args).status.code(), Some(2));
    }
    let _ = std::fs::remove_dir_all(&dir);
}

fn saved_plan_fixture(tag: &str) -> (PathBuf, PathBuf, PathBuf, String, String, String) {
    let dir = workdir(tag);
    let source = dir.join("source");
    let output = dir.join("output");
    std::fs::create_dir_all(&source).expect("the source root is created");
    std::fs::create_dir_all(&output).expect("the output root is created");
    photo(&source, "shot.png");
    let plan = dir.join("saved-plan.json");
    let source_text = source.to_string_lossy().into_owned();
    let output_text = output.to_string_lossy().into_owned();
    let plan_text = plan.to_string_lossy().into_owned();
    (dir, source, output, source_text, output_text, plan_text)
}

fn create_saved_plan(source: &str, output: &str, plan: &str) -> Output {
    run_owned(&[
        "plan".into(),
        "--root".into(),
        source.into(),
        "--output".into(),
        output.into(),
        "--plan".into(),
        plan.into(),
        "--json".into(),
    ])
}

fn execute_saved_plan(plan: &str, source: &str, output: &str, mode: &str) -> Output {
    run_owned(&[
        "execute".into(),
        plan.into(),
        "--root".into(),
        source.into(),
        "--output".into(),
        output.into(),
        mode.into(),
        "--json".into(),
    ])
}

fn reconcile_saved_plan(plan: &str, source: &str, output: &str) -> Output {
    run_owned(&[
        "reconcile".into(),
        plan.into(),
        "--root".into(),
        source.into(),
        "--output".into(),
        output.into(),
        "--json".into(),
    ])
}

#[test]
fn saved_plan_round_trip_and_reconcile_reuses_the_receipt() {
    let (dir, _source, output, source, output_root, plan) = saved_plan_fixture("saved-round-trip");
    let planned = create_saved_plan(&source, &output_root, &plan);
    assert_eq!(planned.status.code(), Some(0), "{}", stderr(&planned));
    let document = stdout_json(&planned);
    assert_eq!(document["command"], "plan");
    assert_eq!(document["status"], "planned");
    assert_eq!(document["plan"]["write_scope"], "outputs_only");
    assert_eq!(document["plan"]["sources"][0]["source"], "shot.png");
    assert_eq!(
        document["plan"]["targets"][0]["mappings"][0]["output"],
        "shot.webp"
    );
    assert!(
        !output.join("shot.webp").exists(),
        "planning writes no image"
    );

    let executed = execute_saved_plan(&plan, &source, &output_root, "--continue-unstarted");
    assert_eq!(executed.status.code(), Some(0), "{}", stderr(&executed));
    let receipt = stdout_json(&executed);
    assert_eq!(receipt["status"], "complete");
    assert_eq!(receipt["counts"]["written"], 1);
    assert!(receipt["items"][0]["output_hash"].is_string());
    let bytes = std::fs::read(output.join("shot.webp")).expect("the planned output is written");

    let reconciled = reconcile_saved_plan(&plan, &source, &output_root);
    assert_eq!(reconciled.status.code(), Some(0), "{}", stderr(&reconciled));
    let repaired = stdout_json(&reconciled);
    assert_eq!(repaired["command"], "reconcile");
    assert_eq!(repaired["status"], "complete");
    assert_eq!(repaired["counts"]["written"], 1);
    assert_eq!(std::fs::read(output.join("shot.webp")).unwrap(), bytes);
    let _ = std::fs::remove_dir_all(dir);
}

#[test]
fn saved_plan_cancel_keeps_unstarted_items_distinct_from_failures() {
    let (dir, _source, output, source, output_root, plan) = saved_plan_fixture("saved-cancel");
    assert_eq!(
        create_saved_plan(&source, &output_root, &plan)
            .status
            .code(),
        Some(0)
    );
    let cancelled = execute_saved_plan(&plan, &source, &output_root, "--cancel");
    assert_eq!(cancelled.status.code(), Some(1), "{}", stderr(&cancelled));
    let report = stdout_json(&cancelled);
    assert_eq!(report["status"], "partial");
    assert_eq!(report["counts"]["cancelled"], 1);
    assert_eq!(report["counts"]["failed"], 0);
    assert_eq!(report["items"][0]["status"], "cancelled");
    assert!(
        !output.join("shot.webp").exists(),
        "cancellation writes no image"
    );
    let _ = std::fs::remove_dir_all(dir);
}

#[test]
fn saved_plan_refuses_a_same_size_source_edit_even_with_the_old_mtime() {
    let (dir, source_root, _output, source, output, plan) =
        saved_plan_fixture("saved-source-stale");
    let source_path = source_root.join("shot.png");
    let old_modified = std::fs::metadata(&source_path)
        .expect("the source stats exist")
        .modified()
        .expect("the source mtime exists");
    let mut changed = photo_png();
    let last = changed.len() - 1;
    changed[last] ^= 1;
    std::fs::write(&source_path, changed).expect("the replacement source is written");
    std::fs::File::options()
        .write(true)
        .open(&source_path)
        .expect("the replacement source opens")
        .set_modified(old_modified)
        .expect("the old mtime is restored");

    let planned = create_saved_plan(&source, &output, &plan);
    assert_eq!(planned.status.code(), Some(0), "{}", stderr(&planned));
    // Change after planning, while restoring the old stats. The checked decoder
    // refuses the snapshot before it has to understand the replacement bytes.
    let mut changed_again = photo_png();
    let last = changed_again.len() - 1;
    changed_again[last] ^= 2;
    std::fs::write(&source_path, changed_again).expect("the stale replacement is written");
    std::fs::File::options()
        .write(true)
        .open(&source_path)
        .expect("the stale replacement opens")
        .set_modified(old_modified)
        .expect("the old mtime is restored again");

    let executed = execute_saved_plan(&plan, &source, &output, "--continue-unstarted");
    assert_eq!(executed.status.code(), Some(1));
    let report = stdout_json(&executed);
    assert_eq!(report["status"], "partial");
    assert_eq!(report["counts"]["failed"], 1);
    assert_eq!(report["items"][0]["status"], "failed");
    assert!(
        report["items"][0]["error"]
            .as_str()
            .is_some_and(|error| error.contains("changed"))
    );
    assert!(!dir.join("output/shot.webp").exists());
    let _ = std::fs::remove_dir_all(dir);
}

#[test]
fn saved_plan_preserves_an_edited_unrecorded_output_at_the_writer_boundary() {
    let (dir, _source_root, output, source, output_root, plan) =
        saved_plan_fixture("saved-output-edited");
    let planned = create_saved_plan(&source, &output_root, &plan);
    assert_eq!(planned.status.code(), Some(0), "{}", stderr(&planned));
    let edited = b"an external file that the plan does not own".to_vec();
    let expected_path = output.join("shot.webp");
    std::fs::write(&expected_path, &edited).expect("the external output is written");

    let executed = execute_saved_plan(&plan, &source, &output_root, "--continue-unstarted");
    assert_eq!(executed.status.code(), Some(1));
    let report = stdout_json(&executed);
    assert_eq!(report["items"][0]["status"], "failed");
    assert!(
        report["items"][0]["error"]
            .as_str()
            .is_some_and(|error| error.contains("output"))
    );
    assert_eq!(std::fs::read(&expected_path).unwrap(), edited);
    let _ = std::fs::remove_dir_all(dir);
}

#[test]
fn saved_plan_refuses_a_collision_mapping_changed_after_creation() {
    let (dir, _source_root, output, source, output_root, plan) =
        saved_plan_fixture("saved-collision");
    let planned = create_saved_plan(&source, &output_root, &plan);
    assert_eq!(planned.status.code(), Some(0), "{}", stderr(&planned));
    let occupied = output.join("shot.webp");
    std::fs::write(&occupied, b"owned by another source").expect("the occupied output is written");
    let modified = std::fs::metadata(&occupied)
        .expect("the occupied output stats exist")
        .modified()
        .expect("the occupied output mtime exists")
        .duration_since(std::time::UNIX_EPOCH)
        .expect("the occupied output time is unix")
        .as_secs();
    let record = serde_json::json!({
        "source": "other.png",
        "source_bytes": 1,
        "source_modified": null,
        "source_hash": null,
        "output": "shot.webp",
        "output_bytes": b"owned by another source".len(),
        "output_modified": modified,
        "output_hash": null,
        "format": "webp",
        "quality": "80",
        "max_edge": null,
        "avif_speed": null,
        "recipe": null,
        "written": 1,
        "backup": null
    });
    std::fs::write(
        output.join(".press-manifest.jsonl"),
        format!("{}\n", serde_json::to_string(&record).unwrap()),
    )
    .expect("the foreign receipt is written");

    let executed = execute_saved_plan(&plan, &source, &output_root, "--continue-unstarted");
    assert_eq!(executed.status.code(), Some(2));
    let report = stdout_json(&executed);
    assert_eq!(report["status"], "failed");
    assert!(
        report["error"]
            .as_str()
            .is_some_and(|error| error.contains("collision mapping changed"))
    );
    assert_eq!(
        std::fs::read(&occupied).unwrap(),
        b"owned by another source"
    );
    let _ = std::fs::remove_dir_all(dir);
}

#[test]
fn saved_plan_reconcile_repairs_a_receipt_left_running_by_an_interruption() {
    let (dir, _source_root, output, source, output_root, plan) =
        saved_plan_fixture("saved-receipt-recovery");
    assert_eq!(
        create_saved_plan(&source, &output_root, &plan)
            .status
            .code(),
        Some(0)
    );
    assert_eq!(
        execute_saved_plan(&plan, &source, &output_root, "--continue-unstarted")
            .status
            .code(),
        Some(0)
    );
    let state_path = std::fs::read_dir(&dir)
        .expect("the plan folder is readable")
        .filter_map(Result::ok)
        .map(|entry| entry.path())
        .find(|path| path.to_string_lossy().contains(".run-"))
        .expect("execution leaves a binding-specific state file");
    let mut state: serde_json::Value =
        serde_json::from_slice(&std::fs::read(&state_path).expect("the run state is readable"))
            .expect("the run state is JSON");
    state["items"][0]["status"] = serde_json::json!("running");
    state["items"][0]["output_hash"] = serde_json::Value::Null;
    state["items"][0]["output_bytes"] = serde_json::Value::Null;
    state["items"][0]["width"] = serde_json::Value::Null;
    state["items"][0]["height"] = serde_json::Value::Null;
    state["items"][0]["checks"] = serde_json::json!([]);
    state["items"][0]["error"] = serde_json::Value::Null;
    std::fs::write(&state_path, serde_json::to_vec_pretty(&state).unwrap())
        .expect("the interrupted state is written");

    let reconciled = reconcile_saved_plan(&plan, &source, &output_root);
    assert_eq!(reconciled.status.code(), Some(0), "{}", stderr(&reconciled));
    let report = stdout_json(&reconciled);
    assert_eq!(report["status"], "complete");
    assert_eq!(report["items"][0]["status"], "written");
    assert!(report["items"][0]["output_hash"].is_string());
    assert!(output.join("shot.webp").is_file());
    let _ = std::fs::remove_dir_all(dir);
}

#[test]
fn saved_plan_malformed_and_oversize_files_have_named_json_errors() {
    let (dir, _source_root, _output, source, output, plan) = saved_plan_fixture("saved-bounds");
    std::fs::write(&plan, br#"{"schema_version":1}"#).expect("the malformed plan is written");
    let malformed = execute_saved_plan(&plan, &source, &output, "--continue-unstarted");
    assert_eq!(malformed.status.code(), Some(2));
    let malformed_report = stdout_json(&malformed);
    assert_eq!(malformed_report["command"], "execute");
    assert_eq!(malformed_report["status"], "failed");
    assert!(malformed_report["error"].is_string());

    let oversized = vec![b'{'; (saved_plan_limit() + 1) as usize];
    std::fs::write(&plan, oversized).expect("the oversized plan is written");
    let oversized_run = execute_saved_plan(&plan, &source, &output, "--continue-unstarted");
    assert_eq!(oversized_run.status.code(), Some(2));
    let oversized_report = stdout_json(&oversized_run);
    assert!(
        oversized_report["error"]
            .as_str()
            .is_some_and(|error| error.contains("limit"))
    );
    let _ = std::fs::remove_dir_all(dir);
}

fn saved_plan_limit() -> u64 {
    8 * 1024 * 1024
}

#[test]
fn saved_plan_treats_shell_like_names_as_path_data() {
    let dir = workdir("saved-shell-data");
    let source_root = dir.join("source");
    let output_root = dir.join("output");
    std::fs::create_dir_all(&source_root).expect("the source root is created");
    std::fs::create_dir_all(&output_root).expect("the output root is created");
    let shell_name = "$(touch-press-plan-marker).png";
    photo(&source_root, shell_name);
    let plan = dir.join("plan.json");
    let source = source_root.to_string_lossy().into_owned();
    let output = output_root.to_string_lossy().into_owned();
    let plan_text = plan.to_string_lossy().into_owned();
    let planned = create_saved_plan(&source, &output, &plan_text);
    assert_eq!(planned.status.code(), Some(0), "{}", stderr(&planned));
    let executed = execute_saved_plan(&plan_text, &source, &output, "--continue-unstarted");
    assert_eq!(executed.status.code(), Some(0), "{}", stderr(&executed));
    assert!(
        output_root
            .join("$(touch-press-plan-marker).webp")
            .is_file()
    );
    assert!(!dir.join("press-plan-marker").exists());
    let _ = std::fs::remove_dir_all(dir);
}

fn supplier_home(tag: &str) -> PathBuf {
    let home =
        std::env::temp_dir().join(format!("press-supplier-home-{tag}-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&home);
    std::fs::create_dir_all(&home).expect("the fake home is created");
    home
}

fn config_root(home: &Path) -> PathBuf {
    if cfg!(target_os = "macos") {
        home.join("Library/Application Support")
    } else {
        home.to_path_buf()
    }
}

fn supplier_run(home: &Path, args: &[&str]) -> Output {
    Command::new(env!("CARGO_BIN_EXE_press"))
        .args(args)
        .env("HOME", home)
        .env("XDG_CONFIG_HOME", home)
        .env("APPDATA", home)
        .output()
        .expect("the binary runs")
}

/// A saved job, assignment and photo set that the rehearsal verbs can run
/// against: the job lives in the isolated config library, the photo in root.
fn supplier_fixture(home: &Path, tag: &str) -> (PathBuf, PathBuf) {
    let root = home.join(tag);
    std::fs::create_dir_all(&root).expect("the root is created");
    let file = root.join("hero.png");
    std::fs::write(&file, photo_png()).expect("the photo is written");
    let bytes = std::fs::metadata(&file).expect("the photo stats").len();
    let jobs = config_root(home).join("imageguide").join("jobs");
    std::fs::create_dir_all(&jobs).expect("the job library is created");
    let job = format!(
        r#"{{"schema":1,"id":"rehearse","name":"Rehearsal","revision":1,"source_roots":[{root:?}],"target_recipe":null,"products":[{{"id":"hero","name":"Hero","sku_hint":"SKU-1","roles":[{{"id":"main","label":"Main","required":true}}],"mappings":[{{"id":"m1","role_id":"main","source":{{"path":{file:?},"bytes":{bytes},"modified":1700000000}}}}],"binding":null}}]}}"#,
        root = root.to_string_lossy(),
        file = file.to_string_lossy(),
    );
    std::fs::write(jobs.join("rehearse.json"), job).expect("the job is saved");
    let assignment = home.join("assignment.json");
    std::fs::write(
        &assignment,
        r#"{"schema":1,"id":"assign-1","workspace":"retailer","supplier":"studio-9","products":[{"id":"hero","slots":[{"id":"main","policy_revision":"policy-7","requirements":[]}]}],"retrieved_at":1}"#,
    )
    .expect("the assignment is written");
    (root, assignment)
}

fn fake_script(home: &Path, name: &str, body: &str) -> String {
    let path = home.join(name);
    std::fs::write(&path, body).expect("the script is written");
    path.to_string_lossy().into_owned()
}

#[test]
fn supplier_prepare_submit_and_status_rehearse() {
    let home = supplier_home("round-trip");
    let (root, assignment) = supplier_fixture(&home, "photos");
    let root = root.to_string_lossy().into_owned();
    let assignment = assignment.to_string_lossy().into_owned();
    let script = fake_script(
        &home,
        "submit.json",
        r#"{"attempts":{"a1":[{"accept":{"receipt":"r-1"}}]}}"#,
    );
    let prepare = supplier_run(
        &home,
        &["supplier", &root, "prepare", "--assignment", &assignment],
    );
    assert_eq!(prepare.status.code(), Some(0), "{}", stderr(&prepare));
    assert!(
        config_root(&home)
            .join("imageguide")
            .join("jobs")
            .join("rehearse.attempts.json")
            .is_file(),
        "prepare persists the queue"
    );
    let submit = supplier_run(
        &home,
        &[
            "supplier",
            &root,
            "submit",
            "--assignment",
            &assignment,
            "--fake",
            &script,
        ],
    );
    assert_eq!(submit.status.code(), Some(0), "{}", stderr(&submit));
    let status = supplier_run(&home, &["supplier", &root, "status", "--json"]);
    assert_eq!(status.status.code(), Some(0));
    let doc = stdout_json(&status);
    assert_eq!(doc["attempts"][0]["state"], "transferred");
    assert_eq!(doc["attempts"][0]["receipt"], "r-1");
    let _ = std::fs::remove_dir_all(&home);
}

#[test]
fn supplier_corrupt_attempt_log_is_reported_and_preserved() {
    let home = supplier_home("corrupt-log");
    let (root, assignment) = supplier_fixture(&home, "photos");
    let root = root.to_string_lossy().into_owned();
    let assignment = assignment.to_string_lossy().into_owned();
    assert_eq!(
        supplier_run(
            &home,
            &["supplier", &root, "prepare", "--assignment", &assignment]
        )
        .status
        .code(),
        Some(0)
    );
    let log = config_root(&home)
        .join("imageguide")
        .join("jobs")
        .join("rehearse.attempts.json");
    let damaged = br"{broken";
    std::fs::write(&log, damaged).expect("the log is damaged");
    let output = supplier_run(
        &home,
        &[
            "supplier",
            &root,
            "prepare",
            "--assignment",
            &assignment,
            "--json",
        ],
    );
    assert_eq!(output.status.code(), Some(1), "{}", stderr(&output));
    assert!(
        stderr(&output).contains("does not parse"),
        "{}",
        stderr(&output)
    );
    assert_eq!(std::fs::read(&log).unwrap(), damaged);
    let _ = std::fs::remove_dir_all(&home);
}

#[test]
fn supplier_legacy_attempts_are_preserved_and_refused() {
    let home = supplier_home("legacy-log");
    let (root, assignment) = supplier_fixture(&home, "photos");
    let root = root.to_string_lossy().into_owned();
    let assignment = assignment.to_string_lossy().into_owned();
    let log = config_root(&home)
        .join("imageguide")
        .join("jobs")
        .join("rehearse.attempts.json");
    let legacy = br#"{"schema":1,"job_id":"rehearse","client_job_id":"cj-old","next_attempt":2,"attempts":[{"id":"a1","mapping_id":"m1","source_hash":"old","source_bytes":1,"slot":"main","recipe_fingerprint":"fp","policy_revision":"policy-7","state":"prepared","receipt":null,"correction_of":null}]}"#;
    std::fs::write(&log, legacy).expect("the legacy log is written");
    let output = supplier_run(
        &home,
        &["supplier", &root, "prepare", "--assignment", &assignment],
    );
    assert_eq!(output.status.code(), Some(1), "{}", stderr(&output));
    assert!(stderr(&output).contains("legacy"), "{}", stderr(&output));
    assert_eq!(std::fs::read(&log).unwrap(), legacy);
    let _ = std::fs::remove_dir_all(&home);
}

#[test]
fn supplier_refuses_to_choose_between_jobs_for_one_root() {
    let home = supplier_home("ambiguous-job");
    let (root, assignment) = supplier_fixture(&home, "photos");
    let jobs = config_root(&home).join("imageguide").join("jobs");
    let original = std::fs::read_to_string(jobs.join("rehearse.json")).unwrap();
    let other = original.replace("\"id\":\"rehearse\"", "\"id\":\"other\"");
    std::fs::write(jobs.join("other.json"), other).unwrap();
    let root = root.to_string_lossy().into_owned();
    let assignment = assignment.to_string_lossy().into_owned();
    let output = supplier_run(
        &home,
        &["supplier", &root, "prepare", "--assignment", &assignment],
    );
    assert_eq!(output.status.code(), Some(2), "{}", stderr(&output));
    assert!(
        stderr(&output).contains("multiple saved jobs"),
        "{}",
        stderr(&output)
    );
    let _ = std::fs::remove_dir_all(&home);
}

#[test]
fn supplier_transferred_attempt_cannot_be_cancelled() {
    let home = supplier_home("cancel-transferred");
    let (root, assignment) = supplier_fixture(&home, "photos");
    let root = root.to_string_lossy().into_owned();
    let assignment = assignment.to_string_lossy().into_owned();
    let script = fake_script(
        &home,
        "submit.json",
        r#"{"attempts":{"a1":[{"accept":{"receipt":"r-1"}}]}}"#,
    );
    assert_eq!(
        supplier_run(
            &home,
            &["supplier", &root, "prepare", "--assignment", &assignment]
        )
        .status
        .code(),
        Some(0)
    );
    assert_eq!(
        supplier_run(
            &home,
            &[
                "supplier",
                &root,
                "submit",
                "--assignment",
                &assignment,
                "--fake",
                &script,
            ]
        )
        .status
        .code(),
        Some(0)
    );
    let cancelled = supplier_run(&home, &["supplier", &root, "cancel", "a1"]);
    assert_eq!(cancelled.status.code(), Some(1), "{}", stderr(&cancelled));
    assert!(
        stderr(&cancelled).contains("Transferred"),
        "{}",
        stderr(&cancelled)
    );
    let _ = std::fs::remove_dir_all(&home);
}

#[test]
fn supplier_reconcile_adopts_the_server_answer_across_restarts() {
    let home = supplier_home("reconcile");
    let (root, assignment) = supplier_fixture(&home, "photos");
    let root = root.to_string_lossy().into_owned();
    let assignment = assignment.to_string_lossy().into_owned();
    let submit_script = fake_script(
        &home,
        "submit.json",
        r#"{"attempts":{"a1":[{"accept":{"receipt":"r-1"}}]}}"#,
    );
    assert_eq!(
        supplier_run(
            &home,
            &["supplier", &root, "prepare", "--assignment", &assignment]
        )
        .status
        .code(),
        Some(0)
    );
    assert_eq!(
        supplier_run(
            &home,
            &[
                "supplier",
                &root,
                "submit",
                "--assignment",
                &assignment,
                "--fake",
                &submit_script
            ]
        )
        .status
        .code(),
        Some(0)
    );
    // A new process meets a server that already took the bytes: the journal
    // beside the script remembers the acceptance the attempt log cannot see.
    let reconcile_script = fake_script(
        &home,
        "reconcile.json",
        r#"{"attempts":{"a1":[{"advance":{"receipt":"r-1","state":"accepted"}}]}}"#,
    );
    let reconcile = supplier_run(
        &home,
        &[
            "supplier",
            &root,
            "reconcile",
            "--fake",
            &reconcile_script,
            "--json",
        ],
    );
    assert_eq!(reconcile.status.code(), Some(0), "{}", stderr(&reconcile));
    let doc = stdout_json(&reconcile);
    assert_eq!(doc["attempts"][0]["state"], "accepted");
    assert_eq!(doc["attempts"][0]["receipt"], "r-1");
    let _ = std::fs::remove_dir_all(&home);
}

#[test]
fn supplier_submit_names_refusals_and_duplicates() {
    let home = supplier_home("refusals");
    let (root, assignment) = supplier_fixture(&home, "photos");
    let root = root.to_string_lossy().into_owned();
    let assignment = assignment.to_string_lossy().into_owned();
    assert_eq!(
        supplier_run(
            &home,
            &["supplier", &root, "prepare", "--assignment", &assignment]
        )
        .status
        .code(),
        Some(0)
    );
    // A revoked assignment keeps its name and sends nothing.
    let revoked = fake_script(&home, "revoked.json", r#"{"attempts":{"a1":["revoked"]}}"#);
    let output = supplier_run(
        &home,
        &[
            "supplier",
            &root,
            "submit",
            "--assignment",
            &assignment,
            "--fake",
            &revoked,
        ],
    );
    assert_eq!(output.status.code(), Some(1));
    assert!(stderr(&output).contains("revoked"), "{}", stderr(&output));
    // The accepted bytes meet their duplicate on retry, not a second job.
    let accept = fake_script(
        &home,
        "accept.json",
        r#"{"attempts":{"a1":[{"accept":{"receipt":"r-1"}}]}}"#,
    );
    assert_eq!(
        supplier_run(
            &home,
            &[
                "supplier",
                &root,
                "submit",
                "--assignment",
                &assignment,
                "--fake",
                &accept
            ]
        )
        .status
        .code(),
        Some(0)
    );
    let again = supplier_run(
        &home,
        &[
            "supplier",
            &root,
            "submit",
            "--assignment",
            &assignment,
            "--fake",
            &accept,
            "--json",
        ],
    );
    assert_eq!(again.status.code(), Some(0));
    assert_eq!(stdout_json(&again)["failed"], serde_json::json!([]));
    let _ = std::fs::remove_dir_all(&home);
}

#[test]
fn supplier_verbs_refuse_bad_usage() {
    let home = supplier_home("usage");
    let (root, assignment) = supplier_fixture(&home, "photos");
    let root = root.to_string_lossy().into_owned();
    let assignment = assignment.to_string_lossy().into_owned();
    // No verb, unknown verb, missing attempt id, missing files, unknown job.
    assert_eq!(
        supplier_run(&home, &["supplier", &root]).status.code(),
        Some(2)
    );
    assert_eq!(
        supplier_run(&home, &["supplier", &root, "launch"])
            .status
            .code(),
        Some(2)
    );
    assert_eq!(
        supplier_run(&home, &["supplier", &root, "cancel"])
            .status
            .code(),
        Some(2)
    );
    assert_eq!(
        supplier_run(&home, &["supplier", &root, "prepare"])
            .status
            .code(),
        Some(2),
        "prepare needs its assignment"
    );
    assert_eq!(
        supplier_run(
            &home,
            &["supplier", &root, "submit", "--assignment", &assignment]
        )
        .status
        .code(),
        Some(2),
        "submit needs its rehearsal intake"
    );
    let empty = home.join("empty");
    std::fs::create_dir_all(&empty).expect("the empty folder is created");
    assert_eq!(
        supplier_run(&home, &["supplier", &empty.to_string_lossy(), "status"])
            .status
            .code(),
        Some(2),
        "no saved job covers the folder"
    );
    let _ = std::fs::remove_dir_all(&home);
}

#[test]
fn supplier_cancel_and_correct_relink_history() {
    let home = supplier_home("cancel");
    let (root, assignment) = supplier_fixture(&home, "photos");
    let root = root.to_string_lossy().into_owned();
    let assignment = assignment.to_string_lossy().into_owned();
    let script = fake_script(
        &home,
        "s.json",
        r#"{"attempts":{"a1":[{"accept":{"receipt":"r-1"}}]}}"#,
    );
    assert_eq!(
        supplier_run(
            &home,
            &["supplier", &root, "prepare", "--assignment", &assignment]
        )
        .status
        .code(),
        Some(0)
    );
    assert_eq!(
        supplier_run(&home, &["supplier", &root, "cancel", "a1"])
            .status
            .code(),
        Some(0)
    );
    assert_eq!(
        supplier_run(&home, &["supplier", &root, "cancel", "a1", "--json"])
            .status
            .code(),
        Some(1),
        "withdrawing twice fails"
    );
    assert_eq!(
        supplier_run(&home, &["supplier", &root, "correct", "a1"])
            .status
            .code(),
        Some(1),
        "a cancelled attempt takes no correction"
    );
    // Rehearse the rejection loop: accept, advance to rejected, correct.
    let home = supplier_home("correct");
    let (root, assignment) = supplier_fixture(&home, "photos");
    let root = root.to_string_lossy().into_owned();
    let assignment = assignment.to_string_lossy().into_owned();
    let accept = fake_script(
        &home,
        "accept.json",
        r#"{"attempts":{"a1":[{"accept":{"receipt":"r-9"}}]}}"#,
    );
    let reject = fake_script(
        &home,
        "reject.json",
        r#"{"attempts":{"a1":[{"advance":{"receipt":"too dark","state":"rejected"}}]}}"#,
    );
    assert_eq!(
        supplier_run(
            &home,
            &["supplier", &root, "prepare", "--assignment", &assignment]
        )
        .status
        .code(),
        Some(0)
    );
    assert_eq!(
        supplier_run(
            &home,
            &[
                "supplier",
                &root,
                "submit",
                "--assignment",
                &assignment,
                "--fake",
                &accept
            ]
        )
        .status
        .code(),
        Some(0)
    );
    assert_eq!(
        supplier_run(&home, &["supplier", &root, "reconcile", "--fake", &reject])
            .status
            .code(),
        Some(0)
    );
    // New bytes for the correction: review refused the old ones.
    std::fs::write(root.clone() + "/hero.png", photo_png())
        .expect("identical bytes stay identical");
    let correct = supplier_run(&home, &["supplier", &root, "correct", "a1", "--json"]);
    assert_eq!(correct.status.code(), Some(0), "{}", stderr(&correct));
    assert_eq!(stdout_json(&correct)["correction_of"], "a1");
    let _ = std::fs::remove_dir_all(&home);
    let _ = script;
}
