//! The command line's Sirv and live Studio verbs: the window's folder sync and
//! hosted tools without a window.
//!
//! Every run that sends a file off this computer says so with its own flag, every
//! run that spends Studio credits with another, and every run that would replace
//! a file on either side with a third.
//! Nothing is remembered between runs: consent given once is not consent for the
//! next command an agent decides to type.

use std::collections::{HashMap, HashSet};
use std::path::{Path, PathBuf};

use serde::Serialize;

use crate::scan::{self, format_bytes};
use crate::{Args, command_error, path_text, sirv, studio, write_json_with_code};

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum SirvVerb {
    Status,
    Push,
    Pull,
}

impl SirvVerb {
    pub(crate) fn named(name: &str) -> Option<Self> {
        match name {
            "status" => Some(Self::Status),
            "push" => Some(Self::Push),
            "pull" => Some(Self::Pull),
            _ => None,
        }
    }

    fn name(self) -> &'static str {
        match self {
            Self::Status => "status",
            Self::Push => "push",
            Self::Pull => "pull",
        }
    }
}

/// Failures in a row that end a transfer. A dead network or a revoked key fails
/// every file the same way, and a thousand timeouts say nothing five do not.
const GIVE_UP_AFTER: usize = 5;

#[derive(Serialize)]
struct Transferred {
    key: String,
    status: &'static str,
    bytes: u64,
    error: Option<String>,
}

#[derive(Serialize)]
struct SirvReport {
    schema_version: u32,
    command: &'static str,
    verb: &'static str,
    root: String,
    remote: String,
    subfolders: bool,
    /// Size-only comparison, as the window draws it: equal bytes are `same_size`,
    /// never a claim that the contents match.
    only_local: Vec<String>,
    different_size: Vec<String>,
    only_remote: Vec<String>,
    same_size: usize,
    files: Vec<Transferred>,
    /// True when the run gave up after repeated failures, leaving files untried.
    stopped: bool,
}

/// Compare a local folder with a Sirv folder by name and size, and push or pull
/// what differs. `status` reads both sides and writes nothing anywhere.
pub(crate) fn sirv_headless(verb: SirvVerb, target: &Path, args: &Args) -> i32 {
    let fail =
        |code: i32, message: String| -> ! { command_error("sirv", message, args.json, code) };
    if !target.is_dir() {
        fail(2, format!("{} is not a folder", target.display()));
    }
    let root = std::fs::canonicalize(target).unwrap_or_else(|error| {
        fail(
            2,
            format!("could not resolve {}: {error}", target.display()),
        )
    });
    let remote = args
        .sirv_remote
        .clone()
        .or_else(|| sirv::load_pairings().remove(&root))
        .unwrap_or_else(|| {
            fail(
                2,
                format!(
                    "{} has no paired Sirv folder; name one with --remote /folder",
                    target.display()
                ),
            )
        });
    let dir = remote.trim_end_matches('/').to_string();
    if !dir.starts_with('/') || dir.is_empty() {
        fail(
            2,
            format!("--remote needs a Sirv folder such as /photos, got {remote:?}"),
        );
    }
    let Some(credentials) = sirv::load_credentials() else {
        fail(
            2,
            "no Sirv API keys; add them in the Press window's Sirv settings".into(),
        );
    };
    let mut client = sirv::Client::new(credentials);

    let deep = args.subfolders;
    let local = if deep {
        scan::scan(&root, &root.join(scan::OUTPUT_DIR))
    } else {
        scan::browse(&root, &root.join(scan::OUTPUT_DIR))
            .map(|browsed| browsed.scan)
            .unwrap_or_else(|error| fail(2, format!("{}: {error}", target.display())))
    };
    // A folder that is not on Sirv yet is an empty one: the first push creates it.
    let files = match sirv::walk_remote(&dir, deep, |folder| {
        client.readdir_cancellable(folder, None)
    }) {
        Ok(files) => files.unwrap_or_default(),
        Err(error) if error.status == 404 => HashMap::new(),
        Err(error) => fail(1, format!("could not list {dir} on Sirv: {error}")),
    };

    let mut only_local = Vec::new();
    let mut different_size = Vec::new();
    let mut same_size = 0;
    let mut local_keys = HashSet::new();
    for entry in &local.entries {
        let Some(key) = sirv::paired_key(&root, &entry.path, deep) else {
            continue;
        };
        match sirv::classify(entry.bytes, files.get(&key)) {
            sirv::SyncState::OnlyLocal => only_local.push((key.clone(), entry.path.clone())),
            sirv::SyncState::DifferentSize => {
                different_size.push((key.clone(), entry.path.clone()))
            }
            sirv::SyncState::SameSize => same_size += 1,
        }
        local_keys.insert(key);
    }
    // The disk, not the scan, says what exists here: a pull must not overwrite a
    // RAW file or a text file the image scan never listed.
    let on_disk = sirv::local_sizes_for(&root, files.keys().map(String::as_str));
    let mut only_remote = sirv::pull_plan(&files, &on_disk, false);
    only_local.sort();
    different_size.sort();
    only_remote.sort();

    let mut files_out = Vec::new();
    let mut stopped = false;
    match verb {
        SirvVerb::Status => {}
        SirvVerb::Push => {
            let mut plan: Vec<(String, PathBuf, bool)> = only_local
                .iter()
                .map(|(key, path)| (key.clone(), path.clone(), false))
                .collect();
            if args.replace_changed {
                plan.extend(
                    different_size
                        .iter()
                        .map(|(key, path)| (key.clone(), path.clone(), true)),
                );
            }
            // A file sent to Sirv is public; a link would send its target.
            plan.retain(|(key, _, _)| sirv::plain_file_below(&root, key));
            plan.sort();
            if !plan.is_empty() && !args.allow_upload {
                let bytes = plan
                    .iter()
                    .filter_map(|(_, path, _)| std::fs::metadata(path).ok())
                    .map(|metadata| metadata.len())
                    .sum();
                fail(
                    2,
                    format!(
                        "push would upload {} {} ({}) to {dir} on Sirv, where they are public; \
                         run again with --allow-upload",
                        plan.len(),
                        if plan.len() == 1 { "file" } else { "files" },
                        format_bytes(bytes)
                    ),
                );
            }
            if !plan.is_empty() {
                let folders = std::iter::once(dir.clone()).chain(
                    sirv::push_folders(plan.iter().map(|(key, _, _)| key))
                        .into_iter()
                        .map(|folder| format!("{dir}/{folder}")),
                );
                for folder in folders {
                    if let Err(error) = client.mkdir(&folder) {
                        fail(1, format!("could not create {folder} on Sirv: {error}"));
                    }
                }
            }
            let total = plan.len();
            let mut failures_in_a_row = 0;
            for (key, path, replacing) in plan {
                if failures_in_a_row >= GIVE_UP_AFTER {
                    stopped = true;
                    break;
                }
                let outcome = push_one(&mut client, &dir, &key, &path, replacing);
                failures_in_a_row = if outcome.is_ok() {
                    0
                } else {
                    failures_in_a_row + 1
                };
                files_out.push(transferred(key, outcome, "pushed"));
                report_progress(args, files_out.last(), files_out.len(), total);
            }
        }
        SirvVerb::Pull => {
            let mut plan: Vec<(String, bool)> =
                only_remote.iter().map(|key| (key.clone(), false)).collect();
            if args.replace_changed {
                plan.extend(
                    sirv::pull_plan(&files, &on_disk, true)
                        .into_iter()
                        .map(|key| (key, true)),
                );
            }
            plan.sort();
            let total = plan.len();
            let mut failures_in_a_row = 0;
            for (key, replacing) in plan {
                if failures_in_a_row >= GIVE_UP_AFTER {
                    stopped = true;
                    break;
                }
                let outcome = client
                    .download(&format!("{dir}/{key}"))
                    .map_err(|error| error.to_string())
                    .and_then(|bytes| {
                        sirv::write_pulled(&root, &key, &bytes, replacing)
                            .map(|()| bytes.len() as u64)
                    });
                failures_in_a_row = if outcome.is_ok() {
                    0
                } else {
                    failures_in_a_row + 1
                };
                files_out.push(transferred(key, outcome, "pulled"));
                report_progress(args, files_out.last(), files_out.len(), total);
            }
            // What this run brought down is no longer remote-only.
            let pulled: HashSet<&str> = files_out
                .iter()
                .filter(|file| file.error.is_none())
                .map(|file| file.key.as_str())
                .collect();
            only_remote.retain(|key| !pulled.contains(key.as_str()));
        }
    }

    let failed = files_out.iter().filter(|file| file.error.is_some()).count();
    let code = i32::from(failed > 0 || stopped);
    let keys = |list: &[(String, PathBuf)]| list.iter().map(|(key, _)| key.clone()).collect();
    let report = SirvReport {
        schema_version: 1,
        command: "sirv",
        verb: verb.name(),
        root: path_text(&root),
        remote: dir.clone(),
        subfolders: deep,
        only_local: keys(&only_local),
        different_size: keys(&different_size),
        only_remote,
        same_size,
        files: files_out,
        stopped,
    };
    if args.json {
        if let Err(error) = write_json_with_code(&report, code) {
            eprintln!("press: could not write JSON: {error}");
            return 1;
        }
        return code;
    }
    for file in &report.files {
        match &file.error {
            None => outln!(
                "{} {} ({})",
                file.status,
                file.key,
                format_bytes(file.bytes)
            ),
            Some(error) => outln!("failed {}: {error}", file.key),
        }
    }
    outln!(
        "{} {}: {} only here, {} only on Sirv, {} different size, {} same size{}",
        path_text(&root),
        dir,
        report.only_local.len(),
        report.only_remote.len(),
        report.different_size.len(),
        report.same_size,
        if verb == SirvVerb::Status {
            String::new()
        } else {
            format!(
                "; {} {}, {failed} failed",
                report.files.len() - failed,
                if verb == SirvVerb::Push {
                    "pushed"
                } else {
                    "pulled"
                }
            )
        }
    );
    if stopped {
        outln!("stopped after {GIVE_UP_AFTER} failures in a row; the rest were not tried");
    }
    code
}

/// Upload one file. A file planned as new is asked about first: the listing it
/// was planned from can be minutes old, and somebody else's upload since then is
/// not this run's to overwrite.
fn push_one(
    client: &mut sirv::Client,
    dir: &str,
    key: &str,
    path: &Path,
    replacing: bool,
) -> Result<u64, String> {
    let size = std::fs::metadata(path)
        .map_err(|error| error.to_string())?
        .len();
    if size > sirv::MAX_TRANSFER {
        return Err(format!(
            "larger than the {}-byte transfer cap",
            sirv::MAX_TRANSFER
        ));
    }
    let bytes = std::fs::read(path).map_err(|error| error.to_string())?;
    let remote = format!("{dir}/{key}");
    if !replacing && client.exists(&remote).map_err(|error| error.to_string())? {
        return Err("now on Sirv, added after the listing; not overwritten".into());
    }
    client
        .upload(&remote, &bytes, sirv::content_type(key))
        .map_err(|error| error.to_string())?;
    Ok(size)
}

fn transferred(key: String, outcome: Result<u64, String>, done: &'static str) -> Transferred {
    match outcome {
        Ok(bytes) => Transferred {
            key,
            status: done,
            bytes,
            error: None,
        },
        Err(error) => Transferred {
            key,
            status: "failed",
            bytes: 0,
            error: Some(error),
        },
    }
}

fn report_progress(args: &Args, file: Option<&Transferred>, done: usize, total: usize) {
    if let Some(file) = file
        && args.progress
    {
        eprintln!("[{done}/{total}] {} {}", file.key, file.status);
    }
}

/// The Studio verbs that reach the live service, beside the fixture rehearsal.
pub(crate) const STUDIO_LIVE_VERBS: &[&str] = &["credits", "run", "alt-text"];

/// Run one live Studio verb with the key the user saved in the window.
///
/// `credits` only reads the balance. `run` sends one image to Studio and
/// `alt-text` has Studio read a public URL; both spend credits, so both need
/// `--allow-spend` on the run itself, and `run` also needs `--allow-upload`.
/// Studio quotes no price before the work and reports the charge after it, so
/// a credit ceiling here could only ever be a promise: the report states what
/// the run used and what the account has left instead.
pub(crate) fn studio_live(verb: &str, args: &Args) -> i32 {
    let fail =
        |code: i32, message: String| -> ! { command_error("studio", message, args.json, code) };
    let Some(key) = studio::load_key() else {
        fail(
            2,
            "no Studio API key; add one in the Press window's Studio settings".into(),
        );
    };
    let mut report = serde_json::Map::from_iter([
        ("schema_version".to_string(), serde_json::json!(1)),
        ("command".to_string(), serde_json::json!("studio")),
        ("verb".to_string(), serde_json::json!(verb)),
    ]);
    let balance = |key: &str| {
        studio::verify_key(key)
            .ok()
            .and_then(|account| account.credits)
    };
    let text = match verb {
        "credits" => {
            let account = studio::verify_key(&key).unwrap_or_else(|error| fail(1, error));
            report.insert("credits".into(), serde_json::json!(account.credits));
            match account.credits {
                Some(credits) => format!("{} available", studio::format_credits(credits)),
                None => "the key works; Studio did not say the balance".into(),
            }
        }
        "alt-text" => {
            let url = args
                .studio_url
                .as_deref()
                .unwrap_or_else(|| fail(2, "alt-text needs --url <https://…>".into()));
            if !url.starts_with("https://") {
                fail(2, format!("alt-text needs a public https URL, got {url:?}"));
            }
            if !args.allow_spend {
                fail(
                    2,
                    "alt text spends Studio credits; run again with --allow-spend".into(),
                );
            }
            let (alt, used) = studio::alt_text(&key, url).unwrap_or_else(|error| fail(1, error));
            let left = balance(&key);
            report.insert("url".into(), serde_json::json!(url));
            report.insert("alt_text".into(), serde_json::json!(alt));
            report.insert("credits_used".into(), serde_json::json!(used));
            report.insert("credits_left".into(), serde_json::json!(left));
            format!("{alt}\n{}", spend_line(used, left))
        }
        _ => {
            let slug = args.studio_tool.as_deref().unwrap_or_default();
            let Some(tool) = studio::TOOLS
                .iter()
                .copied()
                .find(|tool| tool.slug() == slug)
            else {
                fail(2, format!("run needs --tool, one of {}", studio_tools()));
            };
            let image = args
                .studio_image
                .as_deref()
                .unwrap_or_else(|| fail(2, "run needs --image <file>".into()));
            let Some(entry) = image.is_file().then(|| scan::probe(image)).flatten() else {
                fail(2, format!("{} is not an image file", image.display()));
            };
            let prompt = args.studio_prompt.clone().unwrap_or_default();
            if tool.needs_prompt() && prompt.trim().is_empty() {
                fail(2, format!("{} needs --prompt", tool.slug()));
            }
            let missing: Vec<&str> = [
                (!args.allow_upload).then_some("--allow-upload"),
                (!args.allow_spend).then_some("--allow-spend"),
            ]
            .into_iter()
            .flatten()
            .collect();
            if !missing.is_empty() {
                fail(
                    2,
                    format!(
                        "{} sends {} ({}) to Sirv Studio and spends credits; run again with {}",
                        tool.slug(),
                        image.display(),
                        format_bytes(entry.bytes),
                        missing.join(" ")
                    ),
                );
            }
            let cancelled = std::sync::atomic::AtomicBool::new(false);
            let upload = match studio::prepare_upload(image, &cancelled) {
                Ok(studio::Preflight::Ready(upload)) => upload,
                Ok(studio::Preflight::NeedsConfirmation(_)) => fail(
                    2,
                    format!(
                        "{} only fits Studio's upload limit as a lossy or smaller copy; \
                         make one first, for example with press convert --max-edge 4000, \
                         and run that file",
                        image.display()
                    ),
                ),
                Err(error) => fail(1, error),
            };
            let audited = std::fs::canonicalize(image).unwrap_or_else(|error| {
                fail(2, format!("could not resolve {}: {error}", image.display()))
            });
            let root = audited.parent().unwrap_or(Path::new(".")).to_path_buf();
            let destination = match args.output.clone() {
                Some(folder) => crate::settings::Output::Folder(folder),
                None => crate::settings::Output::Optimized,
            };
            let out_dir = destination
                .context(&root)
                .map(|context| context.output_root().to_path_buf())
                .unwrap_or_else(|message| fail(2, message));
            let (written, used) = studio::process_prepared(
                &key, &root, &out_dir, &upload, &audited, tool, &prompt, &cancelled,
            )
            .unwrap_or_else(|error| fail(1, error));
            let left = balance(&key);
            report.insert("tool".into(), serde_json::json!(tool.slug()));
            report.insert("source".into(), serde_json::json!(path_text(image)));
            report.insert("output".into(), serde_json::json!(path_text(&written)));
            report.insert("credits_used".into(), serde_json::json!(used));
            report.insert("credits_left".into(), serde_json::json!(left));
            format!(
                "{} {} -> {}\n{}",
                tool.slug(),
                image.display(),
                written.display(),
                spend_line(used, left)
            )
        }
    };
    if args.json {
        if let Err(error) = write_json_with_code(&report, 0) {
            eprintln!("press: could not write JSON: {error}");
            return 1;
        }
    } else {
        outln!("{text}");
    }
    0
}

fn spend_line(used: Option<f64>, left: Option<f64>) -> String {
    let used = used.map_or("Studio did not say the charge".to_string(), |used| {
        format!("{} used", studio::format_credits(used))
    });
    match left {
        Some(left) => format!("{used}, {} left", studio::format_credits(left)),
        None => used,
    }
}

pub(crate) fn studio_tools() -> String {
    studio::TOOLS
        .iter()
        .map(|tool| tool.slug())
        .collect::<Vec<_>>()
        .join(", ")
}
