//! The status bar: summary counts, notices row, finding buttons.

use super::*;

impl Audit {
    /// A local inference result stays visible after its comparison closes. Unlike
    /// scan notices, this is normal work and uses info/success/error semantics.
    pub(super) fn local_ai_notice(&self, _cx: &mut Context<Self>) -> Option<gpui_kit::AnyElement> {
        let job = self.local_ai_job.as_ref()?;
        let message = job.message(&self.root);
        let alert = match job.state {
            LocalAiJobState::SettingUp | LocalAiJobState::Running => {
                Alert::info("local-ai-status", message)
            }
            LocalAiJobState::Done(_) => Alert::success("local-ai-status", message),
            LocalAiJobState::Failed(_) => Alert::error("local-ai-status", message),
        };
        Some(alert.py_1().into_any_element())
    }

    pub(super) fn studio_notice(&self, _cx: &mut Context<Self>) -> Option<gpui_kit::AnyElement> {
        let job = self.studio_job.as_ref()?;
        let message = job.message(&self.root);
        let alert = match job.state {
            StudioJobState::Preparing | StudioJobState::Running => {
                Alert::info("studio-status", message)
            }
            StudioJobState::AwaitingConfirmation(_) => Alert::warning("studio-status", message),
            StudioJobState::Done(_) => Alert::success("studio-status", message),
            StudioJobState::Failed(_) => Alert::error("studio-status", message),
        };
        Some(alert.py_1().into_any_element())
    }

    /// A finished run, still said after its results view is closed. A fast
    /// conversion can be over before you have looked up, and "it worked" plus
    /// the way to the files is what you want to find when you look back.
    pub(super) fn conversion_notice(&self, cx: &mut Context<Self>) -> Option<gpui_kit::AnyElement> {
        if self.converting || self.results.is_empty() {
            return None;
        }
        let (before, after) = self.converted_totals();
        let grew = after > before;
        let delta = before.abs_diff(after);
        let message = format!(
            "Converted {} {} to {} · {} {} · in {}",
            self.results.len(),
            if self.results.len() == 1 {
                "image"
            } else {
                "images"
            },
            self.format.display(),
            format_bytes(delta),
            if grew { "larger" } else { "saved" },
            self.output.label(),
        );
        Some(
            div()
                .flex()
                .flex_wrap()
                .items_center()
                .gap_2()
                .px_3()
                .py_1()
                .child(div().flex_1().min_w(px(260.)).child(if grew {
                    Alert::warning("conversion-done", message)
                } else {
                    Alert::success("conversion-done", message)
                }))
                .child(if self.published_results.is_empty() {
                    let waiting = self.publish_waiting();
                    Button::new("conversion-publish")
                        .small()
                        .outline()
                        .icon(IconName::ArrowUp)
                        .label(if self.sirv_pairing.is_some() {
                            "Publish to Sirv"
                        } else {
                            "Connect & publish"
                        })
                        .tooltip(waiting.clone().unwrap_or_else(|| {
                            "Upload the converted files to optimized/ on Sirv".into()
                        }))
                        .disabled(
                            self.scan_blocks_delivery() || self.sirv_busy() || waiting.is_some(),
                        )
                        .on_click(cx.listener(|audit, _, _, cx| audit.publish_results(cx)))
                } else {
                    Button::new("conversion-copy-embed")
                        .small()
                        .outline()
                        .icon(IconName::Copy)
                        .label("Copy embed")
                        .tooltip("Copy responsive Sirv image markup")
                        .on_click(cx.listener(|audit, _, _, cx| audit.copy_result_embeds(cx)))
                })
                .child(
                    Button::new("conversion-done-reveal")
                        .small()
                        .ghost()
                        .icon(IconName::FolderOpen)
                        .label("Show output")
                        .tooltip("Open the output folder in the file manager")
                        .on_click(cx.listener(|audit, _, _, cx| audit.reveal_output(cx))),
                )
                .child(
                    Button::new("conversion-done-results")
                        .small()
                        .outline()
                        .label("See results")
                        .tooltip("Look at what the run produced")
                        .on_click(cx.listener(|audit, _, _, cx| {
                            if let Some(first) = audit.result_rows().first().copied() {
                                audit.open_result(first, cx);
                            }
                        })),
                )
                .into_any_element(),
        )
    }

    /// Everything the scan could not take at face value, in one line rather than
    /// three scattered ones. The mislabelled count is a button: it is the audit's best
    /// finding, and a number you cannot act on is a dead end.
    pub(super) fn notices(&self, cx: &mut Context<Self>) -> Option<gpui_kit::AnyElement> {
        let mut parts = Vec::new();
        // Counts only. The names were announced in the scan toast; inline they
        // truncate into a blob nobody can read.
        if !self.unreadable.is_empty() {
            parts.push(unreadable_summary(self.unreadable.len()));
        }
        if !self.walk_errors.is_empty() {
            parts.push(format!("{} folders unreachable", self.walk_errors.len()));
        }
        if !self.failures.is_empty() {
            parts.push(format!(
                "{} failed: {}",
                self.failures.len(),
                self.failure_summary
            ));
        }
        // Behind the `updater` feature, this is what tells a windowed user their
        // next launch will be different. Nothing renders while the updater is idle.
        #[cfg(feature = "updater")]
        if let Some(line) = crate::update::notice() {
            parts.push(line);
        }
        if let Some(pairing) = &self.sirv_pairing
            && let Listing::Failed(reason) = &pairing.files
        {
            parts.push(format!("could not list {}: {reason}", pairing.dir));
        }
        if parts.is_empty() {
            return None;
        }

        // Left-aligned and only as wide as its text. A full-bleed box for six words
        // was a bigger shape on screen than the finding it was reporting. Findings
        // you can act on (heavy, mislabelled) live as chips in the toolbar; this
        // row is only for things that went wrong. One line tall unless opened;
        // scan findings render as counts here because their names live in the toast.
        let expanded = self.notices_expanded;
        Some(
            div()
                .flex()
                .items_center()
                .gap_2()
                .child(
                    div()
                        .flex_1()
                        .min_w_0()
                        .overflow_hidden()
                        .when(!expanded, |row| row.whitespace_nowrap().text_ellipsis())
                        .child(
                            Alert::warning("notices", parts.join("  ·  "))
                                .icon(IconName::TriangleAlert)
                                .py_1(),
                        ),
                )
                .child(
                    Button::new("notices-toggle")
                        .small()
                        .ghost()
                        .label(if expanded { "Less" } else { "Details" })
                        .tooltip(if expanded {
                            "Fold the warning back to one line"
                        } else {
                            "Show the whole warning"
                        })
                        .on_click(cx.listener(|audit, _, _, cx| {
                            audit.notices_expanded = !audit.notices_expanded;
                            cx.notify();
                        })),
                )
                .into_any_element(),
        )
    }

    /// Every notice in one stable lane. The workspace used to stack up to five
    /// blocks that each appeared and vanished on their own, so the list below
    /// jumped. One lane with a fixed priority order and a minimum height keeps
    /// the list still: errors first, then the finished run, then Studio, then
    /// local AI, then the spin preflight behind its extras gate. Each inner
    /// block keeps its own selector; the lane root carries `notice-lane`.
    pub(super) fn notice_lane(&self, cx: &mut Context<Self>) -> Option<gpui_kit::AnyElement> {
        // Element ids never reach `debug_bounds`, so each block rides in a
        // named wrapper: the lane root plus these names are the selector
        // contract the tests assert on.
        let mut blocks: Vec<(&'static str, gpui_kit::AnyElement)> = Vec::new();
        if let Some(block) = self.notices(cx) {
            blocks.push(("notice-lane-notices", block));
        }
        if let Some(block) = self.conversion_notice(cx) {
            blocks.push(("notice-lane-conversion", block));
        }
        if let Some(block) = self.studio_notice(cx) {
            blocks.push(("notice-lane-studio", block));
        }
        if let Some(block) = self.local_ai_notice(cx) {
            blocks.push(("notice-lane-local-ai", block));
        }
        if acquisition::SHOW_ACQUISITION_EXTRAS
            && let Some(block) = self.spin_notice(cx)
        {
            blocks.push(("notice-lane-spin", block));
        }
        if blocks.is_empty() {
            return None;
        }
        // One flag for the whole lane: expanding the lane also unfolds the long
        // warning text, so Details always means "show everything".
        let count = blocks.len();
        let expanded = self.notices_expanded;
        let wrap = |name: &'static str, block: gpui_kit::AnyElement| {
            div().debug_selector(move || name.into()).child(block)
        };
        let toggle = |label: String, tooltip: &'static str, cx: &mut Context<Self>| {
            div().debug_selector(|| "notice-lane-toggle".into()).child(
                Button::new("notice-lane-toggle")
                    .small()
                    .ghost()
                    .label(label)
                    .tooltip(tooltip)
                    .on_click(cx.listener(|audit, _, _, cx| {
                        audit.notices_expanded = !audit.notices_expanded;
                        cx.notify();
                    })),
            )
        };
        let mut lane = div()
            .flex()
            .flex_col()
            .w_full()
            .min_h(px(36.))
            .debug_selector(|| "notice-lane".into());
        if expanded {
            lane = lane
                .children(blocks.into_iter().map(|(name, block)| wrap(name, block)))
                .child(toggle(
                    "Less".to_string(),
                    "Fold the extra notices back to one lane",
                    cx,
                ));
        } else {
            let mut pending = blocks.into_iter();
            if let Some((name, first)) = pending.next() {
                lane = lane.child(wrap(name, first));
            }
            if count > 1 {
                lane = lane.child(toggle(
                    format!("{count} notices · Details"),
                    "Show every notice at once",
                    cx,
                ));
            }
        }
        Some(lane.into_any_element())
    }

    /// Folders behind the current list. A dropped batch names its count
    /// directly; a browsed folder counts the child folders the list can enter,
    /// leaving out the output folder nobody browses into.
    fn status_folder_count(&self) -> Option<usize> {
        if let Some(count) = self.batch_folders {
            return Some(count);
        }
        let count = self
            .folders
            .iter()
            .filter(|path| !path.starts_with(&self.browser_output_root))
            .count();
        (count > 0).then_some(count)
    }

    /// The bottom line: what the list holds, and what the scan left out. The
    /// totals live here alone now that the header carries no counts, so a long
    /// list cannot scroll them away. A string rather than inline elements: what
    /// a scan left out is as much a fact as what it found, and both are asserted on.
    pub(super) fn status_line(&self, count: usize) -> String {
        if self.sirv_scope == Some(SirvScope::OnlyRemote) {
            return match count {
                1 => "1 file only on Sirv".to_string(),
                _ => format!("{count} files only on Sirv"),
            };
        }
        let total = self.entries.len();
        let images = if count == total {
            match count {
                1 => "1 image".to_string(),
                _ => format!("{count} images"),
            }
        } else {
            format!("{count} of {total} images")
        };
        let bytes = if count == 0 {
            String::new()
        } else {
            format!(" · {}", format_bytes(self.visible_bytes()))
        };
        let warnings = self.warning_stats();
        // Scope reads where the totals are: the header chip is gone, and the
        // menu owns the decision, so the bar says what the numbers cover.
        // Quiet when off, like every other default.
        let scope = if self.dataset_subfolders {
            " · including subfolders"
        } else {
            ""
        };
        match self.status_folder_count() {
            Some(folders) => {
                let noun = if folders == 1 { "folder" } else { "folders" };
                format!("{folders} {noun}, {images}{bytes}{warnings}{scope}")
            }
            None => format!("{images}{bytes}{warnings}{scope}"),
        }
    }

    /// The conversion the verbs beside the list would run, in one glance. The
    /// right side of the bar would otherwise sit empty until something is
    /// ticked, and the target is exactly what an empty selection hides.
    pub(super) fn output_plan(&self) -> String {
        let edge = match self.max_edge.0 {
            None => String::new(),
            Some(edge) => format!(" · {edge}px"),
        };
        format!(
            "{} {}{} → {}",
            self.format.display(),
            self.quality.label(),
            edge,
            self.output.label(),
        )
    }

    /// The bar pinned to the window foot. One fixed height in every state, so
    /// the list above never moves when the selection or the run state changes.
    pub(super) fn status_bar(&self, count: usize, cx: &mut Context<Self>) -> impl IntoElement {
        let left = self.status_line(count);
        let right = if self.converting {
            let done = self.results.len() + self.failures.len();
            let total = self
                .active_target_count
                .unwrap_or_else(|| self.target_count());
            format!("{done} of {total} converting")
        } else if self.target_count() > 0 {
            match self.target_count() {
                1 => "1 selected".to_string(),
                selected => format!("{selected} selected"),
            }
        } else if !self.failures.is_empty() {
            match self.failures.len() {
                1 => "1 failed".to_string(),
                failed => format!("{failed} failed"),
            }
        } else {
            self.output_plan()
        };
        div()
            .debug_selector(|| "status-bar".into())
            .flex()
            .items_center()
            .justify_between()
            .gap_2()
            .px_3()
            .h(px(28.))
            .flex_shrink_0()
            .bg(cx.theme().table_head)
            .border_t_1()
            .border_color(cx.theme().border)
            .child(
                div()
                    .font_family(cx.theme().mono_font_family.clone())
                    .text_size(px(11.))
                    .font_weight(FontWeight::SEMIBOLD)
                    .text_color(cx.theme().foreground)
                    .whitespace_nowrap()
                    .overflow_hidden()
                    .text_ellipsis()
                    .min_w_0()
                    .child(left),
            )
            .child(
                div()
                    .font_family(cx.theme().mono_font_family.clone())
                    .text_size(px(11.))
                    .text_color(cx.theme().muted_foreground)
                    .whitespace_nowrap()
                    .flex_shrink_0()
                    .child(right),
            )
    }

    /// A finding shown as the control that narrows the list to it. Lit while it is the
    /// one in force, so the count and the list below it never disagree.
    pub(super) fn finding_button(
        &self,
        finding: Finding,
        icon: IconName,
        label: String,
        tooltip: &'static str,
        cx: &mut Context<Self>,
    ) -> impl IntoElement {
        let active = self.finding == Some(finding);
        Button::new(("finding", finding as usize))
            .small()
            .icon(icon)
            .label(label)
            .tooltip(tooltip)
            // `set_finding` refuses to move the list under a running conversion, so
            // the chip that asks for it says so rather than looking dead.
            .disabled(self.converting)
            .selected(active)
            .when(!active, |button| button.ghost())
            .when(active, |button| button.warning())
            .on_click(cx.listener(move |audit, _, _, cx| audit.set_finding(finding, cx)))
    }
}
