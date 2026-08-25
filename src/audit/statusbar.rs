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
        if let Some(job) = &self.sirv_job
            && (!job.finished || job.failed > 0)
        {
            let verb = match job.kind {
                SirvJobKind::Pull => "Sirv pull",
                SirvJobKind::PullChanged => "Sirv pull (overwrite)",
                SirvJobKind::Push => "Sirv push",
                SirvJobKind::PushChanged => "Sirv push (overwrite)",
            };
            let failures = if job.failed == 0 {
                String::new()
            } else {
                let rest = job.failed.saturating_sub(job.failures.len());
                format!(
                    ", {} failed: {}{}",
                    job.failed,
                    job.failures.join(", "),
                    if rest == 0 {
                        String::new()
                    } else {
                        format!(" and {rest} more")
                    },
                )
            };
            parts.push(format!("{verb}: {} of {}{failures}", job.done, job.total));
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
