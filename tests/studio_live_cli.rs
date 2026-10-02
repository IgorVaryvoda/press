//! The live `press studio` verbs against a loopback stand-in for Studio. Debug
//! builds read `PRESS_STUDIO_API`, so no key or credit leaves the machine. A
//! completed `run` downloads its result over https only, so this file proves the
//! verbs and the consent that guards them; `studio.rs` proves the image path.

mod common;

use std::path::PathBuf;
use std::process::{Command, Output};
use std::sync::{Arc, Mutex};

struct Fixture {
    config: PathBuf,
    api: String,
    requests: Arc<Mutex<Vec<String>>>,
}

impl Fixture {
    fn new(tag: &str) -> Self {
        let config =
            std::env::temp_dir().join(format!("press-studio-live-{tag}-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&config);
        let store = if cfg!(target_os = "macos") {
            config.join("Library/Application Support/imageguide")
        } else {
            config.join("imageguide")
        };
        std::fs::create_dir_all(&store).unwrap();
        std::fs::write(store.join("studio"), "api_key=sk_live_test\n").unwrap();
        let requests = Arc::new(Mutex::new(Vec::new()));
        let seen = requests.clone();
        let api = common::serve(move |method, target, _| {
            seen.lock().unwrap().push(format!("{method} {target}"));
            match (method, target) {
                ("GET", "/api/zapier/me") => (200, br#"{"credits":41}"#.to_vec()),
                ("POST", "/api/zapier/alt-text") => (
                    200,
                    br#"{"alt_text":" A red chair. ","credits_used":1}"#.to_vec(),
                ),
                _ => (404, b"{}".to_vec()),
            }
        });
        Self {
            config,
            api,
            requests,
        }
    }

    fn run(&self, args: &[&str]) -> Output {
        Command::new(env!("CARGO_BIN_EXE_press"))
            .args(args)
            .env("PRESS_STUDIO_API", &self.api)
            .env("HOME", &self.config)
            .env("XDG_CONFIG_HOME", &self.config)
            .env("APPDATA", &self.config)
            .output()
            .expect("the binary runs")
    }

    fn spent(&self) -> bool {
        self.requests
            .lock()
            .unwrap()
            .iter()
            .any(|request| request.starts_with("POST"))
    }
}

impl Drop for Fixture {
    fn drop(&mut self) {
        let _ = std::fs::remove_dir_all(&self.config);
    }
}

fn json(output: &Output) -> serde_json::Value {
    serde_json::from_slice(&output.stdout).unwrap_or_else(|error| {
        panic!(
            "stdout is one JSON document ({error}): {}{}",
            String::from_utf8_lossy(&output.stdout),
            String::from_utf8_lossy(&output.stderr)
        )
    })
}

#[test]
fn credits_reads_the_balance_with_the_saved_key() {
    let fixture = Fixture::new("credits");
    let output = fixture.run(&["studio", "credits", "--json"]);
    assert_eq!(output.status.code(), Some(0), "{output:?}");
    assert_eq!(json(&output)["credits"], 41.0);
    assert!(!fixture.spent());
}

#[test]
fn alt_text_spends_only_with_allow_spend_and_says_what_it_cost() {
    let fixture = Fixture::new("alt");
    let url = "https://example.com/chair.jpg";
    let refused = fixture.run(&["studio", "alt-text", "--url", url, "--json"]);
    assert_eq!(refused.status.code(), Some(2));
    assert!(
        json(&refused)["error"]
            .as_str()
            .is_some_and(|error| error.contains("--allow-spend"))
    );
    assert!(!fixture.spent(), "a refused run sends nothing");

    let output = fixture.run(&[
        "studio",
        "alt-text",
        "--url",
        url,
        "--allow-spend",
        "--json",
    ]);
    assert_eq!(output.status.code(), Some(0), "{output:?}");
    let report = json(&output);
    assert_eq!(report["alt_text"], "A red chair.");
    assert_eq!(report["credits_used"], 1.0);
    assert_eq!(report["credits_left"], 41.0);
}

#[test]
fn run_names_each_missing_consent_before_anything_leaves() {
    let fixture = Fixture::new("run");
    let image = fixture.config.join("shot.png");
    image::RgbImage::from_fn(8, 8, |x, y| image::Rgb([x as u8 * 30, y as u8 * 30, 9]))
        .save(&image)
        .unwrap();
    let image = image.to_string_lossy().into_owned();
    let base = [
        "studio", "run", "--tool", "upscale", "--image", &image, "--json",
    ];
    for (extra, missing) in [
        (&[][..], "--allow-upload --allow-spend"),
        (&["--allow-upload"][..], "--allow-spend"),
        (&["--allow-spend"][..], "--allow-upload"),
    ] {
        let mut args = base.to_vec();
        args.extend_from_slice(extra);
        let output = fixture.run(&args);
        assert_eq!(output.status.code(), Some(2), "{extra:?}");
        let error = json(&output)["error"].as_str().unwrap().to_string();
        assert!(error.ends_with(missing), "{error}");
    }
    assert!(
        fixture.requests.lock().unwrap().is_empty(),
        "nothing was sent"
    );
    assert!(!fixture.config.join("optimized").exists());
}
