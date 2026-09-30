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

/// The column between the two sides of the split view, holding the mark that
/// says how they stand.
const SPLIT_GUTTER: f32 = 44.;
/// The split view's tick column.
const SPLIT_TICK: f32 = 36.;
/// A split row's thumbnail: small enough for a 32px row, big enough to tell
/// one product shot from the next.
const SPLIT_THUMB: f32 = 24.;

impl Audit {
    pub(super) fn sirv_pair_disabled(&self, at_root: bool, listed: bool) -> bool {
        at_root || !listed || self.batch_folders.is_some() || self.scan_blocks_delivery()
    }

    /// The pairing in one line: which two folders, how far apart they are, a
    /// filter, and the split-view switch. Everything else a pairing can do sits
    /// behind the `⋯` menu or, in the split view, beside the rows it acts on.
    fn sirv_reconciliation(&self, cx: &mut Context<Self>) -> Option<gpui_kit::AnyElement> {
        let pairing = self.sirv_pairing.as_ref()?;
        let busy =
            self.sirv_busy() || self.scan_blocks_delivery() || self.converting || self.restoring;
        let stopping = self.sirv_job.as_ref().is_some_and(|job| job.stopping);
        let ready = matches!(pairing.files, Listing::Ready(_));
        let failed = matches!(pairing.files, Listing::Failed);
        let transferring = self.sirv_busy();
        // Scanned images the one-level pairing cannot see: they sit in
        // subfolders. Said, so the count is not read as the whole folder.
        let nested = self.entries.len().saturating_sub(
            self.sirv_rows
                .iter()
                .filter(|row| row.local.is_some())
                .count(),
        );
        let host_ready = matches!(pairing.cdn_host, CdnHost::Ready(_));
        let checking = self.sirv_delivery_running();
        let (to_push, changed, to_pull) = self.sirv_counts.unwrap_or((0, 0, 0));
        let synced = self
            .sirv_rows
            .iter()
            .filter(|row| row.state() == SplitState::InSync)
            .count();
        let local_name = self
            .root
            .file_name()
            .map(|name| name.to_string_lossy().into_owned())
            .unwrap_or_else(|| self.root.display().to_string());
        let remote_name = match pairing.dir.trim_end_matches('/').rsplit('/').next() {
            Some(name) if !name.is_empty() => name.to_string(),
            _ => "/".to_string(),
        };
        let full_pair = format!("{}  ↔  Sirv:{}", self.root.display(), pairing.dir);

        // Running, or ended short: a clean finish says so in a toast and
        // leaves the bar.
        let job_line = self
            .sirv_job
            .as_ref()
            .filter(|job| !job.finished || job.failed > 0)
            .map(|job| {
                let verb = match job.kind {
                    SirvJobKind::Pull => "Downloading",
                    SirvJobKind::PullChanged => "Replacing local copies",
                    SirvJobKind::Push => "Uploading",
                    SirvJobKind::PushChanged => "Replacing on Sirv",
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
                if job.total == 0 && !job.finished {
                    format!("{verb}: getting ready…")
                } else if job.finished {
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

        let scope_index = match self.sirv_scope {
            None => 0,
            Some(SirvScope::OnlyLocal) => 1,
            Some(SirvScope::Changed) => 2,
            Some(SirvScope::OnlyRemote) => 3,
        };
        let filters = ButtonGroup::new("sirv-filter")
            .small()
            .outline()
            .compact()
            .children(
                [
                    toolbar::segment("sirv-filter-all", "All", scope_index == 0),
                    toolbar::segment(
                        "sirv-filter-local",
                        format!("Only here {to_push}"),
                        scope_index == 1,
                    ),
                    toolbar::segment(
                        "sirv-filter-changed",
                        format!("Different {changed}"),
                        scope_index == 2,
                    ),
                    toolbar::segment(
                        "sirv-filter-remote",
                        format!("Only on Sirv {to_pull}"),
                        scope_index == 3,
                    ),
                ]
                .map(|segment| segment.disabled(!ready)),
            )
            .on_click(cx.listener(|audit, clicked: &Vec<usize>, _, cx| {
                let scope = match clicked.first() {
                    Some(1) => Some(SirvScope::OnlyLocal),
                    Some(2) => Some(SirvScope::Changed),
                    Some(3) => Some(SirvScope::OnlyRemote),
                    _ => None,
                };
                audit.choose_sirv_scope(scope, cx);
            }));

        let menu_source = cx.entity().downgrade();
        let more = Button::new("sirv-more")
            .small()
            .ghost()
            .icon(IconName::Ellipsis)
            .tooltip("More Sirv actions")
            .dropdown_menu(move |menu, _, _| {
                let item = |label: String,
                            icon: Option<IconName>,
                            disabled: bool,
                            act: fn(&mut Audit, &mut Context<Audit>)| {
                    let source = menu_source.clone();
                    let mut item = PopupMenuItem::new(label).disabled(disabled);
                    if let Some(icon) = icon {
                        item = item.icon(icon);
                    }
                    item.on_click(move |_, _, cx| {
                        if let Some(audit) = source.upgrade() {
                            audit.update(cx, act);
                        }
                    })
                };
                menu.item(item(
                    format!("Upload all {to_push} to Sirv"),
                    Some(IconName::ArrowUp),
                    busy || to_push == 0,
                    Audit::start_push,
                ))
                .item(item(
                    format!("Download all {to_pull} here"),
                    Some(IconName::ArrowDown),
                    busy || to_pull == 0,
                    Audit::start_pull,
                ))
                .separator()
                .item(item(
                    if checking {
                        "Stop delivery check".into()
                    } else {
                        "Check delivery".into()
                    },
                    None,
                    !checking && (busy || !ready || !host_ready),
                    |audit, cx| {
                        if audit.sirv_delivery_running() {
                            audit.stop_sirv_delivery(cx);
                        } else {
                            audit.check_sirv_delivery(cx);
                        }
                    },
                ))
                .item(item(
                    "Refresh".into(),
                    None,
                    busy || !(ready || failed),
                    Audit::walk_sirv_pairing,
                ))
                .item(item(
                    "Change Sirv folder…".into(),
                    None,
                    busy,
                    Audit::open_sirv_browser,
                ))
                .separator()
                .item(item("Unpair".into(), None, busy, Audit::unpair_sirv))
            });

        let muted = cx.theme().muted_foreground;
        let mono = cx.theme().mono_font_family.clone();
        let status = |selector: &'static str, line: String, colour: gpui_kit::Hsla| {
            div()
                .debug_selector(move || selector.into())
                .min_w_0()
                .overflow_hidden()
                .text_ellipsis()
                .whitespace_nowrap()
                .font_family(mono.clone())
                .text_size(px(11.))
                .text_color(colour)
                .child(line)
        };
        let job_colour = if self.sirv_job.as_ref().is_some_and(|job| job.failed > 0) {
            cx.theme().yellow
        } else {
            muted
        };
        let delivery_line = self.sirv_delivery_line();
        let has_status = transferring || job_line.is_some() || delivery_line.is_some();

        Some(
            div()
                .debug_selector(|| "sirv-reconciliation".into())
                .flex()
                .flex_col()
                .gap_1()
                .px_3()
                .py_1p5()
                .bg(cx.theme().secondary)
                .border_b_1()
                .border_color(cx.theme().border)
                .child(
                    div()
                        .flex()
                        .items_center()
                        .gap_3()
                        .child(
                            div()
                                .id("sirv-pair-identity")
                                .flex()
                                .items_center()
                                .gap_1p5()
                                .min_w_0()
                                .text_size(px(12.))
                                .tooltip(move |window, cx| {
                                    Tooltip::new(full_pair.clone()).build(window, cx)
                                })
                                .child(Icon::new(IconName::Globe).size_3p5().text_color(muted))
                                .child(
                                    div()
                                        .min_w_0()
                                        .overflow_hidden()
                                        .text_ellipsis()
                                        .whitespace_nowrap()
                                        .child(format!("{local_name}  ⇄  {remote_name}")),
                                ),
                        )
                        .child(
                            div()
                                .debug_selector(|| "sirv-in-sync".into())
                                .min_w_0()
                                .overflow_hidden()
                                .text_ellipsis()
                                .whitespace_nowrap()
                                .text_size(px(12.))
                                .text_color(if failed { cx.theme().yellow } else { muted })
                                .child(match (ready, failed) {
                                    // "Same size", the table's words, not "in
                                    // sync": equal bytes are evidence, not proof.
                                    (true, _) if nested > 0 => format!(
                                        "{synced} of {} same size · {nested} in subfolders not compared",
                                        self.sirv_rows.len()
                                    ),
                                    (true, _) => format!(
                                        "{synced} of {} same size",
                                        self.sirv_rows.len()
                                    ),
                                    (_, true) => "Couldn’t list the Sirv folder".to_string(),
                                    _ => "Listing Sirv…".to_string(),
                                }),
                        )
                        .when(failed && !busy, |row| {
                            row.child(
                                Button::new("sirv-retry-listing")
                                    .xsmall()
                                    .outline()
                                    .label("Retry")
                                    .on_click(
                                        cx.listener(|audit, _, _, cx| audit.walk_sirv_pairing(cx)),
                                    ),
                            )
                        })
                        .child(div().flex_1())
                        .child(div().flex_shrink_0().child(filters))
                        .child(
                            div().flex_shrink_0().child(
                                Button::new("sirv-split-toggle")
                                    .small()
                                    .ghost()
                                    .icon(Icon::default().path("icons/columns-2.svg"))
                                    .selected(self.sirv_split)
                                    .tooltip(if self.sirv_split {
                                        "Back to the image list"
                                    } else {
                                        "Side by side: this folder and Sirv, file by file"
                                    })
                                    .on_click(
                                        cx.listener(|audit, _, _, cx| audit.toggle_sirv_split(cx)),
                                    ),
                            ),
                        )
                        .child(div().flex_shrink_0().child(more)),
                )
                .when(has_status, |bar| {
                    bar.child(
                        div()
                            .flex()
                            .items_center()
                            .gap_2()
                            .when_some(job_line, |line, text| {
                                line.child(status("sirv-job-line", text, job_colour))
                            })
                            .when_some(delivery_line, |line, text| {
                                line.child(status("sirv-delivery-line", text, muted))
                            })
                            .when(transferring, |line| {
                                line.child(div().flex_1()).child(
                                    Button::new("sirv-stop-audit")
                                        .xsmall()
                                        .outline()
                                        .label(if stopping { "Stopping…" } else { "Stop" })
                                        .disabled(stopping)
                                        .on_click(cx.listener(|audit, _, _, cx| {
                                            if let Some(job) = audit.sirv_job.as_mut() {
                                                job.stopped_by_user = true;
                                            }
                                            audit.cancel_sirv_transfer();
                                            cx.notify();
                                        })),
                                )
                            }),
                    )
                })
                .into_any_element(),
        )
    }

    /// The delivery check in one sentence: progress while it runs, then what
    /// Sirv sends against what is on disk and what this run wrote.
    fn sirv_delivery_line(&self) -> Option<String> {
        let progress = self
            .sirv_delivery_job
            .as_ref()
            .filter(|job| !job.finished)
            .map(|job| format!("Checking Sirv delivery {} of {}…", job.done, job.total));
        let summary = self.sirv_delivery_summary().map(|summary| {
            let edge = self
                .max_edge
                .0
                .map(|edge| format!(" at {edge}px"))
                .unwrap_or_default();
            let converted = summary
                .converted
                .map(|bytes| format!(" · Press output {}", format_bytes(bytes)))
                .unwrap_or_default();
            format!(
                "Sirv delivery for {} {}{edge}: {} on disk → {} sent to a browser as {}{converted}",
                summary.count,
                if summary.count == 1 { "file" } else { "files" },
                format_bytes(summary.on_disk),
                format_bytes(summary.served),
                summary.formats,
            )
        });
        match (progress, summary) {
            (Some(progress), Some(summary)) => Some(format!("{progress} {summary}")),
            (progress, summary) => progress.or(summary),
        }
    }

    /// Both sides of the pairing in one list, a row per file: this computer on
    /// the left, Sirv on the right, and between them how the two stand. A gap
    /// on one side is the difference, which reads faster than a status word.
    /// The arrow between the sides copies that one file; ticked rows are
    /// copied together from the footer.
    fn sirv_split_view(&mut self, cx: &mut Context<Self>) -> gpui_kit::AnyElement {
        // Refilled by the rows drawn this frame; the thumbnail queue reads it
        // in place of the hidden table's viewport.
        self.split_thumb_wanted.clear();
        let Some(pairing) = self.sirv_pairing.as_ref() else {
            return div().into_any_element();
        };
        let rows = self
            .sirv_rows
            .iter()
            .enumerate()
            .filter(|(_, row)| self.split_shows(row))
            .map(|(index, _)| index)
            .collect::<Vec<_>>();
        let all_ticked = !rows.is_empty()
            && rows
                .iter()
                .all(|&row| self.sirv_selected.contains(&self.sirv_rows[row].key));
        let local_name = self
            .root
            .file_name()
            .map(|name| name.to_string_lossy().into_owned())
            .unwrap_or_else(|| self.root.display().to_string());
        let heading = |title: &'static str, detail: String, cx: &Context<Self>| {
            div()
                .flex_1()
                .min_w_0()
                .flex()
                .items_baseline()
                .gap_2()
                .px_3()
                .child(
                    div()
                        .flex_shrink_0()
                        .font_weight(FontWeight::SEMIBOLD)
                        .text_size(px(12.))
                        .child(title),
                )
                .child(
                    div()
                        .min_w_0()
                        .overflow_hidden()
                        .text_ellipsis()
                        .whitespace_nowrap()
                        .text_size(px(11.))
                        .text_color(cx.theme().muted_foreground)
                        .child(detail),
                )
        };
        let header = div()
            .debug_selector(|| "sirv-split-header".into())
            .flex()
            .items_center()
            .h(px(36.))
            .flex_shrink_0()
            .bg(cx.theme().table_head)
            .border_b_1()
            .border_color(cx.theme().border)
            .child(
                div()
                    .w(px(SPLIT_TICK))
                    .flex_shrink_0()
                    .flex()
                    .justify_center()
                    .on_key_down(cx.listener(|_, event, _, cx| {
                        if is_checkbox_activation_key(event) {
                            cx.stop_propagation();
                        }
                    }))
                    .child(
                        Checkbox::new("sirv-split-all")
                            .checked(all_ticked)
                            .disabled(rows.is_empty())
                            .tooltip("Select every file shown")
                            .on_click(cx.listener(|audit, _: &bool, _, cx| {
                                cx.stop_propagation();
                                audit.toggle_split_rows(cx);
                            })),
                    ),
            )
            .child(heading("This computer", local_name, cx))
            .child(div().w(px(SPLIT_GUTTER)).flex_shrink_0())
            .child(heading("Sirv", pairing.dir.clone(), cx));
        let body = match &pairing.files {
            Listing::Walking => Self::split_message("Listing the Sirv folder…", cx),
            Listing::Failed => Self::split_message("Couldn’t list the Sirv folder.", cx),
            Listing::Ready(_) if rows.is_empty() => Self::split_message(
                if self.sirv_rows.is_empty() {
                    "Both folders are empty."
                } else {
                    "No files in this category."
                },
                cx,
            ),
            Listing::Ready(_) => uniform_list(
                "sirv-split-list",
                rows.len(),
                cx.processor(move |audit, range: std::ops::Range<usize>, _, cx| {
                    range
                        .filter_map(|shown| rows.get(shown).map(|&row| (shown, row)))
                        .filter_map(|(shown, row)| {
                            // Viewport-driven, like the table: only the rows
                            // on screen ask for a thumbnail.
                            let shown_row = audit.sirv_rows.get(row)?;
                            let (entry, state) = (shown_row.entry, shown_row.state());
                            let remote = shown_row.remote.map(|_| shown_row.key.clone());
                            if let Some(entry) = entry {
                                audit.split_thumb_wanted.insert(entry);
                                audit.request_thumb(entry, cx);
                            }
                            // In sync, Sirv's side shows the local thumbnail:
                            // same bytes, no request.
                            if state != SplitState::InSync
                                && let Some(key) = remote
                            {
                                audit.request_sirv_thumb(&key, cx);
                            }
                            let row = audit.sirv_rows.get(row)?;
                            Some(audit.split_row(shown, row, cx))
                        })
                        .collect::<Vec<_>>()
                }),
            )
            .track_scroll(&self.sirv_split_scroll)
            .size_full()
            .into_any_element(),
        };
        div()
            .debug_selector(|| "sirv-split".into())
            .flex()
            .flex_col()
            .flex_1()
            .min_h_0()
            .overflow_hidden()
            .bg(cx.theme().table)
            .child(header)
            .child(div().flex_1().min_h_0().child(body))
            .child(self.split_footer(cx))
            .into_any_element()
    }

    fn split_message(message: &'static str, cx: &Context<Self>) -> gpui_kit::AnyElement {
        div()
            .size_full()
            .flex()
            .items_center()
            .justify_center()
            .text_size(px(12.))
            .text_color(cx.theme().muted_foreground)
            .child(message)
            .into_any_element()
    }

    fn split_row(
        &self,
        shown: usize,
        row: &SyncRow,
        cx: &mut Context<Self>,
    ) -> gpui_kit::AnyElement {
        let state = row.state();
        let ticked = self.sirv_selected.contains(&row.key);
        let busy =
            self.sirv_busy() || self.scan_blocks_delivery() || self.converting || self.restoring;
        // The same bytes on both sides when in sync, so Sirv's side can show
        // the local thumbnail; a Sirv-only file has none to show yet.
        let thumb = row.entry.and_then(|entry| self.thumbs.get(&entry).cloned());
        // "12.1 KB ≠ 12.1 KB" reads as a bug. When rounding hides the
        // difference, both sides say their exact bytes.
        let exact = matches!(
            (row.local, row.remote),
            (Some(local), Some(remote))
                if local != remote && format_bytes(local) == format_bytes(remote)
        );
        let size_label = move |bytes: u64| {
            if exact {
                format!("{bytes} B")
            } else {
                format_bytes(bytes)
            }
        };
        let side = |size: Option<u64>, thumb: Option<Arc<RenderImage>>| {
            let slot = div()
                .size(px(SPLIT_THUMB))
                .flex_shrink_0()
                .flex()
                .items_center()
                .justify_center()
                .rounded_sm()
                .overflow_hidden()
                .bg(cx.theme().background)
                .map(|slot| match thumb {
                    Some(image) => {
                        slot.child(img(image).max_w(px(SPLIT_THUMB)).max_h(px(SPLIT_THUMB)))
                    }
                    None => slot.child(
                        Icon::new(IconName::File)
                            .size_3()
                            .text_color(cx.theme().muted_foreground.opacity(0.45)),
                    ),
                });
            let cell = div()
                .flex_1()
                .min_w_0()
                .h_full()
                .flex()
                .items_center()
                .gap_2()
                .px_3();
            match size {
                // The file is missing here; the tint marks the gap as a gap
                // rather than a row that failed to draw.
                None => cell.bg(cx.theme().muted.opacity(0.35)),
                Some(bytes) => cell
                    .child(slot)
                    .child(
                        div()
                            .flex_1()
                            .min_w_0()
                            .overflow_hidden()
                            .text_ellipsis()
                            .whitespace_nowrap()
                            .text_size(px(12.))
                            .child(row.key.clone()),
                    )
                    .child(
                        div()
                            .flex_shrink_0()
                            .font_family(cx.theme().mono_font_family.clone())
                            .text_size(px(11.))
                            .text_color(cx.theme().muted_foreground)
                            .child(size_label(bytes)),
                    ),
            }
        };
        let right_thumb = if state == SplitState::InSync {
            thumb.clone()
        } else {
            self.sirv_thumbs.get(&row.key).cloned().flatten()
        };
        let (left, right) = (side(row.local, thumb), side(row.remote, right_thumb));
        // The arrow is the action: it points the way the file would go. Only
        // the two safe copies act on one click; a size mismatch replaces a
        // file, so it goes through the footer's two-click confirmation.
        let copy = |toward_sirv: bool, cx: &mut Context<Self>| {
            let key = row.key.clone();
            Button::new((
                if toward_sirv {
                    "sirv-row-up"
                } else {
                    "sirv-row-down"
                },
                shown,
            ))
            .xsmall()
            .ghost()
            .icon(if toward_sirv {
                IconName::ArrowRight
            } else {
                IconName::ArrowLeft
            })
            .text_color(cx.theme().blue)
            .tooltip(if toward_sirv {
                "Upload to Sirv"
            } else {
                "Download to this computer"
            })
            .disabled(busy)
            .on_click(cx.listener(move |audit, _, _, cx| {
                cx.stop_propagation();
                let state = if toward_sirv {
                    SplitState::OnlyLocal
                } else {
                    SplitState::OnlyRemote
                };
                audit.transfer_split(state, toward_sirv, [key.clone()].into(), cx);
            }))
            .into_any_element()
        };
        let mark: gpui_kit::AnyElement = match state {
            SplitState::InSync => Icon::new(IconName::Check)
                .size_4()
                .text_color(cx.theme().green)
                .into_any_element(),
            SplitState::Different => div()
                .text_size(px(14.))
                .font_weight(FontWeight::SEMIBOLD)
                .text_color(cx.theme().yellow)
                .child("≠")
                .into_any_element(),
            SplitState::OnlyLocal => copy(true, cx),
            SplitState::OnlyRemote => copy(false, cx),
        };
        let key = row.key.clone();
        let menu = self.split_row_menu(row, cx);
        div()
            .id(("sirv-split-row", shown))
            .flex()
            .w_full()
            .h(px(32.))
            .items_center()
            .border_b_1()
            .border_color(cx.theme().border)
            .cursor_pointer()
            .when(ticked, |row| row.bg(cx.theme().list_active))
            .when(shown == self.sirv_split_cursor, |row| {
                row.border_1().border_color(cx.theme().list_active_border)
            })
            .when(!ticked, |row| {
                row.hover(|row| row.bg(cx.theme().list_hover))
            })
            .on_click(cx.listener(move |audit, _, _, cx| {
                audit.sirv_split_cursor = shown;
                audit.toggle_split_row(&key, cx)
            }))
            .child(
                div()
                    .w(px(SPLIT_TICK))
                    .flex_shrink_0()
                    .flex()
                    .justify_center()
                    .on_key_down(cx.listener(|_, event, _, cx| {
                        if is_checkbox_activation_key(event) {
                            cx.stop_propagation();
                        }
                    }))
                    .child({
                        let key = row.key.clone();
                        Checkbox::new(("sirv-split-tick", shown))
                            .checked(ticked)
                            .on_click(cx.listener(move |audit, _: &bool, _, cx| {
                                cx.stop_propagation();
                                audit.toggle_split_row(&key, cx);
                            }))
                    }),
            )
            .child(left)
            .child(
                div()
                    .w(px(SPLIT_GUTTER))
                    .h_full()
                    .flex_shrink_0()
                    .flex()
                    .items_center()
                    .justify_center()
                    .border_x_1()
                    .border_color(cx.theme().border)
                    .child(mark),
            )
            .child(right)
            .context_menu(menu)
            .into_any_element()
    }

    /// Right-click on a split row: look at the file on either side. The copy
    /// verbs stay on the arrow and the footer; this menu only opens things.
    fn split_row_menu(
        &self,
        row: &SyncRow,
        cx: &mut Context<Self>,
    ) -> impl Fn(PopupMenu, &mut Window, &mut Context<PopupMenu>) -> PopupMenu + 'static {
        let audit = cx.entity().downgrade();
        let entry = row.entry;
        let local = row.local.map(|_| self.root.join(&row.key));
        let url = self.sirv_pairing.as_ref().and_then(|pairing| {
            let CdnHost::Ready(host) = &pairing.cdn_host else {
                return None;
            };
            row.remote?;
            sirv::public_url(host, &format!("{}/{}", pairing.dir, row.key)).ok()
        });
        move |menu, _, _| {
            let mut menu = menu;
            if let Some(index) = entry {
                let audit = audit.clone();
                menu = menu.item(PopupMenuItem::new("Preview").icon(IconName::Eye).on_click(
                    move |_, _, cx| {
                        if let Some(audit) = audit.upgrade() {
                            audit.update(cx, |audit, cx| audit.open_preview(index, cx));
                        }
                    },
                ));
            }
            if let Some(url) = url.clone() {
                let copy = url.clone();
                menu = menu
                    .item(
                        PopupMenuItem::new("Open on Sirv")
                            .icon(IconName::ExternalLink)
                            .on_click(move |_, _, cx| cx.open_url(&url)),
                    )
                    .item(
                        PopupMenuItem::new("Copy Sirv link")
                            .icon(IconName::Copy)
                            .on_click(move |_, _, cx| {
                                cx.write_to_clipboard(gpui_kit::ClipboardItem::new_string(
                                    copy.clone(),
                                ))
                            }),
                    );
            }
            if let Some(path) = local.clone() {
                let audit = audit.clone();
                menu = menu.item(
                    PopupMenuItem::new("Show in file manager")
                        .icon(IconName::FolderOpen)
                        .on_click(move |_, _, cx| {
                            if let Some(audit) = audit.upgrade() {
                                audit.update(cx, |audit, cx| {
                                    audit.reveal_path(&path, "Couldn’t show the file", cx)
                                });
                            }
                        }),
                );
            }
            menu
        }
    }

    /// What the ticked rows can become, one button per direction. With nothing
    /// ticked the footer says how to tick, rather than showing dead buttons.
    fn split_footer(&self, cx: &mut Context<Self>) -> gpui_kit::AnyElement {
        // Transfers wait for a conversion or a restore, which work on the
        // same files; the buttons say so rather than do nothing.
        let busy =
            self.sirv_busy() || self.scan_blocks_delivery() || self.converting || self.restoring;
        let converting = self.converting;
        // Counted per frame; the key sets are built only when a button is
        // clicked, so a select-all over a big folder costs no clones here.
        let (mut upload, mut download, mut replace) = (0, 0, 0);
        if !self.sirv_selected.is_empty() {
            for row in &self.sirv_rows {
                if self.sirv_selected.contains(&row.key) {
                    match row.state() {
                        SplitState::OnlyLocal => upload += 1,
                        SplitState::OnlyRemote => download += 1,
                        SplitState::Different => replace += 1,
                        SplitState::InSync => {}
                    }
                }
            }
        }
        let ticked = self.sirv_selected.len();
        let push_confirm = self.sirv_confirm == Some(SirvJobKind::PushChanged);
        let pull_confirm = self.sirv_confirm == Some(SirvJobKind::PullChanged);
        let action = |id: &'static str,
                      label: String,
                      icon: Option<IconName>,
                      primary: bool,
                      state: SplitState,
                      toward_sirv: bool,
                      cx: &mut Context<Self>| {
            Button::new(id)
                .small()
                .when(primary, |button| button.primary())
                .when(!primary, |button| button.outline())
                .when_some(icon, |button, icon| button.icon(icon))
                .label(label)
                .disabled(busy || (converting && state == SplitState::Different && !toward_sirv))
                .on_click(cx.listener(move |audit, _, _, cx| {
                    let keys = audit.split_selected(state);
                    audit.transfer_split(state, toward_sirv, keys, cx);
                }))
        };
        div()
            .debug_selector(|| "sirv-split-footer".into())
            .flex()
            .items_center()
            .gap_2()
            .h(px(44.))
            .px_3()
            .flex_shrink_0()
            .bg(cx.theme().table_head)
            .border_t_1()
            .border_color(cx.theme().border)
            .child(
                div()
                    .flex_1()
                    .min_w_0()
                    .text_size(px(12.))
                    .text_color(cx.theme().muted_foreground)
                    .child(match ticked {
                        0 => "Tick files to copy them, or use the arrows".to_string(),
                        1 => "1 file selected".to_string(),
                        many => format!("{many} files selected"),
                    }),
            )
            .when(upload > 0, |footer| {
                footer.child(action(
                    "sirv-split-upload",
                    format!("Upload {upload}"),
                    Some(IconName::ArrowRight),
                    false,
                    SplitState::OnlyLocal,
                    true,
                    cx,
                ))
            })
            .when(download > 0, |footer| {
                footer.child(action(
                    "sirv-split-download",
                    format!("Download {download}"),
                    Some(IconName::ArrowLeft),
                    false,
                    SplitState::OnlyRemote,
                    false,
                    cx,
                ))
            })
            .when(replace > 0, |footer| {
                footer
                    .child(action(
                        "sirv-split-replace-remote",
                        if push_confirm {
                            format!("Really replace {replace} on Sirv?")
                        } else {
                            format!("Replace {replace} on Sirv")
                        },
                        None,
                        push_confirm,
                        SplitState::Different,
                        true,
                        cx,
                    ))
                    .child(action(
                        "sirv-split-replace-local",
                        if pull_confirm {
                            format!("Really replace {replace} here?")
                        } else {
                            format!("Replace {replace} here")
                        },
                        None,
                        pull_confirm,
                        SplitState::Different,
                        false,
                        cx,
                    ))
            })
            .when(ticked > 0, |footer| {
                footer.child(
                    Button::new("sirv-split-clear")
                        .small()
                        .ghost()
                        .label("Clear")
                        .on_click(cx.listener(|audit, _, _, cx| {
                            audit.sirv_selected.clear();
                            audit.sirv_confirm = None;
                            cx.notify();
                        })),
                )
            })
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
                        .text_color(if browser.needs_credentials {
                            cx.theme().muted_foreground
                        } else {
                            cx.theme().yellow
                        })
                        .child(message.clone()),
                )
                // Retrying a listing that failed is worth offering. Retrying
                // one that never started, because there are no keys, only
                // fails again; the row below already offers "Set up Sirv…".
                .children((!browser.needs_credentials).then(|| {
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
                    )
                }))
                .into_any_element(),
            Some(Ok(nodes)) => {
                // The filter narrows what is already on screen; it never lists
                // again, so typing in a folder of hundreds costs no request.
                let needle = self.sirv_browser_filter.trim().to_lowercase();
                // Sirv keeps its own machinery in dot folders (.Trash,
                // .processed, .well-known); nobody pairs a photo folder there.
                let visible_folder = |node: &&sirv::Node| {
                    node.is_folder()
                        && !node
                            .filename
                            .rsplit('/')
                            .next()
                            .is_some_and(|name| name.starts_with('.'))
                };
                let total_folders = nodes.iter().filter(visible_folder).count();
                let mut rows: Vec<gpui_kit::AnyElement> = Vec::new();
                if browser.path != "/" {
                    rows.push(
                        Button::new("sirv-up")
                            .ghost()
                            .small()
                            .icon(IconName::ArrowUp)
                            .label("..")
                            .on_click(
                                cx.listener(|audit, _, window, cx| audit.ascend_sirv(window, cx)),
                            )
                            .into_any_element(),
                    );
                }
                let mut shown: usize = 0;
                for node in nodes.iter().filter(visible_folder) {
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
                                    .on_click(cx.listener(move |audit, _, window, cx| {
                                        audit.descend_sirv(descend_to.clone(), window, cx);
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
                            .on_key_down(cx.listener(
                                |audit, event: &gpui_kit::KeyDownEvent, window, cx| {
                                    let key = event.keystroke.key.as_str();
                                    if key == "enter" {
                                        if let Some(name) = audit.sirv_browser_match() {
                                            audit.descend_sirv(name, window, cx);
                                        }
                                        cx.stop_propagation();
                                        return;
                                    }
                                    let modifiers = &event.keystroke.modifiers;
                                    let plain = !modifiers.control
                                        && !modifiers.platform
                                        && !modifiers.alt
                                        && !modifiers.function;
                                    let editing = matches!(key, "backspace" | "delete" | "tab")
                                        || (plain && key == "space")
                                        || (plain
                                            && key.chars().count() == 1
                                            && key.chars().all(|c| c.is_alphanumeric()));
                                    if key != "escape" && !editing {
                                        cx.stop_propagation();
                                    }
                                },
                            ))
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
                            .debug_selector(|| "sirv-close".into())
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
                                    // The keys were only ever the way to this
                                    // dialog, so good ones lead straight back.
                                    if let Some(panel) = audit.settings_panel.as_mut() {
                                        panel.then_browse = true;
                                    }
                                })),
                        )
                    })
                    .when(!browser.needs_credentials && at_root, |row| {
                        row.child(
                            div()
                                .text_size(px(12.))
                                .text_color(cx.theme().muted_foreground)
                                .child("Open the Sirv folder that belongs with this one."),
                        )
                    })
                    .when(!browser.needs_credentials && !at_root, |row| {
                        let local = self
                            .root
                            .file_name()
                            .map(|name| name.to_string_lossy().into_owned())
                            .unwrap_or_default();
                        let remote = browser
                            .path
                            .trim_end_matches('/')
                            .rsplit('/')
                            .next()
                            .unwrap_or_default()
                            .to_string();
                        row.child(
                            div().debug_selector(|| "sirv-pair".into()).child(
                                Button::new("sirv-pair")
                                    .primary()
                                    .small()
                                    .label(if local.chars().count() > 24 {
                                        format!(
                                            "Pair with {}…",
                                            local.chars().take(23).collect::<String>()
                                        )
                                    } else {
                                        format!("Pair with {local}")
                                    })
                                    .tooltip(format!("Keep {remote} on Sirv in step with {local}"))
                                    .disabled(self.sirv_pair_disabled(
                                        at_root,
                                        matches!(browser.nodes, Some(Ok(_))),
                                    ))
                                    .on_click(cx.listener(|audit, _, window, cx| {
                                        audit.clear_sirv_browser_filter(window, cx);
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
        if self.update_is_applying() {
            return div()
                .size_full()
                .bg(cx.theme().background)
                .flex()
                .items_center()
                .justify_center()
                .child("Applying update… Press will restart when it is ready.")
                .into_any_element();
        }
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
            rail_width: Some(self.rail_size),
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
                        })),
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
                        .occlude()
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
            // Into the filter once there is a listing to filter, so typing a
            // folder's name finds it; the scrim until then, so Escape works
            // while it loads. Each new listing takes focus again.
            cx.defer_in(window, |audit, window, cx| {
                if let Some(browser) = audit.sirv_browser.as_mut()
                    && !browser.focused
                {
                    if matches!(browser.nodes, Some(Ok(_))) {
                        browser.focused = true;
                        let filter = audit.sirv_browser_filter_input.read(cx).focus_handle(cx);
                        window.focus(&filter, cx);
                    } else {
                        window.focus(&browser.focus, cx);
                    }
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
                        .occlude()
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
                    if event.keystroke.modifiers != gpui_kit::Modifiers::none() {
                        return;
                    }
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
            .relative()
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
                    if audit.shortcuts_open {
                        if event.keystroke.key == "escape" {
                            audit.shortcuts_open = false;
                            window.focus(&audit.focus, cx);
                            cx.notify();
                        }
                        cx.stop_propagation();
                        return;
                    }
                    if audit.text_input_focused(window, cx) {
                        return;
                    }
                    if audit.sirv_split_shown() && audit.split_key(event, cx) {
                        return;
                    }
                    let modifiers = event.keystroke.modifiers;
                    if modifiers.alt && !modifiers.shift && !modifiers.control {
                        match event.keystroke.key.as_str() {
                            "left" => return audit.step_history(false, cx),
                            "right" => return audit.step_history(true, cx),
                            _ => {}
                        }
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
                            if !audit.selected.is_empty() && !audit.converting {
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
                        // The keyboard reached everything except the thing the
                        // folder was opened for. It opens the panel rather than
                        // starting the run, exactly like the bar's own button:
                        // committing a replace run stays a deliberate click.
                        "enter"
                            if (event.keystroke.modifiers.control
                                || event.keystroke.modifiers.platform)
                                && !audit.converting =>
                        {
                            audit.open_rail(Rail::Convert, cx);
                        }
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
            // The panel's grab edge is six pixels wide and the pointer leaves
            // it at once, so the workspace follows the drag on its behalf.
            .on_mouse_move(
                cx.listener(|audit, event: &gpui_kit::MouseMoveEvent, window, cx| {
                    if audit.rail_drag {
                        let viewport = f32::from(window.viewport_size().width);
                        audit.drag_rail(event, viewport, cx);
                    }
                }),
            )
            .on_mouse_up(
                gpui_kit::MouseButton::Left,
                cx.listener(|audit, _, _, cx| audit.end_rail_drag(cx)),
            )
            // The mouse's side buttons, as every file manager reads them.
            .on_mouse_down(
                gpui_kit::MouseButton::Navigate(gpui_kit::NavigationDirection::Back),
                cx.listener(|audit, _, _, cx| audit.step_history(false, cx)),
            )
            .on_mouse_down(
                gpui_kit::MouseButton::Navigate(gpui_kit::NavigationDirection::Forward),
                cx.listener(|audit, _, _, cx| audit.step_history(true, cx)),
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
                                    && !self.sirv_split_shown()
                                    && !self.visible.is_empty())
                                .then(|| self.action_bar(list_width, cx)),
                            ),
                    )
                    .child(self.rail_view(cx))
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
            .children(self.shortcuts_open.then(|| self.shortcuts_overlay(cx)))
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
        if self.sirv_split_shown() {
            return self.sirv_split_view(cx);
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
                    // The zebra is drawn per row in `render_tr`, not by the
                    // library: its own `stripe` fills the space below the last
                    // file with striped blank rows, and a folder of thirteen
                    // images then looked like a folder of thirty.
                    .child(DataTable::new(table).stripe(false).bordered(false))
                    .children(self.marquee_overlay(cx))
                    .into_any_element()
            } else {
                div().into_any_element()
            })
            .into_any_element()
    }
}
