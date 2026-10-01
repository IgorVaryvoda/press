//! Sirv REST access for folder sync.
//!
//! The smallest surface the sync feature needs: one token cache, one directory
//! read, and the pure helpers the diff view classifies with. Everything runs
//! blocking and belongs on a background executor; nothing here touches gpui.
//!
//! Secrets live in their own file next to the window settings, because the
//! window settings are rewritten on every viewport change and must never be
//! the thing that silently drops a credential.

use serde::Deserialize;
use std::collections::{HashMap, HashSet};
use std::io::{Read, Write};
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicBool, Ordering};
use std::time::{Duration, Instant};

const API: &str = "https://api.sirv.com";
/// Tokens live 20 minutes on the server. Refresh a minute early so an upload
/// started at minute 19 does not die mid-flight.
const TOKEN_MARGIN: Duration = Duration::from_secs(60);
/// Used until the first response names the real lifetime (`expiresIn`).
const DEFAULT_TOKEN_LIFETIME: Duration = Duration::from_secs(1200);
const TIMEOUT: Duration = Duration::from_secs(30);
/// File transfers get their own, much looser ceiling: a photo shoot folder
/// holds files that legitimately take minutes.
const TRANSFER_TIMEOUT: Duration = Duration::from_secs(600);
const RETRY_DELAYS: [Duration; 2] = [Duration::from_millis(150), Duration::from_millis(500)];
#[derive(Clone, Debug, PartialEq)]
pub struct Credentials {
    pub client_id: String,
    pub client_secret: String,
}

/// A hard cap on one transfer, so a confused server cannot grow memory forever.
pub const MAX_TRANSFER: u64 = 512 * 1024 * 1024;
/// A listing that finds more entries than this is treated as an error rather than
/// listed forever.
const LISTING_LIMIT: usize = 20_000;
/// A broken server can mint unique continuation tokens forever without adding
/// entries. This stays comfortably above any listing that could fit below the
/// file limit.
const READDIR_PAGE_LIMIT: usize = 512;
/// One entry as Sirv reports it from readdir. Unknown fields are ignored, so a
/// server-side addition never breaks the parse.
#[derive(Clone, Debug, Deserialize)]
pub struct Node {
    #[serde(default)]
    pub filename: String,
    /// Byte size; folders report 0.
    #[serde(default)]
    pub size: u64,
    /// True for folders. The docs' field is `isDirectory`; older drafts said
    /// `type`, so both are accepted.
    #[serde(default, rename = "isDirectory")]
    pub is_directory: bool,
    #[serde(default, rename = "type")]
    pub kind: Option<String>,
}

/// One readdir page. `contents` holds up to 100 entries;
/// `continuation` is the token for the next page when there is one.
#[derive(Clone, Debug, Deserialize)]
pub struct Listing {
    #[serde(default)]
    pub contents: Vec<Node>,
    #[serde(default)]
    pub continuation: Option<String>,
}

impl Node {
    pub fn is_folder(&self) -> bool {
        self.is_directory || self.kind.as_deref() == Some("folder")
    }
}

/// An upstream failure: the HTTP status, and whatever Sirv said that adds to
/// it. `status` 0 is a failure before any answer, and `message` then is
/// already a sentence for a person.
#[derive(Debug)]
pub struct Error {
    pub status: u16,
    pub message: String,
}

/// What a person reads. Every toast, dialog and per-file failure prints an
/// `Error` through this, so it says what happened and what to do — not the
/// JSON body, which read as `{"statusCode": 401, "error": "Unauthorized"}`.
impl std::fmt::Display for Error {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        let detail = if self.message.is_empty() {
            String::new()
        } else {
            format!(" Sirv said: {}", self.message)
        };
        match self.status {
            0 => write!(f, "{}", self.message),
            401 => write!(
                f,
                "Sirv didn’t accept this Client ID and secret. Copy both again from your Sirv \
                 account (Settings → API)."
            ),
            403 => write!(
                f,
                "Sirv refused: these API keys aren’t allowed to do this.{detail}"
            ),
            404 => write!(
                f,
                "Sirv can’t find that file or folder. It may have been moved or deleted."
            ),
            413 => write!(f, "The file is too large for Sirv to accept."),
            429 => write!(
                f,
                "Sirv is limiting requests from this account. Wait a minute, then try again."
            ),
            500..=599 => write!(
                f,
                "Sirv is having trouble (error {}). Try again in a few minutes.",
                self.status
            ),
            status => write!(f, "Sirv refused the request (error {status}).{detail}"),
        }
    }
}

/// The part of an error body worth showing: Sirv's own `message`, when it says
/// more than the status already does. "Unauthorized" under a 401 does not.
pub(crate) fn server_message(body: &str) -> String {
    let message = serde_json::from_str::<serde_json::Value>(body)
        .ok()
        .and_then(|value| {
            // Studio puts its sentence in `error`, the Sirv API in `message`.
            value
                .get("message")
                .and_then(|message| message.as_str())
                .filter(|message| !message.trim().is_empty())
                .or_else(|| value.get("error")?.as_str())
                .map(str::to_string)
        })
        .unwrap_or_else(|| {
            // A short plain-text body is a sentence; HTML or JSON is not.
            let body = body.trim();
            if body.len() <= 160 && !body.starts_with(['{', '[', '<']) {
                body.to_string()
            } else {
                String::new()
            }
        });
    let message = message.trim();
    let generic = [
        "unauthorized",
        "forbidden",
        "not found",
        "too many requests",
        "internal server error",
        "bad request",
        "bad gateway",
        "service unavailable",
    ];
    if generic.contains(&message.to_ascii_lowercase().as_str()) {
        String::new()
    } else {
        message.chars().take(200).collect()
    }
}

impl std::error::Error for Error {}

impl Error {
    pub fn retryable(&self) -> bool {
        self.status == 0 || self.status == 429 || self.status >= 500
    }
}

/// Percent-encode a path for a Sirv query string. Everything outside the
/// unreserved set escapes, including `/` as `%2F`, which is what the API docs
/// show for `filename` and `dirname` parameters.
pub fn encode_path(path: &str) -> String {
    let mut out = String::with_capacity(path.len());
    for byte in path.bytes() {
        match byte {
            b'A'..=b'Z' | b'a'..=b'z' | b'0'..=b'9' | b'-' | b'_' | b'.' | b'~' => {
                out.push(byte as char)
            }
            _ => out.push_str(&format!("%{byte:02X}")),
        }
    }
    out
}

fn validate_cdn_host(host: &str) -> Result<(), Error> {
    if !host.is_empty()
        && host
            .bytes()
            .all(|byte| byte.is_ascii_alphanumeric() || matches!(byte, b'.' | b'-'))
    {
        return Ok(());
    }
    Err(Error {
        status: 0,
        message: format!("Sirv returned an invalid CDN host: {host}"),
    })
}

/// The public HTTPS URL for one absolute filename returned by Sirv.
pub fn public_url(cdn_host: &str, remote_filename: &str) -> Result<String, Error> {
    validate_cdn_host(cdn_host)?;
    let path = remote_filename
        .trim_start_matches('/')
        .split('/')
        .map(encode_path)
        .collect::<Vec<_>>()
        .join("/");
    Ok(format!("https://{cdn_host}/{path}"))
}

/// What the CDN sends a current browser for one public URL: the bytes on the
/// wire and the format Sirv chose.
#[derive(Clone, Debug, PartialEq)]
pub struct Delivered {
    pub bytes: u64,
    pub format: String,
}

/// The query that asks Sirv for what Press's max edge would write: the longest
/// side scaled down to `edge`, never up. No edge, no query: Sirv's own default.
pub fn delivery_query(edge: Option<u32>) -> String {
    edge.map(|edge| format!("?s={edge}&scale.option=noup"))
        .unwrap_or_default()
}

/// A browser's image `Accept` header. Sirv negotiates the format from it, so
/// this is what makes the answer AVIF or WebP rather than the stored JPEG.
const BROWSER_ACCEPT: &str = "image/avif,image/webp,image/apng,image/svg+xml,image/*,*/*;q=0.8";

/// Ask the public CDN, with a HEAD request, how many bytes it would deliver.
/// No credentials: this is exactly what a visitor's browser would be sent.
pub fn delivered(url: &str) -> Result<Delivered, Error> {
    let response = ureq::AgentBuilder::new()
        .timeout(TIMEOUT)
        .build()
        .head(url)
        .set("Accept", BROWSER_ACCEPT)
        .call()
        .map_err(sirv_error("delivery check"))?;
    let bytes = response
        .header("Content-Length")
        .and_then(|length| length.trim().parse::<u64>().ok())
        .ok_or_else(|| Error {
            status: 0,
            message: "the CDN did not say how large the image is".into(),
        })?;
    let format = response
        .header("Content-Type")
        .and_then(|kind| kind.split(';').next())
        .map(|kind| {
            kind.trim()
                .trim_start_matches("image/")
                .to_ascii_uppercase()
        })
        .unwrap_or_default();
    Ok(Delivered { bytes, format })
}

/// A small preview of a Sirv file, straight from its CDN: a 48px JPEG, so
/// the split view can show what a file only Sirv holds looks like.
pub fn preview_url(cdn_host: &str, remote_filename: &str) -> Result<String, Error> {
    Ok(format!(
        "{}?w=48&h=48&scale.option=fit&format=jpg",
        public_url(cdn_host, remote_filename)?
    ))
}

/// Whether the CDN can draw `key` as an image at all. Asking for a text file
/// or a video downloaded up to half a megabyte for nothing.
pub fn previewable(key: &str) -> bool {
    let extension = key
        .rsplit_once('.')
        .map(|(_, ext)| ext.to_ascii_lowercase());
    matches!(
        extension.as_deref(),
        Some(
            "jpg"
                | "jpeg"
                | "png"
                | "webp"
                | "avif"
                | "gif"
                | "tif"
                | "tiff"
                | "bmp"
                | "heic"
                | "jxl"
                | "psd"
        )
    )
}

/// The preview's bytes, capped: a wrong or hostile response cannot fill memory.
pub fn preview(url: &str) -> Result<Vec<u8>, Error> {
    let response = ureq::AgentBuilder::new()
        .timeout(TIMEOUT)
        .build()
        .get(url)
        .call()
        .map_err(sirv_error("preview"))?;
    read_capped(response.into_reader(), 512 * 1024).map_err(|message| Error {
        status: 0,
        message: format!("preview body: {message}"),
    })
}

/// Where a person with no Sirv account starts. The credentials form is the
/// only door into sync, so it has to name the way in as well as the way through.
pub const SIGNUP_URL: &str =
    "https://sirv.com/?utm_source=press&utm_medium=desktop&utm_campaign=connect";

/// Where the client ID and secret in that form come from.
pub const API_KEYS_URL: &str = "https://sirv.com/help/articles/sirv-api/?utm_source=press&utm_medium=desktop&utm_campaign=connect";

/// How a local file stands against the paired remote folder.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum SyncState {
    /// No file with this relative path on Sirv.
    OnlyLocal,
    /// Same relative path and equal byte size. Size only: equal bytes are not
    /// proof the contents match.
    SameSize,
    /// Same path, unequal byte size.
    DifferentSize,
}

/// Classify one local file against the remote listing. Size is the only
/// comparator on purpose: local and server clocks disagree often enough that
/// mtime comparison would report lies as changes.
pub fn classify(local_size: u64, remote: Option<&Node>) -> SyncState {
    match remote {
        None => SyncState::OnlyLocal,
        Some(node) if node.size == local_size => SyncState::SameSize,
        Some(_) => SyncState::DifferentSize,
    }
}

/// The word the window says a comparison state in. Size only, on purpose:
/// equal bytes are `same size`, never `synced` or `matching`.
pub(crate) fn sync_state_word(state: SyncState) -> &'static str {
    match state {
        SyncState::SameSize => "same size",
        SyncState::DifferentSize => "different size",
        // The bar's filter word, and no claim about content.
        SyncState::OnlyLocal => "only here",
    }
}

/// The key a local file carries inside the paired folder: its path below
/// `root`, forward-slashed, so `/photos/a.jpg` under `/photos` becomes
/// `a.jpg`. `None` when the file sits outside the root, which cannot happen
/// for scanned entries but keeps the function total.
pub fn relative_key(root: &Path, path: &Path) -> Option<String> {
    let relative = path.strip_prefix(root).ok()?;
    let mut key = String::new();
    for component in relative.components() {
        if !key.is_empty() {
            key.push('/');
        }
        key.push_str(&component.as_os_str().to_string_lossy());
    }
    Some(key)
}

/// The key a local file has in a pairing. The pairing lists one folder level,
/// so a file in a subfolder has none: its Sirv twin was never listed. Keyed
/// anyway, every nested file read as new, and an upload then overwrote
/// whatever Sirv held under that name without the two-click confirmation.
///
/// A deep pairing, listed through its subfolders, keys nested files too.
pub fn paired_key(root: &Path, path: &Path, deep: bool) -> Option<String> {
    relative_key(root, path).filter(|key| {
        let mut folders = key.split('/').rev().skip(1).collect::<Vec<_>>();
        folders.reverse();
        folders.is_empty()
            || deep
                && folders
                    .iter()
                    .enumerate()
                    .all(|(depth, name)| compared_folder(name, depth == 0))
    })
}

/// True when nothing on the way from `root` to `key`, the file included, is
/// a symlink. A file sent to Sirv is public; a link would send its target.
pub fn plain_file_below(root: &Path, key: &str) -> bool {
    let mut path = root.to_path_buf();
    key.split('/').all(|part| {
        path.push(part);
        path.symlink_metadata()
            .is_ok_and(|meta| !meta.file_type().is_symlink())
    }) && path.is_file()
}

/// Strip the paired folder off a remote filename so both sides of the diff
/// speak the same key language. `/photos/a.jpg` paired at `/photos` is
/// `a.jpg`; anything outside the pair is skipped by the caller via `None`.
///
/// readdir's filenames come back relative to the listed folder on some
/// account shapes (`a.jpg` for `dirname=/photos`), absolute on others. A
/// relative name under its own listing folder *is* inside the pair, so it is
/// joined before the prefix check instead of being discarded.
pub fn unpair_remote(dir: &str, filename: &str) -> Option<String> {
    let dir = dir.trim_end_matches('/');
    if let Some(stripped) = filename.strip_prefix(&format!("{dir}/")) {
        return Some(stripped.to_string());
    }
    if filename.starts_with('/') {
        None
    } else {
        Some(filename.to_string())
    }
}

/// Whether a deep pairing compares what is inside folder `name`, on either
/// side. One rule for both, so a folder never has files on one side of the
/// comparison and none on the other: that read as "only here" for ever, and
/// an upload of it could land on a copy the listing never saw. Dot folders,
/// the backup folder and packages anywhere; `optimized/` at the top, where
/// published results live.
pub(crate) fn compared_folder(name: &str, top: bool) -> bool {
    !(name.starts_with('.')
        || name == crate::scan::BACKUP_DIR
        || (top && name == crate::scan::OUTPUT_DIR)
        || crate::scan::is_opaque_package(Path::new(name)))
}

/// Every file of a paired folder, keyed below it. Shallow, only its direct
/// files, matching the local one-folder list. Deep, the walk enters subfolders
/// the way the local scan does with "Include subfolders": dot folders and the
/// backup folder are skipped everywhere, and `optimized/` at the top, where
/// published results live. `list` reads one folder, `None` when cancelled; the
/// whole walk shares one entry cap.
pub(crate) fn walk_remote(
    dir: &str,
    deep: bool,
    mut list: impl FnMut(&str) -> Result<Option<Vec<Node>>, Error>,
) -> Result<Option<HashMap<String, Node>>, Error> {
    let dir = dir.trim_end_matches('/');
    let mut files = HashMap::new();
    let mut pending = vec![String::new()];
    let mut entries = 0;
    while let Some(prefix) = pending.pop() {
        let folder = if prefix.is_empty() {
            dir.to_string()
        } else {
            format!("{dir}/{prefix}")
        };
        let Some(nodes) = list(&folder)? else {
            return Ok(None);
        };
        entries += nodes.len();
        if entries > LISTING_LIMIT {
            return Err(listing_limit_error());
        }
        for node in nodes {
            // Only this folder's own children: some accounts list deeper names.
            let Some(name) = unpair_remote(&folder, &node.filename)
                .filter(|name| !name.is_empty() && !name.contains('/') && !name.contains('\\'))
            else {
                continue;
            };
            let key = if prefix.is_empty() {
                name.clone()
            } else {
                format!("{prefix}/{name}")
            };
            if node.is_folder() {
                if deep && compared_folder(&name, prefix.is_empty()) {
                    pending.push(key);
                }
            } else if safe_key(&key) {
                files.insert(key, node);
            }
        }
    }
    Ok(Some(files))
}

/// True when this continuation token has not been seen before in this
/// listing. A repeated token — adjacent or in a cycle — means the server is
/// looping, and following it would list forever.
pub fn continuation_advances(seen: &mut HashSet<String>, token: &str) -> bool {
    seen.insert(token.to_string())
}

fn listing_limit_error() -> Error {
    Error {
        status: 0,
        message: format!("holds more than {LISTING_LIMIT} entries; open or sync a smaller folder"),
    }
}

/// True when a remote key is safe to join onto a local folder.
///
/// A pull turns a name the server chose into a path this machine writes to. Rust's
/// `Path::join` replaces the whole path when handed an absolute one, so a listing
/// entry of `/etc/cron.d/x` would have written to `/etc/cron.d/x`, and `..` would have
/// climbed out of the paired folder. Neither is something a Sirv account is expected
/// to contain; both are cheap to refuse, and this is the boundary to refuse them at.
pub fn safe_key(key: &str) -> bool {
    !key.is_empty()
        && !key.starts_with('/')
        && !key.starts_with('\\')
        // A Windows drive or UNC prefix is absolute too, and `..` at any depth climbs.
        && !Path::new(key).has_root()
        && Path::new(key)
            .components()
            .all(|part| matches!(part, std::path::Component::Normal(_)))
}

/// Safe relative keys missing locally, or differing when explicitly requested.
pub fn pull_plan(
    remote: &[Node],
    dir: &str,
    local_sizes: &HashMap<String, u64>,
    differing: bool,
) -> Vec<String> {
    remote
        .iter()
        .filter_map(|node| unpair_remote(dir, &node.filename).map(|key| (key, node)))
        .filter(|(key, _)| safe_key(key))
        .filter(|(key, node)| match local_sizes.get(key) {
            Some(local_size) => {
                differing && classify(*local_size, Some(node)) == SyncState::DifferentSize
            }
            None => !differing,
        })
        .map(|(key, _)| key)
        .collect()
}

/// The on-disk size of every remote key's local twin. The image scan is the
/// wrong source for this: it drops RAW files, non-images and `optimized/`
/// output, and a pull that trusts it will overwrite exactly those. The disk
/// is the only honest witness for "exists locally". `symlink_metadata`, not
/// `metadata`: a symlink counts as "something is here".
pub fn local_sizes_for<'a>(
    root: &Path,
    keys: impl IntoIterator<Item = &'a str>,
) -> HashMap<String, u64> {
    keys.into_iter()
        .filter(|key| safe_key(key))
        .filter_map(|key| {
            let meta = root.join(key).symlink_metadata().ok()?;
            Some((key.to_string(), meta.len()))
        })
        .collect()
}

/// Read at most `cap` bytes and refuse a body that reaches past it. The old
/// `.take(cap)` alone returned a silently truncated buffer as success, and a
/// truncated image written to disk is corruption with a success message.
pub fn read_capped(reader: impl Read, cap: u64) -> Result<Vec<u8>, String> {
    let mut bytes = Vec::new();
    reader
        .take(cap + 1)
        .read_to_end(&mut bytes)
        .map_err(|error| error.to_string())?;
    if bytes.len() as u64 > cap {
        return Err(format!("larger than the {cap}-byte transfer cap"));
    }
    Ok(bytes)
}

/// Write pulled bytes for `key` under `root`.
///
/// Three properties, each load-bearing:
/// - **Confinement.** `safe_key` is lexical; a local symlink ancestor
///   (`root/sub -> /outside`) still redirects the write. Every existing
///   ancestor between `root` and the target is checked with
///   `symlink_metadata` and refused if it is a symlink.
/// - **No silent replace.** Without `overwrite`, the final installation is
///   `hard_link(part, target)`, which fails atomically if the target
///   exists — there is no check-then-rename window for another writer.
/// - **No partial files.** Bytes land in an exclusively created `.part`
///   sibling first (`create_new` refuses to follow a symlink or truncate a
///   leftover), then move into place.
///
/// Filesystems without hard-link support fail with their OS error rather than
/// silently falling back to replacement.
pub fn write_pulled(root: &Path, key: &str, bytes: &[u8], overwrite: bool) -> Result<(), String> {
    if !safe_key(key) {
        return Err("unsafe remote name".into());
    }
    let target = root.join(key);

    // Refuse symlinked ancestors before creating anything through them.
    let mut ancestor = root.to_path_buf();
    let parts: Vec<&str> = key.split('/').collect();
    for part in &parts[..parts.len().saturating_sub(1)] {
        ancestor = ancestor.join(part);
        match ancestor.symlink_metadata() {
            Ok(meta) if meta.file_type().is_symlink() => {
                return Err(format!(
                    "{} is a symlink; refusing to write through it",
                    ancestor.display()
                ));
            }
            _ => {}
        }
    }
    if let Some(parent) = target.parent() {
        std::fs::create_dir_all(parent)
            .map_err(|error| format!("could not create folder: {error}"))?;
    }

    let mut part_name = target.as_os_str().to_owned();
    part_name.push(".part");
    let part = PathBuf::from(part_name);
    {
        use std::io::Write;
        let mut file = std::fs::OpenOptions::new()
            .write(true)
            .create_new(true)
            .open(&part)
            .map_err(|error| format!("could not create {}: {error}", part.display()))?;
        file.write_all(bytes).map_err(|error| {
            let _ = std::fs::remove_file(&part);
            error.to_string()
        })?;
    }

    let installed = if overwrite {
        std::fs::rename(&part, &target).map_err(|error| error.to_string())
    } else {
        // hard_link is the atomic "create only if absent" install; a target
        // that appeared since planning fails here instead of being replaced.
        std::fs::hard_link(&part, &target)
            .map_err(|error| {
                if error.kind() == std::io::ErrorKind::AlreadyExists {
                    "exists locally; use overwrite to replace it".to_string()
                } else {
                    error.to_string()
                }
            })
            .and_then(|()| std::fs::remove_file(&part).map_err(|error| error.to_string()))
    };
    if installed.is_err() {
        let _ = std::fs::remove_file(&part);
    }
    installed
}

/// The ancestor folders a relative key needs, in creation order:
/// `sub/deep/a.jpg` gives `["sub", "sub/deep"]`.
pub fn ancestor_dirs(key: &str) -> Vec<String> {
    let mut out = Vec::new();
    let mut seen = String::new();
    let parts: Vec<&str> = key.split('/').collect();
    for part in &parts[..parts.len().saturating_sub(1)] {
        if !seen.is_empty() {
            seen.push('/');
        }
        seen.push_str(part);
        out.push(seen.clone());
    }
    out
}

/// Every folder a push needs, each named once, shallowest first.
///
/// Ensuring a key's ancestors inside its own upload task meant one round trip per
/// ancestor per file: 2,000 photos two levels deep spent 4,000 requests re-creating
/// the same two folders.
pub fn push_folders(keys: impl IntoIterator<Item = impl AsRef<str>>) -> Vec<String> {
    let mut folders = Vec::new();
    let mut seen = HashSet::new();
    for key in keys {
        for ancestor in ancestor_dirs(key.as_ref()) {
            if seen.insert(ancestor.clone()) {
                folders.push(ancestor);
            }
        }
    }
    // A parent has to exist before its child, and `ancestor_dirs` already emits each
    // chain in order; sorting by depth keeps that true once chains interleave.
    folders.sort_by_key(|folder| folder.matches('/').count());
    folders
}

/// The Content-Type an upload declares. Sirv sniffs images anyway; declaring
/// correctly keeps the API honest about what it stored.
pub fn content_type(key: &str) -> &'static str {
    match key
        .rsplit('.')
        .next()
        .unwrap_or("")
        .to_ascii_lowercase()
        .as_str()
    {
        "jpg" | "jpeg" => "image/jpeg",
        "png" => "image/png",
        "webp" => "image/webp",
        "avif" => "image/avif",
        "gif" => "image/gif",
        _ => "application/octet-stream",
    }
}

pub struct Client {
    credentials: Credentials,
    token: Option<(String, Instant)>,
    cdn_host: Option<String>,
    token_lifetime: Duration,
    agent: ureq::Agent,
    api: String,
}

impl Client {
    pub fn new(credentials: Credentials) -> Self {
        Self {
            credentials,
            token: None,
            cdn_host: None,
            token_lifetime: DEFAULT_TOKEN_LIFETIME,
            agent: ureq::AgentBuilder::new().timeout(TIMEOUT).build(),
            api: API.to_string(),
        }
    }

    #[cfg(test)]
    fn with_api(credentials: Credentials, api: String) -> Self {
        Self {
            api,
            ..Self::new(credentials)
        }
    }

    /// A valid token, fetching or refreshing one when needed.
    fn token(&mut self) -> Result<String, Error> {
        if let Some((token, fetched_at)) = &self.token
            && token_is_fresh(*fetched_at, self.token_lifetime)
        {
            return Ok(token.clone());
        }
        self.fetch_token()
    }

    fn fetch_token(&mut self) -> Result<String, Error> {
        #[derive(Deserialize)]
        struct Issued {
            token: String,
            #[serde(rename = "expiresIn", default = "default_expiry")]
            expires_in: u64,
        }
        fn default_expiry() -> u64 {
            1200
        }

        let response = self
            .agent
            .post(&format!("{}/v2/token", self.api))
            .send_json(serde_json::json!({
                "clientId": self.credentials.client_id,
                "clientSecret": self.credentials.client_secret,
            }))
            .map_err(sirv_error("token request"))?;
        let issued: Issued = response.into_json().map_err(|error| Error {
            status: 0,
            message: format!("token body: {error}"),
        })?;
        // Store when the token was fetched, not when it expires: `elapsed()`
        // on a future instant panics, and the old `now + expires_in` value made
        // every refresh check after the first one a time bomb.
        self.token = Some((issued.token.clone(), Instant::now()));
        self.token_lifetime = Duration::from_secs(issued.expires_in);
        Ok(issued.token)
    }

    /// Ask for a token and nothing else: the cheapest proof the keys work.
    pub fn check(&mut self) -> Result<(), Error> {
        self.fetch_token().map(drop)
    }

    /// Run one authenticated call. A token that expired between check and use
    /// is routine: refresh once and try again rather than surfacing a login
    /// error.
    fn authenticated<T>(
        &mut self,
        call: impl Fn(&mut Self) -> Result<T, Error>,
    ) -> Result<T, Error> {
        let mut refreshed = false;
        let mut retry = 0;
        loop {
            match call(self) {
                Err(Error { status: 401, .. }) if !refreshed => {
                    self.token = None;
                    refreshed = true;
                }
                Err(error) if error.retryable() && retry < RETRY_DELAYS.len() => {
                    std::thread::sleep(RETRY_DELAYS[retry]);
                    retry += 1;
                }
                result => return result,
            }
        }
    }

    fn bearer(&mut self) -> Result<String, Error> {
        Ok(format!("Bearer {}", self.token()?))
    }

    /// The account's configured public image host, cached with its credentials.
    pub fn cdn_host(&mut self) -> Result<String, Error> {
        if let Some(host) = &self.cdn_host {
            return Ok(host.clone());
        }
        #[derive(Deserialize)]
        struct Account {
            #[serde(rename = "cdnURL")]
            cdn_url: String,
        }

        let url = format!("{}/v2/account", self.api);
        let account: Account = self.authenticated(|client| {
            let authorization = client.bearer()?;
            let response = client
                .agent
                .get(&url)
                .set("Authorization", &authorization)
                .call()
                .map_err(sirv_error("account"))?;
            response.into_json().map_err(|error| Error {
                status: 0,
                message: format!("account body: {error}"),
            })
        })?;
        validate_cdn_host(&account.cdn_url)?;
        self.cdn_host = Some(account.cdn_url.clone());
        Ok(account.cdn_url)
    }

    /// One directory listing, following `continuation` pages. The API returns
    /// up to 100 entries per page; stopping early would make the sync diff lie.
    pub fn readdir(&mut self, dirname: &str) -> Result<Vec<Node>, Error> {
        self.readdir_cancellable(dirname, None)
            .map(|nodes| nodes.unwrap_or_default())
    }

    pub(crate) fn readdir_cancellable(
        &mut self,
        dirname: &str,
        cancelled: Option<&AtomicBool>,
    ) -> Result<Option<Vec<Node>>, Error> {
        let mut nodes = Vec::new();
        let mut continuation: Option<String> = None;
        let mut seen = HashSet::new();
        let mut pages = 0;
        loop {
            if cancelled.is_some_and(|flag| flag.load(Ordering::Relaxed)) {
                return Ok(None);
            }
            pages += 1;
            if pages > READDIR_PAGE_LIMIT {
                return Err(Error {
                    status: 0,
                    message: format!(
                        "{dirname}: listing did not finish after {READDIR_PAGE_LIMIT} pages"
                    ),
                });
            }
            let mut url = format!(
                "{}/v2/files/readdir?dirname={}",
                self.api,
                encode_path(dirname)
            );
            if let Some(token) = &continuation {
                url.push_str(&format!("&continuation={}", encode_path(token)));
            }
            let listing: Listing = self.authenticated(|client| {
                let authorization = client.bearer()?;
                let response = client
                    .agent
                    .get(&url)
                    .set("Authorization", &authorization)
                    .call()
                    .map_err(sirv_error("readdir"))?;
                response.into_json().map_err(|error| Error {
                    status: 0,
                    message: format!("readdir body: {error}"),
                })
            })?;
            let Listing {
                contents,
                continuation: next,
            } = listing;
            if cancelled.is_some_and(|flag| flag.load(Ordering::Relaxed)) {
                return Ok(None);
            }
            nodes.extend(contents);
            if nodes.len() > LISTING_LIMIT {
                return Err(listing_limit_error());
            }
            match next {
                Some(token) => {
                    if !continuation_advances(&mut seen, &token) {
                        return Err(Error {
                            status: 0,
                            message: format!("{dirname}: readdir repeated a continuation token"),
                        });
                    }
                    continuation = Some(token);
                }
                None => return Ok(Some(nodes)),
            }
        }
    }

    /// One file's bytes.
    pub fn download(&mut self, filename: &str) -> Result<Vec<u8>, Error> {
        let url = format!(
            "{}/v2/files/download?filename={}",
            self.api,
            encode_path(filename)
        );
        self.authenticated(|client| {
            let authorization = client.bearer()?;
            let response = client
                .agent
                .get(&url)
                .set("Authorization", &authorization)
                .timeout(TRANSFER_TIMEOUT)
                .call()
                .map_err(sirv_error("download"))?;
            let bytes =
                read_capped(response.into_reader(), MAX_TRANSFER).map_err(|message| Error {
                    status: 0,
                    message: format!("download body: {message}"),
                })?;
            Ok(bytes)
        })
    }

    /// Whether `filename` is on Sirv now. The listing a push plans from can be
    /// an hour old; asking just before the upload is what keeps "only here" from
    /// overwriting a file somebody else put there since.
    pub fn exists(&mut self, filename: &str) -> Result<bool, Error> {
        let url = format!(
            "{}/v2/files/stat?filename={}",
            self.api,
            encode_path(filename)
        );
        self.authenticated(|client| {
            let authorization = client.bearer()?;
            match client
                .agent
                .get(&url)
                .set("Authorization", &authorization)
                .call()
            {
                Ok(_) => Ok(true),
                Err(ureq::Error::Status(404, _)) => Ok(false),
                Err(error) => Err(sirv_error("stat")(error)),
            }
        })
    }

    /// Put bytes at `filename`, creating nothing on the way — the caller makes
    /// folders explicitly so a partial push is visible in the listing.
    pub fn upload(
        &mut self,
        filename: &str,
        bytes: &[u8],
        content_type: &str,
    ) -> Result<(), Error> {
        if bytes.len() as u64 > MAX_TRANSFER {
            return Err(Error {
                status: 0,
                message: format!("upload is larger than the {MAX_TRANSFER}-byte transfer cap"),
            });
        }
        let url = format!(
            "{}/v2/files/upload?filename={}",
            self.api,
            encode_path(filename)
        );
        self.authenticated(|client| {
            let authorization = client.bearer()?;
            client
                .agent
                .post(&url)
                .set("Authorization", &authorization)
                .set("Content-Type", content_type)
                .timeout(TRANSFER_TIMEOUT)
                .send_bytes(bytes)
                .map_err(sirv_error("upload"))?;
            Ok(())
        })
    }

    /// Create a folder. The one that already exists is success, not conflict:
    /// pushes re-check ancestors for every file.
    pub fn mkdir(&mut self, dirname: &str) -> Result<(), Error> {
        let url = format!(
            "{}/v2/files/mkdir?dirname={}",
            self.api,
            encode_path(dirname)
        );
        self.authenticated(|client| {
            let authorization = client.bearer()?;
            match client
                .agent
                .post(&url)
                .set("Authorization", &authorization)
                .call()
            {
                Ok(_) => Ok(()),
                Err(ureq::Error::Status(409, _)) => Ok(()),
                Err(other) => Err(sirv_error("mkdir")(other)),
            }
        })
    }
}

fn token_is_fresh(fetched_at: Instant, lifetime: Duration) -> bool {
    fetched_at.elapsed() < lifetime.saturating_sub(TOKEN_MARGIN)
}

/// `stage` names the request for a log; a person reads the status, through
/// `Display`, and whatever Sirv added to it.
fn sirv_error(stage: &'static str) -> impl Fn(ureq::Error) -> Error {
    move |error| match error {
        ureq::Error::Status(status, response) => Error {
            status,
            message: server_message(&response.into_string().unwrap_or_default()),
        },
        ureq::Error::Transport(transport) => {
            let cause = transport.to_string();
            eprintln!("sirv {stage}: {cause}");
            let timed_out = cause.to_ascii_lowercase().contains("timed out");
            Error {
                status: 0,
                message: if timed_out {
                    "Sirv took too long to answer. Try again.".into()
                } else {
                    "Couldn’t reach Sirv. Check your internet connection.".into()
                },
            }
        }
    }
}

// ── Credential store ────────────────────────────────────────────────────────

/// Where the Sirv credentials live, resolved like the window settings file.
/// `IMAGEGUIDE_CONFIG_DIR` overrides the platform base, which is how tests keep
/// their hands off a real credentials file — and only in a test build. Neither
/// `settings::path` nor `studio::store_path` ever read the variable, so in a
/// shipped build it did nothing but let whoever set the environment choose where
/// the Sirv secret was read and written, one folder away from the other two.
fn store_path() -> Option<PathBuf> {
    #[cfg(test)]
    if let Ok(dir) = std::env::var("IMAGEGUIDE_CONFIG_DIR") {
        return Some(store_path_in(PathBuf::from(dir)));
    }
    let base = if cfg!(target_os = "windows") {
        std::env::var_os("APPDATA").map(PathBuf::from)
    } else if cfg!(target_os = "macos") {
        std::env::var_os("HOME").map(|home| PathBuf::from(home).join("Library/Application Support"))
    } else {
        std::env::var_os("XDG_CONFIG_HOME")
            .map(PathBuf::from)
            .or_else(|| std::env::var_os("HOME").map(|home| PathBuf::from(home).join(".config")))
    }?;
    Some(store_path_in(&base))
}

fn store_path_in(base: impl AsRef<Path>) -> PathBuf {
    // Still `imageguide`, matching `settings::path`. The rename to Press left
    // on-disk state alone: an orphaned credentials file reads as "Press forgot
    // my keys", which is worse than a folder whose name is out of date.
    base.as_ref().join("imageguide").join("sirv")
}

/// Which Sirv folder each local folder is paired with, one `remote<TAB>local`
/// line per pair. Beside the keys, so a pairing survives a restart the way the
/// keys do; before this, every launch and every folder change asked for the
/// same pairing again.
fn pairings_path() -> Option<PathBuf> {
    Some(store_path()?.parent()?.join("sirv-pairs"))
}

/// Tolerant, like every other file Press reads: a line it cannot use is skipped.
pub fn parse_pairings(text: &str) -> HashMap<PathBuf, String> {
    text.lines()
        .filter_map(|line| line.split_once('\t'))
        .filter(|(remote, local)| remote.starts_with('/') && !local.is_empty())
        .map(|(remote, local)| (PathBuf::from(local), remote.to_string()))
        .collect()
}

pub fn render_pairings(pairings: &HashMap<PathBuf, String>) -> String {
    let mut lines = pairings
        .iter()
        .map(|(local, remote)| format!("{remote}\t{}\n", local.display()))
        .collect::<Vec<_>>();
    lines.sort();
    lines.concat()
}

pub fn load_pairings() -> HashMap<PathBuf, String> {
    pairings_path()
        .and_then(|path| std::fs::read_to_string(path).ok())
        .map(|text| parse_pairings(&text))
        .unwrap_or_default()
}

/// `remember_pairing`, except that a test build never touches the real
/// config: window tests pair and unpair constantly.
pub fn remember_pairing_unless_test(local: &Path, remote: Option<&str>) -> Result<(), String> {
    if cfg!(test) {
        return Ok(());
    }
    remember_pairing(local, remote)
}

/// Remember `local`'s pairing, or forget it with `None`.
pub fn remember_pairing(local: &Path, remote: Option<&str>) -> Result<(), String> {
    let path = pairings_path().ok_or("no config directory on this system")?;
    let mut pairings = load_pairings();
    match remote {
        Some(remote) => pairings.insert(local.to_path_buf(), remote.to_string()),
        None => pairings.remove(local),
    };
    if let Some(parent) = path.parent() {
        std::fs::create_dir_all(parent).map_err(|error| error.to_string())?;
    }
    let mut temporary = path.as_os_str().to_owned();
    temporary.push(format!(".{}.part", std::process::id()));
    let temporary = PathBuf::from(temporary);
    std::fs::write(&temporary, render_pairings(&pairings)).map_err(|error| error.to_string())?;
    replace_file(&temporary, &path).map_err(|error| {
        let _ = std::fs::remove_file(&temporary);
        error.to_string()
    })
}

pub fn load_credentials() -> Option<Credentials> {
    load_credentials_from(store_path().as_deref())
}

pub fn load_credentials_from(path: Option<&Path>) -> Option<Credentials> {
    parse_credentials(&std::fs::read_to_string(path?).ok()?)
}

fn parse_credentials(text: &str) -> Option<Credentials> {
    let mut client_id = None;
    let mut client_secret = None;
    for line in text.lines() {
        let Some((key, value)) = line.split_once('=') else {
            continue;
        };
        match key.trim() {
            "client_id" => client_id = Some(value.trim().to_string()),
            "client_secret" => client_secret = Some(value.trim().to_string()),
            // `studio_key` may still sit in an older file. Ignored, and dropped on
            // the next save: nothing ever read it but the panel that displayed it.
            _ => {}
        }
    }
    Some(Credentials {
        client_id: client_id?,
        client_secret: client_secret?,
    })
}

// The settings panel writes credentials directly; the tests keep the file
// format from drifting.
/// Store the credentials, or say why not.
///
/// A `Result`, because the window used to report "Saved." whether or not anything
/// reached the disk. A full disk or a read-only config directory looked exactly like
/// success, and the user found out the next time the app started with no keys.
pub fn save_credentials(credentials: &Credentials) -> Result<(), String> {
    let Some(path) = store_path() else {
        return Err("no config directory on this system".into());
    };
    write_credentials(&path, credentials)
}

/// Write the credentials file itself.
///
/// Both callers used to go through `save_credentials_at`, which takes a *base* and
/// joins `imageguide/sirv` onto it — but `save_credentials` handed it `store_path()`,
/// which is already the file. Credentials landed in
/// `<base>/imageguide/sirv/imageguide/sirv` while `load_credentials` read
/// `<base>/imageguide/sirv`, so the settings panel said "Saved." and the keys were
/// gone by the next launch. Taking the finished path here leaves nowhere for the two
/// to disagree.
fn write_credentials(path: &Path, credentials: &Credentials) -> Result<(), String> {
    if let Some(parent) = path.parent() {
        std::fs::create_dir_all(parent).map_err(|error| error.to_string())?;
    }
    let body = format!(
        "client_id={}\nclient_secret={}\n",
        credentials.client_id, credentials.client_secret
    );
    let mut temporary = path.as_os_str().to_owned();
    temporary.push(format!(".{}.part", std::process::id()));
    let temporary = PathBuf::from(temporary);
    let _ = std::fs::remove_file(&temporary);
    let result = (|| {
        let mut options = std::fs::OpenOptions::new();
        options.write(true).create_new(true);
        #[cfg(unix)]
        {
            use std::os::unix::fs::OpenOptionsExt;
            options.mode(0o600);
        }
        let mut file = options
            .open(&temporary)
            .map_err(|error| error.to_string())?;
        file.write_all(body.as_bytes())
            .map_err(|error| error.to_string())?;
        file.sync_all().map_err(|error| error.to_string())?;
        owner_only(&temporary)?;
        replace_file(&temporary, path).map_err(|error| error.to_string())
    })();
    if result.is_err() {
        let _ = std::fs::remove_file(&temporary);
    }
    result
}

/// Take the group and world bits off a file. An API secret written under the usual
/// umask is 0644, which every other account on the machine can read.
#[cfg(unix)]
fn owner_only(path: &Path) -> Result<(), String> {
    use std::os::unix::fs::PermissionsExt;
    std::fs::set_permissions(path, std::fs::Permissions::from_mode(0o600))
        .map_err(|error| error.to_string())
}

/// Windows has no Unix mode bits.
#[cfg(not(unix))]
fn owner_only(_path: &Path) -> Result<(), String> {
    Ok(())
}

fn replace_file(from: &Path, to: &Path) -> std::io::Result<()> {
    match std::fs::rename(from, to) {
        Ok(()) => Ok(()),
        #[cfg(windows)]
        Err(error) if to.exists() => {
            std::fs::remove_file(to)?;
            std::fs::rename(from, to).map_err(|_| error)
        }
        Err(error) => Err(error),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn pull_test_dir(name: &str) -> PathBuf {
        let root =
            std::env::temp_dir().join(format!("imageguide-pull-{name}-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&root);
        std::fs::create_dir_all(&root).expect("the pull test directory is created");
        root
    }

    fn read_http_request(stream: &mut std::net::TcpStream) -> String {
        let mut request = Vec::new();
        loop {
            let mut chunk = [0; 2048];
            let read = stream.read(&mut chunk).unwrap();
            assert!(
                read > 0,
                "the test client closed before sending its request"
            );
            request.extend_from_slice(&chunk[..read]);

            let Some(headers_end) = request
                .windows(4)
                .position(|window| window == b"\r\n\r\n")
                .map(|index| index + 4)
            else {
                continue;
            };
            let headers = std::str::from_utf8(&request[..headers_end]).unwrap();
            let content_length = headers
                .lines()
                .find_map(|line| {
                    let (name, value) = line.split_once(':')?;
                    name.eq_ignore_ascii_case("content-length")
                        .then(|| value.trim().parse::<usize>().unwrap())
                })
                .unwrap_or(0);
            if request.len() >= headers_end + content_length {
                return String::from_utf8(request).unwrap();
            }
        }
    }

    #[test]
    fn a_delivery_check_asks_like_a_browser_and_reads_the_headers() {
        let listener = std::net::TcpListener::bind("127.0.0.1:0").unwrap();
        let url = format!(
            "http://{}/shop/a.jpg{}",
            listener.local_addr().unwrap(),
            delivery_query(Some(1600))
        );
        let server = std::thread::spawn(move || {
            use std::io::{BufRead, Write};
            let (stream, _) = listener.accept().unwrap();
            let mut reader = std::io::BufReader::new(stream.try_clone().unwrap());
            let mut head = String::new();
            loop {
                let mut line = String::new();
                reader.read_line(&mut line).unwrap();
                if line == "\r\n" {
                    break;
                }
                head.push_str(&line);
            }
            assert!(head.starts_with("HEAD /shop/a.jpg?s=1600&scale.option=noup "));
            assert!(head.contains("Accept: image/avif,image/webp"));
            assert!(!head.contains("Authorization"));
            let mut stream = stream;
            write!(
                stream,
                "HTTP/1.1 200 OK\r\nContent-Type: image/avif\r\nContent-Length: 47165\r\nConnection: close\r\n\r\n"
            )
            .unwrap();
        });
        assert_eq!(
            delivered(&url).unwrap(),
            Delivered {
                bytes: 47165,
                format: "AVIF".into()
            }
        );
        server.join().unwrap();
        assert_eq!(delivery_query(None), "");
    }

    #[test]
    fn a_401_reads_as_a_credentials_problem_without_the_json() {
        let error = Error {
            status: 401,
            message: server_message(
                "{\n  \"statusCode\": 401,\n  \"error\": \"Unauthorized\",\n  \"message\": \"Unauthorized\"\n}",
            ),
        };
        let shown = error.to_string();

        assert!(shown.starts_with("Sirv didn’t accept this Client ID and secret"));
        assert!(!shown.contains('{') && !shown.contains("statusCode"));
    }

    #[test]
    fn sirvs_own_words_stay_when_they_add_something() {
        assert_eq!(
            server_message(r#"{"statusCode":403,"message":"Read-only API client"}"#),
            "Read-only API client"
        );
        assert_eq!(server_message("<html><body>502</body></html>"), "");
        let error = Error {
            status: 403,
            message: "Read-only API client".into(),
        };
        assert!(
            error
                .to_string()
                .ends_with("Sirv said: Read-only API client")
        );
    }

    #[test]
    fn a_transport_error_keeps_its_message() {
        let error = Error {
            status: 0,
            message: "connection refused".into(),
        };

        assert_eq!(error.to_string(), "connection refused");
    }

    #[test]
    fn a_server_fault_keeps_its_code_and_says_to_wait() {
        let error = Error {
            status: 500,
            message: "upstream unavailable".into(),
        };
        let message = error.to_string();

        // A server fault is Sirv's, and the person's move is to wait.
        assert!(message.contains("500"));
        assert!(message.contains("Try again"));
    }

    #[test]
    fn a_plain_pull_refuses_an_existing_file() {
        let root = pull_test_dir("existing");
        let target = root.join("a.jpg");
        std::fs::write(&target, b"old").unwrap();

        let error = write_pulled(&root, "a.jpg", b"new", false).unwrap_err();
        assert!(error.contains("exists locally"));
        assert_eq!(std::fs::read(&target).unwrap(), b"old");

        let _ = std::fs::remove_dir_all(root);
    }

    #[test]
    fn an_overwrite_pull_replaces_atomically() {
        let root = pull_test_dir("overwrite");
        let target = root.join("a.jpg");
        std::fs::write(&target, b"old").unwrap();

        write_pulled(&root, "a.jpg", b"new", true).unwrap();
        assert_eq!(std::fs::read(&target).unwrap(), b"new");
        assert!(!root.join("a.jpg.part").exists());

        let _ = std::fs::remove_dir_all(root);
    }

    #[test]
    fn a_missing_parent_is_created() {
        let root = pull_test_dir("parents");

        write_pulled(&root, "one/two/a.jpg", b"new", false).unwrap();
        assert_eq!(std::fs::read(root.join("one/two/a.jpg")).unwrap(), b"new");

        let _ = std::fs::remove_dir_all(root);
    }

    #[test]
    fn a_leftover_part_file_is_never_truncated() {
        let root = pull_test_dir("part");
        let part = root.join("a.jpg.part");
        std::fs::write(&part, b"unfinished").unwrap();

        assert!(write_pulled(&root, "a.jpg", b"new", false).is_err());
        assert_eq!(std::fs::read(&part).unwrap(), b"unfinished");

        let _ = std::fs::remove_dir_all(root);
    }

    #[cfg(unix)]
    #[test]
    fn a_symlinked_ancestor_is_refused() {
        use std::os::unix::fs::symlink;

        let root = pull_test_dir("ancestor-symlink");
        let outside =
            root.with_file_name(format!("imageguide-pull-outside-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&outside);
        std::fs::create_dir_all(&outside).unwrap();
        symlink(&outside, root.join("sub")).unwrap();

        let error = write_pulled(&root, "sub/a.jpg", b"new", false).unwrap_err();
        assert!(error.contains("is a symlink"));
        assert!(!outside.join("a.jpg").exists());

        let _ = std::fs::remove_dir_all(root);
        let _ = std::fs::remove_dir_all(outside);
    }

    #[cfg(unix)]
    #[test]
    fn a_symlink_at_the_target_counts_as_existing() {
        use std::os::unix::fs::symlink;

        let root = pull_test_dir("target-symlink");
        let outside = root.join("outside.jpg");
        std::fs::write(&outside, b"old").unwrap();
        symlink(&outside, root.join("a.jpg")).unwrap();

        let error = write_pulled(&root, "a.jpg", b"new", false).unwrap_err();
        assert!(error.contains("exists locally"));
        assert_eq!(std::fs::read(&outside).unwrap(), b"old");

        let _ = std::fs::remove_dir_all(root);
    }

    #[test]
    fn local_sizes_sees_every_file_kind() {
        let root = pull_test_dir("sizes");
        std::fs::write(root.join("a.jpg"), b"one").unwrap();
        std::fs::write(root.join("notes.txt"), b"twos").unwrap();
        std::fs::create_dir_all(root.join("optimized")).unwrap();
        std::fs::write(root.join("optimized/out.webp"), b"three").unwrap();

        assert_eq!(
            local_sizes_for(&root, ["a.jpg", "notes.txt", "optimized/out.webp"],),
            HashMap::from([
                ("a.jpg".to_string(), 3),
                ("notes.txt".to_string(), 4),
                ("optimized/out.webp".to_string(), 5),
            ])
        );

        let _ = std::fs::remove_dir_all(root);
    }

    #[test]
    fn a_body_at_the_cap_passes_and_one_past_it_fails() {
        assert_eq!(
            read_capped(std::io::Cursor::new(b"1234"), 4).unwrap(),
            b"1234"
        );
        assert!(
            read_capped(std::io::Cursor::new(b"12345"), 4)
                .unwrap_err()
                .contains("transfer cap")
        );
    }

    #[test]
    fn a_differing_pull_never_selects_files_the_audit_does_not_show() {
        let remote = vec![
            Node {
                filename: "/d/notes.txt".into(),
                is_directory: false,
                kind: None,
                size: 5,
            },
            Node {
                filename: "/d/a.jpg".into(),
                is_directory: false,
                kind: None,
                size: 5,
            },
        ];
        let local = HashMap::from([("a.jpg".into(), 9)]);

        assert_eq!(pull_plan(&remote, "/d", &local, true), ["a.jpg"]);
    }

    #[test]
    fn paths_escape_for_query_strings() {
        assert_eq!(encode_path("/a b/c.jpg"), "%2Fa%20b%2Fc.jpg");
        assert_eq!(encode_path("/plain/file.webp"), "%2Fplain%2Ffile.webp");
        assert_eq!(encode_path("-_.~"), "-_.~");
    }

    #[test]
    fn a_public_url_preserves_folders_and_encodes_segments() {
        assert_eq!(
            public_url("demo.sirv.com", "/folder with spaces/café#%.png",).unwrap(),
            "https://demo.sirv.com/folder%20with%20spaces/caf%C3%A9%23%25.png"
        );
    }

    #[test]
    fn an_invalid_cdn_host_is_rejected() {
        assert!(public_url("https://demo.sirv.com", "/a.jpg").is_err());
        assert!(public_url("demo.sirv.com/folder", "/a.jpg").is_err());
    }

    #[test]
    fn classification_covers_the_three_states() {
        let node = Node {
            filename: "/d/a.png".into(),
            is_directory: false,
            kind: None,
            size: 100,
        };
        assert_eq!(classify(100, Some(&node)), SyncState::SameSize);
        assert_eq!(classify(101, Some(&node)), SyncState::DifferentSize);
        assert_eq!(classify(100, None), SyncState::OnlyLocal);
    }

    /// Size-only words: equal bytes are `same size`, never `synced`, `same`,
    /// `changed`, or `matching`. No hash or download stands behind them.
    #[test]
    fn sirv_size_evidence_words_name_only_sizes() {
        assert_eq!(sync_state_word(SyncState::SameSize), "same size");
        assert_eq!(sync_state_word(SyncState::DifferentSize), "different size");
        assert_eq!(sync_state_word(SyncState::OnlyLocal), "only here");
        for word in [
            sync_state_word(SyncState::SameSize),
            sync_state_word(SyncState::DifferentSize),
        ] {
            for banned in ["synced", "same", "changed", "matching"] {
                assert_ne!(word, banned, "size evidence must not claim {banned}");
            }
        }
    }

    #[test]
    fn relative_keys_use_forward_slashes() {
        let root = Path::new("/photos");
        assert_eq!(
            relative_key(root, Path::new("/photos/sub/a.jpg")),
            Some("sub/a.jpg".into())
        );
        assert_eq!(relative_key(root, Path::new("/elsewhere/a.jpg")), None);
        assert_eq!(
            paired_key(root, Path::new("/photos/sub/a.jpg"), false),
            None
        );
        assert_eq!(
            paired_key(root, Path::new("/photos/sub/a.jpg"), true).as_deref(),
            Some("sub/a.jpg")
        );
        assert_eq!(
            paired_key(root, Path::new("/photos/a.jpg"), false).as_deref(),
            Some("a.jpg")
        );
        // The folders the Sirv walk skips are left out here too.
        for skipped in [
            "/photos/.cache/a.jpg",
            "/photos/optimized/a.jpg",
            "/photos/sub/press-originals/a.jpg",
        ] {
            assert_eq!(
                paired_key(root, Path::new(skipped), true),
                None,
                "{skipped}"
            );
        }
        assert_eq!(
            paired_key(root, Path::new("/photos/sub/optimized/a.jpg"), true).as_deref(),
            Some("sub/optimized/a.jpg")
        );
    }

    #[test]
    fn remote_names_unpair_against_the_folder() {
        // Relative entries (some account shapes) belong to their listing folder.
        assert_eq!(unpair_remote("/ER", "a.jpg"), Some("a.jpg".into()));
        assert_eq!(unpair_remote("/ER", "sub/b.jpg"), Some("sub/b.jpg".into()));
        // Absolute ones keep the prefix rule.
        assert_eq!(
            unpair_remote("/photos", "/photos/sub/a.jpg"),
            Some("sub/a.jpg".into())
        );
        assert_eq!(
            unpair_remote("/photos/", "/photos/a.jpg"),
            Some("a.jpg".into())
        );
        // Absolute and outside the pair is skipped.
        assert_eq!(unpair_remote("/photos", "/other/a.jpg"), None);
    }

    fn walk_listing(folder: &str) -> Result<Option<Vec<Node>>, Error> {
        let node = |filename: &str, is_directory: bool| Node {
            filename: filename.into(),
            is_directory,
            kind: None,
            size: 10,
        };
        Ok(Some(match folder {
            "/photos" => vec![
                node("direct.jpg", false),
                node("/photos/sub", true),
                node(".processed", true),
                node("optimized", true),
                node("press-originals", true),
                // Listed one level too deep by some accounts: not this folder's.
                node("/photos/sub/stray.jpg", false),
            ],
            "/photos/sub" => vec![node("nested.jpg", false), node("deeper", true)],
            "/photos/sub/deeper" => vec![node("/photos/sub/deeper/leaf.jpg", false)],
            other => panic!("the walk listed {other}, which it should have skipped"),
        }))
    }

    #[test]
    fn a_shallow_pairing_keeps_only_direct_remote_files() {
        let files = walk_remote("/photos", false, walk_listing)
            .unwrap()
            .unwrap();
        assert_eq!(
            files.keys().map(String::as_str).collect::<Vec<_>>(),
            ["direct.jpg"]
        );
    }

    #[test]
    fn a_deep_pairing_walks_subfolders_but_not_hidden_output_or_backup_ones() {
        let files = walk_remote("/photos/", true, walk_listing)
            .unwrap()
            .unwrap();
        let mut keys = files.keys().map(String::as_str).collect::<Vec<_>>();
        keys.sort();
        assert_eq!(
            keys,
            ["direct.jpg", "sub/deeper/leaf.jpg", "sub/nested.jpg"]
        );
        // A cancelled listing ends the walk without a partial answer.
        assert!(
            walk_remote("/photos", true, |_| Ok(None))
                .unwrap()
                .is_none()
        );
    }

    #[test]
    fn a_fresh_token_advances_the_listing() {
        let mut seen = HashSet::new();
        assert!(continuation_advances(&mut seen, "next"));
    }

    #[test]
    fn a_token_cycle_is_refused_even_when_not_adjacent() {
        let mut seen = HashSet::new();
        assert!(continuation_advances(&mut seen, "a"));
        assert!(continuation_advances(&mut seen, "b"));
        assert!(!continuation_advances(&mut seen, "a"));
    }

    #[test]
    fn readdir_parses_the_documented_contents_envelope() {
        // Shape taken from the Sirv API docs (GET /v2/files/readdir): a top-level
        // object whose `contents` array holds the entries. Folders carry
        // `"isDirectory": true`, not a `type` field.
        let listing: Listing = serde_json::from_str(
            r#"{
                "contents": [
                    {"filename": "video", "mtime": "2020-07-17T15:36:52.477Z",
                     "size": 0, "isDirectory": true, "meta": {}},
                    {"filename": "aurora.jpg", "mtime": "2026-02-23T10:22:34.659Z",
                     "contentType": "image/jpeg", "size": 260864,
                     "isDirectory": false,
                     "meta": {"width": 2500, "height": 1667, "duration": 0}}
                ]
            }"#,
        )
        .unwrap();
        assert_eq!(listing.contents.len(), 2);
        assert!(listing.contents[0].is_folder());
        assert_eq!(listing.contents[1].size, 260864);
        assert_eq!(listing.contents[1].filename, "aurora.jpg");
    }

    // Regression: fetch_token stored `Instant::now() + expires_in` in the
    // slot that `token()` reads with `elapsed()`. `elapsed()` on a future
    // instant panics, so one minute into any session with real credentials
    // the next refresh check blew up instead of refreshing.
    #[test]
    fn a_fetched_token_is_stored_as_a_fetch_time_not_an_expiry() {
        let mut client = Client::new(Credentials {
            client_id: "id".into(),
            client_secret: "secret".into(),
        });
        client.token_lifetime = Duration::from_secs(1200);
        client.token = Some(("t".into(), Instant::now()));
        // Just inside the fresh window: no refresh attempted. This only
        // proves it does not panic; the panic was the bug.
        assert_eq!(client.token().unwrap(), "t");
    }

    #[test]
    fn pairings_round_trip_and_skip_junk() {
        let pairings = HashMap::from([
            (PathBuf::from("/home/a/shop"), "/Shop images".to_string()),
            (PathBuf::from("/home/a/with\ttab"), "/x".to_string()),
        ]);
        let text = render_pairings(&pairings);
        let mut back = parse_pairings(&text);
        // A tab inside the local path survives: only the first tab splits.
        assert_eq!(
            back.remove(Path::new("/home/a/with\ttab")).as_deref(),
            Some("/x")
        );
        assert_eq!(
            back.remove(Path::new("/home/a/shop")).as_deref(),
            Some("/Shop images")
        );
        assert!(back.is_empty());
        assert!(parse_pairings("garbage\nno-slash\t/local\n/remote\t\n").is_empty());
    }

    #[test]
    fn an_expired_token_is_refreshed_not_returned() {
        // Fetched 20 minutes ago: outside any fresh window.
        assert!(!token_is_fresh(
            Instant::now() - Duration::from_secs(1200),
            Duration::from_secs(1200)
        ));
    }

    #[test]
    fn a_transient_authenticated_failure_is_retried() {
        let mut client = Client::new(Credentials {
            client_id: "id".into(),
            client_secret: "secret".into(),
        });
        let calls = std::cell::Cell::new(0);
        let result = client.authenticated(|_| {
            calls.set(calls.get() + 1);
            if calls.get() == 1 {
                Err(Error {
                    status: 500,
                    message: "temporary".into(),
                })
            } else {
                Ok("done")
            }
        });

        assert_eq!(result.unwrap(), "done");
        assert_eq!(calls.get(), 2);
    }

    #[cfg(unix)]
    #[test]
    fn a_symlink_on_the_way_is_not_a_plain_file() {
        let root = tempfile::tempdir().unwrap();
        let secret = tempfile::tempdir().unwrap();
        std::fs::write(secret.path().join("key"), b"secret").unwrap();
        std::fs::write(root.path().join("plain.txt"), b"ok").unwrap();
        std::os::unix::fs::symlink(secret.path().join("key"), root.path().join("link.txt"))
            .unwrap();
        std::os::unix::fs::symlink(secret.path(), root.path().join("sub")).unwrap();
        assert!(plain_file_below(root.path(), "plain.txt"));
        assert!(!plain_file_below(root.path(), "link.txt"));
        assert!(!plain_file_below(root.path(), "sub/key"));
        assert!(!plain_file_below(root.path(), "missing.txt"));
    }

    #[test]
    fn exists_reads_a_404_as_absent_and_a_200_as_present() {
        let listener = std::net::TcpListener::bind("127.0.0.1:0").unwrap();
        let address = listener.local_addr().unwrap();
        let server = std::thread::spawn(move || {
            for (status, body) in [
                ("200 OK", r#"{"token":"local","expiresIn":1200}"#),
                ("200 OK", r#"{"size":5}"#),
                ("404 Not Found", r#"{"statusCode":404}"#),
            ] {
                let (mut stream, _) = listener.accept().unwrap();
                read_http_request(&mut stream);
                write!(
                    stream,
                    "HTTP/1.1 {status}\r\nContent-Type: application/json\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{body}",
                    body.len()
                )
                .unwrap();
                stream.flush().unwrap();
            }
        });
        let mut client = Client::with_api(
            Credentials {
                client_id: "id".into(),
                client_secret: "secret".into(),
            },
            format!("http://{address}"),
        );

        assert!(client.exists("/a/there.jpg").unwrap());
        assert!(!client.exists("/a/gone.jpg").unwrap());
        server.join().unwrap();
    }

    #[test]
    fn an_account_cdn_host_comes_from_the_authenticated_endpoint() {
        let listener = std::net::TcpListener::bind("127.0.0.1:0").unwrap();
        let address = listener.local_addr().unwrap();
        let server = std::thread::spawn(move || {
            for (expected, body) in [
                ("POST /v2/token ", r#"{"token":"local","expiresIn":1200}"#),
                ("GET /v2/account ", r#"{"cdnURL":"demo.sirv.com"}"#),
            ] {
                let (mut stream, _) = listener.accept().unwrap();
                let request = read_http_request(&mut stream);
                assert!(
                    request.starts_with(expected),
                    "unexpected request: {request}"
                );
                if expected.starts_with("GET") {
                    assert!(request.contains("Authorization: Bearer local"));
                }
                write!(
                    stream,
                    "HTTP/1.1 200 OK\r\nContent-Type: application/json\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{body}",
                    body.len()
                )
                .unwrap();
                stream.flush().unwrap();
            }
        });
        let mut client = Client::with_api(
            Credentials {
                client_id: "id".into(),
                client_secret: "secret".into(),
            },
            format!("http://{address}"),
        );

        assert_eq!(client.cdn_host().unwrap(), "demo.sirv.com");
        assert_eq!(client.cdn_host().unwrap(), "demo.sirv.com");
        server.join().unwrap();
    }

    #[test]
    fn a_cancelled_listing_makes_no_request() {
        let mut client = Client::with_api(
            Credentials {
                client_id: "id".into(),
                client_secret: "secret".into(),
            },
            "http://127.0.0.1:1".into(),
        );
        let cancelled = AtomicBool::new(true);

        assert!(
            client
                .readdir_cancellable("/photos", Some(&cancelled))
                .unwrap()
                .is_none()
        );
    }

    #[test]
    fn a_readdir_page_keeps_its_continuation_token() {
        let listing: Listing = serde_json::from_str(
            r#"{"contents": [{"filename": "a.jpg", "size": 1}], "continuation": "next-page-token"}"#,
        )
        .unwrap();
        assert_eq!(listing.continuation.as_deref(), Some("next-page-token"));
    }

    #[test]
    fn credentials_round_trip_through_the_store() {
        // The resolver is environment-shaped; the round trip runs against a
        // temp base so a developer's real credentials file is never touched.
        let base =
            std::env::temp_dir().join(format!("imageguide-sirv-test-{}", std::process::id()));
        let path = store_path_in(&base);

        assert_eq!(load_credentials_from(Some(&path)), None);
        let credentials = Credentials {
            client_id: "an id with spaces".into(),
            client_secret: "s3cret/with:colons".into(),
        };
        write_credentials(&path, &credentials).expect("the store is writable");
        assert_eq!(load_credentials_from(Some(&path)), Some(credentials));

        // The file holds an API secret, so it must not be readable by anyone else on
        // the machine. The usual umask would have written it 0644.
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            let mode = std::fs::metadata(&path).unwrap().permissions().mode();
            assert_eq!(mode & 0o077, 0, "group or world can read {path:?}");
        }
        let _ = std::fs::remove_dir_all(&base);
    }

    /// The pair the old test never put together. `save_credentials_at` and
    /// `load_credentials_from` agreed with each other while `save_credentials` and
    /// `load_credentials` — the two the window actually calls — did not: the save
    /// joined `imageguide/sirv` on twice and the load looked at the shorter path, so
    /// the panel said "Saved." and the keys were gone by the next launch.
    ///
    /// This is the only test that touches `IMAGEGUIDE_CONFIG_DIR`. Keep it that way:
    /// the variable is process-wide and the test harness runs threads.
    #[test]
    fn what_the_window_saves_is_what_the_window_loads() {
        let dir = std::env::temp_dir().join(format!("imageguide-store-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        // SAFETY: no other test reads or writes this variable, and nothing else in the
        // process is reading credentials while it runs.
        unsafe { std::env::set_var("IMAGEGUIDE_CONFIG_DIR", &dir) };

        let credentials = Credentials {
            client_id: "id".into(),
            client_secret: "secret".into(),
        };
        save_credentials(&credentials).expect("a fresh temp directory is writable");

        assert_eq!(
            load_credentials(),
            Some(credentials),
            "saved credentials must come back; they were landing one folder deeper"
        );
        assert!(
            store_path().is_some_and(|path| path.is_file()),
            "the path the window reports is the file that exists"
        );

        unsafe { std::env::remove_var("IMAGEGUIDE_CONFIG_DIR") };
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn a_push_names_each_folder_once_and_parents_before_children() {
        let folders = push_folders([
            "top.jpg",
            "a/one.jpg",
            "a/two.jpg",
            "a/deep/three.jpg",
            "b/four.jpg",
        ]);

        assert_eq!(folders, ["a", "b", "a/deep"], "no folder is created twice");
        // Order matters upstream: `a` has to exist before `a/deep`.
        let depth: Vec<usize> = folders.iter().map(|f| f.matches('/').count()).collect();
        assert!(depth.windows(2).all(|pair| pair[0] <= pair[1]));
        assert!(
            push_folders(["flat.jpg"]).is_empty(),
            "a flat folder needs none"
        );
    }

    /// A pull turns a name the server chose into a local path. Absolute keys and `..`
    /// must never reach `root.join`.
    #[test]
    fn a_remote_key_that_escapes_the_folder_is_not_pulled() {
        assert!(safe_key("a.jpg"));
        assert!(safe_key("sub/deep/a.jpg"));
        assert!(!safe_key(""));
        assert!(!safe_key("/etc/cron.d/x"), "Path::join would take the lot");
        assert!(!safe_key("../../.bashrc"));
        assert!(!safe_key("sub/../../escape.jpg"));
        assert!(!safe_key("./a.jpg"), "a bare dot is not a name either");

        let remote = vec![
            Node {
                filename: "/d/ok.jpg".into(),
                is_directory: false,
                kind: None,
                size: 1,
            },
            Node {
                filename: "/d/../../.bashrc".into(),
                is_directory: false,
                kind: None,
                size: 1,
            },
        ];
        assert_eq!(
            pull_plan(&remote, "/d", &HashMap::new(), false),
            vec!["ok.jpg".to_string()],
            "the escaping key is left out of the plan entirely"
        );
    }

    #[test]
    fn pull_plan_lists_only_keys_the_local_side_lacks() {
        let remote = vec![
            Node {
                filename: "/d/a.jpg".into(),
                is_directory: false,
                kind: None,
                size: 1,
            },
            Node {
                filename: "/d/b.jpg".into(),
                is_directory: false,
                kind: None,
                size: 2,
            },
            Node {
                filename: "/d/sub/c.jpg".into(),
                is_directory: false,
                kind: None,
                size: 3,
            },
        ];
        let local = HashMap::from([("a.jpg".into(), 1), ("b.jpg".into(), 2)]);
        assert_eq!(
            pull_plan(&remote, "/d", &local, false),
            vec!["sub/c.jpg".to_string()]
        );
    }

    #[test]
    fn changed_keys_only_when_asked() {
        let remote = vec![
            Node {
                filename: "/d/missing.jpg".into(),
                is_directory: false,
                kind: None,
                size: 1,
            },
            Node {
                filename: "/d/same.jpg".into(),
                is_directory: false,
                kind: None,
                size: 2,
            },
            Node {
                filename: "/d/changed.jpg".into(),
                is_directory: false,
                kind: None,
                size: 3,
            },
        ];
        let local = HashMap::from([("same.jpg".into(), 2), ("changed.jpg".into(), 4)]);

        assert_eq!(
            pull_plan(&remote, "/d", &local, false),
            ["missing.jpg".to_string()]
        );
        assert_eq!(
            pull_plan(&remote, "/d", &local, true),
            ["changed.jpg".to_string()]
        );
    }

    #[test]
    fn ancestor_dirs_walk_from_the_top() {
        assert_eq!(ancestor_dirs("a.jpg"), Vec::<String>::new());
        assert_eq!(ancestor_dirs("sub/a.jpg"), vec!["sub".to_string()]);
        assert_eq!(
            ancestor_dirs("sub/deep/a.jpg"),
            vec!["sub".to_string(), "sub/deep".to_string()]
        );
    }

    #[test]
    fn content_types_follow_the_extension() {
        assert_eq!(content_type("a.JPG"), "image/jpeg");
        assert_eq!(content_type("b.png"), "image/png");
        assert_eq!(content_type("c.webp"), "image/webp");
        assert_eq!(content_type("d.avif"), "image/avif");
        assert_eq!(content_type("e.tif"), "application/octet-stream");
    }
}
