//! Sirv actions: browsing, pairing, and both transfer directions.

use super::*;

const FAILURE_EXAMPLES: usize = 3;
const CONSECUTIVE_REMOTE_FAILURES: usize = 3;

/// How much one failure counts toward stopping the run. Keys Sirv refuses
/// (401, 403) are refused for every file, so one stops it; overload and
/// network trouble stop it after a few in a row; a problem with one local file
/// is that file's alone. Revoked keys used to fail two thousand files one by one.
fn failure_weight(error: &sirv::Error) -> usize {
    match error.status {
        401 | 403 => CONSECUTIVE_REMOTE_FAILURES,
        _ if error.retryable() => 1,
        _ => 0,
    }
}

pub(super) fn remember_failure(count: &mut usize, examples: &mut Vec<String>, message: String) {
    *count += 1;
    if examples.len() < FAILURE_EXAMPLES {
        examples.push(message);
    }
}

pub(super) fn transfer_failure(job: &SirvJob) -> Option<(&'static str, String)> {
    if job.failed == 0 && job.failures.is_empty() {
        return None;
    }
    let title = match job.kind {
        SirvJobKind::Pull | SirvJobKind::PullChanged => "Sirv download incomplete",
        SirvJobKind::Push | SirvJobKind::PushChanged => "Sirv upload incomplete",
        SirvJobKind::Publish => "Sirv publish incomplete",
    };
    let rest = job.failed.saturating_sub(job.failures.len());
    let examples = job.failures.join(", ");
    // A run that stopped early never tried the rest. "3 of 20 failed" alone
    // read as seventeen copied.
    let untried = job.total.saturating_sub(job.done);
    let head = if job.failed == 0 {
        examples
    } else {
        format!(
            "{} of {} failed: {}{}",
            job.failed,
            job.total,
            examples,
            if rest == 0 {
                String::new()
            } else {
                format!(" and {rest} more")
            }
        )
    };
    Some((
        title,
        if untried == 0 {
            head
        } else {
            format!("{head}. {untried} not tried")
        },
    ))
}

/// The toast a clean transfer ends with. The progress line goes away when the
/// job is done, so this is where "it worked" is said, once.
pub(super) fn transfer_success(job: &SirvJob) -> Option<(&'static str, String)> {
    if !job.finished || job.failed > 0 || job.done == 0 {
        return None;
    }
    let files = if job.done == 1 {
        "1 file".to_string()
    } else {
        format!("{} files", job.done)
    };
    if job.done < job.total {
        return Some(if job.stopped_by_user {
            (
                "Sirv transfer stopped",
                format!("{files} of {} copied before Stop.", job.total),
            )
        } else {
            (
                "Sirv transfer interrupted",
                format!(
                    "{files} of {} copied before the folder or the keys changed.",
                    job.total
                ),
            )
        });
    }
    Some(match job.kind {
        SirvJobKind::Pull => (
            "Downloaded from Sirv",
            format!("{files} copied to this computer."),
        ),
        SirvJobKind::PullChanged => ("Replaced from Sirv", format!("{files} replaced here.")),
        SirvJobKind::Push => ("Uploaded to Sirv", format!("{files} copied to Sirv.")),
        SirvJobKind::PushChanged => ("Replaced on Sirv", format!("{files} replaced on Sirv.")),
        SirvJobKind::Publish => ("Published to Sirv", format!("{files} published.")),
    })
}

/// True when a finished walk still describes the current world: same
/// dataset, same pairing as when it started. A walk that outlives either
/// must land nowhere — installing folder A's listing under folder B's
/// pairing arms a full-folder push at the wrong remote directory.
pub(super) fn walk_landing_applies(
    dataset_then: u64,
    dataset_now: u64,
    pairing_then: u64,
    pairing_now: u64,
) -> bool {
    dataset_then == dataset_now && pairing_then == pairing_now
}

pub(super) fn browser_landing_applies(
    session_then: u64,
    session_now: u64,
    request_then: u64,
    request_now: u64,
    path_then: &str,
    path_now: &str,
) -> bool {
    session_then == session_now && request_then == request_now && path_then == path_now
}

impl Audit {
    /// Open the remote-folder browser. Credentials come from the Sirv store; a
    /// missing store routes directly to the existing settings form.
    pub(super) fn open_sirv_browser(&mut self, cx: &mut Context<Self>) {
        if self.batch_folders.is_some() {
            return;
        }
        self.clear_error("sirv-browser", cx);
        self.sirv_confirm = None;
        self.sirv_browser_generation = self.sirv_browser_generation.wrapping_add(1);
        let session = self.sirv_browser_generation;
        // A live pairing already holds a warm client; reuse it so the browser
        // and later pushes share one token cache.
        let client = self
            .sirv_pairing
            .as_ref()
            .map(|pairing| pairing.client.clone());
        let client = match client {
            Some(client) => client,
            None => {
                let Some(credentials) = sirv::load_credentials() else {
                    self.sirv_browser = Some(SirvBrowser {
                        // Never used on this path: the listing is already an error.
                        client: Arc::new(parking_lot::Mutex::new(sirv::Client::new(
                            sirv::Credentials {
                                client_id: String::new(),
                                client_secret: String::new(),
                            },
                        ))),
                        path: "/".into(),
                        needs_credentials: true,
                        // Not an error: nobody has set this up yet. Phrased as
                        // a failure, with a Retry beside it, the first click on
                        // Sirv reported a fault the user had not caused and
                        // could not clear by retrying.
                        nodes: Some(Err(
                            "Press has no Sirv keys yet. Add them to browse your Sirv folders."
                                .into(),
                        )),
                        generation: 0,
                        session,
                        focused: false,
                        focus: cx.focus_handle(),
                    });
                    cx.notify();
                    return;
                };
                Arc::new(parking_lot::Mutex::new(sirv::Client::new(credentials)))
            }
        };
        let mut browser = SirvBrowser {
            client,
            path: "/".into(),
            needs_credentials: false,
            nodes: None,
            generation: 0,
            session,
            focused: false,
            focus: cx.focus_handle(),
        };
        if let Some(pairing) = &self.sirv_pairing {
            browser.path = pairing.dir.clone();
        }
        self.sirv_browser = Some(browser);
        let Some(state) = self.sirv_browser.as_mut() else {
            return;
        };
        Self::browse_sirv_path(state, cx);
        cx.notify();
    }

    /// Fetch the listing for the browser's current path in the background.
    ///
    /// Clicking into two folders in quick succession used to leave whichever listing
    /// answered last on screen, under whichever path the header showed. The generation
    /// makes a superseded listing land nowhere.
    pub(super) fn browse_sirv_path(browser: &mut SirvBrowser, cx: &mut Context<Self>) {
        browser.generation = browser.generation.wrapping_add(1);
        browser.focused = false;
        let request = browser.generation;
        let session = browser.session;
        browser.nodes = None;
        let client = browser.client.clone();
        let path = browser.path.clone();
        cx.spawn(async move |this, cx| {
            let requested_path = path.clone();
            let result = cx
                .background_executor()
                .spawn(async move {
                    client
                        .lock()
                        .readdir(&requested_path)
                        .map_err(|error| error.to_string())
                })
                .await;
            this.update(cx, |audit, cx| {
                let mut failure = None;
                let mut landed = false;
                if let Some(browser) = audit.sirv_browser.as_mut()
                    && browser_landing_applies(
                        session,
                        browser.session,
                        request,
                        browser.generation,
                        &path,
                        &browser.path,
                    )
                {
                    landed = true;
                    failure = result.as_ref().err().cloned();
                    browser.nodes = Some(result);
                    cx.notify();
                }
                if let Some(message) = failure {
                    audit.notify_error("sirv-browser", "Couldn’t list Sirv folder", message, cx);
                } else if landed {
                    audit.clear_error("sirv-browser", cx);
                }
            })
        })
        .detach();
    }

    pub(super) fn close_settings(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        self.clear_error("sirv-settings", cx);
        self.settings_panel = None;
        Self::restore_audit_focus(window, cx);
    }

    pub(super) fn close_sirv_browser(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        self.clear_error("sirv-browser", cx);
        self.clear_sirv_browser_filter(window, cx);
        self.sirv_browser = None;
        self.sirv_confirm = None;
        Self::restore_audit_focus(window, cx);
    }

    pub(super) fn restore_audit_focus(window: &mut Window, cx: &mut Context<Self>) {
        cx.defer_in(window, |audit, window, cx| window.focus(&audit.focus, cx));
        cx.notify();
    }

    /// Enter a folder of the listing.
    pub(super) fn descend_sirv(
        &mut self,
        name: String,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        self.clear_error("sirv-browser", cx);
        self.clear_sirv_browser_filter(window, cx);
        let Some(browser) = self.sirv_browser.as_mut() else {
            return;
        };
        if !browser.path.ends_with('/') {
            browser.path.push('/');
        }
        browser.path.push_str(&name);
        Self::browse_sirv_path(browser, cx);
        cx.notify();
    }

    /// A filter typed to find one folder means nothing inside it.
    pub(super) fn clear_sirv_browser_filter(
        &mut self,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        self.sirv_browser_filter_input
            .update(cx, |input, cx| input.set_value("", window, cx));
        self.sirv_browser_filter.clear();
    }

    /// The folder Enter in the browser's filter opens: the first visible
    /// folder whose name holds the typed text.
    pub(super) fn sirv_browser_match(&self) -> Option<String> {
        let needle = self.sirv_browser_filter.trim().to_lowercase();
        // Enter on an empty box is not a choice of folder.
        if needle.is_empty() {
            return None;
        }
        let Some(Ok(nodes)) = self.sirv_browser.as_ref()?.nodes.as_ref() else {
            return None;
        };
        nodes
            .iter()
            .filter(|node| node.is_folder())
            .filter_map(|node| node.filename.rsplit('/').next())
            .filter(|name| !name.starts_with('.'))
            .find(|name| name.to_lowercase().contains(&needle))
            .map(str::to_string)
    }

    /// Go up one folder. The root has no parent, so the button only exists
    /// below it.
    pub(super) fn ascend_sirv(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        self.clear_error("sirv-browser", cx);
        self.clear_sirv_browser_filter(window, cx);
        let Some(browser) = self.sirv_browser.as_mut() else {
            return;
        };
        let trimmed = browser.path.trim_end_matches('/').to_string();
        let Some((parent, _)) = trimmed.rsplit_once('/') else {
            return;
        };
        browser.path = if parent.is_empty() {
            "/".into()
        } else {
            parent.to_string()
        };
        Self::browse_sirv_path(browser, cx);
        cx.notify();
    }

    /// Pair the browsed folder, then list its direct files in the background.
    /// The pairing exists immediately (the header names it); its diff arrives
    /// when the listing lands.
    pub(super) fn pair_sirv(&mut self, cx: &mut Context<Self>) {
        if self.batch_folders.is_some() || self.scan_blocks_delivery() {
            return;
        }
        let (client, dir) = {
            let Some(browser) = self.sirv_browser.as_ref() else {
                return;
            };
            (
                browser.client.clone(),
                browser.path.trim_end_matches('/').to_string(),
            )
        };
        // The account root pairs to "", which `walk` rejects; a pairing
        // whose header reads "Unpair " with no name is worse than no
        // pairing. The browser's button says why; this guard holds even
        // if a future caller forgets to.
        if dir.is_empty() {
            return;
        }
        self.clear_error("sirv-browser", cx);
        self.sirv_browser = None;
        if let Err(message) = sirv::remember_pairing_unless_test(&self.root, Some(&dir)) {
            self.notify_error("sirv-listing", "Couldn’t remember the pairing", message, cx);
        }
        // Pairing is for comparing: open on the comparison, with the room it needs.
        self.sirv_split = true;
        self.sidebar_open = false;
        self.install_pairing(client, dir, cx);
    }

    /// Put `root`'s remembered pairing back when its folder opens. Quiet when
    /// there is none or no keys to use it with: nobody asked for anything yet.
    pub(super) fn restore_sirv_pairing(&mut self, cx: &mut Context<Self>) {
        if cfg!(test)
            || self.sirv_pairing.is_some()
            || self.batch_folders.is_some()
            || self.batch_size.is_some()
            || self.single_file
            || self.root.as_os_str().is_empty()
        {
            return;
        }
        let Some(dir) = sirv::load_pairings().remove(&self.root) else {
            return;
        };
        let Some(credentials) = sirv::load_credentials() else {
            return;
        };
        let client = Arc::new(parking_lot::Mutex::new(sirv::Client::new(credentials)));
        self.install_pairing(client, dir, cx);
    }

    fn install_pairing(
        &mut self,
        client: Arc<parking_lot::Mutex<sirv::Client>>,
        dir: String,
        cx: &mut Context<Self>,
    ) {
        // A transfer aimed at the old pairing must not outlive it, same rule as unpair.
        self.cancel_sirv_transfer();
        self.forget_sirv_delivery();
        self.sirv_pairing_generation = self.sirv_pairing_generation.wrapping_add(1);
        self.sirv_pairing = Some(SirvPairing {
            dir,
            deep: false,
            files: Listing::Walking,
            cdn_host: CdnHost::Loading,
            client,
        });
        self.sirv_counts = None;
        self.sirv_scope = None;
        self.sirv_remote_only.clear();
        self.sirv_rows.clear();
        self.sirv_selected.clear();
        cx.notify();
        self.walk_sirv_pairing(cx);
    }

    /// List the paired folder's direct files and rebuild its diff. Also the
    /// refresh a push finishes with, so pushed files stop reading as new.
    pub(super) fn walk_sirv_pairing(&mut self, cx: &mut Context<Self>) {
        self.clear_error("sirv-listing", cx);
        let Some(pairing) = self.sirv_pairing.as_mut() else {
            return;
        };
        let client = pairing.client.clone();
        let dir = pairing.dir.clone();
        let deep = self.dataset_subfolders;
        pairing.deep = deep;
        pairing.files = Listing::Walking;
        // A new listing may carry new bytes under old names; a preview asked
        // for under the old one must not land in the new cache.
        self.sirv_thumbs.clear();
        self.sirv_thumbs_epoch = self.sirv_thumbs_epoch.wrapping_add(1);
        self.sirv_counts = None;
        self.sirv_remote_only.clear();
        self.sirv_rows.clear();
        self.refresh_visible();
        let walked_dir = dir;
        let root = self.root.clone();
        let generation = self.dataset_generation;
        let pairing_generation = self.sirv_pairing_generation;
        if let Some(cancelled) = self.sirv_walk_cancel.take() {
            cancelled.store(true, std::sync::atomic::Ordering::Relaxed);
        }
        let cancelled = Arc::new(std::sync::atomic::AtomicBool::new(false));
        self.sirv_walk_cancel = Some(cancelled.clone());
        // A later walk cancels this one; its answer must then land nowhere,
        // even when it finished before the cancel was read.
        let superseded = cancelled.clone();
        cx.notify();
        cx.spawn(async move |this, cx| {
            let walked = cx
                .background_executor()
                .spawn(async move {
                    let (cdn_host, walked) = {
                        let mut client = client.lock();
                        let cdn_host = client.cdn_host().map_err(|error| error.to_string());
                        let walked = sirv::walk_remote(&walked_dir, deep, |folder| {
                            client.readdir_cancellable(folder, Some(&cancelled))
                        });
                        (cdn_host, walked)
                    };
                    let files = match walked {
                        Ok(Some(files)) => files,
                        Ok(None) => return None,
                        Err(error) => return Some((cdn_host, Err(error.to_string()))),
                    };
                    // Sizes, not just presence: a file on both sides that the
                    // scan does not read (RAW, text) still gets a row.
                    let presence = sirv::local_sizes_for(&root, files.keys().map(String::as_str));
                    Some((cdn_host, Ok((files, presence))))
                })
                .await;
            this.update(cx, |audit, cx| {
                if superseded.load(std::sync::atomic::Ordering::Relaxed)
                    || !walk_landing_applies(
                        generation,
                        audit.dataset_generation,
                        pairing_generation,
                        audit.sirv_pairing_generation,
                    )
                {
                    return;
                }
                let Some((cdn_host, walked)) = walked else {
                    return;
                };
                let cdn_failure = cdn_host.as_ref().err().cloned();
                let walk_failure = walked.as_ref().err().cloned();
                audit.sirv_walk_cancel = None;
                let Some(pairing) = audit.sirv_pairing.as_mut() else {
                    return;
                };
                pairing.cdn_host = match cdn_host {
                    Ok(host) => CdnHost::Ready(host),
                    Err(message) => CdnHost::Failed(message),
                };
                match walked {
                    Ok((files, presence)) => {
                        pairing.files = Listing::Ready(files);
                        audit.sirv_local_presence = presence;
                        audit.refresh_sirv_counts();
                    }
                    // A listing that failed is not a transfer that failed. It used to
                    // be reported as "Sirv pull: 0 of 0, 1 failed", which named the
                    // wrong operation and left `files` looking like a walk still
                    // running.
                    Err(_) => pairing.files = Listing::Failed,
                }
                if let Some(message) = walk_failure {
                    audit.notify_error("sirv-listing", "Couldn’t compare with Sirv", message, cx);
                } else if let Some(message) = cdn_failure {
                    audit.notify_error("sirv-listing", "Couldn’t read Sirv account", message, cx);
                } else {
                    audit.clear_error("sirv-listing", cx);
                }
                cx.notify();
            })
        })
        .detach();
    }

    /// The user's Unpair: forget the pairing for good, not just for now.
    pub(super) fn unpair_sirv(&mut self, cx: &mut Context<Self>) {
        if self.sirv_pairing.is_some()
            && let Err(message) = sirv::remember_pairing_unless_test(&self.root, None)
        {
            self.notify_error("sirv-listing", "Couldn’t forget the pairing", message, cx);
        }
        self.drop_sirv_pairing(cx);
    }

    /// Let go of the pairing without forgetting it: the folder changed, and
    /// coming back to it brings the pairing back.
    pub(super) fn drop_sirv_pairing(&mut self, cx: &mut Context<Self>) {
        for scope in ["sirv-browser", "sirv-listing", "sirv-transfer"] {
            self.clear_error(scope, cx);
        }
        self.forget_sirv_delivery();
        self.sirv_pairing_generation = self.sirv_pairing_generation.wrapping_add(1);
        self.sirv_pairing = None;
        self.sirv_counts = None;
        self.sirv_scope = None;
        self.sirv_remote_only.clear();
        self.sirv_rows.clear();
        self.sirv_selected.clear();
        self.sirv_local_presence.clear();
        self.sirv_browser = None;
        if let Some(cancelled) = self.sirv_walk_cancel.take() {
            cancelled.store(true, std::sync::atomic::Ordering::Relaxed);
        }
        self.cancel_sirv_transfer();
        // The detached loop may finish one file, but has no pairing to update.
        self.sirv_job = None;
        self.sirv_confirm = None;
        self.refresh_visible();
        cx.notify();
    }

    /// True when this transfer is no longer the one the window wants, or the window
    /// is gone. Checked before each file rather than after, so nothing new starts.
    pub(super) fn sirv_superseded(
        this: &gpui_kit::WeakEntity<Self>,
        cx: &mut gpui_kit::AsyncApp,
        generation: u64,
    ) -> bool {
        this.read_with(cx, |audit, _| audit.sirv_generation != generation)
            .unwrap_or(true)
    }

    /// Ask the running transfer to stop. The loop checks before each file,
    /// so the file in flight finishes and nothing after it starts. The job
    /// stays busy until the loop acknowledges, preventing another transfer
    /// from racing that last file.
    fn forget_sirv_delivery(&mut self) {
        self.sirv_delivery.clear();
        if let Some(job) = self.sirv_delivery_job.take() {
            job.cancelled
                .store(true, std::sync::atomic::Ordering::Release);
        }
    }

    pub(super) fn sirv_delivery_running(&self) -> bool {
        self.sirv_delivery_job
            .as_ref()
            .is_some_and(|job| !job.finished)
    }

    /// Visible rows whose file is on Sirv at the same size, with their keys
    /// and remote sizes: the only rows where Sirv's delivery answers for the
    /// local file.
    pub(super) fn sirv_delivery_targets(&self) -> Vec<(usize, String, u64)> {
        let Some(Listing::Ready(files)) = self.sirv_pairing.as_ref().map(|pairing| &pairing.files)
        else {
            return Vec::new();
        };
        let deep = self
            .sirv_pairing
            .as_ref()
            .is_some_and(|pairing| pairing.deep);
        self.visible
            .iter()
            .filter_map(|&index| {
                let entry = self.entries.get(index)?;
                let key = sirv::paired_key(&self.root, &entry.path, deep)?;
                let node = files.get(&key)?;
                (sirv::classify(entry.bytes, Some(node)) == sirv::SyncState::SameSize)
                    .then_some((index, key, node.size))
            })
            .collect()
    }

    /// The answer for one row, if it still holds: same remote size, same edge.
    pub(super) fn sirv_delivered(&self, entry: &Entry) -> Option<&sirv::Delivered> {
        let pairing = self.sirv_pairing.as_ref()?;
        let Listing::Ready(files) = &pairing.files else {
            return None;
        };
        let key = sirv::paired_key(&self.root, &entry.path, pairing.deep)?;
        let check = self.sirv_delivery.get(&key)?;
        (files.get(&key)?.size == check.remote_size && check.edge == self.max_edge.0)
            .then_some(&check.delivered)
    }

    /// Totals for the bar: files answered, their bytes on disk, what Sirv
    /// sends for them, the formats it chose, and what this session's
    /// conversion wrote for the same rows when it wrote all of them.
    pub(super) fn sirv_delivery_summary(&self) -> Option<DeliverySummary> {
        if self.sirv_delivery.is_empty() {
            return None;
        }
        let mut formats = std::collections::BTreeSet::new();
        let mut count = 0;
        let mut on_disk = 0;
        let mut served = 0;
        let mut converted = Some(0u64);
        for &index in &self.visible {
            let Some(entry) = self.entries.get(index) else {
                continue;
            };
            let Some(delivered) = self.sirv_delivered(entry) else {
                continue;
            };
            count += 1;
            on_disk += entry.bytes;
            served += delivered.bytes;
            formats.insert(delivered.format.as_str());
            converted = converted
                .zip(self.results.get(&index))
                .map(|(sum, bytes)| sum + bytes);
        }
        (count > 0).then(|| DeliverySummary {
            count,
            on_disk,
            served,
            formats: formats.into_iter().collect::<Vec<_>>().join("/"),
            converted,
        })
    }

    pub(super) fn stop_sirv_delivery(&mut self, cx: &mut Context<Self>) {
        if let Some(job) = self.sirv_delivery_job.as_ref() {
            job.cancelled
                .store(true, std::sync::atomic::Ordering::Release);
        }
        cx.notify();
    }

    /// Ask the public CDN what it would send a browser for each paired row,
    /// at the current max edge. Anonymous HEAD requests: no credits, no
    /// credentials, nothing uploaded.
    // ponytail: one request at a time; run a few in parallel if large folders feel slow.
    pub(super) fn check_sirv_delivery(&mut self, cx: &mut Context<Self>) {
        if self.sirv_delivery_running() {
            return;
        }
        let Some(CdnHost::Ready(host)) =
            self.sirv_pairing.as_ref().map(|pairing| &pairing.cdn_host)
        else {
            return;
        };
        let host = host.clone();
        let Some(dir) = self
            .sirv_pairing
            .as_ref()
            .map(|pairing| pairing.dir.clone())
        else {
            return;
        };
        let edge = self.max_edge.0;
        let targets = self.sirv_delivery_targets();
        if targets.is_empty() {
            return;
        }
        self.clear_error("sirv-delivery", cx);
        let cancelled = Arc::new(std::sync::atomic::AtomicBool::new(false));
        self.sirv_delivery_job = Some(DeliveryJob {
            done: 0,
            total: targets.len(),
            failure: None,
            finished: false,
            cancelled: cancelled.clone(),
        });
        cx.notify();

        cx.spawn(async move |this, cx| {
            let owns = |audit: &Audit| {
                audit
                    .sirv_delivery_job
                    .as_ref()
                    .is_some_and(|job| Arc::ptr_eq(&job.cancelled, &cancelled))
            };
            for (_, key, remote_size) in targets {
                if cancelled.load(std::sync::atomic::Ordering::Acquire) {
                    break;
                }
                let url = sirv::public_url(&host, &format!("{}/{key}", dir.trim_end_matches('/')))
                    .map(|url| url + &sirv::delivery_query(edge));
                let result = cx
                    .background_executor()
                    .spawn(async move { url.and_then(|url| sirv::delivered(&url)) })
                    .await;
                let failed = this
                    .update(cx, |audit, cx| {
                        if !owns(audit) {
                            return true;
                        }
                        let failed = match result {
                            Ok(delivered) => {
                                audit.sirv_delivery.insert(
                                    key,
                                    DeliveryCheck {
                                        remote_size,
                                        edge,
                                        delivered,
                                    },
                                );
                                None
                            }
                            Err(error) => Some(format!("{key}: {error}")),
                        };
                        let Some(job) = audit.sirv_delivery_job.as_mut() else {
                            return true;
                        };
                        job.done += 1;
                        let stop = failed.is_some();
                        if failed.is_some() {
                            job.failure = failed;
                        }
                        cx.notify();
                        stop
                    })
                    .unwrap_or(true);
                if failed {
                    break;
                }
            }
            let _ = this.update(cx, |audit, cx| {
                if !owns(audit) {
                    return;
                }
                let Some(job) = audit.sirv_delivery_job.as_mut() else {
                    return;
                };
                job.finished = true;
                if let Some(failure) = job.failure.clone() {
                    audit.notify_error("sirv-delivery", "Sirv delivery check stopped", failure, cx);
                }
                cx.notify();
            });
        })
        .detach();
    }

    pub(super) fn cancel_sirv_transfer(&mut self) {
        self.sirv_generation = self.sirv_generation.wrapping_add(1);
        if let Some(job) = self.sirv_job.as_mut()
            && !job.finished
        {
            job.stopping = true;
        }
    }

    /// Open settings, prefilled with whatever is stored.
    pub(super) fn open_settings(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let stored = sirv::load_credentials();
        let mut make_input = |value: Option<String>, masked| {
            cx.new(|cx| {
                let mut state = InputState::new(window, cx).masked(masked);
                if let Some(value) = value {
                    state.set_value(value, window, cx);
                }
                state
            })
        };
        let client_id = make_input(stored.as_ref().map(|c| c.client_id.clone()), false);
        let client_secret = make_input(stored.as_ref().map(|c| c.client_secret.clone()), true);
        for input in [&client_id, &client_secret] {
            cx.subscribe(input, |_, _, event: &InputEvent, cx| {
                if matches!(event, InputEvent::Change) {
                    cx.notify();
                }
            })
            .detach();
        }
        self.settings_panel = Some(SettingsPanel {
            client_id,
            client_secret,
            cdn_status: None,
            focus_ix: 0,
            focused: false,
            checking: false,
            then_browse: false,
        });
        cx.notify();
    }

    /// Ask Sirv whether the keys work, and store them only if they do. Keys
    /// saved unchecked failed later, in the middle of a pull or a push.
    pub(super) fn save_sirv_settings(&mut self, cx: &mut Context<Self>) {
        self.clear_error("sirv-settings", cx);
        let credentials = {
            let Some(panel) = self.settings_panel.as_mut() else {
                return;
            };
            if panel.checking {
                return;
            }
            sirv::Credentials {
                client_id: panel.client_id.read(cx).value().trim().to_string(),
                client_secret: panel.client_secret.read(cx).value().trim().to_string(),
            }
        };
        if !credentials_complete(&credentials.client_id, &credentials.client_secret) {
            let message = "Both fields are required.";
            if let Some(panel) = self.settings_panel.as_mut() {
                panel.cdn_status = Some((false, message.into()));
            }
            self.notify_error("sirv-settings", "Couldn’t save Sirv settings", message, cx);
            cx.notify();
            return;
        }
        if let Some(panel) = self.settings_panel.as_mut() {
            panel.checking = true;
            panel.cdn_status = None;
        }
        cx.notify();

        cx.spawn(async move |this, cx| {
            let checked = cx
                .background_executor()
                .spawn({
                    let credentials = credentials.clone();
                    async move {
                        sirv::Client::new(credentials)
                            .check()
                            .map_err(|error| error.to_string())
                    }
                })
                .await;
            let _ = this.update_in(cx, |audit, window, cx| {
                audit.finish_sirv_check(credentials, checked, window, cx);
            });
        })
        .detach();
    }

    /// Land a credential check. A panel closed, reopened or edited while Sirv
    /// answered is no longer asking about these keys, so the answer is dropped.
    pub(super) fn finish_sirv_check(
        &mut self,
        credentials: sirv::Credentials,
        checked: Result<(), String>,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let Some(panel) = self.settings_panel.as_mut() else {
            return;
        };
        if !panel.checking
            || panel.client_id.read(cx).value().trim() != credentials.client_id
            || panel.client_secret.read(cx).value().trim() != credentials.client_secret
        {
            return;
        }
        panel.checking = false;
        // Report what happened, not what was attempted. A read-only config directory
        // used to look exactly like success.
        // The same keys saved again change nothing, so they must not stop a
        // running transfer or re-list the pairing.
        let unchanged = sirv::load_credentials().as_ref() == Some(&credentials);
        let saved = checked.and_then(|()| {
            sirv::save_credentials(&credentials).map_err(|error| format!("Could not save: {error}"))
        });
        match saved {
            Ok(()) => {
                let then_browse = panel.then_browse;
                if !unchanged {
                    self.adopt_new_credentials(credentials, cx);
                }
                self.close_settings(window, cx);
                self.notify_success("sirv-settings", "Sirv connected", "Credentials saved.", cx);
                if then_browse {
                    self.open_sirv_browser(cx);
                }
            }
            Err(message) => {
                panel.cdn_status = Some((false, message.clone()));
                self.notify_error("sirv-settings", "Couldn’t save Sirv settings", message, cx);
            }
        }
        cx.notify();
    }

    /// New credentials mean a possibly different account: the old client,
    /// its cached token, any listing built under it, any transfer, and any
    /// walk in flight all describe a world that may no longer exist. Retire
    /// all of them and re-list.
    pub(super) fn adopt_new_credentials(
        &mut self,
        credentials: sirv::Credentials,
        cx: &mut Context<Self>,
    ) {
        if let Some(pairing) = self.sirv_pairing.as_mut() {
            pairing.client = Arc::new(parking_lot::Mutex::new(sirv::Client::new(credentials)));
            pairing.files = Listing::Walking;
            pairing.cdn_host = CdnHost::Loading;
        } else {
            return;
        }
        // A walk started under the old credentials must land nowhere: it
        // carries the old account's listing. Same invalidation pair_sirv
        // and unpair_sirv already use.
        self.sirv_pairing_generation = self.sirv_pairing_generation.wrapping_add(1);
        self.cancel_sirv_transfer();
        self.sirv_local_presence.clear();
        self.sirv_counts = None;
        self.sirv_remote_only.clear();
        self.sirv_rows.clear();
        self.sirv_selected.clear();
        self.refresh_visible();
        self.walk_sirv_pairing(cx);
    }

    pub(super) fn sirv_split_shown(&self) -> bool {
        self.sirv_split && self.sirv_pairing.is_some()
    }

    /// The split view is a Sirv task, not a conversion one: it takes the
    /// Convert panel's width, as the Sirv-only filter already does.
    /// The one way in and out of the comparison. It is a view like List and
    /// Grid, and those two leave it: it used to sit on top of them, so
    /// clicking Grid changed a list nobody could see.
    pub(super) fn set_sirv_split(&mut self, on: bool, cx: &mut Context<Self>) {
        if self.converting || self.sirv_split == on {
            return;
        }
        self.sirv_split = on;
        if self.sirv_split {
            self.sidebar_open = false;
        } else if self.sirv_scope == Some(SirvScope::OnlyRemote) {
            // Back in the list, a filter it cannot show would leave it empty.
            self.choose_sirv_scope(None, cx);
        }
        cx.notify();
    }

    /// Whether the split view shows `row`: the Sirv filter and the name box
    /// narrow it the same way they narrow the list.
    pub(super) fn split_shows(&self, row: &SyncRow) -> bool {
        row.in_scope(self.sirv_scope)
            && (self.filter.is_empty()
                || row.key.to_lowercase().contains(&self.filter.to_lowercase()))
    }

    /// The split view's keys, before the list's own: the list's cursor lives
    /// under a view nobody can see while this one is up. `true` when handled.
    pub(super) fn split_key(
        &mut self,
        event: &gpui_kit::KeyDownEvent,
        cx: &mut Context<Self>,
    ) -> bool {
        let modifiers = event.keystroke.modifiers;
        let command = modifiers.control || modifiers.platform;
        let shown = self
            .sirv_rows
            .iter()
            .filter(|row| self.split_shows(row))
            .map(|row| row.key.clone())
            .collect::<Vec<_>>();
        let last = shown.len().saturating_sub(1);
        let cursor = self.sirv_split_cursor.min(last);
        let target = match event.keystroke.key.as_str() {
            "down" if !command => Some((cursor + 1).min(last)),
            "up" if !command => Some(cursor.saturating_sub(1)),
            "pagedown" => Some((cursor + 10).min(last)),
            "pageup" => Some(cursor.saturating_sub(10)),
            "home" => Some(0),
            "end" => Some(last),
            "space" if !command => {
                if let Some(key) = shown.get(cursor) {
                    self.sirv_split_anchor = cursor;
                    self.toggle_split_row(&key.clone(), cx);
                }
                return true;
            }
            "a" if command => {
                self.toggle_split_rows(cx);
                return true;
            }
            "enter" if !command => {
                let entry = shown
                    .get(cursor)
                    .and_then(|key| self.sirv_rows.iter().find(|row| &row.key == key))
                    .and_then(|row| row.entry);
                if let Some(entry) = entry {
                    self.open_preview(entry, cx);
                }
                return true;
            }
            // The hidden list's cursor must not move under the comparison.
            "left" | "right" if !command => return true,
            "escape" if !self.sirv_selected.is_empty() => {
                self.sirv_selected.clear();
                self.sirv_confirm = None;
                cx.notify();
                return true;
            }
            // Nothing ticked: Escape closes the comparison, back to the list.
            "escape" => {
                self.set_sirv_split(false, cx);
                return true;
            }
            _ => None,
        };
        let Some(target) = target else {
            return false;
        };
        // Shift extends the ticks from the anchor, as Shift does in the list.
        if event.keystroke.modifiers.shift {
            self.tick_split_range(target, cx);
        }
        self.sirv_split_cursor = target;
        self.sirv_split_scroll
            .scroll_to_item(target, ScrollStrategy::Nearest);
        cx.notify();
        true
    }

    /// A click on a split row at position `shown`: Shift ticks the range from
    /// the anchor, anything else toggles the one row and becomes the anchor.
    pub(super) fn click_split_row(
        &mut self,
        shown: usize,
        key: &str,
        shift: bool,
        cx: &mut Context<Self>,
    ) {
        self.sirv_split_cursor = shown;
        if shift {
            self.tick_split_range(shown, cx);
        } else {
            self.sirv_split_anchor = shown;
            self.toggle_split_row(key, cx);
        }
    }

    /// Tick every shown row between the anchor and `to`, both included.
    /// Ticking only adds, like the list's Shift range: nothing already ticked
    /// outside the range is lost.
    fn tick_split_range(&mut self, to: usize, cx: &mut Context<Self>) {
        let (from, to) = (
            self.sirv_split_anchor.min(to),
            self.sirv_split_anchor.max(to),
        );
        let keys = self
            .sirv_rows
            .iter()
            .filter(|row| self.split_shows(row))
            .skip(from)
            .take(to - from + 1)
            .map(|row| row.key.clone())
            .collect::<Vec<_>>();
        self.sirv_selected.extend(keys);
        self.sirv_confirm = None;
        cx.notify();
    }

    /// Tick or untick one row of the split view. A pending "really replace?"
    /// was asked about the old selection, so it goes.
    pub(super) fn toggle_split_row(&mut self, key: &str, cx: &mut Context<Self>) {
        if !self.sirv_selected.remove(key) {
            self.sirv_selected.insert(key.to_string());
        }
        self.sirv_confirm = None;
        cx.notify();
    }

    /// The header checkbox: every row the filter shows, or none of them.
    pub(super) fn toggle_split_rows(&mut self, cx: &mut Context<Self>) {
        let shown = self
            .sirv_rows
            .iter()
            .filter(|row| self.split_shows(row))
            .map(|row| row.key.clone())
            .collect::<Vec<_>>();
        if shown.iter().all(|key| self.sirv_selected.contains(key)) {
            for key in &shown {
                self.sirv_selected.remove(key);
            }
        } else {
            self.sirv_selected.extend(shown);
        }
        self.sirv_confirm = None;
        cx.notify();
    }

    /// Selected keys that `state` describes: what one footer button acts on.
    pub(super) fn split_selected(&self, state: SplitState) -> HashSet<String> {
        self.sirv_rows
            .iter()
            .filter(|row| row.state() == state && self.sirv_selected.contains(&row.key))
            .map(|row| row.key.clone())
            .collect()
    }

    /// Copy `keys` in the direction `state` implies. Replacing either side is
    /// destructive, so those two ask twice, like the bulk buttons always did.
    pub(super) fn transfer_split(
        &mut self,
        state: SplitState,
        toward_sirv: bool,
        keys: HashSet<String>,
        cx: &mut Context<Self>,
    ) {
        if keys.is_empty() || self.sirv_busy() {
            return;
        }
        let replace = if toward_sirv {
            SirvJobKind::PushChanged
        } else {
            SirvJobKind::PullChanged
        };
        if state == SplitState::Different && self.sirv_confirm != Some(replace) {
            self.sirv_confirm = Some(replace);
            cx.notify();
            return;
        }
        self.sirv_confirm = None;
        for key in &keys {
            self.sirv_selected.remove(key);
        }
        match (state, toward_sirv) {
            (SplitState::OnlyLocal, true) => {
                self.run_push(sirv::SyncState::OnlyLocal, Some(&keys), cx)
            }
            (SplitState::OnlyRemote, false) => self.run_pull(false, Some(keys), cx),
            (SplitState::Different, true) => {
                self.run_push(sirv::SyncState::DifferentSize, Some(&keys), cx)
            }
            (SplitState::Different, false) => self.run_pull(true, Some(keys), cx),
            _ => {}
        }
        cx.notify();
    }

    /// The one toast a transfer ends with: what failed, or what was copied.
    pub(super) fn report_transfer(&mut self, cx: &mut Context<Self>) {
        self.sirv_queued.clear();
        let Some(job) = self.sirv_job.as_ref() else {
            return;
        };
        if let Some((title, message)) = transfer_failure(job) {
            self.notify_error("sirv-transfer", title, message, cx);
        } else if let Some((title, message)) = transfer_success(job) {
            self.notify_success("sirv-transfer", title, message, cx);
        } else {
            self.clear_error("sirv-transfer", cx);
        }
    }

    /// A transfer is already running. One at a time: the client serialises on
    /// its token cache anyway, and two progress lines would lie about order.
    pub(super) fn sirv_busy(&self) -> bool {
        self.sirv_job.as_ref().is_some_and(|job| !job.finished)
    }

    /// Download every remote file the local folder lacks. Existing files are
    /// never overwritten — pull is additive by design, so it can never destroy
    /// local work. Installation enforces that promise with an atomic no-replace
    /// link, not an inference from the image scan.
    pub(super) fn start_pull(&mut self, cx: &mut Context<Self>) {
        self.sirv_confirm = None;
        self.run_pull(false, None, cx);
    }

    /// `only` narrows the pull to those keys: the split view's selection. The
    /// plan's own rules still apply inside it, so a selected file that exists
    /// locally is never overwritten by a plain download.
    pub(super) fn run_pull(
        &mut self,
        differing: bool,
        only: Option<HashSet<String>>,
        cx: &mut Context<Self>,
    ) {
        if self.batch_folders.is_some()
            || self.scan_blocks_delivery()
            || self.converting
            || self.restoring
        {
            return;
        }
        let Some(pairing) = &self.sirv_pairing else {
            return;
        };
        if self.sirv_busy() {
            return;
        }
        let Listing::Ready(files) = &pairing.files else {
            return;
        };
        let files = files.clone();
        let dir = pairing.dir.clone();
        let client = pairing.client.clone();
        self.clear_error("sirv-transfer", cx);
        self.sirv_generation = self.sirv_generation.wrapping_add(1);
        let generation = self.sirv_generation;
        let root = self.root.clone();
        self.sirv_queued = only.clone().unwrap_or_default();
        // The job exists from the click, not from when the plan lands: until
        // then the window read as idle, and a second arrow clicked meanwhile
        // superseded the first download without a word.
        self.sirv_job = Some(SirvJob {
            kind: if differing {
                SirvJobKind::PullChanged
            } else {
                SirvJobKind::Pull
            },
            done: 0,
            total: 0,
            failed: 0,
            failures: Vec::new(),
            current: None,
            finished: false,
            stopping: false,
            stopped_by_user: false,
            generation,
        });
        cx.notify();
        cx.spawn(async move |this, cx| {
            let plan = cx
                .background_executor()
                .spawn({
                    let root = root.clone();
                    let files = files.clone();
                    async move {
                        // The disk, both ways: the scan leaves out RAW, text and
                        // unreadable files, and a replace must see those too.
                        let local_sizes =
                            sirv::local_sizes_for(&root, files.keys().map(String::as_str));
                        let mut plan = sirv::pull_plan(&files, &local_sizes, differing);
                        if let Some(only) = &only {
                            plan.retain(|key| only.contains(key));
                        }
                        // In the comparison's row order, so progress runs down
                        // the list rather than jumping about it.
                        plan.sort();
                        plan
                    }
                })
                .await;
            let Some(_) = this
                .update(cx, |audit, cx| {
                    let owned = audit
                        .sirv_job
                        .as_ref()
                        .is_some_and(|job| job.generation == generation);
                    if !owned
                        || audit.scan_blocks_delivery()
                        || audit.sirv_generation != generation
                        || plan.is_empty()
                    {
                        if owned {
                            audit.sirv_job = None;
                            cx.notify();
                        }
                        return None;
                    }
                    let total = plan.len();
                    if let Some(job) = audit.sirv_job.as_mut() {
                        job.total = total;
                    }
                    audit.sirv_queued = plan.iter().cloned().collect();
                    cx.notify();
                    Some(total)
                })
                .ok()
                .flatten()
            else {
                return;
            };
            let mut failed = 0;
            let mut failures = Vec::new();
            let mut consecutive_remote_failures = 0;
            for (ix, key) in plan.iter().enumerate() {
                if Self::sirv_superseded(&this, cx, generation) {
                    this.update(cx, |audit, cx| {
                        let acknowledged = if let Some(job) = audit.sirv_job.as_mut()
                            && job.generation == generation
                            && !job.finished
                        {
                            job.finished = true;
                            true
                        } else {
                            false
                        };
                        if acknowledged {
                            audit.report_transfer(cx);
                            audit.refresh_sirv_counts();
                            audit.refresh_target_summary();
                            audit.schedule_estimate(cx);
                            cx.notify();
                        }
                    })
                    .ok();
                    return;
                }
                this.update(cx, |audit, cx| {
                    if let Some(job) = audit.sirv_job.as_mut()
                        && job.generation == generation
                    {
                        job.current = Some(key.clone());
                        cx.notify();
                    }
                })
                .ok();
                let outcome = cx
                    .background_executor()
                    .spawn({
                        let client = client.clone();
                        let remote_path = format!("{dir}/{key}");
                        let root = root.clone();
                        let key = key.clone();
                        async move {
                            let downloaded = client.lock().download(&remote_path);
                            match downloaded {
                                // Read back here, off the window's thread, so the
                                // file joins the list without a rescan.
                                Ok(bytes) => {
                                    let size = bytes.len() as u64;
                                    sirv::write_pulled(&root, &key, &bytes, differing)
                                        .map(|()| (scan::probe(&root.join(&key)), size))
                                        .map_err(|error| (error, 0))
                                }
                                Err(error) => Err((error.to_string(), failure_weight(&error))),
                            }
                        }
                    })
                    .await;
                // Keep the reason. "1 failed: a.jpg" sends the user hunting;
                // "a.jpg: 403 forbidden" or "a.jpg: No space left on device"
                // says what to do about it.
                let succeeded = outcome.is_ok();
                let pulled = match outcome {
                    Ok(pulled) => {
                        consecutive_remote_failures = 0;
                        Some(pulled)
                    }
                    Err((error, weight)) => {
                        consecutive_remote_failures = match weight {
                            0 => 0,
                            weight => consecutive_remote_failures + weight,
                        };
                        remember_failure(&mut failed, &mut failures, format!("{key}: {error}"));
                        None
                    }
                };
                let abort = consecutive_remote_failures >= CONSECUTIVE_REMOTE_FAILURES;
                this.update(cx, |audit, cx| {
                    // Only onto this loop's own job. A slow last file can land after
                    // the user has already started another transfer.
                    if let Some(job) = audit.sirv_job.as_mut()
                        && job.generation == generation
                    {
                        job.done = ix + 1;
                        audit.sirv_queued.remove(key);
                        job.current = None;
                        job.failed = failed;
                        job.failures = failures.clone();
                        if let Some((entry, size)) = pulled {
                            audit.sirv_local_presence.insert(key.clone(), size);
                            if let Some(entry) = entry {
                                audit.adopt_pulled_entry(entry);
                            }
                        }
                        if succeeded {
                            audit.refresh_sirv_counts();
                        }
                        cx.notify();
                    }
                })
                .ok();
                if abort {
                    break;
                }
            }
            this.update(cx, |audit, cx| {
                let owns_job = if let Some(job) = audit.sirv_job.as_mut()
                    && job.generation == generation
                {
                    job.finished = true;
                    job.current = None;
                    true
                } else {
                    false
                };
                if owns_job {
                    audit.report_transfer(cx);
                    audit.refresh_sirv_counts();
                    audit.refresh_target_summary();
                    audit.schedule_estimate(cx);
                    cx.notify();
                }
            })
            .ok();
        })
        .detach();
    }

    /// Put a downloaded file into the list where it belongs. A rescan did this
    /// before, and a rescan replaces the dataset: it stopped a running Studio
    /// or local AI job, a saved plan, and dropped the conversion results.
    /// Indices never move, so a new file goes on the end and a replaced one
    /// keeps its row, losing only what described the old bytes.
    pub(super) fn adopt_pulled_entry(&mut self, entry: Entry) {
        if let Some(index) = self.entries.iter().position(|old| old.path == entry.path) {
            // The run's totals counted the old bytes and their output.
            if let Some(output) = self.results.remove(&index) {
                self.converted_totals.0 = self
                    .converted_totals
                    .0
                    .saturating_sub(self.entries[index].bytes);
                self.converted_totals.1 = self.converted_totals.1.saturating_sub(output);
            }
            self.entries[index] = entry;
            self.thumbs.remove(&index);
            self.requested.remove(&index);
            self.result_paths.remove(&index);
            if self.failures.remove(&index).is_some() {
                self.failure_summary = named(self.failure_names().into_iter());
            }
            self.completed_outputs.retain(|(done, _)| *done != index);
        } else {
            self.entries.push(entry);
        }
        self.heavy = self
            .entries
            .iter()
            .filter(|entry| Finding::Heavy.holds(entry))
            .count();
        self.mislabelled = self
            .entries
            .iter()
            .filter(|entry| entry.extension_lies())
            .count();
        // The caller refreshes the target summary and the estimate once, when
        // the pull ends: per file, a big pull started an estimate per file.
        self.refresh_visible();
    }

    /// Upload every local file Sirv lacks.
    pub(super) fn start_push(&mut self, cx: &mut Context<Self>) {
        self.sirv_confirm = None;
        self.run_push(sirv::SyncState::OnlyLocal, None, cx);
    }

    /// `only` narrows the push to those keys, as for `run_pull`.
    pub(super) fn run_push(
        &mut self,
        accept: sirv::SyncState,
        only: Option<&HashSet<String>>,
        cx: &mut Context<Self>,
    ) {
        if self.batch_folders.is_some()
            || self.scan_blocks_delivery()
            || self.converting
            || self.restoring
        {
            return;
        }
        let Some(pairing) = &self.sirv_pairing else {
            return;
        };
        if self.sirv_busy() {
            return;
        }
        let Listing::Ready(files) = &pairing.files else {
            return;
        };
        let mut plan = sirv_push_plan(&self.root, &self.entries, files, accept, pairing.deep);
        // Replacing covers every differing row, including files the scan does
        // not read; they have rows, so they have to have the verb too.
        if accept == sirv::SyncState::DifferentSize {
            plan.extend(
                self.sirv_rows
                    .iter()
                    .filter(|row| row.entry.is_none() && row.state() == SplitState::Different)
                    // Never through a link: the upload is public.
                    .filter(|row| sirv::plain_file_below(&self.root, &row.key))
                    .map(|row| (row.key.clone(), self.root.join(&row.key))),
            );
        }
        if let Some(only) = only {
            plan.retain(|(key, _)| only.contains(key));
        }
        let kind = if accept == sirv::SyncState::DifferentSize {
            SirvJobKind::PushChanged
        } else {
            SirvJobKind::Push
        };
        self.run_upload_plan(plan, kind, None, kind == SirvJobKind::Push, cx);
    }

    /// The one upload loop used by sync and converted results. Both need
    /// the same caps, cancellation and named failures.
    pub(super) fn run_upload_plan(
        &mut self,
        mut plan: Vec<(String, PathBuf)>,
        kind: SirvJobKind,
        completion: Option<Vec<String>>,
        // Uploads that promise not to replace anything ask Sirv first: the
        // listing they were planned from can be an hour old.
        check_absent: bool,
        cx: &mut Context<Self>,
    ) {
        if self.scan_blocks_delivery() || self.converting || self.restoring {
            return;
        }
        let Some(pairing) = &self.sirv_pairing else {
            return;
        };
        if self.sirv_busy() {
            return;
        }
        if plan.is_empty() {
            return;
        }
        self.clear_error("sirv-transfer", cx);
        let dir = pairing.dir.clone();
        let client = pairing.client.clone();
        // Row order, as for a pull.
        plan.sort_by(|a, b| a.0.cmp(&b.0));
        let total = plan.len();
        self.sirv_queued = plan.iter().map(|(key, _)| key.clone()).collect();
        self.sirv_generation = self.sirv_generation.wrapping_add(1);
        let generation = self.sirv_generation;
        self.sirv_job = Some(SirvJob {
            kind,
            done: 0,
            total,
            failed: 0,
            failures: Vec::new(),
            current: None,
            finished: false,
            stopping: false,
            stopped_by_user: false,
            generation,
        });
        cx.notify();

        let folders = sirv::push_folders(plan.iter().map(|(key, _)| key));

        cx.spawn(async move |this, cx| {
            let mut failed = 0;
            let mut failures = Vec::new();
            let mut consecutive_remote_failures = 0;

            if Self::sirv_superseded(&this, cx, generation) {
                this.update(cx, |audit, cx| {
                    let acknowledged = if let Some(job) = audit.sirv_job.as_mut()
                        && job.generation == generation
                        && !job.finished
                    {
                        job.finished = true;
                        true
                    } else {
                        false
                    };
                    if acknowledged {
                        audit.report_transfer(cx);
                        audit.walk_sirv_pairing(cx);
                        cx.notify();
                    }
                })
                .ok();
                return;
            }

            // A cancel during this locked task completes one provisioning batch.
            let made = cx
                .background_executor()
                .spawn({
                    let client = client.clone();
                    let dir = dir.clone();
                    async move {
                        let mut client = client.lock();
                        for folder in &folders {
                            // mkdir on an existing folder is success upstream, so this
                            // is "ensure", not "create".
                            if let Err(error) = client.mkdir(&format!("{dir}/{folder}")) {
                                return Err(format!("could not create folder {folder}: {error}"));
                            }
                        }
                        Ok(())
                    }
                })
                .await;
            if let Err(message) = made {
                // Not a file that failed: nothing was tried. The report says
                // so, rather than "1 of 5 failed … 5 not tried".
                failures.push(message);
                this.update(cx, |audit, cx| {
                    if let Some(job) = audit.sirv_job.as_mut()
                        && job.generation == generation
                    {
                        job.failed = failed;
                        job.failures = failures;
                        job.finished = true;
                        let failure = transfer_failure(job);
                        let success = transfer_success(job);
                        if let Some((title, message)) = failure {
                            audit.notify_error("sirv-transfer", title, message, cx);
                        } else if let Some((title, message)) = success {
                            audit.notify_success("sirv-transfer", title, message, cx);
                        }
                        audit.walk_sirv_pairing(cx);
                        cx.notify();
                    }
                })
                .ok();
                return;
            }

            if Self::sirv_superseded(&this, cx, generation) {
                this.update(cx, |audit, cx| {
                    let acknowledged = if let Some(job) = audit.sirv_job.as_mut()
                        && job.generation == generation
                        && !job.finished
                    {
                        job.finished = true;
                        true
                    } else {
                        false
                    };
                    if acknowledged {
                        audit.report_transfer(cx);
                        audit.walk_sirv_pairing(cx);
                        cx.notify();
                    }
                })
                .ok();
                return;
            }

            for (ix, (key, path)) in plan.iter().enumerate() {
                if Self::sirv_superseded(&this, cx, generation) {
                    this.update(cx, |audit, cx| {
                        let acknowledged = if let Some(job) = audit.sirv_job.as_mut()
                            && job.generation == generation
                            && !job.finished
                        {
                            job.finished = true;
                            true
                        } else {
                            false
                        };
                        if acknowledged {
                            audit.report_transfer(cx);
                            audit.walk_sirv_pairing(cx);
                            cx.notify();
                        }
                    })
                    .ok();
                    return;
                }
                this.update(cx, |audit, cx| {
                    if let Some(job) = audit.sirv_job.as_mut()
                        && job.generation == generation
                    {
                        job.current = Some(key.clone());
                        cx.notify();
                    }
                })
                .ok();
                let outcome = cx
                    .background_executor()
                    .spawn({
                        let client = client.clone();
                        let key = key.clone();
                        let path = path.clone();
                        let dir = dir.clone();
                        async move {
                            let size = std::fs::metadata(&path)
                                .map_err(|error| (format!("{key}: {error}"), 0))?
                                .len();
                            if size > sirv::MAX_TRANSFER {
                                return Err((
                                    format!(
                                        "{key}: larger than the {}-byte transfer cap",
                                        sirv::MAX_TRANSFER
                                    ),
                                    0,
                                ));
                            }
                            let bytes = std::fs::read(&path)
                                .map_err(|error| (format!("{key}: {error}"), 0))?;
                            let remote = format!("{dir}/{key}");
                            let mut client = client.lock();
                            if check_absent {
                                match client.exists(&remote) {
                                    Ok(false) => {}
                                    Ok(true) => {
                                        return Err((
                                            format!(
                                                "{key}: now on Sirv, added after the last listing; \
                                                 not overwritten"
                                            ),
                                            0,
                                        ));
                                    }
                                    Err(error) => {
                                        return Err((
                                            format!("{key}: {error}"),
                                            failure_weight(&error),
                                        ));
                                    }
                                }
                            }
                            client
                                .upload(&remote, &bytes, sirv::content_type(&key))
                                .map_err(|error| {
                                    (format!("{key}: {error}"), failure_weight(&error))
                                })
                        }
                    })
                    .await;
                match outcome {
                    Ok(()) => consecutive_remote_failures = 0,
                    Err((message, weight)) => {
                        consecutive_remote_failures = match weight {
                            0 => 0,
                            weight => consecutive_remote_failures + weight,
                        };
                        remember_failure(&mut failed, &mut failures, message);
                    }
                }
                let abort = consecutive_remote_failures >= CONSECUTIVE_REMOTE_FAILURES;
                this.update(cx, |audit, cx| {
                    // Only onto this loop's own job. A slow last file can land after
                    // the user has already started another transfer.
                    if let Some(job) = audit.sirv_job.as_mut()
                        && job.generation == generation
                    {
                        job.done = ix + 1;
                        audit.sirv_queued.remove(key);
                        job.current = None;
                        job.failed = failed;
                        job.failures = failures.clone();
                        cx.notify();
                    }
                })
                .ok();
                if abort {
                    break;
                }
            }
            this.update(cx, |audit, cx| {
                let owns_job = if let Some(job) = audit.sirv_job.as_mut()
                    && job.generation == generation
                {
                    job.finished = true;
                    job.current = None;
                    true
                } else {
                    false
                };
                if owns_job {
                    if failed == 0
                        && let Some(urls) = completion
                    {
                        audit.published_results = urls;
                    }
                    audit.report_transfer(cx);
                    // Re-list the pair: pushed files must stop reading as new.
                    audit.walk_sirv_pairing(cx);
                    cx.notify();
                }
            })
            .ok();
        })
        .detach();
    }

    /// Count push / differs / pull across the whole dataset, not just the
    /// visible rows, so the header numbers do not move with the filter.
    pub(super) fn refresh_sirv_counts(&mut self) {
        // A "really replace?" was asked about the rows as they were.
        self.sirv_confirm = None;
        self.sirv_remote_only.clear();
        self.sirv_rows.clear();
        let Some(pairing) = self.sirv_pairing.as_ref() else {
            self.sirv_counts = None;
            self.refresh_visible();
            return;
        };
        let Listing::Ready(files) = &pairing.files else {
            self.sirv_counts = None;
            self.refresh_visible();
            return;
        };
        // One row per key on either side. Scanned images carry their entry;
        // a file both sides hold that the scan does not read (RAW, text) is a
        // row too, sized from the disk, so "N of M" counts every file.
        let mut rows = std::collections::BTreeMap::new();
        for (index, entry) in self.entries.iter().enumerate() {
            if let Some(key) = sirv::paired_key(&self.root, &entry.path, pairing.deep) {
                let remote = files.get(&key).map(|node| node.size);
                rows.insert(key, (Some(entry.bytes), remote, Some(index)));
            }
        }
        for (key, node) in files {
            rows.entry(key.clone()).or_insert((
                self.sirv_local_presence.get(key).copied(),
                Some(node.size),
                None,
            ));
        }
        self.sirv_selected.retain(|key| rows.contains_key(key));
        self.sirv_rows = rows
            .into_iter()
            .map(|(key, (local, remote, entry))| SyncRow {
                key,
                local,
                remote,
                entry,
            })
            .collect();
        // Counted from the rows, so the bar and the comparison cannot disagree.
        let (mut to_push, mut changed, mut to_pull) = (0, 0, 0);
        for row in &self.sirv_rows {
            match row.state() {
                SplitState::OnlyLocal => to_push += 1,
                SplitState::Different => changed += 1,
                SplitState::OnlyRemote => {
                    to_pull += 1;
                    self.sirv_remote_only.push(row.key.clone());
                }
                SplitState::InSync => {}
            }
        }
        self.sirv_counts = Some((to_push, changed, to_pull));
        self.refresh_visible();
    }

    /// Fetch Sirv's preview of `key` for the split view. Only rows on screen
    /// ask, six at a time; each answer redraws, which asks for the next ones.
    pub(super) fn request_sirv_thumb(&mut self, key: &str, cx: &mut Context<Self>) {
        // Tests never reach a CDN.
        if cfg!(test)
            || !sirv::previewable(key)
            || self.sirv_thumbs.contains_key(key)
            || self.sirv_thumbs_loading.contains(key)
            || self.sirv_thumbs_loading.len() >= 6
        {
            return;
        }
        let Some(pairing) = self.sirv_pairing.as_ref() else {
            return;
        };
        let CdnHost::Ready(host) = &pairing.cdn_host else {
            return;
        };
        let Ok(url) = sirv::preview_url(host, &format!("{}/{key}", pairing.dir)) else {
            self.sirv_thumbs.insert(key.to_string(), None);
            return;
        };
        let pairing_generation = self.sirv_pairing_generation;
        let epoch = self.sirv_thumbs_epoch;
        let key = key.to_string();
        self.sirv_thumbs_loading.insert(key.clone());
        cx.spawn(async move |this, cx| {
            let image = cx
                .background_executor()
                .spawn(async move {
                    sirv::preview(&url)
                        .ok()
                        .and_then(|bytes| crate::thumbs::drawable_from_bytes(&bytes))
                })
                .await;
            this.update(cx, |audit, cx| {
                audit.sirv_thumbs_loading.remove(&key);
                if audit.sirv_pairing_generation == pairing_generation
                    && audit.sirv_thumbs_epoch == epoch
                {
                    audit.sirv_thumbs.insert(key, image);
                }
                cx.notify();
            })
            .ok();
        })
        .detach();
    }

    /// The pairing's listing goes as deep as the list does. A subfolder
    /// switch changes the list after its rescan lands, so this re-lists Sirv
    /// when the two no longer agree.
    pub(super) fn sync_sirv_depth(&mut self, cx: &mut Context<Self>) {
        if self
            .sirv_pairing
            .as_ref()
            .is_some_and(|pairing| pairing.deep != self.dataset_subfolders)
        {
            self.walk_sirv_pairing(cx);
        }
    }

    /// Show one reconciliation category, or all of them with `None`.
    pub(super) fn choose_sirv_scope(&mut self, scope: Option<SirvScope>, cx: &mut Context<Self>) {
        if self.converting {
            return;
        }
        self.sirv_scope = scope;
        // The image list cannot show a file that is not on this computer, so
        // "only on Sirv" is a question only the comparison can answer.
        let unlisted_differences = self.sirv_scope == Some(SirvScope::Changed)
            && self
                .sirv_rows
                .iter()
                .any(|row| row.entry.is_none() && row.state() == SplitState::Different);
        if (self.sirv_scope == Some(SirvScope::OnlyRemote) || unlisted_differences)
            && !self.sirv_split
        {
            self.sirv_split = true;
            self.sidebar_open = false;
        }
        self.selected.clear();
        self.sirv_confirm = None;
        self.refresh_visible();
        self.schedule_estimate(cx);
        cx.notify();
    }
}
