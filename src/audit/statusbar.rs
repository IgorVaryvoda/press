//! The status bar: summary counts, notices row, finding buttons.

use super::*;

impl Audit {
    /// A local inference result stays visible after its comparison closes. Unlike
    /// scan notices, this is normal work and uses info/success/error semantics.
    pub(super) fn local_ai_notice(&self, _cx: &mut Context<Self>) -> Option<gpui::AnyElement> {
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

    pub(super) fn studio_notice(&self, _cx: &mut Context<Self>) -> Option<gpui::AnyElement> {
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
    pub(super) fn conversion_notice(&self, cx: &mut Context<Self>) -> Option<gpui::AnyElement> {
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
    pub(super) fn notices(&self, _cx: &mut Context<Self>) -> Option<gpui::AnyElement> {
        let mut parts = Vec::new();
        if !self.unreadable.is_empty() {
            parts.push(format!(
                "would not decode: {}",
                named(self.unreadable.iter().filter_map(|path| {
                    path.file_name()
                        .map(|name| name.to_string_lossy().into_owned())
                }))
            ));
        }
        if !self.walk_errors.is_empty() {
            parts.push(format!(
                "could not enter: {}",
                named(self.walk_errors.iter().filter_map(|path| {
                    path.file_name()
                        .map(|name| name.to_string_lossy().into_owned())
                }))
            ));
        }
        if !self.failures.is_empty() {
            parts.push(format!("failed: {}", named(self.failures.iter().cloned())));
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
        // row is only for things that went wrong.
        Some(
            Alert::warning("notices", parts.join("  ·  "))
                .icon(IconName::TriangleAlert)
                .py_1()
                .into_any_element(),
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
            .selected(active)
            .when(!active, |button| button.ghost())
            .when(active, |button| button.warning())
            .on_click(cx.listener(move |audit, _, _, cx| audit.set_finding(finding, cx)))
    }
}
