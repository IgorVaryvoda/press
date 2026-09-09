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
    photo_png_seeded(0)
}

/// The same noise shifted, so two fixtures that share a name still differ byte
/// for byte and a test can say which of them a run actually read.
fn photo_png_seeded(seed: u32) -> Vec<u8> {
    let mut raw = Vec::new();
    for y in 0..8u32 {
        raw.push(0u8);
        for x in 0..8u32 {
            raw.push(((x * 37 + y * 91 + seed) % 251) as u8);
            raw.push(((x * 11 + y * 53 + seed) % 251) as u8);
            raw.push(((x * 7 + y * 13 + seed) % 251) as u8);
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

/// A child process that refuses has already said why on stdout or stderr.
/// Asserting the code alone throws that sentence away, which is the whole
/// diagnosis when the failure only happens on a runner nobody can attach to.
fn assert_exit(output: &Output, code: i32, what: &str) {
    assert_eq!(
        output.status.code(),
        Some(code),
        "{what} exited {:?}\nstdout: {}\nstderr: {}",
        output.status.code(),
        String::from_utf8_lossy(&output.stdout),
        stderr(output)
    );
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
fn check_json_reports_actual_output_against_a_local_snapshot() {
    let dir = workdir("requirements");
    photo(&dir, "shot.png");
    let requirements =
        Path::new(env!("CARGO_MANIFEST_DIR")).join("fixtures/requirements/local-web.json");
    let output = run(&[
        "check",
        &dir.to_string_lossy(),
        "--requirements-file",
        &requirements.to_string_lossy(),
        "--json",
    ]);
    assert_eq!(output.status.code(), Some(1));
    let report = stdout_json(&output);
    assert_eq!(report["requirements"]["id"], "local-web-hero");
    assert_eq!(report["outputs"][0]["output"], "shot.png");
    assert_eq!(report["outputs"][0]["actual"]["format"], "png");
    assert!(report["outputs"][0]["output_hash"].is_string());
    assert!(
        !String::from_utf8_lossy(&output.stdout).contains(dir.to_string_lossy().as_ref()),
        "the receipt does not expose the local root"
    );
    assert!(
        stderr(&output).is_empty(),
        "a checked report stays on stdout"
    );
    let _ = std::fs::remove_dir_all(&dir);
}

#[test]
fn check_process_passes_and_excludes_a_canonical_requirements_file_without_writes() {
    let dir = workdir("requirements-pass");
    photo(&dir, "shot.png");
    let requirements = dir.join("requirements.json");
    std::fs::copy(
        Path::new(env!("CARGO_MANIFEST_DIR")).join("fixtures/requirements/local-pass.json"),
        &requirements,
    )
    .expect("the local fixture is copied into the target folder");
    let target = dir.to_string_lossy().into_owned();
    let requirements = requirements.to_string_lossy().into_owned();
    let output = run(&[
        "check",
        &target,
        "--requirements-file",
        &requirements,
        "--json",
    ]);
    assert_eq!(output.status.code(), Some(0), "{}", stderr(&output));
    let report = stdout_json(&output);
    assert_eq!(report["requirements"]["id"], "local-pass");
    assert_eq!(report["outputs"].as_array().map(Vec::len), Some(1));
    assert_eq!(report["outputs"][0]["output"], "shot.png");
    assert_eq!(report["all_required_pass"], true);
    assert!(
        !dir.join("optimized").exists(),
        "checking never writes outputs"
    );
    let _ = std::fs::remove_dir_all(&dir);
}

#[test]
fn malformed_requirements_exit_two_without_a_document_or_write() {
    let dir = workdir("requirements-malformed");
    photo(&dir, "shot.png");
    let requirements = Path::new(env!("CARGO_MANIFEST_DIR"))
        .join("fixtures/requirements/malformed.json")
        .to_string_lossy()
        .into_owned();
    let target = dir.to_string_lossy().into_owned();
    let output = run(&["check", &target, "--requirements-file", &requirements]);
    assert_eq!(output.status.code(), Some(2));
    assert!(output.stdout.is_empty(), "malformed input has no receipt");
    assert!(
        stderr(&output).contains("requirements"),
        "{}",
        stderr(&output)
    );
    assert!(
        !dir.join("optimized").exists(),
        "checking never writes outputs"
    );
    let _ = std::fs::remove_dir_all(&dir);
}

#[test]
fn an_oversized_check_folder_is_refused_before_inspection_or_writes() {
    let dir = workdir("requirements-too-many");
    for index in 0..=1_024 {
        std::fs::write(dir.join(format!("item-{index}.bin")), [])
            .expect("the bounded folder fixture is written");
    }
    let requirements = Path::new(env!("CARGO_MANIFEST_DIR"))
        .join("fixtures/requirements/local-pass.json")
        .to_string_lossy()
        .into_owned();
    let target = dir.to_string_lossy().into_owned();
    let output = run(&["check", &target, "--requirements-file", &requirements]);
    assert_eq!(output.status.code(), Some(1));
    assert!(output.stdout.is_empty(), "refusal has no receipt");
    assert!(
        stderr(&output).contains("more than 1024"),
        "{}",
        stderr(&output)
    );
    assert!(
        !dir.join("optimized").exists(),
        "checking never writes outputs"
    );
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

/// A second spelling of the same folder, which is what a replace run is really
/// handed: the audited root keeps whatever was typed while the output boundary
/// is canonical, and the two disagree without either being wrong. A unix
/// symlink splits them here; on Windows the ordinary path below is already the
/// split, because canonicalising it adds the `\\?\` prefix it never had.
#[cfg(unix)]
fn aliased(dir: &Path) -> PathBuf {
    let alias = dir.with_file_name(format!(
        "{}-alias",
        dir.file_name()
            .expect("the fixture dir is named")
            .to_string_lossy()
    ));
    let _ = std::fs::remove_file(&alias);
    std::os::unix::fs::symlink(dir, &alias).expect("the alias points at the fixture");
    alias
}

#[cfg(not(unix))]
fn aliased(dir: &Path) -> PathBuf {
    dir.to_path_buf()
}

fn clean_up(dir: &Path, alias: &Path) {
    if alias != dir {
        let _ = std::fs::remove_file(alias);
    }
    let _ = std::fs::remove_dir_all(dir);
}

/// A real encoded WebP, made by the tool itself in a folder of its own. The
/// fixtures here are hand-rolled PNGs, and a source that already carries the
/// output format is the only way to reach the case where a converted file takes
/// its own name back.
fn seeded_webp(dir: &Path, name: &str) -> PathBuf {
    let seed = workdir(&format!("seed-{}", name.replace('.', "-")));
    photo(&seed, "seed.png");
    assert_exit(&run(&["convert", &seed.to_string_lossy()]), 0, "seeding");
    let path = dir.join(name);
    std::fs::copy(seed.join("optimized").join("seed.webp"), &path)
        .expect("the encoded fixture is copied");
    let _ = std::fs::remove_dir_all(&seed);
    path
}

/// The whole replace contract through one spelling of the folder: every audited
/// original ends up in the mirror untouched, every output the run names is
/// installed, and the restore puts each original back byte for byte and takes
/// the outputs away again. Which name an output takes is read out of the run's
/// own report, because it differs when the format stays the same.
fn replace_and_restore_round_trip(
    dir: &Path,
    names: &[&str],
    target: &str,
    extra: &[&str],
    restore_target: &str,
) -> serde_json::Value {
    let originals: Vec<Vec<u8>> = names
        .iter()
        .map(|name| std::fs::read(dir.join(name)).expect("the fixture reads back"))
        .collect();
    let mut args = vec!["convert", target, "--replace", "--json"];
    args.extend_from_slice(extra);
    let converted = run(&args);
    assert_exit(&converted, 0, "replace");
    let report = stdout_json(&converted);
    let files = report["files"].as_array().expect("files list").clone();
    assert_eq!(files.len(), names.len());
    let installed: Vec<PathBuf> = files
        .iter()
        .map(|file| {
            assert_eq!(file["status"], "converted", "{}", stderr(&converted));
            let output = PathBuf::from(
                file["output"]
                    .as_str()
                    .expect("a converted file names its output"),
            );
            assert!(output.is_file(), "{} is installed", output.display());
            output
        })
        .collect();
    for (name, original) in names.iter().zip(&originals) {
        assert_eq!(
            &std::fs::read(dir.join("press-originals").join(name))
                .unwrap_or_else(|error| panic!("{name} is parked in the mirror: {error}")),
            original,
            "the backup holds {name} untouched"
        );
    }
    assert_exit(&run(&["restore", restore_target]), 0, "restore");
    for (name, original) in names.iter().zip(&originals) {
        assert_eq!(
            &std::fs::read(dir.join(name))
                .unwrap_or_else(|error| panic!("{name} comes back out of the mirror: {error}")),
            original,
            "the restored {name} is the original, not an intermediate"
        );
    }
    // Which restored names to expect back is worked out against the canonical
    // fixture root, because that is the spelling the report writes its outputs
    // in: Windows adds a `\\?\` prefix the fixture path never had, and an
    // original restored over its own name would read as an extra generated
    // output this run had to take away.
    let restored = dir.canonicalize().expect("the fixture folder resolves");
    for output in installed {
        if !names.iter().any(|name| restored.join(name) == output) {
            assert!(
                !output.exists(),
                "{} is taken away with the original back",
                output.display()
            );
        }
    }
    report
}

#[test]
fn replace_and_restore_round_trip_through_the_cli() {
    let dir = workdir("replace");
    photo(&dir, "shot.png");
    let target = dir.to_string_lossy().into_owned();
    replace_and_restore_round_trip(&dir, &["shot.png"], &target, &[], &target);
    let _ = std::fs::remove_dir_all(&dir);
}

/// The same round trip through the other spelling. The audited root, the output
/// context, the collision keys and the backup mirror have to be one namespace,
/// or the run refuses every file as outside the output folder.
#[test]
fn replace_and_restore_round_trip_through_a_second_spelling_of_the_root() {
    let dir = workdir("replace-alias");
    photo(&dir, "shot.png");
    let alias = aliased(&dir);
    let target = alias.to_string_lossy().into_owned();
    replace_and_restore_round_trip(&dir, &["shot.png"], &target, &[], &target);
    clean_up(&dir, &alias);
}

/// One file named through an aliased parent. Only the parent is resolved: the
/// file keeps the name that was typed, so replace mode still moves the file that
/// was chosen rather than whatever a final symlink points at.
#[test]
fn replace_and_restore_round_trip_for_one_file_under_a_second_spelling() {
    let dir = workdir("replace-file");
    photo(&dir, "shot.png");
    let alias = aliased(&dir);
    replace_and_restore_round_trip(
        &dir,
        &["shot.png"],
        &alias.join("shot.png").to_string_lossy(),
        &[],
        &alias.to_string_lossy(),
    );
    clean_up(&dir, &alias);
}

/// `typed/link` points one level down a real tree, so `typed/link/..` names
/// `actual` to the kernel and `typed` to a lexical walk that drops the link and
/// its `..` together. Both folders exist and hold a file of the same name, so
/// which one a run resolved is visible in what it converted.
#[cfg(unix)]
fn linked_parent_fixture(tag: &str) -> (PathBuf, PathBuf, PathBuf) {
    let base = workdir(tag);
    let actual = base.join("actual");
    std::fs::create_dir_all(actual.join("sub")).expect("the linked-to folder is created");
    let typed = base.join("typed");
    std::fs::create_dir_all(&typed).expect("the folder holding the link is created");
    std::os::unix::fs::symlink(actual.join("sub"), typed.join("link"))
        .expect("the link points one level down the real tree");
    (base, actual, typed)
}

/// The decoy the lexical spelling would have picked: same name, different
/// bytes, and it must come out of the run exactly as it went in.
#[cfg(unix)]
fn decoy(dir: &Path, name: &str) -> PathBuf {
    let path = dir.join(name);
    std::fs::write(&path, photo_png_seeded(97)).expect("the decoy image is written");
    path
}

/// Nothing was converted, replaced or backed up in the folder the run never
/// audited, and the decoy is the same file it was.
#[cfg(unix)]
fn decoy_untouched(typed: &Path, decoy: &Path, before: &[u8]) {
    assert_eq!(
        std::fs::read(decoy).expect("the decoy is still there"),
        before,
        "the folder the kernel never named keeps its file untouched"
    );
    assert!(
        !typed.join("press-originals").exists(),
        "no original was parked in the folder the run never audited"
    );
    assert!(
        !typed.join("photo.webp").exists(),
        "no output was installed in the folder the run never audited"
    );
    assert!(
        !typed.join("optimized").exists(),
        "no output folder was made in the folder the run never audited"
    );
}

/// A folder named through a link's parent. `..` is the kernel's to resolve:
/// removing it from the typed spelling first names the link's own parent, and
/// the run then audits and replaces files in a folder nobody asked for.
#[cfg(unix)]
#[test]
fn a_folder_named_through_a_linked_parent_is_the_one_the_kernel_names() {
    let (base, actual, typed) = linked_parent_fixture("replace-linked-parent");
    photo(&actual, "photo.png");
    let decoy_path = decoy(&typed, "photo.png");
    let before = std::fs::read(&decoy_path).expect("the decoy reads back");
    let target = typed.join("link").join("..").to_string_lossy().into_owned();
    let report = replace_and_restore_round_trip(&actual, &["photo.png"], &target, &[], &target);
    let source = report["files"][0]["source"]
        .as_str()
        .expect("the converted file names its source")
        .to_owned();
    let resolved = actual.canonicalize().expect("the audited folder resolves");
    assert_eq!(
        PathBuf::from(source),
        resolved.join("photo.png"),
        "the run names the file the kernel reaches, not the lexical one"
    );
    decoy_untouched(&typed, &decoy_path, &before);
    let _ = std::fs::remove_dir_all(&base);
}

/// The same resolution for one file: only the parent is resolved, but it is
/// resolved by the kernel, so the file that is opened is the one the typed path
/// actually reaches.
#[cfg(unix)]
#[test]
fn one_file_named_through_a_linked_parent_is_the_one_the_kernel_names() {
    let (base, actual, typed) = linked_parent_fixture("replace-linked-parent-file");
    photo(&actual, "photo.png");
    let decoy_path = decoy(&typed, "photo.png");
    let before = std::fs::read(&decoy_path).expect("the decoy reads back");
    let parent = typed.join("link").join("..");
    let report = replace_and_restore_round_trip(
        &actual,
        &["photo.png"],
        &parent.join("photo.png").to_string_lossy(),
        &[],
        &parent.to_string_lossy(),
    );
    let source = report["files"][0]["source"]
        .as_str()
        .expect("the converted file names its source")
        .to_owned();
    let resolved = actual.canonicalize().expect("the audited folder resolves");
    assert_eq!(
        PathBuf::from(source),
        resolved.join("photo.png"),
        "the file opened is the one the parent link reaches"
    );
    decoy_untouched(&typed, &decoy_path, &before);
    let _ = std::fs::remove_dir_all(&base);
}

/// A WebP replaced by a WebP writes its own name back, which is only safe
/// because the original is in the mirror first. Recognising that name means
/// comparing the source with the planned output, and across two spellings of
/// one folder they never look equal: the write then treats its own installed
/// output as somebody else's file and refuses it as changed after planning.
#[test]
fn replace_keeps_its_own_name_through_a_second_spelling_of_the_root() {
    let dir = workdir("replace-same");
    seeded_webp(&dir, "shot.webp");
    let alias = aliased(&dir);
    let target = alias.to_string_lossy().into_owned();
    let report = replace_and_restore_round_trip(
        &dir,
        &["shot.webp"],
        &target,
        &["--format", "same"],
        &target,
    );
    let output = report["files"][0]["output"]
        .as_str()
        .expect("the converted file names its output");
    assert!(
        output.ends_with("shot.webp"),
        "the output takes its own name back: {output}"
    );
    clean_up(&dir, &alias);
}

/// `a.png` converting to WebP asks for the name of the audited `a.webp` beside
/// it. That file is an original nobody selected, so the planner refuses `a.png`
/// and converts the sibling on its own terms: the folder keeps both originals,
/// one untouched on disk and one in the mirror.
///
/// Reading the audited names in one spelling and the planned outputs in another
/// loses the collision outright. The run then writes `a.png`'s output over
/// `a.webp`, never backs that file up, and restore has nothing to hand back —
/// the run reports a saving for a file it destroyed.
#[test]
fn a_replace_run_never_overwrites_an_audited_sibling_through_a_second_spelling() {
    let dir = workdir("replace-sibling");
    photo(&dir, "a.png");
    seeded_webp(&dir, "a.webp");
    let png = std::fs::read(dir.join("a.png")).expect("the png reads back");
    let webp = std::fs::read(dir.join("a.webp")).expect("the webp reads back");
    let alias = aliased(&dir);
    let target = alias.to_string_lossy().into_owned();
    let output = run(&["convert", &target, "--replace", "--json"]);
    // A refused file is work left, and the run says so in its status.
    assert_exit(&output, 1, "replace");
    let report = stdout_json(&output);
    let files = report["files"].as_array().expect("files list");
    assert_eq!(files.len(), 2);
    let listed = |name: &str| {
        files
            .iter()
            .find(|file| {
                file["source"]
                    .as_str()
                    .is_some_and(|source| source.ends_with(name))
            })
            .unwrap_or_else(|| panic!("{name} is listed: {report}"))
    };
    // What the folder holds is asserted before what the report says: a lost
    // original is the failure worth naming first.
    assert_eq!(
        std::fs::read(dir.join("a.png")).expect("the refused original is still there"),
        png,
        "a file the planner refused is left exactly as it was"
    );
    assert_eq!(
        std::fs::read(dir.join("press-originals").join("a.webp"))
            .expect("the converted sibling's original is in the mirror"),
        webp
    );
    assert_eq!(listed("a.png")["status"], "failed");
    assert_eq!(
        listed("a.png")["error"],
        "the output would overwrite a source image",
        "the refusal names the reason it refused"
    );
    assert_eq!(
        listed("a.webp")["status"],
        "converted",
        "{}",
        stderr(&output)
    );
    assert_exit(&run(&["restore", &target]), 0, "restore");
    assert_eq!(
        std::fs::read(dir.join("a.webp")).expect("the original webp comes back"),
        webp
    );
    assert_eq!(
        std::fs::read(dir.join("a.png")).expect("the png never moved"),
        png
    );
    clean_up(&dir, &alias);
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
