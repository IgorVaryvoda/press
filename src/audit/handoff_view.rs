//! The imported report's review card.
//!
//! Everything drawn here comes from the validated report and from filesystem
//! reads already done in actions: rendering never touches the disk. The card
//! shows what the producer said and what matching found, and keeps the two
//! apart — no automatic verdict is drawn as a confirmation, and an unknown
//! value is drawn as unknown.

use super::handoff_actions::{HandoffReview, ReviewRow, RowState, advisory_line, label};
use super::{Audit, is_checkbox_activation_key};
use gpui_kit::component::button::{Button, ButtonVariants};
use gpui_kit::component::{ActiveTheme, Disableable, Sizable};
use gpui_kit::{Context, FontWeight, div, prelude::*, px};

/// How tall the review's resource list may grow before it scrolls inside
/// itself. The rail's settings area is about 370px on the narrowest window
/// the app allows, and the whole card — heading, provenance, resources,
/// status and actions — has to fit inside that rather than pushing the
/// folder's own controls out of reach.
pub(super) const CARD_MAX_HEIGHT: f32 = 200.;

impl Audit {
    pub(super) fn handoff_section(&self, cx: &Context<Self>) -> impl IntoElement + use<> {
        div()
            .debug_selector(|| "handoff-section".into())
            .flex()
            .flex_col()
            .flex_shrink_0()
            .min_w_0()
            .children(
                self.handoff_review
                    .as_ref()
                    .map(|review| self.handoff_card(review, cx)),
            )
    }

    fn handoff_card(&self, review: &HandoffReview, cx: &Context<Self>) -> impl IntoElement + use<> {
        let handoff = &review.pending.handoff;
        let root_text = match review.root.as_deref() {
            Some(root) => label(root),
            None => "no folder chosen yet".to_string(),
        };
        let confirmed = review.confirmed_count();
        let rows: Vec<_> = review
            .rows
            .iter()
            .enumerate()
            .map(|(index, row)| self.handoff_row(index, row, review.root.is_some(), cx))
            .collect();
        div()
            .debug_selector(|| "handoff-card".into())
            // The card owns the keyboard while it is up. Its handle sits on
            // this wrapper rather than on a button, because gpui-component's
            // Button always renders its own keyed focus handle and discards a
            // caller's; a tab group lets `focus_next` hand a real button the
            // focus, so it keeps its ring and its native Enter and Space.
            .track_focus(&self.handoff_review_focus)
            .tab_group()
            .tab_index(0)
            .tab_stop(false)
            .on_key_down(
                cx.listener(|audit, event: &gpui_kit::KeyDownEvent, window, cx| {
                    if event.keystroke.key == "escape"
                        && event.keystroke.modifiers == gpui_kit::Modifiers::none()
                    {
                        audit.cancel_handoff_review(window, cx);
                        cx.stop_propagation();
                    } else if is_checkbox_activation_key(event) {
                        // The card's buttons activate themselves on Enter and
                        // Space. The list underneath must not also open a
                        // comparison or tick the row under its cursor.
                        cx.stop_propagation();
                    }
                }),
            )
            .flex()
            .flex_col()
            .min_w_0()
            .gap_1()
            .p_2()
            .border_1()
            .border_color(cx.theme().border)
            .child(
                div()
                    .text_size(px(12.))
                    .font_weight(FontWeight::SEMIBOLD)
                    .child("Review imported ImageGuide report"),
            )
            .child(
                div()
                    .id("handoff-body")
                    .debug_selector(|| "handoff-body".into())
                    .flex()
                    .flex_col()
                    .min_w_0()
                    .min_h_0()
                    .max_h(px(CARD_MAX_HEIGHT))
                    .overflow_y_scroll()
                    .track_scroll(&self.handoff_scroll)
                    .gap_1()
                    .child(self.handoff_provenance(review, cx))
                    .child(
                        div()
                            .debug_selector(|| "handoff-resources".into())
                            .flex()
                            .flex_col()
                            .gap_1()
                            .children(rows),
                    ),
            )
            .child(
                div()
                    .debug_selector(|| "handoff-status".into())
                    .text_size(px(11.))
                    .text_color(cx.theme().muted_foreground)
                    .child(format!(
                        "{confirmed} of {} confirmed · nothing is converted from this review",
                        handoff.resources.len()
                    )),
            )
            .child(
                div()
                    .flex()
                    .flex_wrap()
                    .gap_1()
                    .child(
                        Button::new("handoff-root")
                            .small()
                            .outline()
                            .debug_selector(|| "handoff-root".into())
                            .label(if review.root.is_some() {
                                "Change folder…"
                            } else {
                                "Choose folder…"
                            })
                            .disabled(self.converting || review.scanning)
                            .on_click(cx.listener(|audit, _, _, cx| {
                                audit.choose_handoff_root(cx);
                            })),
                    )
                    .child(
                        Button::new("handoff-cancel")
                            .small()
                            .ghost()
                            .debug_selector(|| "handoff-cancel".into())
                            .label("Cancel review")
                            .on_click(cx.listener(|audit, _, window, cx| {
                                audit.cancel_handoff_review(window, cx);
                            })),
                    ),
            )
            .child(
                div()
                    .debug_selector(|| "handoff-root-path".into())
                    .min_w_0()
                    .whitespace_nowrap()
                    .overflow_hidden()
                    .text_ellipsis()
                    .text_size(px(11.))
                    .text_color(cx.theme().muted_foreground)
                    .child(if review.scanning {
                        "Reading the chosen folder…".to_string()
                    } else {
                        format!("Source folder: {root_text}")
                    }),
            )
    }

    /// Who produced the report, when, and what it says about its own limits.
    /// Redactions and preserved unknown fields are part of that: a report that
    /// removed page identity, or carries advisory metadata this Press does not
    /// read, must say so rather than look complete.
    fn handoff_provenance(
        &self,
        review: &HandoffReview,
        cx: &Context<Self>,
    ) -> impl IntoElement + use<> {
        let handoff = &review.pending.handoff;
        let muted = cx.theme().muted_foreground;
        let mut lines = vec![
            format!(
                "Producer: {} {}",
                handoff.producer, handoff.producer_revision
            ),
            format!("Task: {}", handoff.task),
            format!("Observed: {}", handoff.observed),
            format!("File: {}", label(&review.file)),
        ];
        if handoff.redactions.is_empty() {
            lines.push("Redactions: none declared".into());
        } else {
            lines.push(format!("Redactions: {}", handoff.redactions.join(", ")));
        }
        // Scope, limits and every other field this schema does not define are
        // shown exactly as the file holds them. Press has not read them, and
        // saying so is the only honest label for a value it cannot interpret.
        for (key, value) in &handoff.unknown {
            lines.push(advisory_line(key, value));
        }
        for warning in &review.pending.warnings {
            lines.push(format!("Warning: {warning}"));
        }
        div()
            .debug_selector(|| "handoff-provenance".into())
            .flex()
            .flex_col()
            .min_w_0()
            .text_size(px(11.))
            .text_color(muted)
            .children(lines.into_iter().map(|line| {
                div()
                    .min_w_0()
                    .whitespace_nowrap()
                    .overflow_hidden()
                    .text_ellipsis()
                    .child(line)
            }))
    }

    /// One reported resource: what the producer observed, what matching found,
    /// which file the user chose, and whether they have confirmed its bytes.
    fn handoff_row(
        &self,
        index: usize,
        row: &ReviewRow,
        rooted: bool,
        cx: &Context<Self>,
    ) -> impl IntoElement + use<> {
        let resource = self
            .handoff_review
            .as_ref()
            .and_then(|review| {
                review
                    .pending
                    .handoff
                    .resources
                    .iter()
                    .find(|resource| resource.id == row.id)
            })
            .expect("every review row names a resource of its own report");
        let muted = cx.theme().muted_foreground;
        let busy = self.converting || matches!(row.state, RowState::Busy);
        let mut facts = Vec::new();
        facts.push(match (resource.width, resource.height) {
            (Some(width), Some(height)) => format!("Observed: {width}×{height}"),
            // An absent dimension is unknown, not zero and not a default.
            _ => "Observed: dimensions unknown".to_string(),
        });
        facts.push(match resource.bytes {
            Some(bytes) if resource.bytes_measured => {
                format!("Bytes: {} measured", crate::scan::format_bytes(bytes))
            }
            Some(bytes) => format!("Bytes: {} estimated", crate::scan::format_bytes(bytes)),
            None => "Bytes: unknown".to_string(),
        });
        if !resource.findings.is_empty() {
            facts.push(format!("Findings: {}", resource.findings.join(", ")));
        }
        if !resource.formats.is_empty() {
            facts.push(format!(
                "Requested formats: {}",
                resource.formats.join(", ")
            ));
        }
        match resource.max_edge {
            Some(edge) => facts.push(format!("Requested max edge: {edge}px")),
            None => facts.push("Requested max edge: none".into()),
        }
        // Advisory producer metadata, including any observed or recommended
        // format. It is shown, never applied: this review changes no recipe.
        for (key, value) in &resource.unknown {
            facts.push(advisory_line(key, value));
        }
        for note in &row.notes {
            facts.push(format!("Note: {note}"));
        }
        if let RowState::Refused(reason) = &row.state {
            facts.push(format!("Refused: {reason}"));
        }
        if matches!(row.state, RowState::Changed) {
            facts.push("The file's bytes changed since it was confirmed.".into());
        }
        let match_text = match row.verdict {
            None => "Match: choose a source folder first".to_string(),
            Some(verdict) => format!(
                "Match: {} (automatic; not a confirmation)",
                crate::handoff::verdict_word(verdict)
            ),
        };
        let chosen_text = match row.chosen.as_deref() {
            Some(path) => format!("Chosen: {}", label(path)),
            None => "Chosen: no file".to_string(),
        };
        let hidden_text = (row.hidden > 0).then(|| {
            format!(
                "{} more files match; reach them with Choose file…",
                row.hidden
            )
        });
        let mut choices = Vec::new();
        for (position, path) in row.choices.iter().enumerate() {
            let id = row.id.clone();
            let selected = row.chosen.as_deref() == Some(path.as_path());
            let name = path
                .file_name()
                .map(|name| name.to_string_lossy().into_owned())
                .unwrap_or_else(|| label(path));
            choices.push(
                Button::new(("handoff-choice", index * 100 + position))
                    .small()
                    .when(selected, |chip| chip.outline())
                    .when(!selected, |chip| chip.ghost())
                    .debug_selector(move || format!("handoff-choice-{index}-{position}"))
                    .label(name)
                    .disabled(busy)
                    .on_click(cx.listener(move |audit, _, _, cx| {
                        // Chosen by position in this row's own list of paths,
                        // never by parsing the label above back into a file.
                        audit.select_handoff_choice(&id, position, cx);
                    })),
            );
        }
        let manual_id = row.id.clone();
        let confirm_id = row.id.clone();
        let recheck_id = row.id.clone();
        let confirmable = rooted && row.chosen.is_some() && !busy;
        let recheckable = matches!(row.state, RowState::Confirmed(_)) && !busy;
        div()
            .debug_selector(move || format!("handoff-row-{index}"))
            .flex()
            .flex_col()
            .min_w_0()
            .gap_1()
            .p_1()
            .border_1()
            .border_color(cx.theme().border)
            .child(
                div()
                    .flex()
                    .items_center()
                    .gap_1()
                    .min_w_0()
                    .child(
                        div()
                            .flex_1()
                            .min_w_0()
                            .whitespace_nowrap()
                            .overflow_hidden()
                            .text_ellipsis()
                            .text_size(px(11.))
                            .font_weight(FontWeight::SEMIBOLD)
                            .child(row.id.clone()),
                    )
                    .child(
                        div()
                            .flex_shrink_0()
                            .debug_selector(move || format!("handoff-state-{index}"))
                            .text_size(px(11.))
                            .text_color(muted)
                            .child(row.state.word()),
                    ),
            )
            .child(
                div()
                    .flex()
                    .flex_col()
                    .min_w_0()
                    .text_size(px(11.))
                    .text_color(muted)
                    .child(
                        div()
                            .debug_selector(move || format!("handoff-match-{index}"))
                            .min_w_0()
                            .whitespace_nowrap()
                            .overflow_hidden()
                            .text_ellipsis()
                            .child(match_text),
                    )
                    .child(
                        div()
                            .debug_selector(move || format!("handoff-chosen-{index}"))
                            .min_w_0()
                            .whitespace_nowrap()
                            .overflow_hidden()
                            .text_ellipsis()
                            .child(chosen_text),
                    )
                    .children(hidden_text.map(|text| {
                        div()
                            .min_w_0()
                            .whitespace_nowrap()
                            .overflow_hidden()
                            .text_ellipsis()
                            .child(text)
                    }))
                    .children(facts.into_iter().map(|fact| {
                        div()
                            .min_w_0()
                            .whitespace_nowrap()
                            .overflow_hidden()
                            .text_ellipsis()
                            .child(fact)
                    })),
            )
            .child(
                div()
                    .flex()
                    .flex_wrap()
                    .gap_1()
                    .min_w_0()
                    .children(choices)
                    .child(
                        Button::new(("handoff-pick", index))
                            .small()
                            .ghost()
                            .debug_selector(move || format!("handoff-pick-{index}"))
                            .label("Choose file…")
                            .disabled(!rooted || busy)
                            .on_click(cx.listener(move |audit, _, _, cx| {
                                audit.choose_handoff_source(&manual_id, cx);
                            })),
                    )
                    .child(
                        Button::new(("handoff-confirm", index))
                            .small()
                            .outline()
                            .debug_selector(move || format!("handoff-confirm-{index}"))
                            .label("Confirm")
                            .disabled(!confirmable)
                            .on_click(cx.listener(move |audit, _, _, cx| {
                                audit.confirm_handoff_row(&confirm_id, cx);
                            })),
                    )
                    .child(
                        Button::new(("handoff-recheck", index))
                            .small()
                            .ghost()
                            .debug_selector(move || format!("handoff-recheck-{index}"))
                            .label("Recheck")
                            .disabled(!recheckable)
                            .on_click(cx.listener(move |audit, _, _, cx| {
                                audit.recheck_handoff_row(&recheck_id, cx);
                            })),
                    ),
            )
    }
}
