//! Sirv actions: browsing, pairing, and both transfer directions.

use super::*;

const FAILURE_EXAMPLES: usize = 3;
const CONSECUTIVE_REMOTE_FAILURES: usize = 3;

pub(super) fn remember_failure(count: &mut usize, examples: &mut Vec<String>, message: String) {
    *count += 1;
    if examples.len() < FAILURE_EXAMPLES {
        examples.push(message);
    }
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
        self.sirv_confirm = None;
        self.studio_confirm = None;
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
                        nodes: Some(Err("No Sirv credentials are saved.".into())),
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
        let state = self.sirv_browser.as_mut().unwrap();
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
                    browser.nodes = Some(result);
                    cx.notify();
                }
            })
        })
        .detach();
    }

    pub(super) fn close_settings(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        self.settings_panel = None;
        Self::restore_audit_focus(window, cx);
    }

    pub(super) fn close_sirv_browser(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        self.sirv_browser = None;
        self.sirv_confirm = None;
        Self::restore_audit_focus(window, cx);
    }

    pub(super) fn restore_audit_focus(window: &mut Window, cx: &mut Context<Self>) {
        cx.defer_in(window, |audit, window, cx| window.focus(&audit.focus, cx));
        cx.notify();
    }

    /// Enter a folder of the listing.
    pub(super) fn descend_sirv(&mut self, name: String, cx: &mut Context<Self>) {
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

    /// Go up one folder. The root has no parent, so the button only exists
    /// below it.
    pub(super) fn ascend_sirv(&mut self, cx: &mut Context<Self>) {
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

    /// Pair the browsed folder, then list it recursively in the background.
    /// The pairing exists immediately (the header names it); its diff arrives
    /// when the walk lands.
    pub(super) fn pair_sirv(&mut self, cx: &mut Context<Self>) {
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
        // A transfer aimed at the old pairing must not outlive it, same rule as unpair.
        self.cancel_sirv_transfer();
        self.sirv_pairing_generation = self.sirv_pairing_generation.wrapping_add(1);
        self.sirv_pairing = Some(SirvPairing {
            dir: dir.clone(),
            files: Listing::Walking,
            cdn_host: CdnHost::Loading,
            client,
        });
        self.sirv_counts = None;
        self.sirv_browser = None;
        cx.notify();
        self.walk_sirv_pairing(cx);
    }

    /// List the paired folder end to end and rebuild its diff. Also the
    /// refresh a push finishes with, so pushed files stop reading as new.
    pub(super) fn walk_sirv_pairing(&mut self, cx: &mut Context<Self>) {
        let Some(pairing) = &self.sirv_pairing else {
            return;
        };
        let client = pairing.client.clone();
        let dir = pairing.dir.clone();
        let walked_dir = dir.clone();
        let root = self.root.clone();
        let generation = self.dataset_generation;
        let pairing_generation = self.sirv_pairing_generation;
        if let Some(cancelled) = self.sirv_walk_cancel.take() {
            cancelled.store(true, std::sync::atomic::Ordering::Relaxed);
        }
        let cancelled = Arc::new(std::sync::atomic::AtomicBool::new(false));
        self.sirv_walk_cancel = Some(cancelled.clone());
        cx.spawn(async move |this, cx| {
            let walked = cx
                .background_executor()
                .spawn(async move {
                    let (cdn_host, walked) = {
                        let mut client = client.lock();
                        let cdn_host = client.cdn_host().map_err(|error| error.to_string());
                        let walked = client.walk(&dir, &cancelled);
                        (cdn_host, walked)
                    };
                    let nodes = match walked {
                        Ok(Some(nodes)) => nodes,
                        Ok(None) => return None,
                        Err(error) => return Some((cdn_host, Err(error.to_string()))),
                    };
                    let files: HashMap<String, sirv::Node> = nodes
                        .into_iter()
                        .filter_map(|node| {
                            sirv::unpair_remote(&walked_dir, &node.filename).map(|key| (key, node))
                        })
                        .collect();
                    let presence = sirv::local_sizes_for(&root, files.keys().map(String::as_str))
                        .into_keys()
                        .collect();
                    Some((cdn_host, Ok((files, presence))))
                })
                .await;
            this.update(cx, |audit, cx| {
                if !walk_landing_applies(
                    generation,
                    audit.dataset_generation,
                    pairing_generation,
                    audit.sirv_pairing_generation,
                ) {
                    return;
                }
                let Some((cdn_host, walked)) = walked else {
                    return;
                };
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
                    Err(message) => pairing.files = Listing::Failed(message),
                }
                cx.notify();
            })
        })
        .detach();
    }

    pub(super) fn unpair_sirv(&mut self, cx: &mut Context<Self>) {
        self.sirv_pairing_generation = self.sirv_pairing_generation.wrapping_add(1);
        self.sirv_pairing = None;
        self.sirv_counts = None;
        self.sirv_local_presence.clear();
        self.sirv_browser = None;
        if let Some(cancelled) = self.sirv_walk_cancel.take() {
            cancelled.store(true, std::sync::atomic::Ordering::Relaxed);
        }
        self.cancel_sirv_transfer();
        // The detached loop may finish one file, but has no pairing to update.
        self.sirv_job = None;
        self.sirv_confirm = None;
        self.studio_confirm = None;
        cx.notify();
    }

    /// True when this transfer is no longer the one the window wants, or the window
    /// is gone. Checked before each file rather than after, so nothing new starts.
    pub(super) fn sirv_superseded(
        this: &gpui::WeakEntity<Self>,
        cx: &mut gpui::AsyncApp,
        generation: u64,
    ) -> bool {
        this.read_with(cx, |audit, _| audit.sirv_generation != generation)
            .unwrap_or(true)
    }

    /// Ask the running transfer to stop. The loop checks before each file,
    /// so the file in flight finishes and nothing after it starts. The job
    /// stays busy until the loop acknowledges, preventing another transfer
    /// from racing that last file.
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
        });
        cx.notify();
    }

    /// Store the CDN credentials.
    pub(super) fn save_sirv_settings(&mut self, cx: &mut Context<Self>) {
        let (client_id, client_secret) = {
            let Some(panel) = self.settings_panel.as_mut() else {
                return;
            };
            (
                panel.client_id.read(cx).value().trim().to_string(),
                panel.client_secret.read(cx).value().trim().to_string(),
            )
        };
        if !credentials_complete(&client_id, &client_secret) {
            let panel = self.settings_panel.as_mut().unwrap();
            panel.cdn_status = Some((false, "Both fields are required.".into()));
            cx.notify();
            return;
        }
        // Report what happened, not what was attempted. A read-only config directory
        // used to look exactly like success.
        let mut new_credentials = None;
        let status = match sirv::save_credentials(&sirv::Credentials {
            client_id: client_id.clone(),
            client_secret: client_secret.clone(),
        }) {
            Ok(()) => {
                new_credentials = Some(sirv::Credentials {
                    client_id,
                    client_secret,
                });
                (true, "Saved.".into())
            }
            Err(error) => (false, format!("Could not save: {error}")),
        };
        self.settings_panel.as_mut().unwrap().cdn_status = Some(status);
        if let Some(credentials) = new_credentials {
            self.adopt_new_credentials(credentials, cx);
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
        self.walk_sirv_pairing(cx);
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
        self.run_pull(false, cx);
    }

    /// Deliberately replace every differing local copy with the remote one.
    pub(super) fn start_pull_changed(&mut self, cx: &mut Context<Self>) {
        self.run_pull(true, cx);
    }

    pub(super) fn run_pull(&mut self, differing: bool, cx: &mut Context<Self>) {
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
        let entry_sizes = if differing {
            self.entries
                .iter()
                .filter_map(|entry| {
                    sirv::relative_key(&self.root, &entry.path).map(|key| (key, entry.bytes))
                })
                .collect()
        } else {
            HashMap::new()
        };
        self.sirv_generation = self.sirv_generation.wrapping_add(1);
        let generation = self.sirv_generation;
        let root = self.root.clone();
        cx.spawn(async move |this, cx| {
            let plan = cx
                .background_executor()
                .spawn({
                    let root = root.clone();
                    let files = files.clone();
                    let dir = dir.clone();
                    async move {
                        let remote: Vec<sirv::Node> = files.values().cloned().collect();
                        let local_sizes = if differing {
                            entry_sizes
                        } else {
                            sirv::local_sizes_for(&root, files.keys().map(String::as_str))
                        };
                        sirv::pull_plan(&remote, &dir, &local_sizes, differing)
                    }
                })
                .await;
            let Some(_) = this
                .update(cx, |audit, cx| {
                    if audit.sirv_generation != generation || audit.sirv_busy() || plan.is_empty() {
                        return None;
                    }
                    let total = plan.len();
                    audit.sirv_job = Some(SirvJob {
                        kind: if differing {
                            SirvJobKind::PullChanged
                        } else {
                            SirvJobKind::Pull
                        },
                        done: 0,
                        total,
                        failed: 0,
                        failures: Vec::new(),
                        finished: false,
                        stopping: false,
                        generation,
                    });
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
                            remember_failure(&mut job.failed, &mut job.failures, "stopped".into());
                            true
                        } else {
                            false
                        };
                        if acknowledged {
                            audit.request_path(audit.root.clone(), cx);
                            cx.notify();
                        }
                    })
                    .ok();
                    return;
                }
                let outcome = cx
                    .background_executor()
                    .spawn({
                        let client = client.clone();
                        let remote_path = format!("{dir}/{key}");
                        let root = root.clone();
                        let key = key.clone();
                        async move {
                            match client.lock().download(&remote_path) {
                                Ok(bytes) => sirv::write_pulled(&root, &key, &bytes, differing)
                                    .map_err(|error| (error, false)),
                                Err(error) => {
                                    let retryable = error.retryable();
                                    Err((error.to_string(), retryable))
                                }
                            }
                        }
                    })
                    .await;
                // Keep the reason. "1 failed: a.jpg" sends the user hunting;
                // "a.jpg: 403 forbidden" or "a.jpg: No space left on device"
                // says what to do about it.
                let succeeded = outcome.is_ok();
                match outcome {
                    Ok(()) => consecutive_remote_failures = 0,
                    Err((error, retryable)) => {
                        consecutive_remote_failures = if retryable {
                            consecutive_remote_failures + 1
                        } else {
                            0
                        };
                        remember_failure(&mut failed, &mut failures, format!("{key}: {error}"));
                    }
                }
                let abort = consecutive_remote_failures >= CONSECUTIVE_REMOTE_FAILURES;
                if abort && let Some(last) = failures.last_mut() {
                    *last = "stopped after repeated Sirv failures".into();
                }
                this.update(cx, |audit, cx| {
                    // Only onto this loop's own job. A slow last file can land after
                    // the user has already started another transfer.
                    if let Some(job) = audit.sirv_job.as_mut()
                        && job.generation == generation
                    {
                        job.done = ix + 1;
                        job.failed = failed;
                        job.failures = failures.clone();
                        if succeeded {
                            audit.sirv_local_presence.insert(key.clone());
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
                    true
                } else {
                    false
                };
                if owns_job {
                    // The pulled files belong in the table: a full rescan, through
                    // the same path a folder change takes.
                    audit.request_path(audit.root.clone(), cx);
                    cx.notify();
                }
            })
            .ok();
        })
        .detach();
    }

    /// Upload every local file Sirv lacks.
    pub(super) fn start_push(&mut self, cx: &mut Context<Self>) {
        self.sirv_confirm = None;
        self.run_push(sirv::SyncState::OnlyLocal, cx);
    }

    /// Deliberately replace every differing remote copy with the local one.
    pub(super) fn start_push_changed(&mut self, cx: &mut Context<Self>) {
        self.run_push(sirv::SyncState::Changed, cx);
    }

    pub(super) fn run_push(&mut self, accept: sirv::SyncState, cx: &mut Context<Self>) {
        let Some(pairing) = &self.sirv_pairing else {
            return;
        };
        if self.sirv_busy() {
            return;
        }
        let Listing::Ready(files) = &pairing.files else {
            return;
        };
        let plan = sirv_push_plan(&self.root, &self.entries, files, accept);
        let kind = if accept == sirv::SyncState::Changed {
            SirvJobKind::PushChanged
        } else {
            SirvJobKind::Push
        };
        self.run_upload_plan(plan, kind, UploadCompletion::None, cx);
    }

    /// The one upload loop used by sync, converted results, Studio handoff and
    /// spins. All four need the same caps, cancellation and named failures.
    pub(super) fn run_upload_plan(
        &mut self,
        plan: Vec<(String, PathBuf)>,
        kind: SirvJobKind,
        completion: UploadCompletion,
        cx: &mut Context<Self>,
    ) {
        let Some(pairing) = &self.sirv_pairing else {
            return;
        };
        if self.sirv_busy() {
            return;
        }
        if plan.is_empty() {
            return;
        }
        let dir = pairing.dir.clone();
        let client = pairing.client.clone();
        let total = plan.len();
        self.sirv_generation = self.sirv_generation.wrapping_add(1);
        let generation = self.sirv_generation;
        self.sirv_job = Some(SirvJob {
            kind,
            done: 0,
            total,
            failed: 0,
            failures: Vec::new(),
            finished: false,
            stopping: false,
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
                        remember_failure(&mut job.failed, &mut job.failures, "stopped".into());
                        true
                    } else {
                        false
                    };
                    if acknowledged {
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
                remember_failure(&mut failed, &mut failures, message);
                this.update(cx, |audit, cx| {
                    if let Some(job) = audit.sirv_job.as_mut()
                        && job.generation == generation
                    {
                        job.failed = failed;
                        job.failures = failures;
                        job.finished = true;
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
                        remember_failure(&mut job.failed, &mut job.failures, "stopped".into());
                        true
                    } else {
                        false
                    };
                    if acknowledged {
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
                            remember_failure(&mut job.failed, &mut job.failures, "stopped".into());
                            true
                        } else {
                            false
                        };
                        if acknowledged {
                            audit.walk_sirv_pairing(cx);
                            cx.notify();
                        }
                    })
                    .ok();
                    return;
                }
                let outcome = cx
                    .background_executor()
                    .spawn({
                        let client = client.clone();
                        let key = key.clone();
                        let path = path.clone();
                        let dir = dir.clone();
                        async move {
                            let size = std::fs::metadata(&path)
                                .map_err(|error| (format!("{key}: {error}"), false))?
                                .len();
                            if size > sirv::MAX_TRANSFER {
                                return Err((
                                    format!(
                                        "{key}: larger than the {}-byte transfer cap",
                                        sirv::MAX_TRANSFER
                                    ),
                                    false,
                                ));
                            }
                            let bytes = std::fs::read(&path)
                                .map_err(|error| (format!("{key}: {error}"), false))?;
                            client
                                .lock()
                                .upload(&format!("{dir}/{key}"), &bytes, sirv::content_type(&key))
                                .map_err(|error| {
                                    let retryable = error.retryable();
                                    (format!("{key}: {error}"), retryable)
                                })
                        }
                    })
                    .await;
                match outcome {
                    Ok(()) => consecutive_remote_failures = 0,
                    Err((message, retryable)) => {
                        consecutive_remote_failures = if retryable {
                            consecutive_remote_failures + 1
                        } else {
                            0
                        };
                        remember_failure(&mut failed, &mut failures, message);
                    }
                }
                let abort = consecutive_remote_failures >= CONSECUTIVE_REMOTE_FAILURES;
                if abort && let Some(last) = failures.last_mut() {
                    *last = "stopped after repeated Sirv failures".into();
                }
                this.update(cx, |audit, cx| {
                    // Only onto this loop's own job. A slow last file can land after
                    // the user has already started another transfer.
                    if let Some(job) = audit.sirv_job.as_mut()
                        && job.generation == generation
                    {
                        job.done = ix + 1;
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
                    true
                } else {
                    false
                };
                if owns_job {
                    if failed == 0 {
                        match completion {
                            UploadCompletion::None => {}
                            UploadCompletion::OpenStudio(url) => cx.open_url(&url),
                            UploadCompletion::Results(urls) => {
                                audit.published_results = urls;
                                audit.report_copied = false;
                            }
                            UploadCompletion::Spins(urls) => audit.published_spins = urls,
                        }
                    }
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
        self.sirv_counts = match self.sirv_pairing.as_ref().map(|pairing| &pairing.files) {
            None | Some(Listing::Walking) | Some(Listing::Failed(_)) => None,
            Some(Listing::Ready(files)) => {
                let mut to_push = 0;
                let mut changed = 0;
                for entry in &self.entries {
                    let Some(key) = sirv::relative_key(&self.root, &entry.path) else {
                        continue;
                    };
                    match sirv::classify(entry.bytes, files.get(&key)) {
                        sirv::SyncState::OnlyLocal => to_push += 1,
                        sirv::SyncState::Changed => changed += 1,
                        sirv::SyncState::Same => {}
                    }
                }
                let to_pull = files
                    .keys()
                    .filter(|key| !self.sirv_local_presence.contains(*key))
                    .count();
                Some((to_push, changed, to_pull))
            }
        };
    }
}
