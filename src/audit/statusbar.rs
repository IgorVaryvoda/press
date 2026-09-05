//! The status bar: summary counts and finding filters.

use super::*;

impl Audit {
    /// One sentence for a finished run, toasted once on completion so the
    /// outcome survives the results view closing.
    pub(super) fn conversion_summary(&self) -> String {
        let (before, after) = self.converted_totals();
        let delta = before.abs_diff(after);
        format!(
            "Converted {} {} to {} · {} {} · in {}",
            self.results.len(),
            if self.results.len() == 1 {
                "image"
            } else {
                "images"
            },
            self.format.display(),
            format_bytes(delta),
            if after > before { "larger" } else { "saved" },
            self.output.label(),
        )
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
    /// totals are pinned here so a long list cannot scroll them away; the scan
    /// toast announces them once on arrival. A string rather than inline
    /// elements: what a scan left out is as much a fact as what it found, and
    /// both are asserted on.
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
                    .flex()
                    .items_center()
                    .gap_1()
                    .flex_shrink_0()
                    .children(self.findings_menu(cx))
                    .child(
                        div()
                            .font_family(cx.theme().mono_font_family.clone())
                            .text_size(px(11.))
                            .text_color(cx.theme().muted_foreground)
                            .whitespace_nowrap()
                            .child(right),
                    ),
            )
    }

    /// Hide the finding menu when empty; highlight it while a filter is active.
    fn findings_menu(&self, cx: &mut Context<Self>) -> Option<gpui_kit::AnyElement> {
        let findings = self.available_findings();
        if findings.is_empty() {
            return None;
        }
        let total: usize = findings
            .iter()
            .map(|(finding, _, _)| match finding {
                Finding::Failed => self.failures.len(),
                Finding::Heavy => self.heavy,
                Finding::Mislabelled => self.mislabelled,
            })
            .sum();
        let summary = findings
            .iter()
            .map(|(_, _, label)| label.as_str())
            .collect::<Vec<_>>()
            .join(" · ");
        let active = self.finding;
        let source = cx.entity().downgrade();
        let has_failed = findings
            .iter()
            .any(|(finding, _, _)| *finding == Finding::Failed);
        let menu = div().debug_selector(|| "findings-menu".into()).child(
            Button::new("findings-menu")
                .small()
                .icon(IconName::TriangleAlert)
                .tooltip(format!(
                    "Findings ({total}): {summary} — narrow the list to one finding"
                ))
                .selected(active.is_some())
                .when(active.is_none(), |button| button.ghost())
                .when(active.is_some(), |button| button.warning())
                // `set_finding` refuses to move the list under a running
                // conversion, so the control that asks for it says so rather
                // than looking dead.
                .disabled(self.converting)
                .dropdown_menu(move |menu, _, _| {
                    findings.iter().fold(menu, |menu, (finding, icon, label)| {
                        let finding = *finding;
                        let label = label.clone();
                        let source = source.clone();
                        menu.item(
                            PopupMenuItem::new(label)
                                .icon(icon.clone())
                                .checked(active == Some(finding))
                                .on_click(move |_, _, cx| {
                                    if let Some(audit) = source.upgrade() {
                                        audit
                                            .update(cx, |audit, cx| audit.set_finding(finding, cx));
                                    }
                                }),
                        )
                    })
                }),
        );
        // The failures keep their own selector, so the run that produced them
        // stays one lookup away.
        Some(if has_failed {
            div()
                .debug_selector(|| "finding-failed".into())
                .child(menu)
                .into_any_element()
        } else {
            menu.into_any_element()
        })
    }
}
