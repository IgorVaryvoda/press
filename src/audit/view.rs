use super::*;

/// The sentence under "No supported images found". A folder straight off a phone is
/// all HEIC, and there "no images" is true, useless, and looks like a broken app.
/// Raw files and macOS packages are counted but never listed, so their counts
/// append the same way rather than leaving a bare "no images".
pub(super) fn empty_folder_detail(
    folder: &str,
    skipped_heic: usize,
    skipped_raw: usize,
    skipped_packages: usize,
) -> String {
    let mut detail = match skipped_heic {
        0 => format!("The “{folder}” folder has no direct supported images."),
        1 => format!("The “{folder}” folder has 1 HEIC file, not supported yet."),
        many => format!("The “{folder}” folder has {many} HEIC files, not supported yet."),
    };
    if skipped_raw > 0 {
        detail.push_str(&format!(
            " Plus {skipped_raw} camera raw {} (counted, not listed).",
            if skipped_raw == 1 { "file" } else { "files" }
        ));
    }
    if skipped_packages > 0 {
        detail.push_str(&format!(
            " Plus {skipped_packages} macOS {} (counted, not listed).",
            if skipped_packages == 1 {
                "package"
            } else {
                "packages"
            }
        ));
    }
    detail
}

/// The line under "Opening…" while a tree walk runs. The same plain figure as the
/// header's count, so the number does not change shape when the walk ends.
pub(super) fn scan_progress_line(found: usize) -> String {
    match found {
        1 => "Found 1 image…".to_string(),
        _ => format!("Found {found} images…"),
    }
}

/// A label for the comparison view, which floats over the picture rather than over
/// a theme surface, so it carries its own dark backing.
/// A proportional bar. The audit is a ranking and a column of numbers does not
/// rank — 632 KB and 104 KB were set in the same size and colour, so the shape of
/// the folder was invisible in a list sorted by exactly that.
pub(super) fn meter(
    id: impl Into<gpui_kit::ElementId>,
    fraction: f32,
    colour: gpui_kit::Hsla,
    height: f32,
) -> Progress {
    let fraction = if fraction.is_finite() {
        fraction.clamp(0., 1.)
    } else {
        0.
    };
    Progress::new(id)
        .value(fraction * 100.)
        .color(colour)
        .h(px(height))
}

impl Audit {
    pub(super) fn sirv_pair_disabled(&self, at_root: bool, listed: bool) -> bool {
        at_root || !listed || self.batch_folders.is_some() || self.scan_blocks_delivery()
    }

    fn sirv_reconciliation(&self, cx: &mut Context<Self>) -> Option<gpui_kit::AnyElement> {
        let pairing = self.sirv_pairing.as_ref()?;
        let busy = self.sirv_busy() || self.scan_blocks_delivery();
        let stopping = self.sirv_job.as_ref().is_some_and(|job| job.stopping);
        let ready = matches!(pairing.files, Listing::Ready(_));
        let (to_push, changed, to_pull) = self.sirv_counts.unwrap_or((0, 0, 0));
        let push_changed_confirmed = self.sirv_confirm == Some(SirvJobKind::PushChanged);
        let pull_changed_confirmed = self.sirv_confirm == Some(SirvJobKind::PullChanged);

        let job_line = self.sirv_job.as_ref().map(|job| {
            let verb = match job.kind {
                SirvJobKind::Pull => "Pulling",
                SirvJobKind::PullChanged => "Taking from Sirv",
                SirvJobKind::Push => "Pushing",
                SirvJobKind::PushChanged => "Overwriting on Sirv",
                SirvJobKind::Publish => "Publishing",
            };
            let current = job
                .current
                .as_deref()
                .map(|name| format!(" · {name}"))
                .unwrap_or_default();
            let failures = if job.failed == 0 {
                String::new()
            } else {
                let rest = job.failed.saturating_sub(job.failures.len());
                format!(
                    " · {} failed: {}{}",
                    job.failed,
                    job.failures.join(", "),
                    if rest == 0 {
                        String::new()
                    } else {
                        format!(" and {rest} more")
                    }
                )
            };
            if job.finished {
                format!("{verb}: {} of {} complete{failures}", job.done, job.total)
            } else if job.stopping {
                format!(
                    "Stopping {verb}: {} of {} complete{current}{failures}",
                    job.done, job.total
                )
            } else {
                format!(
                    "{verb}: {} of {} complete{current}{failures}",
                    job.done, job.total
                )
            }
        });

        Some(
            div()
                .debug_selector(|| "sirv-reconciliation".into())
                .flex()
                .flex_col()
                .gap_2()
                .px_3()
                .py_2()
                .bg(cx.theme().secondary)
                .border_b_1()
                .border_color(cx.theme().border)
                .child(
                    div()
                        .flex()
                        .flex_wrap()
                        .items_center()
                        .gap_2()
                        .child(
                            div()
                                .font_family("SF Pro Display")
                                .font_weight(FontWeight::SEMIBOLD)
                                .text_size(px(13.))
                                .child("Paired with Sirv"),
                        )
                        .child(
                            div()
                                .flex_1()
                                .min_w(px(220.))
                                .font_family(cx.theme().mono_font_family.clone())
                                .text_size(px(11.))
                                .text_color(cx.theme().muted_foreground)
                                .whitespace_nowrap()
                                .overflow_hidden()
                                .text_ellipsis()
                                .child(format!("{}  ↔  Sirv:{}", self.root.display(), pairing.dir)),
                        )
                        .child(
                            Button::new("sirv-refresh-pair")
                                .small()
                                .ghost()
                                .label(if ready { "Refresh" } else { "Listing…" })
                                .disabled(busy || !ready)
                                .on_click(
                                    cx.listener(|audit, _, _, cx| audit.walk_sirv_pairing(cx)),
                                ),
                        )
                        .child(
                            Button::new("sirv-change-pair")
                                .small()
                                .ghost()
                                .label("Change folder")
                                .disabled(busy)
                                .on_click(
                                    cx.listener(|audit, _, _, cx| audit.open_sirv_browser(cx)),
                                ),
                        )
                        .child(
                            Button::new("sirv-unpair-audit")
                                .small()
                                .ghost()
                                .label("Unpair")
                                .disabled(busy)
                                .on_click(cx.listener(|audit, _, _, cx| audit.unpair_sirv(cx))),
                        ),
                )
                .child(
                    div()
                        .flex()
                        .flex_wrap()
                        .items_center()
                        .gap_2()
                        .child(
                            Button::new("sirv-filter-local")
                                .small()
                                .ghost()
                                .selected(self.sirv_scope == Some(SirvScope::OnlyLocal))
                                .label(format!("Local only {to_push}"))
                                .disabled(!ready)
                                .on_click(cx.listener(|audit, _, _, cx| {
                                    audit.set_sirv_scope(SirvScope::OnlyLocal, cx)
                                })),
                        )
                        .child(
                            Button::new("sirv-filter-changed")
                                .small()
                                .ghost()
                                .selected(self.sirv_scope == Some(SirvScope::Changed))
                                .label(format!("Differing {changed}"))
                                .disabled(!ready)
                                .on_click(cx.listener(|audit, _, _, cx| {
                                    audit.set_sirv_scope(SirvScope::Changed, cx)
                                })),
                        )
                        .child(
                            Button::new("sirv-filter-remote")
                                .small()
                                .ghost()
                                .selected(self.sirv_scope == Some(SirvScope::OnlyRemote))
                                .label(format!("Sirv only {to_pull}"))
                                .disabled(!ready)
                                .on_click(cx.listener(|audit, _, _, cx| {
                                    audit.set_sirv_scope(SirvScope::OnlyRemote, cx)
                                })),
                        )
                        .child(div().w(px(1.)).h(px(20.)).bg(cx.theme().border))
                        .child(
                            Button::new("sirv-push-audit")
                                .small()
                                .outline()
                                .icon(IconName::ArrowUp)
                                .label(format!("Push {to_push}"))
                                .disabled(busy || to_push == 0)
                                .on_click(cx.listener(|audit, _, _, cx| audit.start_push(cx))),
                        )
                        .child(
                            Button::new("sirv-pull-audit")
                                .small()
                                .outline()
                                .icon(IconName::ArrowDown)
                                .label(format!("Pull {to_pull}"))
                                .disabled(busy || to_pull == 0)
                                .on_click(cx.listener(|audit, _, _, cx| audit.start_pull(cx))),
                        )
                        .when(changed > 0, |row| {
                            row.child(
                                Button::new("sirv-push-changed-audit")
                                    .small()
                                    .when(push_changed_confirmed, |button| button.primary())
                                    .when(!push_changed_confirmed, |button| button.ghost())
                                    .label(if push_changed_confirmed {
                                        format!("Really overwrite {changed} on Sirv?")
                                    } else {
                                        format!("Overwrite {changed} on Sirv")
                                    })
                                    .disabled(busy)
                                    .on_click(cx.listener(|audit, _, _, cx| {
                                        if audit.sirv_confirm == Some(SirvJobKind::PushChanged) {
                                            audit.sirv_confirm = None;
                                            audit.start_push_changed(cx);
                                        } else {
                                            audit.sirv_confirm = Some(SirvJobKind::PushChanged);
                                            cx.notify();
                                        }
                                    })),
                            )
                            .child(
                                Button::new("sirv-pull-changed-audit")
                                    .small()
                                    .when(pull_changed_confirmed, |button| button.primary())
                                    .when(!pull_changed_confirmed, |button| button.ghost())
                                    .label(if pull_changed_confirmed {
                                        format!("Really replace {changed} local files?")
                                    } else {
                                        format!("Take {changed} from Sirv")
                                    })
                                    .disabled(busy)
                                    .on_click(cx.listener(|audit, _, _, cx| {
                                        if audit.sirv_confirm == Some(SirvJobKind::PullChanged) {
                                            audit.sirv_confirm = None;
                                            audit.start_pull_changed(cx);
                                        } else {
                                            audit.sirv_confirm = Some(SirvJobKind::PullChanged);
                                            cx.notify();
                                        }
                                    })),
                            )
                        })
                        .when(busy, |row| {
                            row.child(
                                Button::new("sirv-stop-audit")
                                    .small()
                                    .outline()
                                    .label(if stopping { "Stopping…" } else { "Stop" })
                                    .disabled(stopping)
                                    .on_click(cx.listener(|audit, _, _, cx| {
                                        audit.cancel_sirv_transfer();
                                        cx.notify();
                                    })),
                            )
                        }),
                )
                .when_some(job_line, |bar, line| {
                    bar.child(
                        div()
                            .font_family(cx.theme().mono_font_family.clone())
                            .text_size(px(11.))
                            .text_color(
                                if self.sirv_job.as_ref().is_some_and(|job| job.failed > 0) {
                                    cx.theme().yellow
                                } else {
                                    cx.theme().muted_foreground
                                },
                            )
                            .child(line),
                    )
                })
                .into_any_element(),
        )
    }

    fn sirv_remote_only_view(&self, cx: &mut Context<Self>) -> gpui_kit::AnyElement {
        uniform_list(
            "sirv-remote-only-list",
            self.sirv_remote_only.len(),
            cx.processor(|audit, range: std::ops::Range<usize>, _, cx| {
                range
                    .filter_map(|row| audit.sirv_remote_only.get(row))
                    .map(|key| {
                        div()
                            .flex()
                            .w_full()
                            .items_center()
                            .gap_2()
                            .h(px(36.))
                            .px_3()
                            .border_b_1()
                            .border_color(cx.theme().border)
                            .child(
                                Icon::new(IconName::Globe).text_color(cx.theme().muted_foreground),
                            )
                            .child(
                                div()
                                    .flex_1()
                                    .min_w_0()
                                    .text_size(px(12.))
                                    .child(key.clone()),
                            )
                            .child(
                                div()
                                    .text_size(px(11.))
                                    .text_color(cx.theme().muted_foreground)
                                    .child("only on Sirv"),
                            )
                    })
                    .collect::<Vec<_>>()
            }),
        )
        .size_full()
        .into_any_element()
    }

    /// The remote-folder browser: credentials and choosing one remote folder.
    /// Reconciliation belongs to the audit after the folder is paired.
    fn sirv_browser_view(
        &self,
        browser: &SirvBrowser,
        cx: &mut Context<Self>,
    ) -> gpui_kit::AnyElement {
        let at_root = browser.path.trim_end_matches('/').is_empty();

        let body: gpui_kit::AnyElement = match browser.nodes.as_ref() {
            None => div()
                .flex()
                .items_center()
                .gap_2()
                .text_size(px(12.))
                .text_color(cx.theme().muted_foreground)
                .child(IconName::LoaderCircle)
                .child(format!("Listing {}…", browser.path))
                .into_any_element(),
            Some(Err(message)) => div()
                .flex()
                .flex_col()
                .items_start()
                .gap_2()
                .child(
                    div()
                        .debug_selector(|| "sirv-error".into())
                        .text_size(px(12.))
                        .text_color(cx.theme().yellow)
                        .child(message.clone()),
                )
                .child(
                    // This branch only renders once a listing has landed, so no
                    // listing is ever in flight under it and the button stays live.
                    div().debug_selector(|| "sirv-retry".into()).child(
                        Button::new("sirv-retry")
                            .outline()
                            .small()
                            .label("Retry")
                            .on_click(cx.listener(|audit, _, _, cx| {
                                if let Some(browser) = audit.sirv_browser.as_mut() {
                                    Self::browse_sirv_path(browser, cx);
                                }
                                cx.notify();
                            })),
                    ),
                )
                .into_any_element(),
            Some(Ok(nodes)) => {
                // The filter narrows what is already on screen; it never lists
                // again, so typing in a folder of hundreds costs no request.
                let needle = self.sirv_browser_filter.trim().to_lowercase();
                let total_folders = nodes.iter().filter(|node| node.is_folder()).count();
                let mut rows: Vec<gpui_kit::AnyElement> = Vec::new();
                if browser.path != "/" {
                    rows.push(
                        Button::new("sirv-up")
                            .ghost()
                            .small()
                            .icon(IconName::ArrowUp)
                            .label("..")
                            .on_click(cx.listener(|audit, _, _, cx| audit.ascend_sirv(cx)))
                            .into_any_element(),
                    );
                }
                let mut shown: usize = 0;
                for node in nodes.iter().filter(|node| node.is_folder()) {
                    let name = node
                        .filename
                        .rsplit('/')
                        .next()
                        .unwrap_or(&node.filename)
                        .to_string();
                    if !needle.is_empty() && !name.to_lowercase().contains(&needle) {
                        continue;
                    }
                    let ix = shown;
                    shown += 1;
                    let descend_to = name.clone();
                    rows.push(
                        div()
                            .debug_selector(move || format!("sirv-dir-{ix}"))
                            .child(
                                Button::new(("sirv-dir", ix))
                                    .ghost()
                                    .small()
                                    .icon(IconName::FolderOpen)
                                    .label(name)
                                    .on_click(cx.listener(move |audit, _, _, cx| {
                                        audit.descend_sirv(descend_to.clone(), cx);
                                    })),
                            )
                            .into_any_element(),
                    );
                }
                if shown == 0 {
                    rows.push(
                        div()
                            .text_size(px(12.))
                            .text_color(cx.theme().muted_foreground)
                            .child(if !needle.is_empty() && total_folders > 0 {
                                "No subfolders match."
                            } else {
                                "No subfolders."
                            })
                            .into_any_element(),
                    );
                }
                div()
                    .flex()
                    .flex_col()
                    .items_start()
                    .gap_2()
                    .child(
                        // The box is not in `text_input_focused`, so without
                        // this every key typed here also reaches the audit
                        // root: a space would toggle the row behind the modal,
                        // enter would open it, the arrows would move its
                        // cursor. Escape, tab and the editing keys keep going
                        // so the browser still closes, focus still travels and
                        // the box stays typeable; stopping those too would
                        // swallow the very text it edits.
                        div()
                            .debug_selector(|| "sirv-filter".into())
                            .w_full()
                            .on_key_down(cx.listener(|_, event: &gpui_kit::KeyDownEvent, _, cx| {
                                let key = event.keystroke.key.as_str();
                                let modifiers = &event.keystroke.modifiers;
                                let plain = !modifiers.control
                                    && !modifiers.platform
                                    && !modifiers.alt
                                    && !modifiers.function;
                                let editing = matches!(key, "backspace" | "delete" | "tab")
                                    || (plain
                                        && key.chars().count() == 1
                                        && key.chars().all(|c| c.is_alphanumeric()));
                                if key != "escape" && !editing {
                                    cx.stop_propagation();
                                }
                            }))
                            .child(
                                Input::new(&self.sirv_browser_filter_input)
                                    .small()
                                    .cleanable(true)
                                    .prefix(IconName::Search),
                            ),
                    )
                    .child(
                        div()
                            .id("sirv-list")
                            .flex()
                            .flex_col()
                            .items_start()
                            .gap_0p5()
                            .max_h(px(280.))
                            .overflow_y_scroll()
                            .children(rows),
                    )
                    .into_any_element()
            }
        };

        div()
            .w(px(440.))
            .flex()
            .flex_col()
            .gap_3()
            .p_4()
            .rounded_lg()
            .bg(cx.theme().secondary)
            .border_1()
            .border_color(cx.theme().border)
            .child(
                div()
                    .flex()
                    .flex_col()
                    .gap_0p5()
                    .child(
                        div()
                            .font_family("SF Pro Display")
                            .text_size(px(15.))
                            .font_weight(FontWeight::SEMIBOLD)
                            .text_color(cx.theme().foreground)
                            .child("Sync with Sirv"),
                    )
                    .when(!browser.needs_credentials, |header| {
                        header.child(
                            div()
                                .font_family(cx.theme().mono_font_family.clone())
                                .text_size(px(11.))
                                .text_color(cx.theme().muted_foreground)
                                .whitespace_nowrap()
                                .overflow_hidden()
                                .text_ellipsis()
                                .child(browser.path.clone()),
                        )
                    }),
            )
            .child(body)
            .child(
                div()
                    .flex()
                    .gap_2()
                    .child(
                        Button::new("sirv-close")
                            .ghost()
                            .small()
                            .label("Close")
                            .on_click(cx.listener(|audit, _, window, cx| {
                                audit.close_sirv_browser(window, cx);
                            })),
                    )
                    .when(browser.needs_credentials, |row| {
                        row.child(
                            Button::new("sirv-setup")
                                .primary()
                                .small()
                                .label("Set up Sirv…")
                                .on_click(cx.listener(|audit, _, window, cx| {
                                    audit.sirv_browser = None;
                                    audit.open_settings(window, cx);
                                })),
                        )
                    })
                    .when(!browser.needs_credentials, |row| {
                        row.child(
                            div().debug_selector(|| "sirv-pair".into()).child(
                                Button::new("sirv-pair")
                                    .primary()
                                    .small()
                                    .label(if at_root {
                                        "Open a folder to pair it"
                                    } else {
                                        "Pair this folder"
                                    })
                                    .disabled(self.sirv_pair_disabled(
                                        at_root,
                                        matches!(browser.nodes, Some(Ok(_))),
                                    ))
                                    .on_click(cx.listener(|audit, _, window, cx| {
                                        audit.pair_sirv(cx);
                                        Self::restore_audit_focus(window, cx);
                                    })),
                            ),
                        )
                    }),
            )
            .into_any_element()
    }

    /// Bytes of what is on screen. With a filter active the folder total would be
    /// describing files the list is not showing.
    pub(super) fn visible_bytes(&self) -> u64 {
        self.visible_bytes
    }
}

impl Render for Audit {
    // Three shapes share this method — empty state, comparison, and the list — so it
    // erases to one type rather than making the caller's `impl Trait` pick a winner.
    #[allow(refining_impl_trait)]
    fn render(&mut self, window: &mut Window, cx: &mut Context<Self>) -> gpui_kit::AnyElement {
        let count = if self.sirv_scope == Some(SirvScope::OnlyRemote) {
            self.sirv_remote_only.len()
        } else {
            self.visible.len()
        };

        let title = match self.root.file_name() {
            Some(name) => format!("{} — Press", name.to_string_lossy()),
            None => "Press".to_string(),
        };
        if title != self.titled {
            window.set_window_title(&title);
            self.titled = title;
        }

        // Cheap enough to compare every frame, and it means a crash still leaves the
        // last good size and folder on disk. The write itself is delayed.
        let viewport = window.viewport_size();
        let current = settings::Settings {
            width: Some(f32::from(viewport.width)),
            height: Some(f32::from(viewport.height)),
            folder: self.root.is_dir().then(|| self.root.clone()),
            recent_folders: self.recent_folders.clone(),
            columns: self.column_prefs,
            // Replace mode is not remembered, so the snapshot this compares
            // against must not hold it either, or every frame looks like a change.
            output: match self.output {
                Output::Replace => Output::Optimized,
                ref output => output.clone(),
            },
            include_subfolders: self.include_subfolders,
            sidebar_collapsed: !self.sidebar_open,
            // Read back from the process rather than kept a second time here: the
            // speed is set once at startup and nothing in the window changes it, so
            // a copy on `Audit` would only be a copy to forget to update.
            avif_speed: crate::avif::configured_speed(),
        };
        if current != self.settings {
            self.remember_settings(current, cx);
        }

        if let Some(table) = self.table.clone() {
            // The table lives left of any open rail; handing it the full viewport
            // would make every column calculation 300px too wide. A closed rail
            // takes nothing, and the list gets the whole window.
            let (root_left, root_right) = root_horizontal_chrome(window);
            let width = f32::from(viewport.width)
                - self.rail_width()
                - self.browser_width(window)
                - root_left
                - root_right;
            let prefs = self.column_prefs;
            // A failure lives in the result cell too. A run where nothing landed
            // would otherwise drop the column that carries its only marker.
            let show_result = !self.results.is_empty() || !self.failures.is_empty();
            let show_sync = self.sirv_counts.is_some();
            let signature = (width.round().max(0.) as u32, prefs, show_result, show_sync);
            if self.table_signature != Some(signature) {
                self.table_signature = Some(signature);
                cx.defer(move |cx| {
                    table.update(cx, |table, cx| {
                        table.delegate_mut().set_viewport_width(
                            width,
                            prefs,
                            show_result,
                            show_sync,
                        );
                        table.refresh(cx);
                        cx.notify();
                    });
                });
            }
        }

        if let Some(scanning) = self.scanning.as_ref() {
            let label = scanning.clone();
            let found = self.scan_found.map(scan_progress_line);
            let cancellable = self.scan_cancellation.is_some();
            return div()
                .size_full()
                .flex()
                .items_center()
                .justify_center()
                .p_4()
                .bg(cx.theme().background)
                .child(
                    div()
                        .flex()
                        .flex_col()
                        .items_center()
                        .gap_2()
                        .w(px(420.))
                        .px_4()
                        .py_4()
                        .rounded_lg()
                        .bg(cx.theme().secondary)
                        .border_1()
                        .border_color(cx.theme().border)
                        .child(
                            div()
                                .font_family("SF Pro Display")
                                .text_size(px(18.))
                                .font_weight(FontWeight::SEMIBOLD)
                                .text_color(cx.theme().foreground)
                                .child(format!("Opening {label}…")),
                        )
                        .children(found.map(|found| {
                            div()
                                .font_family(cx.theme().mono_font_family.clone())
                                .text_size(px(12.))
                                .text_color(cx.theme().foreground)
                                .child(found)
                        }))
                        .child(
                            div()
                                .text_size(px(12.))
                                .text_color(cx.theme().muted_foreground)
                                .child(
                                    "The current folder stays untouched until the scan finishes.",
                                ),
                        )
                        // A one-file probe is over before a click; only a walk with a
                        // token gets the way out, and it goes back to the last folder.
                        .children(cancellable.then(|| {
                            div().debug_selector(|| "cancel-scan".into()).child(
                                Button::new("cancel-scan")
                                    .small()
                                    .outline()
                                    .label("Cancel")
                                    .on_click(cx.listener(|audit, _, _, cx| audit.cancel_scan(cx))),
                            )
                        }))
                        .children(self.shortcuts_open.then(|| self.shortcuts_overlay(cx))),
                )
                .on_drop(
                    cx.listener(|audit, paths: &gpui_kit::ExternalPaths, window, cx| {
                        audit.request_paths(paths.paths().to_vec(), window, cx);
                    }),
                )
                .into_any_element();
        }

        if self.entries.is_empty() && self.folders.is_empty() && self.root.as_os_str().is_empty() {
            return div()
                .size_full()
                .flex()
                .items_center()
                .justify_center()
                .p_4()
                .bg(cx.theme().background)
                .border_2()
                .border_color(if self.drag_over {
                    cx.theme().drag_border
                } else {
                    gpui_kit::transparent_black()
                })
                .child(
                    div()
                        .flex()
                        .flex_col()
                        .items_center()
                        .gap_2()
                        .w(px(400.))
                        .px_4()
                        .py_6()
                        .child(
                            div()
                                .font_family("SF Pro Display")
                                .text_size(px(19.))
                                .font_weight(FontWeight::SEMIBOLD)
                                .text_color(cx.theme().foreground)
                                .child("Audit images"),
                        )
                        .child(
                            div()
                                .text_size(px(12.))
                                .text_color(cx.theme().muted_foreground)
                                .text_center()
                                .child(
                                    "Nothing is uploaded. Press audits first; Convert writes optimized copies and leaves originals unchanged.",
                                ),
                        )
                        .child(
                            div()
                                .flex()
                                .gap_2()
                                .pt_2()
                                .child(
                                    Button::new("empty-folder")
                                        .primary()
                                        .icon(IconName::Folder)
                                        .label("Open folder…")
                                        .on_click(
                                            cx.listener(|audit, _, _, cx| audit.pick(true, cx)),
                                        ),
                                )
                                .child(
                                    Button::new("empty-file")
                                        .outline()
                                        .icon(IconName::File)
                                        .label("Open images…")
                                        .on_click(
                                            cx.listener(|audit, _, _, cx| audit.pick(false, cx)),
                                        ),
                                ),
                        )
                        .child(
                            div()
                                .pt_2()
                                .text_size(px(12.))
                                .text_color(cx.theme().muted_foreground)
                                .child(
                                    "Drop one or more sibling folders, or any number of images anywhere in this window",
                                ),
                        ),
                )
                .on_drag_move(cx.listener(
                    |audit, _: &gpui_kit::DragMoveEvent<gpui_kit::ExternalPaths>, _, cx| {
                        if !audit.drag_over {
                            audit.drag_over = true;
                            cx.notify();
                        }
                    },
                ))
                .on_drop(cx.listener(|audit, paths: &gpui_kit::ExternalPaths, window, cx| {
                    audit.drag_over = false;
                    audit.request_paths(paths.paths().to_vec(), window, cx);
                }))
                .into_any_element();
        }

        if self.settings_panel.is_some() {
            let view = self.settings_panel_view(cx);
            // The click that opened the panel left focus on the button it
            // replaced; take focus next frame so typing lands in the first
            // field. Once only: after that the field with focus is whichever
            // one Tab or a click chose. Nothing else in the framework moves Tab
            // between inputs, so this panel cycles them itself.
            cx.defer_in(window, |audit, window, cx| {
                if let Some(panel) = audit.settings_panel.as_mut()
                    && !panel.focused
                {
                    panel.focused = true;
                    let handle = panel.client_id.read(cx).focus_handle(cx);
                    window.focus(&handle, cx);
                }
            });
            let workspace = self.audit_workspace(count, window, cx);
            return div()
                .size_full()
                .relative()
                .child(workspace)
                .child(
                    div()
                        .debug_selector(|| "settings-scrim".into())
                        .absolute()
                        .inset_0()
                        .flex()
                        .items_center()
                        .justify_center()
                        .bg(cx.theme().background.opacity(0.82))
                        .on_key_down(cx.listener(
                            |audit, event: &gpui_kit::KeyDownEvent, window, cx| {
                                match event.keystroke.key.as_str() {
                                    "escape" => {
                                        audit.close_settings(window, cx);
                                    }
                                    "enter" if !event.keystroke.modifiers.modified() => {
                                        let can_save =
                                            audit.settings_panel.as_ref().is_some_and(|panel| {
                                                credentials_complete(
                                                    &panel.client_id.read(cx).value(),
                                                    &panel.client_secret.read(cx).value(),
                                                )
                                            });
                                        if can_save {
                                            cx.stop_propagation();
                                            audit.save_sirv_settings(cx);
                                        }
                                    }
                                    "tab" => {
                                        const FIELDS: usize = 2;
                                        let direction = if event.keystroke.modifiers.shift {
                                            FIELDS - 1
                                        } else {
                                            1
                                        };
                                        if let Some(panel) = audit.settings_panel.as_mut() {
                                            panel.focus_ix = (panel.focus_ix + direction) % FIELDS;
                                            let handle = [
                                                panel.client_id.read(cx).focus_handle(cx),
                                                panel.client_secret.read(cx).focus_handle(cx),
                                            ][panel.focus_ix]
                                                .clone();
                                            window.focus(&handle, cx);
                                        }
                                    }
                                    _ => {}
                                }
                            },
                        ))
                        .child(view),
                )
                .into_any_element();
        }

        if let Some(browser) = self.sirv_browser.take() {
            let view = self.sirv_browser_view(&browser, cx);
            let focus = browser.focus.clone();
            self.sirv_browser = Some(browser);
            // The click that opened the browser left focus on the header
            // button it replaced, so Escape had nowhere to land. Same fix as
            // the comparison: take focus next frame, once this tree exists.
            cx.defer_in(window, |audit, window, cx| {
                if let Some(browser) = audit.sirv_browser.as_mut()
                    && !browser.focused
                {
                    browser.focused = true;
                    window.focus(&browser.focus, cx);
                }
            });
            let workspace = self.audit_workspace(count, window, cx);
            return div()
                .size_full()
                .relative()
                .child(workspace)
                .child(
                    div()
                        .debug_selector(|| "sirv-scrim".into())
                        .absolute()
                        .inset_0()
                        .flex()
                        .items_center()
                        .justify_center()
                        .bg(cx.theme().background.opacity(0.82))
                        .track_focus(&focus)
                        .on_key_down(cx.listener(
                            |audit, event: &gpui_kit::KeyDownEvent, window, cx| {
                                if event.keystroke.key == "escape" {
                                    audit.close_sirv_browser(window, cx);
                                }
                            },
                        ))
                        .child(view),
                )
                .into_any_element();
        }

        if let Some(comparison) = self.compare.take() {
            // Taken and put back so the view can borrow `self` immutably while the
            // listeners it builds hold a mutable handle to the same entity.
            let view = self.compare_view(&comparison, window, cx);
            self.compare = Some(comparison);
            // The click or Enter that opened this view left focus inside the list
            // it replaced. Take focus once after the compare tree exists, then
            // leave its buttons in charge of their own keyboard input.
            cx.defer_in(window, |audit, window, cx| {
                if let Some(comparison) = audit.compare.as_mut()
                    && !comparison.focused
                {
                    comparison.focused = true;
                    window.focus(&audit.focus, cx);
                }
            });
            return div()
                .size_full()
                .relative()
                .track_focus(&self.focus)
                .on_key_down(cx.listener(|audit, event: &gpui_kit::KeyDownEvent, _, cx| {
                    match event.keystroke.key.as_str() {
                        "escape" => {
                            audit.compare = None;
                            cx.notify();
                        }
                        "right" | "down" => audit.step_compare(1, cx),
                        "left" | "up" => audit.step_compare(-1, cx),
                        "f" => {
                            if let Some(comparison) = audit.compare.as_mut() {
                                comparison.zoom = None;
                                comparison.pan = (0., 0.);
                                cx.notify();
                            }
                        }
                        "1" => {
                            if let Some(comparison) = audit.compare.as_mut() {
                                comparison.zoom = Some(1.);
                                comparison.pan = (0., 0.);
                                cx.notify();
                            }
                        }
                        _ => {}
                    }
                }))
                .child(view)
                .into_any_element();
        }

        self.audit_workspace(count, window, cx)
    }
}

impl Audit {
    fn audit_workspace(
        &mut self,
        count: usize,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> gpui_kit::AnyElement {
        // What the list has to itself. The floating bar has to fit inside it,
        // and at the minimum window with a rail open that is 460px.
        let (root_left, root_right) = root_horizontal_chrome(window);
        let list_width = f32::from(window.viewport_size().width)
            - self.rail_width()
            - self.browser_width(window)
            - root_left
            - root_right;
        let persistent_browser = self.browser_persistent(window);
        if persistent_browser {
            self.browser_overlay = false;
        }
        let persistent_sidebar = persistent_browser.then(|| self.folder_sidebar(cx));
        let overlay_sidebar =
            (!persistent_browser && self.browser_overlay).then(|| self.folder_sidebar(cx));
        div()
            .size_full()
            .flex()
            .flex_col()
            .bg(cx.theme().background)
            .track_focus(&self.focus)
            .on_mouse_move(
                cx.listener(|audit, event: &gpui_kit::MouseMoveEvent, _, cx| {
                    audit.move_marquee(event, cx);
                }),
            )
            .on_mouse_up(
                gpui_kit::MouseButton::Left,
                cx.listener(|audit, _: &gpui_kit::MouseUpEvent, _, cx| {
                    audit.finish_marquee(cx);
                }),
            )
            // Always bordered, so a hovering drag recolours the frame instead of
            // shifting the whole window's contents inward by two pixels.
            .border_2()
            .border_color(if self.drag_over {
                cx.theme().drag_border
            } else {
                gpui_kit::transparent_black()
            })
            .on_drag_move(cx.listener(
                |audit, _: &gpui_kit::DragMoveEvent<gpui_kit::ExternalPaths>, _, cx| {
                    if !audit.drag_over {
                        audit.drag_over = true;
                        cx.notify();
                    }
                },
            ))
            .on_key_down(
                cx.listener(|audit, event: &gpui_kit::KeyDownEvent, window, cx| {
                    if audit.text_input_focused(window, cx) {
                        return;
                    }
                    // The filter box swallows its own keys, so these only fire when the
                    // list itself has focus. Shift turns any move into a selection
                    // drag from the anchor.
                    let extend = event.keystroke.modifiers.shift;
                    match event.keystroke.key.as_str() {
                        "down" => audit.step_cursor_vertical(1, extend, window, cx),
                        "up" => audit.step_cursor_vertical(-1, extend, window, cx),
                        "left" => audit.step_cursor_lateral(-1, extend, window, cx),
                        "right" => audit.step_cursor_lateral(1, extend, window, cx),
                        "pagedown" => audit.step_cursor(10, extend, window, cx),
                        "pageup" => audit.step_cursor(-10, extend, window, cx),
                        "home" => audit.step_cursor(isize::MIN / 2, extend, window, cx),
                        "end" => audit.step_cursor(isize::MAX / 2, extend, window, cx),
                        "escape" => {
                            // An open dialog outranks the selection: escape
                            // puts the list down only when nothing covers it.
                            if audit.shortcuts_open {
                                audit.shortcuts_open = false;
                                cx.notify();
                            } else if !audit.selected.is_empty() && !audit.converting {
                                audit.selected.clear();
                                audit.selection_changed(cx);
                            }
                        }
                        "a" if event.keystroke.modifiers.control
                            || event.keystroke.modifiers.platform =>
                        {
                            // Select what the list shows, not what the folder holds:
                            // a filter that hides files from the list must hide them
                            // from Convert too.
                            if !audit.converting {
                                audit.selected.extend(audit.visible.iter().copied());
                                audit.selection_changed(cx);
                            }
                        }
                        "," if event.keystroke.modifiers.control
                            || event.keystroke.modifiers.platform =>
                        {
                            audit.open_settings(window, cx);
                        }
                        // The filter box names its own shortcut in its
                        // placeholder, so this has to honour it everywhere.
                        "k" if event.keystroke.modifiers.control
                            || event.keystroke.modifiers.platform =>
                        {
                            window.focus(&audit.filter_input.read(cx).focus_handle(cx), cx);
                        }
                        "space" => audit.toggle_cursor_selection(cx),
                        "?" => {
                            audit.shortcuts_open = true;
                            cx.notify();
                        }
                        "enter" => {
                            if !audit.converting
                                && let Some(entry) = audit.entry_at(audit.cursor)
                            {
                                audit.open_preview(entry, cx);
                            }
                        }
                        _ => {}
                    }
                }),
            )
            .on_drop(
                cx.listener(|audit, paths: &gpui_kit::ExternalPaths, window, cx| {
                    audit.drag_over = false;
                    audit.request_paths(paths.paths().to_vec(), window, cx);
                }),
            )
            .child(self.header(window, cx))
            // Audit on the left, the output panel on the right: the working
            // area and the settings column split below one shared header.
            .child(
                div()
                    .relative()
                    .flex()
                    .flex_1()
                    .overflow_hidden()
                    .children(persistent_sidebar)
                    .child(
                        // The list, with the action bar floating over its foot.
                        // The bar is four words wide; reserving a column for it
                        // would cost the list a fifth of the window.
                        div()
                            .relative()
                            .flex()
                            .flex_col()
                            .flex_1()
                            .min_w_0()
                            .overflow_hidden()
                            .children(self.sirv_reconciliation(cx))
                            .child(self.audit_content(count, window, cx))
                            .children(
                                (self.sirv_scope != Some(SirvScope::OnlyRemote)
                                    && !self.visible.is_empty())
                                .then(|| self.action_bar(list_width, cx)),
                            ),
                    )
                    .children(self.rail_view(cx))
                    .children(overlay_sidebar.map(|sidebar| {
                        div()
                            .id("folder-overlay")
                            .absolute()
                            .inset_0()
                            .on_key_down(cx.listener(
                                |audit, event: &gpui_kit::KeyDownEvent, window, cx| {
                                    if event.keystroke.key == "escape" {
                                        audit.close_browser_overlay(window, cx);
                                        cx.stop_propagation();
                                    }
                                },
                            ))
                            .child(
                                div()
                                    .absolute()
                                    .inset_0()
                                    .bg(cx.theme().background.opacity(0.55))
                                    .debug_selector(|| "folder-overlay-backdrop".into())
                                    .on_mouse_down(
                                        gpui_kit::MouseButton::Left,
                                        cx.listener(|audit, _, window, cx| {
                                            audit.close_browser_overlay(window, cx);
                                            cx.stop_propagation();
                                        }),
                                    ),
                            )
                            .child(
                                div()
                                    .relative()
                                    .w(px(browser::SIDEBAR_WIDTH))
                                    .h_full()
                                    .shadow_lg()
                                    .occlude()
                                    .child(sidebar)
                                    .child(
                                        div()
                                            .absolute()
                                            .top_2()
                                            .right_2()
                                            .debug_selector(|| "folder-overlay-close".into())
                                            .child(
                                                Button::new("folder-overlay-close")
                                                    .small()
                                                    .ghost()
                                                    .icon(IconName::Close)
                                                    .tooltip("Close folder browser")
                                                    .on_click(cx.listener(
                                                        |audit, _, window, cx| {
                                                            audit.close_browser_overlay(window, cx);
                                                        },
                                                    )),
                                            ),
                                    ),
                            )
                    })),
            )
            // Pinned to the window foot, below the list and the rail alike, so
            // the folder and image totals stay on screen while the list scrolls.
            .child(self.status_bar(count, cx))
            .into_any_element()
    }

    /// The list or the gallery, filling the space left of the panel.
    fn marquee_overlay(&self, cx: &App) -> Option<gpui_kit::AnyElement> {
        let marquee = self.marquee.as_ref()?;
        let bounds = marquee.bounds();
        let surface = self.selection_surface.get();
        Some(
            div()
                .debug_selector(|| "selection-marquee".into())
                .absolute()
                .left(bounds.origin.x - surface.origin.x)
                .top(bounds.origin.y - surface.origin.y)
                .w(bounds.size.width)
                .h(bounds.size.height)
                .border_1()
                .border_color(cx.theme().primary)
                .bg(cx.theme().primary.opacity(0.12))
                .into_any_element(),
        )
    }

    fn audit_content(
        &mut self,
        count: usize,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> gpui_kit::AnyElement {
        self.selection_bounds.borrow_mut().clear();
        if self.sirv_scope == Some(SirvScope::OnlyRemote) {
            return div()
                .flex()
                .flex_col()
                .flex_1()
                .overflow_hidden()
                .bg(cx.theme().table)
                .child(self.sirv_remote_only_view(cx))
                .into_any_element();
        }
        let has_visible_folders = self.has_visible_folders();
        if self.entries.is_empty() && !has_visible_folders {
            // A bare list stalls first contact: the startup state offers the next
            // move, and this one names the counts and does the same. The
            // subfolders way out only shows when it can change the list — scope
            // off with child folders on disk to walk into.
            let offer_subfolders = !self.include_subfolders && !self.folders.is_empty();
            let folder = self
                .root
                .file_name()
                .unwrap_or(self.root.as_os_str())
                .to_string_lossy();
            return div()
                .debug_selector(|| "empty-folder-message".into())
                .flex()
                .flex_col()
                .flex_1()
                .items_center()
                .justify_center()
                .gap_2()
                .bg(cx.theme().table)
                .child(
                    div()
                        .font_family("SF Pro Display")
                        .text_size(px(18.))
                        .font_weight(FontWeight::SEMIBOLD)
                        .text_color(cx.theme().foreground)
                        .child("No supported images found"),
                )
                .child(
                    div()
                        .text_size(px(12.))
                        .text_color(cx.theme().muted_foreground)
                        .child(empty_folder_detail(
                            &folder,
                            self.skipped_heic,
                            self.skipped_raw,
                            self.skipped_packages,
                        )),
                )
                .child(
                    div()
                        .flex()
                        .gap_2()
                        .pt_2()
                        .child(
                            div().debug_selector(|| "empty-open-other".into()).child(
                                Button::new("empty-open-other")
                                    .outline()
                                    .label("Open another folder…")
                                    .on_click(cx.listener(|audit, _, _, cx| audit.pick(true, cx))),
                            ),
                        )
                        .child(
                            div().debug_selector(|| "empty-open-images".into()).child(
                                Button::new("empty-open-images")
                                    .outline()
                                    .label("Open images…")
                                    .on_click(cx.listener(|audit, _, _, cx| audit.pick(false, cx))),
                            ),
                        )
                        .when(offer_subfolders, |row| {
                            row.child(
                                div()
                                    .debug_selector(|| "empty-include-subfolders".into())
                                    .child(
                                        Button::new("empty-include-subfolders")
                                            .outline()
                                            .label("Include subfolders")
                                            .on_click(cx.listener(|audit, _, _, cx| {
                                                audit.toggle_subfolders(cx)
                                            })),
                                    ),
                            )
                        }),
                )
                .into_any_element();
        }
        // The sidebar owns child navigation now; the strip that used to sit
        // here duplicated it and cost the list ~196 px.
        // The list runs to the window edge; hairlines above it, not a
        // card floating in padding. While a run owns the rows they dim a
        // touch, so the eye reads "working" before the progress numbers.
        div()
            .flex()
            .flex_col()
            .flex_1()
            .overflow_hidden()
            .bg(cx.theme().table)
            .when(self.converting, |content| content.opacity(0.9))
            .on_mouse_down(
                gpui_kit::MouseButton::Left,
                cx.listener(|audit, event: &gpui_kit::MouseDownEvent, _, cx| {
                    audit.start_marquee(event, cx);
                }),
            )
            // The action bar floats over this strip rather than over the last
            // row, so every file can be scrolled into the clear.
            .pb(px(panel::BAR_CLEARANCE))
            // Columns take a width, not a share, so the remainder after the
            // fixed ones has to be handed to the name column by hand.
            .child(if self.grid {
                // One virtualised band is one row of fixed-size tiles.
                let (root_left, root_right) = root_horizontal_chrome(window);
                let layout = gallery_layout(
                    f32::from(window.viewport_size().width)
                        - self.rail_width()
                        - self.browser_width(window),
                    root_left,
                    root_right,
                    count,
                );
                if let Some(previous) = self.gallery_columns
                    && previous != layout.columns
                {
                    self.gallery_scroll
                        .scroll_to_item_strict(0, ScrollStrategy::Top);
                }
                self.gallery_columns = Some(layout.columns);
                let gallery = uniform_list(
                    "gallery",
                    layout.rows,
                    cx.processor(move |audit, range: std::ops::Range<usize>, _, cx| {
                        audit.gallery_visible = range.clone();
                        range
                            .map(|band| {
                                // A plain loop: the closure form borrows `audit`
                                // mutably for `request_thumb` and immutably for
                                // `tile`, which nested closures cannot express.
                                // `layout` is captured from the frame: it is a pure
                                // function of the viewport width and the visible
                                // count, so recomputing it per band only burned
                                // viewport and chrome reads on every row.
                                let mut tiles = Vec::new();
                                for row in layout.band_range(band) {
                                    let Some(entry) = audit.entry_at(row) else {
                                        continue;
                                    };
                                    audit.request_thumb(entry, cx);
                                    tiles.push(audit.tile(row, entry, layout.tile, cx));
                                }
                                div().flex().gap_2().pb_2().children(tiles)
                            })
                            .collect::<Vec<_>>()
                    }),
                )
                .track_scroll(&self.gallery_scroll)
                .size_full()
                .p_2();
                div()
                    .flex()
                    .flex_col()
                    .flex_1()
                    .overflow_hidden()
                    .child(self.gallery_sort_bar(cx))
                    .child({
                        let surface = self.selection_surface.clone();
                        div()
                            .relative()
                            .flex_1()
                            .overflow_hidden()
                            .on_prepaint(move |bounds, _, _| surface.set(bounds))
                            .child(gallery)
                            .child(
                                div()
                                    .absolute()
                                    .top_0()
                                    .right_0()
                                    .bottom_0()
                                    .w(Scrollbar::width())
                                    .debug_selector(|| "gallery-scrollbar".into())
                                    .child(
                                        Scrollbar::vertical(&self.gallery_scroll)
                                            .id("gallery-scrollbar")
                                            .mode(ScrollbarMode::Always)
                                            .viewport_from_layout(),
                                    ),
                            )
                            .children(self.marquee_overlay(cx))
                    })
                    .into_any_element()
            } else if let Some(table) = self.table.as_ref() {
                let surface = self.selection_surface.clone();
                div()
                    .relative()
                    .size_full()
                    .overflow_hidden()
                    .on_prepaint(move |bounds, _, _| surface.set(bounds))
                    .child(DataTable::new(table).stripe(true).bordered(false))
                    .children(self.marquee_overlay(cx))
                    .into_any_element()
            } else {
                div().into_any_element()
            })
            .into_any_element()
    }
}
