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
/// let a conversion report zero bytes without proving anything moved. The seed
/// picks one image; two seeds are two different photographs, with different
/// source hashes and different converted bytes.
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
            raw.push(((x * 37 + y * 91 + seed * 17) % 251) as u8);
            raw.push(((x * 11 + y * 53 + seed * 29) % 251) as u8);
            raw.push(((x * 7 + y * 13 + seed * 41) % 251) as u8);
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

/// A different photograph under whatever name is wanted.
fn other_photo(dir: &Path, name: &str) -> PathBuf {
    let path = dir.join(name);
    std::fs::write(&path, photo_png_seeded(97)).expect("the second fixture image is written");
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
    // An exact hint is a path match, a basename match stays a candidate, a
    // foreign absolute hint is out of scope, and nothing is unmatched. No
    // verdict says a human confirmed anything, and nothing converted: mapping
    // only reports.
    assert_eq!(
        verdicts,
        vec!["path_match", "candidate", "out_of_scope", "unmatched"]
    );
    assert!(
        !verdicts.contains(&"confirmed"),
        "no automatic verdict advertises itself as a confirmation"
    );
    assert_eq!(doc["summary"]["path_match"], 1);
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

/// A source root plus a second local folder holding a real converted
/// derivative of it. The second folder is what `--deployed` reads: a
/// directory on this machine, never a website.
fn local_check_tree(dir: &Path) -> (PathBuf, PathBuf) {
    let root = dir.join("photos");
    std::fs::create_dir_all(&root).expect("the root is created");
    std::fs::write(root.join("hero.jpg"), photo_png()).expect("hero is written");
    let checked = dir.join("live");
    std::fs::create_dir_all(&checked).expect("the checked dir is created");
    // A real conversion produces the derivative: same stem, AVIF.
    let output = run(&[
        "convert",
        &root.to_string_lossy(),
        "--output",
        &checked.to_string_lossy(),
        "--format",
        "avif",
    ]);
    assert_eq!(output.status.code(), Some(0));
    assert!(checked.join("hero.avif").is_file());
    (root, checked)
}

fn constrained_resource(id: &str, formats: &[&str]) -> serde_json::Value {
    let mut resource = minimal_resource(id, &["hero.jpg"]);
    resource["formats"] = serde_json::json!(formats);
    resource["max_edge"] = serde_json::json!(1600);
    resource["findings"] = serde_json::json!(["excess-dimensions", "missing-alt"]);
    resource
}

#[test]
fn handoff_local_check_reports_local_matches_and_names_gaps() {
    let dir = workdir("local-check");
    let (root, checked) = local_check_tree(&dir);
    let report = mapping_report(
        &dir,
        "report.json",
        serde_json::json!([
            constrained_resource("r1", &["avif"]),
            constrained_resource("r2", &["jpeg"]),
        ]),
    );
    let output = run(&[
        "handoff",
        &report.to_string_lossy(),
        "--root",
        &root.to_string_lossy(),
        "--deployed",
        &checked.to_string_lossy(),
        "--json",
    ]);
    // One derivative sits there meeting its constraints, the other answers to
    // the name in the wrong format: gaps exit 1, like a partial run.
    assert_eq!(output.status.code(), Some(1));
    let doc = stdout_json(&output);
    let statuses: Vec<&str> = doc["local_checks"]
        .as_array()
        .expect("checks list")
        .iter()
        .map(|check| check["status"].as_str().expect("a status"))
        .collect();
    assert_eq!(statuses, vec!["local_match", "differs"]);
    assert_eq!(doc["local_summary"]["local_match"], 1);
    assert_eq!(doc["local_summary"]["differs"], 1);
    assert_eq!(doc["local_root"], checked.to_string_lossy().as_ref());
    // The scope rides along, so no reader has to infer how far a match reaches.
    let scope = doc["local_evidence_scope"]
        .as_str()
        .expect("the scope is stated");
    for boundary in ["local folder", "not a live site", "markup", "re-audit"] {
        assert!(scope.contains(boundary), "{scope}");
    }
    // The finding a person still has to fix stays listed against the match.
    let findings: Vec<&str> = doc["local_checks"][0]["findings"]
        .as_array()
        .expect("findings list")
        .iter()
        .map(|finding| finding.as_str().expect("a finding"))
        .collect();
    assert_eq!(findings, vec!["excess-dimensions", "missing-alt"]);
    assert!(
        doc["local_checks"][0]["paths"][0]
            .as_str()
            .expect("the matched file")
            .ends_with("hero.avif")
    );
    // Nothing in the document claims a deployment, in a status, a summary
    // key or anywhere else.
    let raw = String::from_utf8_lossy(&output.stdout);
    assert!(
        !raw.contains("deploy"),
        "a local directory check must not report deployment: {raw}"
    );
    let _ = std::fs::remove_dir_all(&dir);
}

#[test]
fn handoff_local_check_text_states_a_local_only_scope() {
    let dir = workdir("local-check-clean");
    let (root, checked) = local_check_tree(&dir);
    let report = mapping_report(
        &dir,
        "report.json",
        serde_json::json!([constrained_resource("r1", &["avif"])]),
    );
    let output = run(&[
        "handoff",
        &report.to_string_lossy(),
        "--root",
        &root.to_string_lossy(),
        "--deployed",
        &checked.to_string_lossy(),
    ]);
    assert_eq!(output.status.code(), Some(0));
    let text = String::from_utf8_lossy(&output.stdout);
    assert!(
        text.contains("local file check under"),
        "the heading says what was read: {text}"
    );
    assert!(
        text.contains("scope: filenames, formats and pixel dimensions")
            && text.contains("not a live site"),
        "the heading is followed by its boundary: {text}"
    );
    assert!(
        text.contains("local_match r1"),
        "the checklist names the matched file: {text}"
    );
    assert!(
        text.contains("open findings") && text.contains("missing-alt"),
        "page work stays open beside a match: {text}"
    );
    assert!(
        !text.contains("deploy") && !text.contains("verified against"),
        "no line claims a verified deployment: {text}"
    );
    let _ = std::fs::remove_dir_all(&dir);
}

#[test]
fn handoff_local_check_empty_tree_reports_missing() {
    let dir = workdir("local-check-missing");
    let root = dir.join("photos");
    std::fs::create_dir_all(&root).expect("the root is created");
    std::fs::write(root.join("hero.jpg"), photo_png()).expect("hero is written");
    let empty = dir.join("live");
    std::fs::create_dir_all(&empty).expect("the empty tree is created");
    let report = mapping_report(
        &dir,
        "report.json",
        serde_json::json!([constrained_resource("r1", &["avif"])]),
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
    assert_eq!(doc["local_checks"][0]["status"], "missing");
    let _ = std::fs::remove_dir_all(&dir);
}

#[test]
fn handoff_local_check_covers_every_resource_and_never_passes_a_hole() {
    let dir = workdir("local-check-unmatched");
    let (root, checked) = local_check_tree(&dir);
    // Neither resource is under the source root, so nothing is looked for.
    let mut resources = vec![
        constrained_resource("absent-one", &["avif"]),
        constrained_resource("absent-two", &["avif"]),
    ];
    resources[0]["path_hints"] = serde_json::json!(["nowhere-one.jpg"]);
    resources[1]["path_hints"] = serde_json::json!(["nowhere-two.jpg"]);
    let report = mapping_report(&dir, "report.json", serde_json::json!(resources));
    let output = run(&[
        "handoff",
        &report.to_string_lossy(),
        "--root",
        &root.to_string_lossy(),
        "--deployed",
        &checked.to_string_lossy(),
        "--json",
    ]);
    // Nothing checked is a short checklist, not a clean one.
    assert_eq!(output.status.code(), Some(1));
    let doc = stdout_json(&output);
    let checks = doc["local_checks"].as_array().expect("checks list");
    assert_eq!(checks.len(), 2, "one row per reported resource");
    for check in checks {
        assert_eq!(check["status"], "not_checked");
        assert!(
            check["notes"][0]
                .as_str()
                .expect("a reason")
                .contains("unmatched"),
            "the hole names the verdict that caused it: {check}"
        );
        // The page work an unmatched image still owes stays on its row.
        assert_eq!(
            check["findings"],
            serde_json::json!(["excess-dimensions", "missing-alt"])
        );
    }
    assert_eq!(doc["local_summary"]["not_checked"], 2);
    assert_eq!(doc["local_summary"]["local_match"], 0);
    // The text listing keeps those findings open too.
    let text = run(&[
        "handoff",
        &report.to_string_lossy(),
        "--root",
        &root.to_string_lossy(),
        "--deployed",
        &checked.to_string_lossy(),
    ]);
    assert_eq!(text.status.code(), Some(1));
    let text = String::from_utf8_lossy(&text.stdout).into_owned();
    assert!(
        text.contains("not_checked absent-one") && text.contains("not_checked absent-two"),
        "every resource is listed: {text}"
    );
    assert!(
        text.contains("open findings") && text.contains("missing-alt"),
        "findings survive a resource nothing looked for: {text}"
    );
    let _ = std::fs::remove_dir_all(&dir);
}

#[test]
fn handoff_local_check_mixes_matches_and_holes_without_losing_either() {
    let dir = workdir("local-check-mixed");
    let (root, checked) = local_check_tree(&dir);
    let mut resources = vec![
        constrained_resource("hero", &["avif"]),
        constrained_resource("absent", &["avif"]),
    ];
    resources[1]["path_hints"] = serde_json::json!(["nowhere.jpg"]);
    let report = mapping_report(&dir, "report.json", serde_json::json!(resources));
    let output = run(&[
        "handoff",
        &report.to_string_lossy(),
        "--root",
        &root.to_string_lossy(),
        "--deployed",
        &checked.to_string_lossy(),
        "--json",
    ]);
    // One real match does not carry the resource beside it.
    assert_eq!(output.status.code(), Some(1));
    let doc = stdout_json(&output);
    let statuses: Vec<&str> = doc["local_checks"]
        .as_array()
        .expect("checks list")
        .iter()
        .map(|check| check["status"].as_str().expect("a status"))
        .collect();
    assert_eq!(statuses, vec!["local_match", "not_checked"]);
    assert_eq!(doc["local_summary"]["local_match"], 1);
    assert_eq!(doc["local_summary"]["not_checked"], 1);
    let _ = std::fs::remove_dir_all(&dir);
}

#[test]
fn handoff_reports_what_each_scan_could_not_read_and_fails() {
    let dir = workdir("local-check-unreadable");
    let (root, checked) = local_check_tree(&dir);
    // A file that claims an image extension and would not decode, in each
    // walked root. Neither is the mapped file, so the match still succeeds
    // and only the short walk stands between it and a clean exit.
    std::fs::write(root.join("broken-source.png"), b"not a png at all")
        .expect("the source fixture is written");
    std::fs::write(checked.join("broken-local.png"), b"not a png at all")
        .expect("the local fixture is written");
    let report = mapping_report(
        &dir,
        "report.json",
        serde_json::json!([constrained_resource("hero", &["avif"])]),
    );
    let output = run(&[
        "handoff",
        &report.to_string_lossy(),
        "--root",
        &root.to_string_lossy(),
        "--deployed",
        &checked.to_string_lossy(),
        "--json",
    ]);
    // The resource still matches, and the run still fails: an unread file may
    // be the competing match nobody saw.
    assert_eq!(output.status.code(), Some(1));
    let doc = stdout_json(&output);
    assert_eq!(doc["local_checks"][0]["status"], "local_match");
    let scans = doc["scans"].as_array().expect("one entry per walked root");
    assert_eq!(scans.len(), 2);
    for (scope, expected_root, broken) in [
        ("source_root", &root, "broken-source.png"),
        ("local_check_root", &checked, "broken-local.png"),
    ] {
        let scan = scans
            .iter()
            .find(|scan| scan["scope"] == scope)
            .unwrap_or_else(|| panic!("{scope} is reported"));
        assert_eq!(scan["root"], expected_root.to_string_lossy().as_ref());
        assert_eq!(scan["complete"], false);
        assert_eq!(scan["unreadable_total"], 1);
        assert_eq!(scan["unreadable_omitted"], 0);
        assert_eq!(scan["walk_errors_total"], 0);
        assert!(
            scan["unreadable"][0]
                .as_str()
                .expect("the named file")
                .ends_with(broken),
            "the diagnostic names the file, not a count: {scan}"
        );
    }
    // The producer's own report never acquires this run's filesystem trouble.
    assert!(
        !serde_json::to_string(&doc["pending"])
            .expect("pending serializes")
            .contains("broken-source.png"),
        "pending stays what the producer sent"
    );
    let text = run(&[
        "handoff",
        &report.to_string_lossy(),
        "--root",
        &root.to_string_lossy(),
        "--deployed",
        &checked.to_string_lossy(),
    ]);
    assert_eq!(text.status.code(), Some(1));
    let text = String::from_utf8_lossy(&text.stdout).into_owned();
    assert_eq!(
        text.matches("scan incomplete").count(),
        2,
        "each root reports its own short walk: {text}"
    );
    assert!(
        text.contains("would not decode: ") && text.contains("broken-local.png"),
        "the human text names the unread files: {text}"
    );
    let _ = std::fs::remove_dir_all(&dir);
}

#[test]
fn handoff_mapping_alone_still_passes_but_not_with_a_short_walk() {
    let dir = workdir("mapping-scan");
    let root = dir.join("photos");
    std::fs::create_dir_all(&root).expect("the root is created");
    std::fs::write(root.join("hero.jpg"), photo_png()).expect("hero is written");
    let report = mapping_report(
        &dir,
        "report.json",
        serde_json::json!([minimal_resource("r1", &["hero.jpg"])]),
    );
    // Mapping on its own is a listing, not a checklist: unmatched resources
    // do not fail it and a clean walk exits 0.
    let clean = run(&[
        "handoff",
        &report.to_string_lossy(),
        "--root",
        &root.to_string_lossy(),
        "--json",
    ]);
    assert_eq!(clean.status.code(), Some(0));
    let doc = stdout_json(&clean);
    assert_eq!(doc["scans"].as_array().map(Vec::len), Some(1));
    assert_eq!(doc["scans"][0]["scope"], "source_root");
    assert_eq!(doc["scans"][0]["complete"], true);
    assert!(
        doc["local_checks"].is_null(),
        "no local check was asked for"
    );
    // The same run over a folder the walk could not fully read does fail.
    std::fs::write(root.join("broken.png"), b"not a png at all").expect("the fixture is written");
    let short = run(&[
        "handoff",
        &report.to_string_lossy(),
        "--root",
        &root.to_string_lossy(),
        "--json",
    ]);
    assert_eq!(short.status.code(), Some(1));
    let doc = stdout_json(&short);
    assert_eq!(doc["scans"][0]["complete"], false);
    assert_eq!(doc["scans"][0]["unreadable_total"], 1);
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
    create_saved_plan_with(source, output, plan, &[])
}

fn create_saved_plan_with(source: &str, output: &str, plan: &str, extra: &[&str]) -> Output {
    let mut args: Vec<String> = vec![
        "plan".into(),
        "--root".into(),
        source.into(),
        "--output".into(),
        output.into(),
        "--plan".into(),
        plan.into(),
        "--json".into(),
    ];
    args.extend(extra.iter().map(|argument| (*argument).to_string()));
    run_owned(&args)
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

/// The last line the folder's manifest holds for one destination.
fn latest_record(output: &Path, name: &str) -> serde_json::Value {
    let text = std::fs::read_to_string(output.join(".press-manifest.jsonl"))
        .expect("the output folder has a manifest");
    text.lines()
        .filter_map(|line| serde_json::from_str::<serde_json::Value>(line).ok())
        .rfind(|record| record["output"] == name)
        .unwrap_or_else(|| panic!("the manifest records {name}"))
}

/// A plan reviewed over this source's own existing output replaces it, even
/// though the person chose different settings after seeing it.
///
/// The reviewed destination is pinned by hash when the plan is created, so the
/// consent is to those bytes rather than to the recipe that made them. Refusing
/// the new settings would make "convert this again, better" unexecutable.
#[test]
fn saved_plan_replaces_the_reviewed_output_it_owns_under_new_settings() {
    let (dir, _source_root, output, source, output_root, plan) =
        saved_plan_fixture("saved-owned-resettings");

    // An ordinary conversion first: this source's own output, recorded.
    let converted = run(&[
        "convert",
        &source,
        "--output",
        &output_root,
        "--quality",
        "80",
        "--json",
    ]);
    assert_eq!(converted.status.code(), Some(0), "{}", stderr(&converted));
    let installed = output.join("shot.webp");
    let reviewed_bytes = std::fs::read(&installed).expect("the first output is written");
    assert_eq!(latest_record(&output, "shot.webp")["quality"], "q80");

    let planned = create_saved_plan_with(&source, &output_root, &plan, &["--quality", "40"]);
    assert_eq!(planned.status.code(), Some(0), "{}", stderr(&planned));
    let document = stdout_json(&planned);
    let mapping = &document["plan"]["targets"][0]["mappings"][0];
    assert_eq!(mapping["output"], "shot.webp");
    assert_eq!(
        mapping["destination"]["state"], "own",
        "the plan pins the output it reviewed as this source's own"
    );
    assert!(mapping["destination"]["sha256"].is_string());
    let recipe = document["plan"]["targets"][0]["recipe"]["fingerprint"]
        .as_str()
        .expect("the plan names its recipe")
        .to_string();

    let executed = execute_saved_plan(&plan, &source, &output_root, "--continue-unstarted");
    assert_eq!(executed.status.code(), Some(0), "{}", stderr(&executed));
    let report = stdout_json(&executed);
    assert_eq!(report["status"], "complete");
    assert_eq!(report["items"][0]["status"], "written");
    let record = latest_record(&output, "shot.webp");
    assert_eq!(
        record["quality"], "q40",
        "the chosen settings wrote the file"
    );
    assert_eq!(record["recipe"], recipe.as_str());
    assert_eq!(
        record["output_hash"], report["items"][0]["output_hash"],
        "the receipt describes the file the record claims"
    );
    assert_ne!(
        std::fs::read(&installed).expect("the output is still there"),
        reviewed_bytes,
        "the reviewed output was replaced, not left alone"
    );
    let _ = std::fs::remove_dir_all(dir);
}

/// A newer output that another image produced after the plan was reviewed is
/// not this plan's to overwrite, even under the same recipe and the same name,
/// and even once the source it displaced is restored byte for byte.
#[test]
fn saved_plan_refuses_a_newer_output_written_from_another_source_after_review() {
    let (dir, source_root, output, source, output_root, plan) =
        saved_plan_fixture("saved-newer-output");
    let reviewed_source = std::fs::read(source_root.join("shot.png")).expect("the fixture reads");

    // Reviewed against a free name: nothing stood at the destination.
    let planned = create_saved_plan(&source, &output_root, &plan);
    assert_eq!(planned.status.code(), Some(0), "{}", stderr(&planned));
    assert_eq!(
        stdout_json(&planned)["plan"]["targets"][0]["mappings"][0]["destination"]["state"],
        "absent"
    );

    // Somebody converts a different photograph into that name afterwards, with
    // the settings this plan also uses.
    other_photo(&source_root, "shot.png");
    let converted = run(&["convert", &source, "--output", &output_root, "--json"]);
    assert_eq!(converted.status.code(), Some(0), "{}", stderr(&converted));
    let installed = output.join("shot.webp");
    let newer = std::fs::read(&installed).expect("the newer output is written");

    // The reviewed source comes back exactly as the plan hashed it, so only the
    // destination is in question.
    std::fs::write(source_root.join("shot.png"), &reviewed_source)
        .expect("the reviewed source is restored");

    let executed = execute_saved_plan(&plan, &source, &output_root, "--continue-unstarted");
    assert_eq!(executed.status.code(), Some(1), "{}", stderr(&executed));
    let report = stdout_json(&executed);
    assert_eq!(report["items"][0]["status"], "failed");
    assert!(
        report["items"][0]["error"]
            .as_str()
            .is_some_and(|error| error.contains("output")),
        "{}",
        report["items"][0]["error"]
    );
    assert_eq!(
        std::fs::read(&installed).expect("the newer output is still there"),
        newer,
        "somebody else's newer result is preserved"
    );
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
        .find(|path| {
            let name = path.to_string_lossy();
            name.contains(".run-") && name.ends_with(".json")
        })
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

/// A saved plan over several named sources, so a run has siblings that must
/// survive each other's outcomes.
fn saved_plan_set(
    tag: &str,
    names: &[&str],
) -> (PathBuf, PathBuf, PathBuf, String, String, String) {
    let dir = workdir(tag);
    let source = dir.join("source");
    let output = dir.join("output");
    std::fs::create_dir_all(&source).expect("the source root is created");
    std::fs::create_dir_all(&output).expect("the output root is created");
    for name in names {
        photo(&source, name);
    }
    let plan = dir.join("saved-plan.json");
    let source_text = source.to_string_lossy().into_owned();
    let output_text = output.to_string_lossy().into_owned();
    let plan_text = plan.to_string_lossy().into_owned();
    (dir, source, output, source_text, output_text, plan_text)
}

fn item_status(report: &serde_json::Value, source: &str) -> String {
    report["items"]
        .as_array()
        .expect("the report lists items")
        .iter()
        .find(|item| item["source"] == source)
        .unwrap_or_else(|| panic!("the report has an item for {source}"))["status"]
        .as_str()
        .expect("an item status is a string")
        .to_string()
}

fn modified_at(path: &Path) -> std::time::SystemTime {
    std::fs::metadata(path)
        .expect("the output exists")
        .modified()
        .expect("the output has an mtime")
}

/// A failed item is retried on its own. Its successful sibling is neither
/// re-encoded nor disturbed, and the retry is a separate explicit mode from
/// continuing unstarted work.
#[test]
fn saved_plan_retries_only_the_failed_sibling_and_leaves_the_written_one_alone() {
    let (dir, source_root, output, source, output_root, plan) =
        saved_plan_set("saved-retry-sibling", &["kept.png", "broken.png"]);
    let planned = create_saved_plan(&source, &output_root, &plan);
    assert_eq!(planned.status.code(), Some(0), "{}", stderr(&planned));

    // A bounded, reversible failure: the source is away when the run reaches
    // it, and comes back byte for byte before the retry.
    let broken = source_root.join("broken.png");
    let bytes = std::fs::read(&broken).expect("the fixture is readable");
    std::fs::remove_file(&broken).expect("the source is taken away");

    let first = execute_saved_plan(&plan, &source, &output_root, "--continue-unstarted");
    assert_eq!(first.status.code(), Some(1), "{}", stderr(&first));
    let report = stdout_json(&first);
    assert_eq!(report["status"], "partial");
    assert_eq!(report["counts"]["written"], 1);
    assert_eq!(report["counts"]["failed"], 1);
    assert_eq!(item_status(&report, "kept.png"), "written");
    assert_eq!(item_status(&report, "broken.png"), "failed");
    let kept = output.join("kept.webp");
    let kept_bytes = std::fs::read(&kept).expect("the successful sibling is written");
    let kept_modified = modified_at(&kept);
    assert!(!output.join("broken.webp").exists());

    // Continuing unstarted work is not retrying a failure.
    let continued = execute_saved_plan(&plan, &source, &output_root, "--continue-unstarted");
    assert_eq!(continued.status.code(), Some(1), "{}", stderr(&continued));
    let continued = stdout_json(&continued);
    assert_eq!(continued["counts"]["failed"], 1);
    assert_eq!(continued["counts"]["written"], 1);
    assert!(!output.join("broken.webp").exists());

    std::fs::write(&broken, &bytes).expect("the source comes back unchanged");
    let retried = execute_saved_plan(&plan, &source, &output_root, "--retry-failed");
    assert_eq!(retried.status.code(), Some(0), "{}", stderr(&retried));
    let retried = stdout_json(&retried);
    assert_eq!(retried["status"], "complete");
    assert_eq!(retried["counts"]["written"], 2);
    assert_eq!(retried["counts"]["failed"], 0);
    assert!(output.join("broken.webp").is_file());
    assert_eq!(
        std::fs::read(&kept).expect("the sibling is still there"),
        kept_bytes,
        "a retry does not rewrite a successful sibling"
    );
    assert_eq!(
        modified_at(&kept),
        kept_modified,
        "a retry does not re-encode a successful sibling"
    );
    let _ = std::fs::remove_dir_all(dir);
}

/// Two targets execute into their own namespaces, and cancelling records the
/// pending work as cancelled without encoding it or reviving it later.
#[test]
fn saved_plan_executes_and_cancels_every_target_namespace() {
    let dir = workdir("saved-multi-target");
    let root = dir.join("images");
    let out = dir.join("output");
    let recipes = config_root(&dir).join("imageguide/recipes");
    std::fs::create_dir_all(&root).expect("the image root is created");
    std::fs::create_dir_all(&out).expect("the output root is created");
    std::fs::create_dir_all(&recipes).expect("the recipe folder is created");
    photo(&root, "one.png");
    photo(&root, "two.png");
    write_target_recipe(&recipes, "fast", "10");
    write_target_recipe(&recipes, "slow", "2");
    let source = root.to_string_lossy().into_owned();
    let output = out.to_string_lossy().into_owned();
    let plan = dir.join("plan.json").to_string_lossy().into_owned();
    let cancelled_plan = dir.join("cancelled.json").to_string_lossy().into_owned();

    let arguments = |plan: &str, output: &str| {
        vec![
            "plan".to_string(),
            "--root".into(),
            source.clone(),
            "--output".into(),
            output.to_string(),
            "--plan".into(),
            plan.to_string(),
            "--target".into(),
            "fast=hero".into(),
            "--target".into(),
            "slow=thumb".into(),
            "--json".into(),
        ]
    };
    let planned = run_owned_in(&dir, &arguments(&plan, &output));
    assert_eq!(planned.status.code(), Some(0), "{}", stderr(&planned));
    let document = stdout_json(&planned);
    assert_eq!(document["plan"]["targets"].as_array().unwrap().len(), 2);

    let executed = execute_saved_plan_in(&dir, &plan, &source, &output, "--continue-unstarted");
    assert_eq!(executed.status.code(), Some(0), "{}", stderr(&executed));
    let report = stdout_json(&executed);
    assert_eq!(report["status"], "complete");
    assert_eq!(report["counts"]["written"], 4);
    for name in [
        "hero/one.avif",
        "hero/two.avif",
        "thumb/one.avif",
        "thumb/two.avif",
    ] {
        assert!(out.join(name).is_file(), "{name} is written");
    }
    assert_eq!(
        target_manifest_speed(&out.join("hero/.press-manifest.jsonl")),
        Some(10)
    );
    assert_eq!(
        target_manifest_speed(&out.join("thumb/.press-manifest.jsonl")),
        Some(2)
    );

    // A second plan into its own output root, cancelled before any encode.
    let pending = dir.join("pending");
    std::fs::create_dir_all(&pending).expect("the second output root is created");
    let pending_text = pending.to_string_lossy().into_owned();
    let planned = run_owned_in(&dir, &arguments(&cancelled_plan, &pending_text));
    assert_eq!(planned.status.code(), Some(0), "{}", stderr(&planned));
    let cancelled =
        execute_saved_plan_in(&dir, &cancelled_plan, &source, &pending_text, "--cancel");
    assert_eq!(cancelled.status.code(), Some(1), "{}", stderr(&cancelled));
    let report = stdout_json(&cancelled);
    assert_eq!(report["status"], "partial");
    assert_eq!(report["counts"]["cancelled"], 4);
    assert_eq!(report["counts"]["written"], 0);
    assert_eq!(report["counts"]["failed"], 0);

    assert!(
        !pending.join("hero").exists() && !pending.join("thumb").exists(),
        "cancellation encodes nothing"
    );

    let resumed = execute_saved_plan_in(
        &dir,
        &cancelled_plan,
        &source,
        &pending_text,
        "--continue-unstarted",
    );
    assert_eq!(resumed.status.code(), Some(1), "{}", stderr(&resumed));
    let report = stdout_json(&resumed);
    assert_eq!(
        report["counts"]["cancelled"], 4,
        "continuing unstarted work does not revive cancelled work"
    );
    assert_eq!(report["counts"]["written"], 0);
    assert!(!pending.join("hero").exists());
    let _ = std::fs::remove_dir_all(dir);
}

fn run_owned_in(home: &Path, args: &[String]) -> Output {
    Command::new(env!("CARGO_BIN_EXE_press"))
        .args(args)
        .env("HOME", home)
        .env("XDG_CONFIG_HOME", home)
        .env("APPDATA", home)
        .output()
        .expect("the binary runs")
}

fn execute_saved_plan_in(
    home: &Path,
    plan: &str,
    source: &str,
    output: &str,
    mode: &str,
) -> Output {
    run_owned_in(
        home,
        &[
            "execute".into(),
            plan.into(),
            "--root".into(),
            source.into(),
            "--output".into(),
            output.into(),
            mode.into(),
            "--json".into(),
        ],
    )
}

/// A source root reached through a link and its parent is the folder the kernel
/// opens, not the one whose name precedes the link in the text.
///
/// `typed/link` points at `actual/sub`, so `typed/link/..` is `actual`. Both
/// `actual` and `typed` hold a `photo.png`, and they are different photographs,
/// so a plan that folded the `..` against the text instead would hash, convert
/// and report the decoy while claiming to describe what the person named.
#[cfg(unix)]
#[test]
fn saved_plan_binds_a_source_root_reached_through_a_link_and_its_parent() {
    let dir = workdir("saved-link-parent");
    let actual = dir.join("actual");
    let typed = dir.join("typed");
    let output_dir = dir.join("out");
    for folder in [&actual, &actual.join("sub"), &typed, &output_dir] {
        std::fs::create_dir_all(folder).expect("the fixture folder is created");
    }
    other_photo(&actual, "photo.png");
    let decoy = photo(&typed, "photo.png");
    let decoy_bytes = std::fs::read(&decoy).expect("the decoy is readable");
    std::os::unix::fs::symlink(actual.join("sub"), typed.join("link"))
        .expect("the link is created");

    let through_link = typed.join("link").join("..").to_string_lossy().into_owned();
    let output = output_dir.to_string_lossy().into_owned();
    let plan = dir.join("through-link.json").to_string_lossy().into_owned();
    let planned = create_saved_plan(&through_link, &output, &plan);
    assert_eq!(planned.status.code(), Some(0), "{}", stderr(&planned));
    let bound = stdout_json(&planned)["plan"]["sources"][0]["sha256"]
        .as_str()
        .expect("the plan hashes its source")
        .to_string();

    // The same question asked of each folder directly, so the fixture proves
    // which of the two the binding actually opened.
    let direct = dir.join("direct.json").to_string_lossy().into_owned();
    let direct = create_saved_plan(
        &actual.to_string_lossy(),
        &output_dir.join("direct").to_string_lossy(),
        &direct,
    );
    assert_eq!(direct.status.code(), Some(0), "{}", stderr(&direct));
    let named = dir.join("named.json").to_string_lossy().into_owned();
    let named = create_saved_plan(
        &typed.to_string_lossy(),
        &output_dir.join("named").to_string_lossy(),
        &named,
    );
    assert_eq!(named.status.code(), Some(0), "{}", stderr(&named));
    assert_eq!(
        bound,
        stdout_json(&direct)["plan"]["sources"][0]["sha256"]
            .as_str()
            .expect("the direct plan hashes its source"),
        "the link's parent is the folder it points into"
    );
    assert_ne!(
        bound,
        stdout_json(&named)["plan"]["sources"][0]["sha256"]
            .as_str()
            .expect("the named plan hashes its source"),
        "the fixture's two photographs are different files"
    );

    let executed = execute_saved_plan(&plan, &through_link, &output, "--continue-unstarted");
    assert_eq!(executed.status.code(), Some(0), "{}", stderr(&executed));
    let report = stdout_json(&executed);
    assert_eq!(report["status"], "complete");
    assert_eq!(report["items"][0]["status"], "written");
    assert!(output_dir.join("photo.webp").is_file());
    assert_eq!(
        std::fs::read(&decoy).expect("the decoy is still there"),
        decoy_bytes,
        "the folder the person did not bind is untouched"
    );
    let _ = std::fs::remove_dir_all(dir);
}

/// The run state a binding leaves behind, once it exists.
///
/// Only the killed-retry tests read it, and those need a FIFO to stop a process
/// inside its source read, so this reads as dead code everywhere else.
#[cfg(unix)]
fn run_state_path(dir: &Path) -> Option<PathBuf> {
    std::fs::read_dir(dir)
        .expect("the plan folder is readable")
        .filter_map(Result::ok)
        .map(|entry| entry.path())
        .find(|path| {
            let name = path.to_string_lossy();
            name.contains(".run-") && name.ends_with(".json")
        })
}

#[cfg(unix)]
fn run_state(path: &Path) -> serde_json::Value {
    serde_json::from_slice(&std::fs::read(path).expect("the run state is readable"))
        .expect("the run state is JSON")
}

/// Every state a retry passes through has to load again, including the one a
/// killed process leaves on disk.
///
/// A written item that loses its output becomes a named failure, and the retry
/// after it becomes running work. Neither still describes an installed file, so
/// neither may keep the receipt that did — a state that kept it was refused by
/// the next command, and the plan could not be resumed at all.
///
/// The interruption is real and deterministic: the source is a FIFO, so the
/// process blocks in its bounded source read after it has recorded the item as
/// running, and the test kills it there.
#[cfg(unix)]
#[test]
fn saved_plan_reloads_the_state_a_killed_retry_left_behind() {
    use std::io::Read as _;

    let (dir, source_root, output, source, output_root, plan) =
        saved_plan_fixture("saved-killed-retry");
    let planned = create_saved_plan(&source, &output_root, &plan);
    assert_eq!(planned.status.code(), Some(0), "{}", stderr(&planned));
    let executed = execute_saved_plan(&plan, &source, &output_root, "--continue-unstarted");
    assert_eq!(executed.status.code(), Some(0), "{}", stderr(&executed));
    let installed = output.join("shot.webp");
    assert!(installed.is_file());

    // The output goes away, so the receipt that vouched for it is not true any
    // more and the item is a named failure.
    std::fs::remove_file(&installed).expect("the installed output is taken away");
    let reconciled = reconcile_saved_plan(&plan, &source, &output_root);
    assert_eq!(reconciled.status.code(), Some(1), "{}", stderr(&reconciled));
    assert_eq!(stdout_json(&reconciled)["items"][0]["status"], "failed");
    let state_path = run_state_path(&dir).expect("execution leaves a state file");
    let failed = run_state(&state_path);
    assert!(
        failed["items"][0]["output_hash"].is_null() && failed["items"][0]["output_bytes"].is_null(),
        "a failure keeps no receipt for a file that is gone: {}",
        failed["items"][0]
    );
    assert!(
        failed["items"][0]["error"].is_string(),
        "the failure is still named"
    );

    let bytes = std::fs::read(source_root.join("shot.png")).expect("the fixture is readable");
    std::fs::remove_file(source_root.join("shot.png")).expect("the source is replaced");
    let made = Command::new("mkfifo")
        .arg(source_root.join("shot.png"))
        .status()
        .expect("mkfifo runs");
    assert!(made.success(), "the blocking source is a fifo");

    let mut child = Command::new(env!("CARGO_BIN_EXE_press"))
        .args([
            "execute",
            &plan,
            "--root",
            &source,
            "--output",
            &output_root,
            "--retry-failed",
            "--json",
        ])
        .stdout(std::process::Stdio::piped())
        .spawn()
        .expect("the retry starts");

    let deadline = std::time::Instant::now() + std::time::Duration::from_secs(60);
    let mut running = None;
    while std::time::Instant::now() < deadline {
        let state = run_state(&state_path);
        if state["items"][0]["status"] == "running" {
            running = Some(state);
            break;
        }
        std::thread::sleep(std::time::Duration::from_millis(50));
    }
    let running = running.unwrap_or_else(|| {
        let _ = child.kill();
        let _ = std::fs::remove_file(source_root.join("shot.png"));
        panic!("the retry never reached the blocking source");
    });
    assert!(
        running["items"][0]["output_hash"].is_null()
            && running["items"][0]["output_bytes"].is_null()
            && running["items"][0]["width"].is_null()
            && running["items"][0]["height"].is_null(),
        "work in progress carries no receipt: {}",
        running["items"][0]
    );

    child.kill().expect("the blocked retry is killed");
    let mut ignored = Vec::new();
    if let Some(mut stdout) = child.stdout.take() {
        let _ = stdout.read_to_end(&mut ignored);
    }
    let status = child.wait().expect("the killed retry is reaped");
    assert!(status.code().is_none(), "the run died rather than exited");

    std::fs::remove_file(source_root.join("shot.png")).expect("the fifo is removed");
    std::fs::write(source_root.join("shot.png"), &bytes).expect("the source comes back unchanged");

    // The plan is still usable: the state loads, says what is true, and the
    // work finishes.
    let reconciled = reconcile_saved_plan(&plan, &source, &output_root);
    assert_eq!(reconciled.status.code(), Some(1), "{}", stderr(&reconciled));
    let report = stdout_json(&reconciled);
    assert!(
        report["error"].is_null(),
        "the killed retry left a state this command reads: {}",
        report["error"]
    );
    assert_eq!(report["items"][0]["status"], "unstarted");
    assert_eq!(report["counts"]["unstarted"], 1);

    let finished = execute_saved_plan(&plan, &source, &output_root, "--continue-unstarted");
    assert_eq!(finished.status.code(), Some(0), "{}", stderr(&finished));
    let finished = stdout_json(&finished);
    assert_eq!(finished["status"], "complete");
    assert_eq!(finished["items"][0]["status"], "written");
    assert!(installed.is_file());
    let _ = std::fs::remove_dir_all(dir);
}

/// A run killed part-way through leaves its finished work identifiable, its
/// interrupted item recoverable and its untouched work unstarted.
///
/// The interruption is deterministic rather than timed: the third source is a
/// FIFO, so the real process blocks in its bounded source read at exactly one
/// place, after it has written the two siblings and recorded the third as
/// running. The test waits for that recorded state before killing it.
#[cfg(unix)]
#[test]
fn saved_plan_resumes_unstarted_work_after_a_killed_process() {
    use std::io::Read as _;

    let (dir, source_root, output, source, output_root, plan) = saved_plan_set(
        "saved-interrupted",
        &["a-first.png", "b-second.png", "c-blocks.png"],
    );
    let planned = create_saved_plan(&source, &output_root, &plan);
    assert_eq!(planned.status.code(), Some(0), "{}", stderr(&planned));

    let blocking = source_root.join("c-blocks.png");
    let bytes = std::fs::read(&blocking).expect("the third fixture is readable");
    std::fs::remove_file(&blocking).expect("the third source is replaced");
    let made = Command::new("mkfifo")
        .arg(&blocking)
        .status()
        .expect("mkfifo runs");
    assert!(made.success(), "the blocking source is a fifo");

    let mut child = Command::new(env!("CARGO_BIN_EXE_press"))
        .args([
            "execute",
            &plan,
            "--root",
            &source,
            "--output",
            &output_root,
            "--continue-unstarted",
            "--json",
        ])
        .stdout(std::process::Stdio::piped())
        .spawn()
        .expect("the run starts");

    let deadline = std::time::Instant::now() + std::time::Duration::from_secs(60);
    let mut state_path = None;
    while std::time::Instant::now() < deadline && state_path.is_none() {
        state_path = std::fs::read_dir(&dir)
            .expect("the plan folder is readable")
            .filter_map(Result::ok)
            .map(|entry| entry.path())
            .find(|path| {
                let name = path.to_string_lossy();
                name.contains(".run-") && name.ends_with(".json")
            });
        if state_path.is_none() {
            std::thread::sleep(std::time::Duration::from_millis(50));
        }
    }
    let state_path = state_path.unwrap_or_else(|| {
        let _ = child.kill();
        let _ = std::fs::remove_file(&blocking);
        panic!("execution never created a binding-specific state file");
    });

    let mut reached = None;
    while std::time::Instant::now() < deadline {
        if let Ok(bytes) = std::fs::read(&state_path)
            && let Ok(state) = serde_json::from_slice::<serde_json::Value>(&bytes)
            && item_status(&state, "c-blocks.png") == "running"
        {
            reached = Some(state);
            break;
        }
        std::thread::sleep(std::time::Duration::from_millis(50));
    }
    let running = reached.unwrap_or_else(|| {
        let _ = child.kill();
        let _ = std::fs::remove_file(&blocking);
        panic!("the run never reached the blocking source");
    });
    assert_eq!(item_status(&running, "a-first.png"), "written");
    assert_eq!(item_status(&running, "b-second.png"), "written");

    // One lock covers the whole command, so a second one refuses rather than
    // reading a state this process is still writing.
    let contended = reconcile_saved_plan(&plan, &source, &output_root);
    assert_eq!(contended.status.code(), Some(2), "{}", stderr(&contended));
    assert!(
        stdout_json(&contended)["error"]
            .as_str()
            .is_some_and(|error| error.contains("another Press command")),
        "{}",
        String::from_utf8_lossy(&contended.stdout)
    );

    child.kill().expect("the blocked run is killed");
    let mut ignored = Vec::new();
    if let Some(mut stdout) = child.stdout.take() {
        let _ = stdout.read_to_end(&mut ignored);
    }
    let status = child.wait().expect("the killed run is reaped");
    assert!(status.code().is_none(), "the run died rather than exited");

    let first = output.join("a-first.webp");
    let second = output.join("b-second.webp");
    let first_modified = modified_at(&first);
    let second_modified = modified_at(&second);
    assert!(!output.join("c-blocks.webp").exists());

    std::fs::remove_file(&blocking).expect("the fifo is removed");
    std::fs::write(&blocking, &bytes).expect("the third source comes back unchanged");

    // Reconcile decides what really happened from the folder, not the record.
    let reconciled = reconcile_saved_plan(&plan, &source, &output_root);
    assert_eq!(reconciled.status.code(), Some(1), "{}", stderr(&reconciled));
    let report = stdout_json(&reconciled);
    assert_eq!(report["counts"]["written"], 2);
    assert_eq!(report["counts"]["failed"], 0);
    assert_eq!(report["counts"]["unstarted"], 1);
    assert_eq!(item_status(&report, "c-blocks.png"), "unstarted");

    let resumed = execute_saved_plan(&plan, &source, &output_root, "--continue-unstarted");
    assert_eq!(resumed.status.code(), Some(0), "{}", stderr(&resumed));
    let report = stdout_json(&resumed);
    assert_eq!(report["status"], "complete");
    assert_eq!(report["counts"]["written"], 3);
    assert!(output.join("c-blocks.webp").is_file());
    assert_eq!(
        modified_at(&first),
        first_modified,
        "finished work is verified, not encoded again"
    );
    assert_eq!(modified_at(&second), second_modified);
    let _ = std::fs::remove_dir_all(dir);
}

/// A matching hash is not being the file that was reviewed: a link that leaves
/// the bound source root is refused before its bytes are consumed.
#[test]
fn saved_plan_refuses_a_matching_source_reached_outside_its_bound_root() {
    let (dir, source_root, output, source, output_root, plan) =
        saved_plan_fixture("saved-source-escape");
    let planned = create_saved_plan(&source, &output_root, &plan);
    assert_eq!(planned.status.code(), Some(0), "{}", stderr(&planned));

    let elsewhere = dir.join("elsewhere");
    std::fs::create_dir_all(&elsewhere).expect("the outside folder is created");
    let inside = source_root.join("shot.png");
    std::fs::rename(&inside, elsewhere.join("shot.png")).expect("the source moves outside");
    #[cfg(unix)]
    std::os::unix::fs::symlink(elsewhere.join("shot.png"), &inside)
        .expect("the link takes its place");
    #[cfg(windows)]
    std::os::windows::fs::symlink_file(elsewhere.join("shot.png"), &inside)
        .expect("the link takes its place");

    let executed = execute_saved_plan(&plan, &source, &output_root, "--continue-unstarted");
    assert_eq!(executed.status.code(), Some(1), "{}", stderr(&executed));
    let report = stdout_json(&executed);
    assert_eq!(report["items"][0]["status"], "failed");
    assert!(
        report["items"][0]["error"]
            .as_str()
            .is_some_and(|error| error.contains("outside the explicit source root")),
        "{}",
        report["items"][0]["error"]
    );
    assert!(!output.join("shot.webp").exists());
    let _ = std::fs::remove_dir_all(dir);
}

/// Saved-plan execution does not inherit ordinary conversion's permission to
/// overwrite an unrecorded output because it is older than its source.
#[test]
fn saved_plan_refuses_a_backdated_unmanaged_output_that_conversion_still_takes() {
    let (dir, _source_root, output, source, output_root, plan) =
        saved_plan_fixture("saved-backdated");
    let planned = create_saved_plan(&source, &output_root, &plan);
    assert_eq!(planned.status.code(), Some(0), "{}", stderr(&planned));

    let destination = output.join("shot.webp");
    let foreign = b"a file the plan never accounted for".to_vec();
    std::fs::write(&destination, &foreign).expect("the foreign output is written");
    let backdated = std::time::SystemTime::UNIX_EPOCH + std::time::Duration::from_secs(1_000_000);
    std::fs::File::options()
        .write(true)
        .open(&destination)
        .expect("the foreign output opens")
        .set_modified(backdated)
        .expect("the foreign output is backdated");

    let executed = execute_saved_plan(&plan, &source, &output_root, "--continue-unstarted");
    assert_eq!(executed.status.code(), Some(1), "{}", stderr(&executed));
    let report = stdout_json(&executed);
    assert_eq!(report["items"][0]["status"], "failed");
    assert_eq!(
        std::fs::read(&destination).expect("the foreign output survives"),
        foreign,
        "a saved plan preserves a file it does not own"
    );

    // The same backdated, unrecorded file is still ordinary conversion's to
    // take: that compatibility is unchanged.
    let converted = run(&["convert", &source, "--output", &output_root, "--json"]);
    assert_eq!(converted.status.code(), Some(0), "{}", stderr(&converted));
    assert_eq!(stdout_json(&converted)["summary"]["converted"], 1);
    assert_ne!(
        std::fs::read(&destination).expect("the converted output is readable"),
        foreign
    );
    let _ = std::fs::remove_dir_all(dir);
}

fn requirements_file(dir: &Path, name: &str, allowed: &str, revision: u32) -> String {
    let path = dir.join(name);
    std::fs::write(
        &path,
        format!(
            r#"{{"schema":1,"id":"plan-rules","name":"Plan rules","revision":{revision},
"provenance":{{"local_author":{{"author":"test"}}}},"target":"test","role":"fixture",
"category":null,"region":null,"last_verified":"2026-09-08","effective_from":null,
"effective_until":null,"engine":"press","engine_version":1,"checker_version":1,
"rules":[{{"id":"format","label":"Delivery format","required":true,
"constraint":{{"kind":"format","allowed":["{allowed}"]}}}},
{{"id":"bytes","label":"Non-empty output","required":true,
"constraint":{{"kind":"bytes","min":1,"max":null}}}}]}}"#
        ),
    )
    .expect("the requirements snapshot is written");
    path.to_string_lossy().into_owned()
}

fn plan_with_requirements(source: &str, output: &str, plan: &str, rules: &str) -> Output {
    run_owned(&[
        "plan".into(),
        "--root".into(),
        source.into(),
        "--output".into(),
        output.into(),
        "--plan".into(),
        plan.into(),
        "--requirements-file".into(),
        rules.into(),
        "--json".into(),
    ])
}

fn execute_with_requirements(
    plan: &str,
    source: &str,
    output: &str,
    mode: &str,
    rules: Option<&str>,
) -> Output {
    let mut args = vec![
        "execute".to_string(),
        plan.into(),
        "--root".into(),
        source.into(),
        "--output".into(),
        output.into(),
        mode.into(),
        "--json".into(),
    ];
    if let Some(rules) = rules {
        args.push("--requirements-file".into());
        args.push(rules.into());
    }
    run_owned(&args)
}

/// A written output whose required checks did not pass is a named non-success,
/// and re-running does not encode it again to try for a different answer.
#[test]
fn saved_plan_reports_written_outputs_that_miss_their_requirements() {
    let (dir, _source_root, output, source, output_root, plan) =
        saved_plan_fixture("saved-requirements");
    let rules = requirements_file(&dir, "rules.json", "avif", 1);
    let planned = plan_with_requirements(&source, &output_root, &plan, &rules);
    assert_eq!(planned.status.code(), Some(0), "{}", stderr(&planned));
    let document = stdout_json(&planned);
    assert_eq!(document["plan"]["requirements"]["id"], "plan-rules");
    assert_eq!(
        document["plan"]["requirements"]["provenance"]["local_author"]["author"],
        "test"
    );

    // The plan was reviewed against rules, so execution has to supply them.
    let bare =
        execute_with_requirements(&plan, &source, &output_root, "--continue-unstarted", None);
    assert_eq!(bare.status.code(), Some(2), "{}", stderr(&bare));
    assert!(
        stdout_json(&bare)["error"]
            .as_str()
            .is_some_and(|error| error.contains("--requirements-file"))
    );
    assert!(
        !output.join("shot.webp").exists(),
        "a refusal writes nothing"
    );

    let executed = execute_with_requirements(
        &plan,
        &source,
        &output_root,
        "--continue-unstarted",
        Some(&rules),
    );
    assert_eq!(executed.status.code(), Some(1), "{}", stderr(&executed));
    let report = stdout_json(&executed);
    assert_eq!(report["status"], "partial");
    assert_eq!(report["counts"]["written"], 1);
    assert_eq!(report["counts"]["failed"], 0);
    assert_eq!(report["counts"]["requirements_failed"], 1);
    let outcome = &report["items"][0]["requirements"];
    assert_eq!(outcome["all_required_pass"], false);
    assert_eq!(outcome["effective"], true);
    assert!(
        outcome["checks"]
            .as_array()
            .expect("the outcome lists checks")
            .iter()
            .any(|check| check["id"] == "format" && check["status"] == "fail")
    );
    let written = output.join("shot.webp");
    let modified = modified_at(&written);

    // Rules that are not the reviewed document cannot be swapped in.
    let other = requirements_file(&dir, "other.json", "webp", 1);
    let swapped = execute_with_requirements(
        &plan,
        &source,
        &output_root,
        "--continue-unstarted",
        Some(&other),
    );
    assert_eq!(swapped.status.code(), Some(2), "{}", stderr(&swapped));
    assert!(
        stdout_json(&swapped)["error"]
            .as_str()
            .is_some_and(|error| error.contains("reviewed against"))
    );

    // Re-running answers from the installed file; it does not encode again.
    let again = execute_with_requirements(
        &plan,
        &source,
        &output_root,
        "--continue-unstarted",
        Some(&rules),
    );
    assert_eq!(again.status.code(), Some(1), "{}", stderr(&again));
    assert_eq!(stdout_json(&again)["counts"]["requirements_failed"], 1);
    assert_eq!(
        modified_at(&written),
        modified,
        "a missed requirement is not a reason to encode again"
    );
    let _ = std::fs::remove_dir_all(dir);
}

/// The same lifecycle when the rules do apply: the receipt records the pass and
/// the run is complete.
#[test]
fn saved_plan_records_a_passing_requirements_receipt() {
    let (dir, _source_root, _output, source, output_root, plan) =
        saved_plan_fixture("saved-requirements-pass");
    let rules = requirements_file(&dir, "rules.json", "webp", 4);
    let planned = plan_with_requirements(&source, &output_root, &plan, &rules);
    assert_eq!(planned.status.code(), Some(0), "{}", stderr(&planned));

    let executed = execute_with_requirements(
        &plan,
        &source,
        &output_root,
        "--continue-unstarted",
        Some(&rules),
    );
    assert_eq!(executed.status.code(), Some(0), "{}", stderr(&executed));
    let report = stdout_json(&executed);
    assert_eq!(report["status"], "complete");
    assert_eq!(report["counts"]["requirements_failed"], 0);
    assert_eq!(report["requirements"]["revision"], 4);
    assert_eq!(
        report["items"][0]["requirements"]["all_required_pass"],
        true
    );
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

/// A plan is data, not a machine's layout. The same document binds to a second
/// pair of roots holding the same bytes and writes there. What it must not do is
/// take a name it never produced, so the third layout plants somebody else's
/// file under the planned output and the run has to leave it exactly as it is.
#[test]
fn a_saved_plan_binds_to_another_layout_and_still_refuses_a_stranger_file() {
    let (dir, source_dir, _output, source, output_root, plan) =
        saved_plan_fixture("saved-portable");
    let planned = create_saved_plan(&source, &output_root, &plan);
    assert_eq!(planned.status.code(), Some(0), "{}", stderr(&planned));

    let second_source = dir.join("second-source");
    let second_output = dir.join("second-output");
    std::fs::create_dir_all(&second_source).expect("the second source root is created");
    std::fs::create_dir_all(&second_output).expect("the second output root is created");
    std::fs::copy(source_dir.join("shot.png"), second_source.join("shot.png"))
        .expect("the same bytes land in the second layout");
    let executed = execute_saved_plan(
        &plan,
        &second_source.to_string_lossy(),
        &second_output.to_string_lossy(),
        "--continue-unstarted",
    );
    assert_eq!(executed.status.code(), Some(0), "{}", stderr(&executed));
    let receipt = stdout_json(&executed);
    assert_eq!(receipt["counts"]["written"], 1);
    assert!(
        second_output.join("shot.webp").exists(),
        "the plan converted under a layout it was never created against"
    );

    let third_source = dir.join("third-source");
    let third_output = dir.join("third-output");
    std::fs::create_dir_all(&third_source).expect("the third source root is created");
    std::fs::create_dir_all(&third_output).expect("the third output root is created");
    std::fs::copy(source_dir.join("shot.png"), third_source.join("shot.png"))
        .expect("the same bytes land in the third layout");
    let stranger = third_output.join("shot.webp");
    std::fs::write(&stranger, b"somebody else's file").expect("the stranger file is planted");
    let refused = execute_saved_plan(
        &plan,
        &third_source.to_string_lossy(),
        &third_output.to_string_lossy(),
        "--continue-unstarted",
    );
    assert_eq!(refused.status.code(), Some(1), "{}", stderr(&refused));
    let receipt = stdout_json(&refused);
    assert_eq!(receipt["counts"]["written"], 0);
    assert_eq!(receipt["counts"]["failed"], 1);
    assert_eq!(
        std::fs::read(&stranger).expect("the stranger file is still there"),
        b"somebody else's file",
        "a plan that pinned an absence never took this name"
    );
    let _ = std::fs::remove_dir_all(&dir);
}
