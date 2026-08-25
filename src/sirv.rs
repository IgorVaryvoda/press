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
/// A walk that finds more files than this is treated as an error rather than
/// listed forever.
const WALK_LIMIT: usize = 20_000;
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

/// An upstream failure with its status and body kept intact. "Sirv said 403"
/// is debuggable; "request failed" is not.
#[derive(Debug)]
pub struct Error {
    pub status: u16,
    pub message: String,
}

impl std::fmt::Display for Error {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self.status {
            0 => write!(f, "{}", self.message),
            401 | 403 => write!(
                f,
                "Sirv rejected the credentials ({}): {}",
                self.status, self.message
            ),
            404 => write!(f, "not found on Sirv ({}): {}", self.status, self.message),
            429 => write!(
                f,
                "Sirv is rate limiting this account ({}): {}",
                self.status, self.message
            ),
            status => write!(f, "Sirv error {status}: {}", self.message),
        }
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

/// How a local file stands against the paired remote folder.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum SyncState {
    /// No file with this relative path on Sirv.
    OnlyLocal,
    /// Same relative path and byte size.
    Same,
    /// Same path, different size.
    Changed,
}

/// Classify one local file against the remote listing. Size is the only
/// comparator on purpose: local and server clocks disagree often enough that
/// mtime comparison would report lies as changes.
pub fn classify(local_size: u64, remote: Option<&Node>) -> SyncState {
    match remote {
        None => SyncState::OnlyLocal,
        Some(node) if node.size == local_size => SyncState::Same,
        Some(_) => SyncState::Changed,
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

/// Join a readdir entry name onto the folder that was listed. Some account
/// shapes return absolute names, others names relative to the listed folder;
/// recursion and diff keys both need the absolute form.
pub fn join_listing_name(current: &str, name: &str) -> String {
    let name = name.trim_end_matches('/');
    if name.starts_with('/') {
        name.to_string()
    } else {
        format!("{}/{name}", current.trim_end_matches('/'))
    }
}

/// True when this continuation token has not been seen before in this
/// listing. A repeated token — adjacent or in a cycle — means the server is
/// looping, and following it would list forever.
pub fn continuation_advances(seen: &mut HashSet<String>, token: &str) -> bool {
    seen.insert(token.to_string())
}

fn walk_limit_error() -> Error {
    Error {
        status: 0,
        message: format!("holds more than {WALK_LIMIT} entries; open or sync a smaller folder"),
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
                differing && classify(*local_size, Some(node)) == SyncState::Changed
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
    token_lifetime: Duration,
    agent: ureq::Agent,
    api: String,
}

impl Client {
    pub fn new(credentials: Credentials) -> Self {
        Self {
            credentials,
            token: None,
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

    /// One directory listing, following `continuation` pages. The API returns
    /// up to 100 entries per page; stopping early would make the sync diff lie.
    pub fn readdir(&mut self, dirname: &str) -> Result<Vec<Node>, Error> {
        self.readdir_cancellable(dirname, None)
            .map(|nodes| nodes.unwrap_or_default())
    }

    fn readdir_cancellable(
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
            if nodes.len() > WALK_LIMIT {
                return Err(walk_limit_error());
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

    /// Every file below `dir`, flattened, folders walked depth-first. Bounded:
    /// a tree that exceeds `WALK_LIMIT` files is an error, not an endless walk.
    ///
    /// readdir's `filename` fields are relative to the listed folder (the API
    /// docs' own example lists `/REST%20API%20Examples` and gets back
    /// `"aurora.jpg"`, not the absolute path). Joining each folder entry onto
    /// the folder being listed is what keeps the recursion on absolute paths;
    /// pushing the bare name made the next call ask Sirv for
    /// `dirname=subfolder` and every nested listing died with a 400.
    pub fn walk(&mut self, dir: &str, cancelled: &AtomicBool) -> Result<Option<Vec<Node>>, Error> {
        let root = format!("/{}", dir.trim().trim_start_matches('/'));
        let root = root.trim_end_matches('/').to_string();
        if root.is_empty() {
            return Err(Error {
                status: 0,
                message: "pairing folder is empty".into(),
            });
        }
        let mut all = Vec::new();
        let mut visited = HashSet::from([root.clone()]);
        let mut stack = vec![root];
        while let Some(current) = stack.pop() {
            if cancelled.load(Ordering::Relaxed) {
                return Ok(None);
            }
            // Name the page that failed. The pairing root in the notice is
            // useless when a subfolder three levels down rejected the call.
            let Some(nodes) = self
                .readdir_cancellable(&current, Some(cancelled))
                .map_err(|mut error| {
                    if !error.message.contains(&current) {
                        error.message = format!("{current}: {}", error.message);
                    }
                    error
                })?
            else {
                return Ok(None);
            };
            for mut node in nodes {
                let name = node.filename.trim_end_matches('/');
                if matches!(name, "" | "." | "..") {
                    continue;
                }
                if node.is_folder() {
                    let child = join_listing_name(&current, name);
                    if visited.insert(child.clone()) {
                        if visited.len() > WALK_LIMIT {
                            return Err(walk_limit_error());
                        }
                        stack.push(child);
                    }
                } else {
                    node.filename = join_listing_name(&current, name);
                    all.push(node);
                }
            }
            if all.len() > WALK_LIMIT {
                return Err(walk_limit_error());
            }
        }
        Ok(Some(all))
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

fn sirv_error(stage: &'static str) -> impl Fn(ureq::Error) -> Error {
    move |error| match error {
        ureq::Error::Status(status, response) => {
            // Bodies arrive pretty-printed; the first line alone would be a
            // bare "{". Keep everything, capped.
            let mut message = response.into_string().unwrap_or_default();
            message = message.trim().to_string();
            if message.chars().count() > 200 {
                message = message.chars().take(200).collect::<String>() + "…";
            }
            Error {
                status,
                message: if message.is_empty() {
                    stage.to_string()
                } else {
                    format!("{stage}: {message}")
                },
            }
        }
        ureq::Error::Transport(transport) => Error {
            status: 0,
            message: format!("{stage}: {transport}"),
        },
    }
}

// ── Credential store ────────────────────────────────────────────────────────

/// The file a user edits to add credentials, named in errors.
pub fn credentials_path() -> Option<PathBuf> {
    store_path()
}

/// Where the Sirv credentials live, resolved like the window settings file.
/// `IMAGEGUIDE_CONFIG_DIR` overrides the platform base, which is how tests
/// keep their hands off a real credentials file.
fn store_path() -> Option<PathBuf> {
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

    #[test]
    fn a_401_reads_as_a_credentials_problem() {
        let error = Error {
            status: 401,
            message: "token: {...}".into(),
        };

        assert!(
            error
                .to_string()
                .starts_with("Sirv rejected the credentials")
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
    fn an_unmapped_status_keeps_its_code_and_detail() {
        let error = Error {
            status: 500,
            message: "upstream unavailable".into(),
        };
        let message = error.to_string();

        assert!(message.contains("500"));
        assert!(message.contains("upstream unavailable"));
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
    fn classification_covers_the_three_states() {
        let node = Node {
            filename: "/d/a.png".into(),
            is_directory: false,
            kind: None,
            size: 100,
        };
        assert_eq!(classify(100, Some(&node)), SyncState::Same);
        assert_eq!(classify(101, Some(&node)), SyncState::Changed);
        assert_eq!(classify(100, None), SyncState::OnlyLocal);
    }

    #[test]
    fn relative_keys_use_forward_slashes() {
        let root = Path::new("/photos");
        assert_eq!(
            relative_key(root, Path::new("/photos/sub/a.jpg")),
            Some("sub/a.jpg".into())
        );
        assert_eq!(relative_key(root, Path::new("/elsewhere/a.jpg")), None);
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

    #[test]
    fn a_relative_file_name_joins_the_folder_being_listed() {
        let filename = join_listing_name("/photos/sub", "c.jpg");
        assert_eq!(filename, "/photos/sub/c.jpg");
        assert_eq!(
            unpair_remote("/photos", &filename),
            Some("sub/c.jpg".into())
        );
    }

    #[test]
    fn an_absolute_file_name_passes_through() {
        assert_eq!(
            join_listing_name("/photos/sub", "/photos/sub/c.jpg"),
            "/photos/sub/c.jpg"
        );
    }

    #[test]
    fn a_trailing_slash_on_the_folder_does_not_double() {
        assert_eq!(join_listing_name("/photos/", "sub"), "/photos/sub");
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

    #[test]
    fn the_test_client_uses_its_injected_api_url() {
        let listener = std::net::TcpListener::bind("127.0.0.1:0").unwrap();
        let address = listener.local_addr().unwrap();
        let server = std::thread::spawn(move || {
            let (mut stream, _) = listener.accept().unwrap();
            let mut request = [0; 2048];
            let read = stream.read(&mut request).unwrap();
            assert!(String::from_utf8_lossy(&request[..read]).starts_with("POST /v2/token "));
            let body = r#"{"token":"local","expiresIn":1200}"#;
            write!(
                stream,
                "HTTP/1.1 200 OK\r\nContent-Type: application/json\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{body}",
                body.len()
            )
            .unwrap();
        });
        let mut client = Client::with_api(
            Credentials {
                client_id: "id".into(),
                client_secret: "secret".into(),
            },
            format!("http://{address}"),
        );

        assert_eq!(client.fetch_token().unwrap(), "local");
        server.join().unwrap();
    }

    #[test]
    fn a_cancelled_walk_makes_no_request() {
        let mut client = Client::with_api(
            Credentials {
                client_id: "id".into(),
                client_secret: "secret".into(),
            },
            "http://127.0.0.1:1".into(),
        );
        let cancelled = AtomicBool::new(true);

        assert!(client.walk("/photos", &cancelled).unwrap().is_none());
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
        assert_eq!(
            load_credentials_from(Some(&path)),
            Some(credentials.clone())
        );

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
            credentials_path().is_some_and(|path| path.is_file()),
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
