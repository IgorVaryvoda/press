//! Sirv actions: browsing, pairing, and both transfer directions.

use super::*;

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

impl Audit {
    /// Open the remote-folder browser. Credentials come from the Sirv store; a
    /// missing store opens the browser on an error that names the file to fix.
    pub(super) fn open_sirv_browser(&mut self, cx: &mut Context<Self>) {
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
                    let message = format!(
                        "No Sirv credentials. Add client_id and client_secret to {}",
                        sirv::credentials_path()
                            .map(|path| path.display().to_string())
                            .unwrap_or_else(|| "the ImageGuide config file".into())
                    );
                    self.sirv_browser = Some(SirvBrowser {
                        // Never used on this path: the listing is already an error.
                        client: Arc::new(parking_lot::Mutex::new(sirv::Client::new(
                            sirv::Credentials {
                                client_id: String::new(),
                                client_secret: String::new(),
                            },
                        ))),
                        path: "/".into(),
                        nodes: Some(Err(message)),
                        generation: 0,
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
            nodes: None,
            generation: 0,
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
        browser.nodes = None;
        let client = browser.client.clone();
        let path = browser.path.clone();
        cx.spawn(async move |this, cx| {
            let result = cx
                .background_executor()
                .spawn(async move {
                    client
                        .lock()
                        .readdir(&path)
                        .map_err(|error| error.to_string())
                })
                .await;
            this.update(cx, |audit, cx| {
                if let Some(browser) = audit.sirv_browser.as_mut()
                    && browser.generation == request
                {
                    browser.nodes = Some(result);
                    cx.notify();
                }
            })
        })
        .detach();
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
        let generation = self.dataset_generation;
        let pairing_generation = self.sirv_pairing_generation;
        cx.spawn(async move |this, cx| {
            let walked = cx
                .background_executor()
                .spawn(async move { client.lock().walk(&dir).map_err(|error| error.to_string()) })
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
                let Some(pairing) = audit.sirv_pairing.as_mut() else {
                    return;
                };
                match walked {
                    Ok(nodes) => {
                        pairing.files = Listing::Ready(
                            nodes
                                .into_iter()
                                .filter_map(|node| {
                                    sirv::unpair_remote(&walked_dir, &node.filename)
                                        .map(|key| (key, node))
                                })
                                .collect(),
                        );
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
        self.sirv_browser = None;
        self.cancel_sirv_transfer();
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

    /// Retire any running transfer. The loop checks the generation before each file,
    /// so the file in flight finishes and nothing after it starts.
    pub(super) fn cancel_sirv_transfer(&mut self) {
        self.sirv_generation = self.sirv_generation.wrapping_add(1);
        if let Some(job) = self.sirv_job.as_mut()
            && !job.finished
        {
            job.finished = true;
            job.failures.push("stopped".into());
        }
    }

    /// Open settings, prefilled with whatever is stored.
    pub(super) fn open_settings(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let stored = sirv::load_credentials();
        let mut make_input = |value: Option<String>| {
            cx.new(|cx| {
                let mut state = InputState::new(window, cx);
                if let Some(value) = value {
                    state.set_value(value, window, cx);
                }
                state
            })
        };
        self.settings_panel = Some(SettingsPanel {
            client_id: make_input(stored.as_ref().map(|c| c.client_id.clone())),
            client_secret: make_input(stored.as_ref().map(|c| c.client_secret.clone())),
            cdn_status: None,
            focus_ix: 0,
            focused: false,
        });
        cx.notify();
    }

    /// Store the CDN credentials.
    pub(super) fn save_sirv_settings(&mut self, cx: &mut Context<Self>) {
        let Some(panel) = self.settings_panel.as_mut() else {
            return;
        };
        let client_id = panel.client_id.read(cx).value().trim().to_string();
        let client_secret = panel.client_secret.read(cx).value().trim().to_string();
        if client_id.is_empty() || client_secret.is_empty() {
            panel.cdn_status = Some((false, "Both fields are required.".into()));
            cx.notify();
            return;
        }
        // Report what happened, not what was attempted. A read-only config directory
        // used to look exactly like success.
        panel.cdn_status = Some(
            match sirv::save_credentials(&sirv::Credentials {
                client_id,
                client_secret,
            }) {
                Ok(()) => (true, "Saved.".into()),
                Err(error) => (false, format!("Could not save: {error}")),
            },
        );
        cx.notify();
    }

    /// A transfer is already running. One at a time: the client serialises on
    /// its token cache anyway, and two progress lines would lie about order.
    pub(super) fn sirv_busy(&self) -> bool {
        self.sirv_job.as_ref().is_some_and(|job| !job.finished)
    }

    /// Download every remote file the local folder lacks. Existing files are
    /// never overwritten — pull is additive by design, so it can never destroy
    /// local work.
    pub(super) fn start_pull(&mut self, cx: &mut Context<Self>) {
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
        let remote: Vec<sirv::Node> = files.values().cloned().collect();
        let local_sizes: HashMap<String, u64> = self
            .entries
            .iter()
            .filter_map(|entry| {
                sirv::relative_key(&self.root, &entry.path).map(|key| (key, entry.bytes))
            })
            .collect();
        let plan = sirv::pull_plan(&remote, &dir, &local_sizes, differing);
        if plan.is_empty() {
            return;
        }
        let total = plan.len();
        self.sirv_generation = self.sirv_generation.wrapping_add(1);
        let generation = self.sirv_generation;
        self.sirv_job = Some(SirvJob {
            kind: if differing {
                SirvJobKind::PullChanged
            } else {
                SirvJobKind::Pull
            },
            done: 0,
            total,
            failures: Vec::new(),
            finished: false,
            generation,
        });
        cx.notify();

        let root = self.root.clone();
        cx.spawn(async move |this, cx| {
            let mut failures = Vec::new();
            for (ix, key) in plan.iter().enumerate() {
                if Self::sirv_superseded(&this, cx, generation) {
                    return;
                }
                let outcome = cx
                    .background_executor()
                    .spawn({
                        let client = client.clone();
                        let remote_path = format!("{dir}/{key}");
                        async move { client.lock().download(&remote_path) }
                    })
                    .await;
                // Keep the reason. "1 failed: a.jpg" sends the user hunting;
                // "a.jpg: 403 forbidden" or "a.jpg: No space left on device"
                // says what to do about it.
                let failure = match outcome {
                    Ok(bytes) => {
                        let target = root.join(key);
                        match target.parent().map(std::fs::create_dir_all) {
                            Some(Err(error)) => {
                                Some(format!("{key}: could not create folder: {error}"))
                            }
                            None | Some(Ok(())) => std::fs::write(&target, bytes)
                                .err()
                                .map(|error| format!("{key}: {error}")),
                        }
                    }
                    Err(error) => Some(format!("{key}: {error}")),
                };
                if let Some(message) = failure {
                    failures.push(message);
                }
                this.update(cx, |audit, cx| {
                    // Only onto this loop's own job. A slow last file can land after
                    // the user has already started another transfer.
                    if let Some(job) = audit.sirv_job.as_mut()
                        && job.generation == generation
                    {
                        job.done = ix + 1;
                        job.failures = failures.clone();
                        cx.notify();
                    }
                })
                .ok();
            }
            this.update(cx, |audit, cx| {
                if let Some(job) = audit.sirv_job.as_mut()
                    && job.generation == generation
                {
                    job.finished = true;
                }
                // The pulled files belong in the table: a full rescan, through
                // the same path a folder change takes.
                audit.request_path(audit.root.clone(), cx);
                cx.notify();
            })
            .ok();
        })
        .detach();
    }

    /// Upload every local file Sirv lacks.
    pub(super) fn start_push(&mut self, cx: &mut Context<Self>) {
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
        let dir = pairing.dir.clone();
        let client = pairing.client.clone();
        let plan = sirv_push_plan(&self.root, &self.entries, files, accept);
        if plan.is_empty() {
            return;
        }
        let total = plan.len();
        self.sirv_generation = self.sirv_generation.wrapping_add(1);
        let generation = self.sirv_generation;
        self.sirv_job = Some(SirvJob {
            kind: if accept == sirv::SyncState::Changed {
                SirvJobKind::PushChanged
            } else {
                SirvJobKind::Push
            },
            done: 0,
            total,
            failures: Vec::new(),
            finished: false,
            generation,
        });
        cx.notify();

        let folders = sirv::push_folders(plan.iter().map(|(key, _)| key));

        cx.spawn(async move |this, cx| {
            let mut failures = Vec::new();

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
                failures.push(message);
            }

            for (ix, (key, path)) in plan.iter().enumerate() {
                if Self::sirv_superseded(&this, cx, generation) {
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
                            let mut client = client.lock();
                            match std::fs::read(&path) {
                                Ok(bytes) => client
                                    .upload(
                                        &format!("{dir}/{key}"),
                                        &bytes,
                                        sirv::content_type(&key),
                                    )
                                    .map_err(|error| format!("{key}: {error}")),
                                Err(error) => Err(format!("{key}: {error}")),
                            }
                        }
                    })
                    .await;
                if let Err(message) = outcome {
                    failures.push(message);
                }
                this.update(cx, |audit, cx| {
                    // Only onto this loop's own job. A slow last file can land after
                    // the user has already started another transfer.
                    if let Some(job) = audit.sirv_job.as_mut()
                        && job.generation == generation
                    {
                        job.done = ix + 1;
                        job.failures = failures.clone();
                        cx.notify();
                    }
                })
                .ok();
            }
            this.update(cx, |audit, cx| {
                if let Some(job) = audit.sirv_job.as_mut()
                    && job.generation == generation
                {
                    job.finished = true;
                }
                // Re-list the pair: pushed files must stop reading as new.
                audit.walk_sirv_pairing(cx);
                cx.notify();
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
                let mut local_keys = HashSet::new();
                for entry in &self.entries {
                    let Some(key) = sirv::relative_key(&self.root, &entry.path) else {
                        continue;
                    };
                    local_keys.insert(key.clone());
                    match sirv::classify(entry.bytes, files.get(&key)) {
                        sirv::SyncState::OnlyLocal => to_push += 1,
                        sirv::SyncState::Changed => changed += 1,
                        sirv::SyncState::Same => {}
                    }
                }
                let to_pull = files
                    .keys()
                    .filter(|key| !local_keys.contains(*key))
                    .count();
                Some((to_push, changed, to_pull))
            }
        };
    }
}
