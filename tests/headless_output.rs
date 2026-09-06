//! A destination that never establishes is fatal in every surface: the text run
//! names the refusal and the JSON run carries it with a null output. Both exit
//! nonzero, even when the scan found nothing to refuse.

use std::path::PathBuf;
use std::process::Command;
use std::sync::atomic::{AtomicU64, Ordering};

static FIXTURES: AtomicU64 = AtomicU64::new(0);

/// An empty audited folder whose default output name is taken by a regular
/// file, so context establishment fails with zero targets.
fn refused_fixture() -> PathBuf {
    let id = FIXTURES.fetch_add(1, Ordering::Relaxed);
    let dir = std::env::temp_dir().join(format!(
        "press-headless-refused-{}-{id}",
        std::process::id()
    ));
    let _ = std::fs::remove_dir_all(&dir);
    std::fs::create_dir_all(&dir).unwrap();
    std::fs::write(dir.join("optimized"), b"not a directory").unwrap();
    dir
}

fn press() -> Command {
    Command::new(env!("CARGO_BIN_EXE_press"))
}

/// Text names the refusal, writes nothing, and exits nonzero.
#[test]
fn invalid_empty_output_context_is_fatal_in_text() {
    let dir = refused_fixture();
    let output = press().arg("convert").arg(&dir).output().unwrap();
    assert!(!output.status.success(), "a refused destination fails");
    let stdout = String::from_utf8_lossy(&output.stdout);
    let stderr = String::from_utf8_lossy(&output.stderr);
    assert!(
        !stdout.contains("written to"),
        "nothing was written, so no destination is announced:\n{stdout}"
    );
    assert!(
        stderr.contains("press:"),
        "the refusal names its reason on stderr:\n{stderr}"
    );
    assert!(
        !dir.join("optimized").is_dir(),
        "the refusal creates no output folder"
    );
    std::fs::remove_dir_all(&dir).ok();
}

/// JSON carries a null output plus the run error with zero counts, and exits
/// nonzero. Stdout stays one parseable document.
#[test]
fn invalid_empty_output_context_is_fatal_in_json() {
    let dir = refused_fixture();
    let output = press()
        .arg("convert")
        .arg(&dir)
        .arg("--json")
        .output()
        .unwrap();
    assert!(!output.status.success(), "a refused destination fails");
    let stdout = String::from_utf8_lossy(&output.stdout);
    let json: serde_json::Value =
        serde_json::from_str(&stdout).expect("stdout stays one JSON document");
    assert_eq!(json["schema_version"], 2);
    assert!(json["output"].is_null());
    assert!(
        json["error"]
            .as_str()
            .is_some_and(|error| !error.is_empty()),
        "the run error is named: {json}"
    );
    assert_eq!(json["summary"]["attempted"], 0);
    assert_eq!(json["summary"]["converted"], 0);
    assert_eq!(json["summary"]["failed"], 0);
    assert!(json["files"].as_array().unwrap().is_empty());
    std::fs::remove_dir_all(&dir).ok();
}
