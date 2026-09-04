//! The status bar: summary counts, notices row, finding buttons.

use super::*;

impl Audit {
    /// A live local inference run stays visible while it works. Its outcome
    /// toasts once at the production site; a green block for a finished file
    /// is just a wide way to say what the toast already said.
    pub(super) fn local_ai_notice(&self, _cx: &mut Context<Self>) -> Option<gpui_kit::AnyElement> {
        let job = self.local_ai_job.as_ref()?;
        let message = job.message(&self.root);
        let alert = match job.state {
            LocalAiJobState::SettingUp | LocalAiJobState::Running => {
                Alert::info("local-ai-status", message)
            }
            LocalAiJobState::Done(_) | LocalAiJobState::Failed(_) => return None,
        };
        Some(alert.py_1().into_any_element())
    }

    /// The same for a live Studio run, except the confirmation prompt: that is
    /// a decision the run is waiting on, and it keeps its warning card.
    pub(super) fn studio_notice(&self, _cx: &mut Context<Self>) -> Option<gpui_kit::AnyElement> {
        let job = self.studio_job.as_ref()?;
        let message = job.message(&self.root);
        let alert = match job.state {
            StudioJobState::Preparing | StudioJobState::Running => {
                Alert::info("studio-status", message)
            }
            StudioJobState::AwaitingConfirmation(_) => Alert::warning("studio-status", message),
            StudioJobState::Done(_) | StudioJobState::Failed(_) => return None,
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

    /// Every notice in one stable lane. The workspace used to stack blocks that
    /// each appeared and vanished on their own, so the list below jumped. One
    /// lane with a fixed priority order and a minimum height keeps the list
    /// still: the finished run first, then Studio, then local AI, then the spin
    /// preflight behind its extras gate. Alerts (scan findings, transfer and
    /// update outcomes) toast instead of sitting here. Each inner block keeps
    /// its own selector; the lane root carries `notice-lane`.
    pub(super) fn notice_lane(&self, cx: &mut Context<Self>) -> Option<gpui_kit::AnyElement> {
        // Element ids never reach `debug_bounds`, so each block rides in a
        // named wrapper: the lane root plus these names are the selector
        // contract the tests assert on.
        let mut blocks: Vec<(&'static str, gpui_kit::AnyElement)> = Vec::new();
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
        // One flag for the whole lane: Details always means "show everything".
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
}
