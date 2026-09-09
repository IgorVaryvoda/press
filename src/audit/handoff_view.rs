//! The imported report's review card.
//!
//! Everything drawn here comes from the validated report and from filesystem
//! reads already done in actions: rendering never touches the disk. The card
//! shows what the producer said and what matching found, and keeps the two
//! apart — no automatic verdict is drawn as a confirmation, and an unknown
//! value is drawn as unknown.

use super::handoff_actions::{
    HandoffReview, ReviewRow, RowState, choice_labels, label, provenance_lines, resource_lines,
    text_line,
};
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
            .map(|(index, row)| self.handoff_row(review, index, row, review.root.is_some(), cx))
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
                    .child(if self.handoff_reading {
                        "Reading one source; the other rows wait for it.".to_string()
                    } else {
                        format!(
                            "{confirmed} of {} confirmed · nothing is converted from this review",
                            handoff.resources.len()
                        )
                    }),
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
        let muted = cx.theme().muted_foreground;
        div()
            .debug_selector(|| "handoff-provenance".into())
            .flex()
            .flex_col()
            .min_w_0()
            .text_size(px(11.))
            .text_color(muted)
            .children(provenance_lines(review).into_iter().map(|line| {
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
        review: &HandoffReview,
        index: usize,
        row: &ReviewRow,
        rooted: bool,
        cx: &Context<Self>,
    ) -> impl IntoElement + use<> {
        let muted = cx.theme().muted_foreground;
        let busy = self.converting || matches!(row.state, RowState::Busy);
        let facts = resource_lines(review, row);
        let root = review.root.as_deref();
        let mut choices = Vec::new();
        for (position, path) in row.choices.iter().enumerate() {
            let id = row.id.clone();
            let selected = row.chosen.as_deref() == Some(path.as_path());
            // Two files can share a basename, so a chip that showed only the
            // name would ask the user to choose between two identical labels.
            // The path under the root distinguishes them, bounded from both
            // ends so a long shared folder prefix cannot make two chips read
            // alike either. The whole path is the announced name and the
            // tooltip, so the keyboard reaches it as well as the pointer.
            let (name, full) = choice_labels(root, path);
            choices.push(
                Button::new(("handoff-choice", index * 100 + position))
                    .small()
                    .when(selected, |chip| chip.outline())
                    .when(!selected, |chip| chip.ghost())
                    .debug_selector(move || format!("handoff-choice-{index}-{position}"))
                    .label(name)
                    .tooltip(full.clone())
                    .accessibility_label(full)
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
        // One source read at a time, across the whole review: while one is
        // out, no row offers to start another.
        let reading = self.handoff_reading;
        let confirmable = rooted && row.chosen.is_some() && !busy && !reading;
        let recheckable = matches!(row.state, RowState::Confirmed(_)) && !busy && !reading;
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
                            // A producer chose this id; it is drawn text like
                            // every other string the report supplies.
                            .child(text_line(&row.id)),
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
                    .debug_selector(move || format!("handoff-facts-{index}"))
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
                            .disabled(!rooted || busy || reading)
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
