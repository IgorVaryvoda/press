use std::path::{Path, PathBuf};
use std::process::{Command, Output};

fn config_dir(tag: &str) -> PathBuf {
    let path = std::env::temp_dir().join(format!("press-studio-cli-{tag}-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&path);
    std::fs::create_dir_all(&path).expect("config folder is created");
    path
}

fn config_root(config: &Path) -> PathBuf {
    if cfg!(target_os = "macos") {
        config.join("Library/Application Support")
    } else {
        config.to_path_buf()
    }
}

fn run(config: &Path, args: &[&str]) -> Output {
    Command::new(env!("CARGO_BIN_EXE_press"))
        .args(args)
        .env("HOME", config)
        .env("XDG_CONFIG_HOME", config)
        .env("APPDATA", config)
        .output()
        .expect("the binary runs")
}

fn json(output: &Output) -> serde_json::Value {
    serde_json::from_slice(&output.stdout).expect("stdout is one JSON document")
}

fn fixture(config: &Path, answers: &str, results: &str) -> (PathBuf, PathBuf, PathBuf) {
    let image = config.join("input.bin");
    let script = config.join("script.json");
    std::fs::write(&image, b"stable input").expect("input is written");
    let base = format!(
        r#"{{"capabilities":[{{"op":"upscale","version":1,"max_input_bytes":1024}}],"pricing":{{"upscale":{{"max_credits":10,"model":"rehearsal-1"}}}},"answers":{answers},"results":{results}}}"#
    );
    std::fs::write(&script, base).expect("script is written");
    (
        image,
        script,
        config_root(config).join("imageguide/studio/jobs"),
    )
}

fn quote(config: &Path, image: &Path, script: &Path) -> String {
    let image = image.to_string_lossy().into_owned();
    let script = script.to_string_lossy().into_owned();
    let output = run(
        config,
        &[
            "studio",
            "quote",
            "--tool",
            "upscale",
            "--image",
            &image,
            "--payer",
            "workspace-1",
            "--fake",
            &script,
            "--json",
        ],
    );
    assert_eq!(
        output.status.code(),
        Some(0),
        "{}",
        String::from_utf8_lossy(&output.stderr)
    );
    let report = json(&output);
    assert_eq!(report["rehearsal"], true);
    assert_eq!(report["state"], "quoted");
    report["job"].as_str().expect("job id").to_owned()
}

#[test]
fn hosted_rehearsal_accepts_reconciles_retrieves_and_will_not_clobber() {
    let config = config_dir("round-trip");
    let (image, script, jobs) = fixture(&config, "{}", "{}");
    let job = quote(&config, &image, &script);
    let script_text = format!(
        r#"{{"capabilities":[{{"op":"upscale","version":1,"max_input_bytes":1024}}],"pricing":{{"upscale":{{"max_credits":10,"model":"rehearsal-1"}}}},"answers":{{"{job}":[{{"accept":{{"server_job":"server-1"}}}},{{"progress":{{"state":"processing","settled":null,"refunded":null}}}},{{"progress":{{"state":"complete","settled":5,"refunded":0}}}}]}},"results":{{"{job}":[1,2,3]}}}}"#
    );
    std::fs::write(&script, script_text).expect("script is updated");
    let image_text = image.to_string_lossy().into_owned();
    let script_text = script.to_string_lossy().into_owned();

    let accept = run(
        &config,
        &[
            "studio",
            "accept",
            "--job",
            &job,
            "--image",
            &image_text,
            "--fake",
            &script_text,
            "--json",
        ],
    );
    assert_eq!(
        accept.status.code(),
        Some(0),
        "{}",
        String::from_utf8_lossy(&accept.stderr)
    );
    assert_eq!(json(&accept)["state"], "accepted");

    let status = run(
        &config,
        &[
            "studio",
            "status",
            "--job",
            &job,
            "--fake",
            &script_text,
            "--json",
        ],
    );
    assert_eq!(status.status.code(), Some(0));
    assert_eq!(json(&status)["state"], "processing");
    let status = run(
        &config,
        &[
            "studio",
            "status",
            "--job",
            &job,
            "--fake",
            &script_text,
            "--json",
        ],
    );
    assert_eq!(status.status.code(), Some(0));
    assert_eq!(json(&status)["state"], "complete");

    let output = config.join("result.bin");
    let output_text = output.to_string_lossy().into_owned();
    let retrieved = run(
        &config,
        &[
            "studio",
            "retrieve",
            "--job",
            &job,
            "--out",
            &output_text,
            "--fake",
            &script_text,
            "--json",
        ],
    );
    assert_eq!(retrieved.status.code(), Some(0));
    assert_eq!(std::fs::read(&output).unwrap(), [1, 2, 3]);
    let again = run(
        &config,
        &[
            "studio",
            "retrieve",
            "--job",
            &job,
            "--out",
            &output_text,
            "--fake",
            &script_text,
            "--json",
        ],
    );
    assert_eq!(again.status.code(), Some(1));
    assert_eq!(std::fs::read(&output).unwrap(), [1, 2, 3]);
    assert!(jobs.join(format!("{job}.hosted.json")).is_file());
    let _ = std::fs::remove_dir_all(config);
}

#[test]
fn hosted_rehearsal_rechecks_input_and_recovers_pending_acceptance() {
    let config = config_dir("recovery");
    let (image, script, _) = fixture(&config, "{}", "{}");
    let job = quote(&config, &image, &script);
    let script_text = format!(
        r#"{{"capabilities":[{{"op":"upscale","version":1,"max_input_bytes":1024}}],"pricing":{{"upscale":{{"max_credits":10,"model":"rehearsal-1"}}}},"answers":{{"{job}":["transport",{{"accept":{{"server_job":"server-2"}}}}]}}}}"#
    );
    std::fs::write(&script, script_text).expect("script is updated");
    let image_text = image.to_string_lossy().into_owned();
    let script_text = script.to_string_lossy().into_owned();
    let first = run(
        &config,
        &[
            "studio",
            "accept",
            "--job",
            &job,
            "--image",
            &image_text,
            "--fake",
            &script_text,
            "--json",
        ],
    );
    assert_eq!(first.status.code(), Some(1));
    assert!(String::from_utf8_lossy(&first.stdout).contains("rehearsal"));
    std::fs::write(&image, b"changed input").expect("input is changed");
    let changed = run(
        &config,
        &[
            "studio",
            "reconcile",
            "--job",
            &job,
            "--fake",
            &script_text,
            "--json",
        ],
    );
    assert_eq!(
        changed.status.code(),
        Some(1),
        "stdout={} stderr={}",
        String::from_utf8_lossy(&changed.stdout),
        String::from_utf8_lossy(&changed.stderr)
    );
    assert!(
        json(&changed)["state"] == "submitting",
        "stdout={} stderr={}",
        String::from_utf8_lossy(&changed.stdout),
        String::from_utf8_lossy(&changed.stderr)
    );
    let stale_accept = run(
        &config,
        &[
            "studio",
            "accept",
            "--job",
            &job,
            "--image",
            &image_text,
            "--fake",
            &script_text,
            "--json",
        ],
    );
    assert_eq!(stale_accept.status.code(), Some(1));
    assert_eq!(json(&stale_accept)["state"], "submitting");
    assert!(
        json(&stale_accept)["error"]
            .as_str()
            .is_some_and(|error| error.contains("input changed"))
    );
    std::fs::write(&image, b"stable input").expect("input is restored");
    let recovered = run(
        &config,
        &[
            "studio",
            "accept",
            "--job",
            &job,
            "--image",
            &image_text,
            "--fake",
            &script_text,
            "--json",
        ],
    );
    assert_eq!(recovered.status.code(), Some(0));
    assert_eq!(json(&recovered)["state"], "accepted");
    let _ = std::fs::remove_dir_all(config);
}

#[test]
fn hosted_rehearsal_refuses_live_shape_and_missing_authority() {
    let config = config_dir("refuse");
    let (image, script, _) = fixture(&config, "{}", "{}");
    let image_text = image.to_string_lossy().into_owned();
    let script_text = script.to_string_lossy().into_owned();
    let output = run(
        &config,
        &[
            "studio",
            "quote",
            "--tool",
            "replace-background",
            "--image",
            &image_text,
            "--payer",
            "workspace-1",
            "--fake",
            &script_text,
        ],
    );
    assert_eq!(output.status.code(), Some(2));
    assert!(String::from_utf8_lossy(&output.stderr).contains("supports only"));
    let output = run(&config, &["studio", "status", "--job", "../escape"]);
    assert_eq!(output.status.code(), Some(2));
    assert!(String::from_utf8_lossy(&output.stderr).contains("--fake"));
    let _ = std::fs::remove_dir_all(config);
}
