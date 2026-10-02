//! `press sirv` against a loopback stand-in for the Sirv REST API. Debug builds
//! read `PRESS_SIRV_API`, so the binary under test talks to this server and
//! nothing reaches the real service. The server keeps files in memory and
//! answers only the routes the command uses.

use std::collections::{BTreeMap, BTreeSet};
use std::io::{BufRead, BufReader, Read, Write};
use std::net::TcpListener;
use std::path::{Path, PathBuf};
use std::process::{Command, Output};
use std::sync::{Arc, Mutex};

#[derive(Default)]
struct Remote {
    files: BTreeMap<String, Vec<u8>>,
    folders: BTreeSet<String>,
}

fn decode(text: &str) -> String {
    let bytes = text.as_bytes();
    let mut out = Vec::new();
    let mut index = 0;
    while index < bytes.len() {
        if bytes[index] == b'%' {
            out.push(u8::from_str_radix(&text[index + 1..index + 3], 16).unwrap());
            index += 3;
        } else {
            out.push(bytes[index]);
            index += 1;
        }
    }
    String::from_utf8(out).unwrap()
}

fn parent(path: &str) -> &str {
    path.rsplit_once('/').map_or("", |(parent, _)| parent)
}

/// One request, one response, then the connection closes.
fn answer(remote: &Mutex<Remote>, method: &str, target: &str, body: Vec<u8>) -> (u16, Vec<u8>) {
    let (route, query) = target.split_once('?').unwrap_or((target, ""));
    let argument = query
        .split_once('=')
        .map(|(_, value)| decode(value))
        .unwrap_or_default();
    let mut remote = remote.lock().unwrap();
    match (method, route) {
        ("POST", "/v2/token") => (200, br#"{"token":"t","expiresIn":1200}"#.to_vec()),
        ("GET", "/v2/files/readdir") => {
            if !remote.folders.contains(&argument) {
                return (404, b"{}".to_vec());
            }
            let mut contents = Vec::new();
            for (path, bytes) in &remote.files {
                if parent(path) == argument {
                    contents.push(serde_json::json!({
                        "filename": path.rsplit('/').next().unwrap(),
                        "size": bytes.len(),
                    }));
                }
            }
            for folder in &remote.folders {
                if parent(folder) == argument {
                    contents.push(serde_json::json!({
                        "filename": folder.rsplit('/').next().unwrap(),
                        "isDirectory": true,
                    }));
                }
            }
            (
                200,
                serde_json::json!({ "contents": contents })
                    .to_string()
                    .into_bytes(),
            )
        }
        ("GET", "/v2/files/stat") => match remote.files.contains_key(&argument) {
            true => (200, b"{}".to_vec()),
            false => (404, b"{}".to_vec()),
        },
        ("GET", "/v2/files/download") => match remote.files.get(&argument) {
            Some(bytes) => (200, bytes.clone()),
            None => (404, b"{}".to_vec()),
        },
        ("POST", "/v2/files/mkdir") => {
            remote.folders.insert(argument);
            (200, b"{}".to_vec())
        }
        ("POST", "/v2/files/upload") => {
            assert!(
                remote.folders.contains(parent(&argument)),
                "{argument} uploaded before its folder was made"
            );
            remote.files.insert(argument, body);
            (200, b"{}".to_vec())
        }
        _ => (404, b"{}".to_vec()),
    }
}

fn serve(remote: Arc<Mutex<Remote>>) -> String {
    let listener = TcpListener::bind("127.0.0.1:0").unwrap();
    let address = format!("http://{}", listener.local_addr().unwrap());
    std::thread::spawn(move || {
        for stream in listener.incoming() {
            let Ok(mut stream) = stream else { continue };
            let mut reader = BufReader::new(stream.try_clone().unwrap());
            let mut line = String::new();
            if reader.read_line(&mut line).is_err() {
                continue;
            }
            let mut parts = line.split_whitespace();
            let (method, target) = (
                parts.next().unwrap_or_default().to_string(),
                parts.next().unwrap_or_default().to_string(),
            );
            let mut length = 0;
            loop {
                let mut header = String::new();
                reader.read_line(&mut header).unwrap();
                if header.trim().is_empty() {
                    break;
                }
                if let Some((name, value)) = header.split_once(':')
                    && name.eq_ignore_ascii_case("content-length")
                {
                    length = value.trim().parse().unwrap();
                }
            }
            let mut body = vec![0; length];
            reader.read_exact(&mut body).unwrap();
            let (status, reply) = answer(&remote, &method, &target, body);
            let _ = write!(
                stream,
                "HTTP/1.1 {status} X\r\nContent-Length: {}\r\nConnection: close\r\n\r\n",
                reply.len()
            );
            let _ = stream.write_all(&reply);
        }
    });
    address
}

struct Fixture {
    dir: PathBuf,
    config: PathBuf,
    api: String,
    remote: Arc<Mutex<Remote>>,
}

impl Fixture {
    fn new(tag: &str) -> Self {
        let base =
            std::env::temp_dir().join(format!("press-sirv-cli-{tag}-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&base);
        let dir = base.join("photos");
        std::fs::create_dir_all(dir.join("sub")).unwrap();
        let config = base.join("config");
        // Where the window keeps the keys on each platform, under a home the
        // test owns.
        let store = if cfg!(target_os = "macos") {
            config.join("Library/Application Support/imageguide")
        } else {
            config.join("imageguide")
        };
        std::fs::create_dir_all(&store).unwrap();
        std::fs::write(store.join("sirv"), "client_id=id\nclient_secret=secret\n").unwrap();
        let remote = Arc::new(Mutex::new(Remote::default()));
        let api = serve(remote.clone());
        Self {
            dir,
            config,
            api,
            remote,
        }
    }

    fn run(&self, args: &[&str]) -> Output {
        Command::new(env!("CARGO_BIN_EXE_press"))
            .args(args)
            .env("PRESS_SIRV_API", &self.api)
            .env("XDG_CONFIG_HOME", &self.config)
            .env("HOME", &self.config)
            .env("APPDATA", &self.config)
            .output()
            .expect("the binary runs")
    }

    fn remote_file(&self, path: &str) -> Option<Vec<u8>> {
        self.remote.lock().unwrap().files.get(path).cloned()
    }
}

impl Drop for Fixture {
    fn drop(&mut self) {
        let _ = std::fs::remove_dir_all(self.dir.parent().unwrap());
    }
}

fn json(output: &Output) -> serde_json::Value {
    serde_json::from_slice(&output.stdout).unwrap_or_else(|error| {
        panic!(
            "stdout is one JSON document ({error})\nstdout: {}\nstderr: {}",
            String::from_utf8_lossy(&output.stdout),
            String::from_utf8_lossy(&output.stderr)
        )
    })
}

/// A real 8x8 PNG, so the header-only scan lists it; the seed makes two files differ.
fn image(path: &Path, seed: u8) {
    image::RgbImage::from_fn(8, 8, |x, y| image::Rgb([x as u8 * 30, y as u8 * 30, seed]))
        .save(path)
        .unwrap();
}

#[test]
fn status_push_and_pull_each_need_their_own_consent() {
    let fixture = Fixture::new("sync");
    image(&fixture.dir.join("a.png"), 1);
    image(&fixture.dir.join("sub/b.png"), 2);
    {
        let mut remote = fixture.remote.lock().unwrap();
        remote.folders.insert("/photos".into());
        remote.files.insert("/photos/a.png".into(), b"x".to_vec());
        remote
            .files
            .insert("/photos/c.png".into(), b"remote c".to_vec());
    }
    let dir = fixture.dir.to_string_lossy().into_owned();
    let base = ["--remote", "/photos", "--json"];
    let with = |verb: &str, extra: &[&str]| {
        let mut args = vec!["sirv", verb, &dir];
        args.extend_from_slice(&base);
        args.extend_from_slice(extra);
        fixture.run(&args)
    };

    let status = with("status", &[]);
    assert_eq!(status.status.code(), Some(0), "{:?}", status);
    let report = json(&status);
    assert_eq!(report["only_local"], serde_json::json!(["sub/b.png"]));
    assert_eq!(report["different_size"], serde_json::json!(["a.png"]));
    assert_eq!(report["only_remote"], serde_json::json!(["c.png"]));

    let refused = with("push", &[]);
    assert_eq!(refused.status.code(), Some(2));
    let error = json(&refused)["error"].as_str().unwrap().to_string();
    assert!(
        error.contains("--allow-upload") && error.contains("1 file"),
        "{error}"
    );
    assert_eq!(
        fixture.remote_file("/photos/sub/b.png"),
        None,
        "nothing left"
    );

    let pushed = with("push", &["--allow-upload"]);
    assert_eq!(pushed.status.code(), Some(0), "{:?}", pushed);
    assert_eq!(json(&pushed)["files"][0]["status"], "pushed");
    assert_eq!(
        fixture.remote_file("/photos/sub/b.png"),
        Some(std::fs::read(fixture.dir.join("sub/b.png")).unwrap())
    );
    assert_eq!(
        fixture.remote_file("/photos/a.png"),
        Some(b"x".to_vec()),
        "a different-size file is not replaced without --replace-changed"
    );

    let pulled = with("pull", &[]);
    assert_eq!(pulled.status.code(), Some(0), "{:?}", pulled);
    assert_eq!(
        std::fs::read(fixture.dir.join("c.png")).unwrap(),
        b"remote c"
    );
    assert_ne!(
        std::fs::read(fixture.dir.join("a.png")).unwrap(),
        b"x",
        "a local file is not replaced without --replace-changed"
    );

    let replaced = with("pull", &["--replace-changed"]);
    assert_eq!(replaced.status.code(), Some(0), "{:?}", replaced);
    assert_eq!(std::fs::read(fixture.dir.join("a.png")).unwrap(), b"x");
}

#[test]
fn push_to_a_new_folder_creates_it_first() {
    let fixture = Fixture::new("new-folder");
    image(&fixture.dir.join("a.png"), 1);
    let dir = fixture.dir.to_string_lossy().into_owned();
    let output = fixture.run(&[
        "sirv",
        "push",
        &dir,
        "--remote",
        "/new/shoot",
        "--allow-upload",
        "--no-subfolders",
        "--json",
    ]);
    assert_eq!(output.status.code(), Some(0), "{:?}", output);
    assert!(fixture.remote_file("/new/shoot/a.png").is_some());
}
