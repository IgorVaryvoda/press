//! The output inspector: the whole conversion story in one right-hand column —
//! presets on top, the fine-tune knobs under them, and at the foot the
//! projected result next to the button that commits it. Settings, feedback and
//! action share one surface instead of being smeared across a toolbar and a
//! status bar that never faced each other.

use super::*;

/// Width of the inspector. The table and the gallery lay themselves out
/// against the viewport minus this, so the panel never silently squeezes
/// their column math.
pub(super) const OUTPUT_PANEL_WIDTH: f32 = 264.;

/// The named outputs most runs want. Listed once; the rows and the settings
/// they apply cannot disagree.
const PRESETS: [(&str, &str, Format, Quality, MaxEdge); 3] = [
    (
        "Recommended",
        "WebP · quality 80 · original size",
        Format::WebP,
        Quality(Some(80.)),
        MaxEdge::FULL,
    ),
    (
        "Small files",
        "AVIF · quality 60 · max 2400px",
        Format::Avif,
        Quality(Some(60.)),
        MaxEdge(Some(2400)),
    ),
    (
        "Pixel-perfect",
        "WebP · lossless · original size",
        Format::WebP,
        Quality::LOSSLESS,
        MaxEdge::FULL,
    ),
];

pub(super) fn active_preset(format: Format, quality: Quality, edge: MaxEdge) -> Option<usize> {
    PRESETS
        .iter()
        .position(|(_, _, preset_format, preset_quality, preset_edge)| {
            format == *preset_format && quality == *preset_quality && edge == *preset_edge
        })
}

pub(super) fn sampling_note(sampled: usize, total: usize) -> String {
    if sampled < total {
        format!(" · {sampled}\u{a0}of\u{a0}{total}\u{a0}sampled")
    } else {
        String::new()
    }
}

impl Audit {
    pub(super) fn output_panel(&self, cx: &mut Context<Self>) -> impl IntoElement {
        div()
            .w(px(OUTPUT_PANEL_WIDTH))
            .flex_shrink_0()
            .flex()
            .flex_col()
            .gap_2()
            .px_3()
            .py_2()
            .border_l_1()
            .border_color(cx.theme().border)
            .bg(cx.theme().secondary)
            .child(
                div()
                    .id("output-settings")
                    .flex()
                    .flex_col()
                    .max_h(px(440.))
                    .overflow_y_scroll()
                    .gap_2()
                    .child(self.panel_heading("Output", cx))
                    .child(
                        div()
                            .debug_selector(|| "output-destination".into())
                            .flex()
                            .items_center()
                            .gap_1()
                            .text_size(px(12.))
                            .text_color(cx.theme().foreground)
                            .child(IconName::FolderOpen)
                            .child("optimized/ · originals unchanged"),
                    )
                    .child(
                        div()
                            .flex()
                            .flex_col()
                            .gap_1()
                            .child(self.preset_row(0, cx))
                            .child(self.preset_row(1, cx))
                            .child(self.preset_row(2, cx)),
                    )
                    .child(
                        div()
                            .flex()
                            .items_center()
                            .justify_between()
                            .child(self.panel_heading("Fine-tune", cx))
                            .when(
                                active_preset(self.format, self.quality, self.max_edge).is_none(),
                                |heading| {
                                    heading.child(
                                        div()
                                            .debug_selector(|| "custom-settings-active".into())
                                            .text_size(px(11.))
                                            .font_weight(FontWeight::SEMIBOLD)
                                            .text_color(cx.theme().blue)
                                            .child("CUSTOM ACTIVE"),
                                    )
                                },
                            ),
                    )
                    .child(
                        div()
                            .flex()
                            .flex_col()
                            .gap_2()
                            .child(self.panel_setting(
                                "Format",
                                self.format_group(cx).small().compact(),
                                cx,
                            ))
                            .child(self.panel_quality(cx))
                            .child(self.panel_setting(
                                "Max size",
                                self.resize_group(cx).small().compact(),
                                cx,
                            )),
                    ),
            )
            // The decision and commit stay visible while the settings above them
            // scroll at short window heights.
            .child(self.output_summary(cx))
    }

    /// A section word, quiet and spaced like an instrument label.
    fn panel_heading(&self, text: &'static str, cx: &Context<Self>) -> impl IntoElement {
        div()
            .text_size(px(11.))
            .font_weight(FontWeight::SEMIBOLD)
            .text_color(cx.theme().muted_foreground)
            .child(text.to_uppercase())
    }

    /// A label over its control, each on its own line: the column is narrow on
    /// purpose, and side-by-side labels were what made the old strip cramped.
    fn panel_setting(
        &self,
        label: &'static str,
        control: impl IntoElement,
        cx: &Context<Self>,
    ) -> impl IntoElement {
        div()
            .flex()
            .flex_col()
            .gap_1()
            .child(
                div()
                    .text_size(px(12.))
                    .text_color(cx.theme().muted_foreground)
                    .child(label),
            )
            .child(control)
    }

    /// One preset as a full-width selectable row: the name, and under it the
    /// exact settings it applies — no memory required. Lit only while the live
    /// settings are exactly what it names, so a hand-tuned output lights none.
    fn preset_row(&self, index: usize, cx: &mut Context<Self>) -> impl IntoElement {
        let (name, summary, _, _, _) = PRESETS[index];
        let selected = active_preset(self.format, self.quality, self.max_edge) == Some(index);
        div()
            .id(("preset", index))
            .flex()
            .flex_col()
            .px_2()
            .py_1()
            .rounded_md()
            .cursor_pointer()
            .when(selected, |row| {
                row.bg(cx.theme().list_active)
                    .border_1()
                    .border_color(cx.theme().list_active_border)
            })
            .when(!selected, |row| {
                row.border_1()
                    .border_color(gpui::transparent_black())
                    .hover(|row| row.bg(cx.theme().list_hover))
            })
            .child(
                div()
                    .text_size(px(13.))
                    .text_color(cx.theme().foreground)
                    .child(name),
            )
            .child(
                div()
                    .text_size(px(11.))
                    .text_color(cx.theme().muted_foreground)
                    .child(summary),
            )
            .on_click(cx.listener(move |audit, _, window, cx| {
                if audit.converting {
                    return;
                }
                let (_, _, format, quality, edge) = PRESETS[index];
                audit.format = format;
                audit.quality = quality;
                audit.max_edge = edge;
                if let Some(value) = quality.0 {
                    // Keep the slider where the preset put things, or the knob
                    // below would contradict the number in the estimate.
                    audit.slider_quality = value;
                    audit
                        .quality_slider
                        .update(cx, |slider, cx| slider.set_value(value, window, cx));
                }
                audit.clear_results();
                audit.schedule_estimate(cx);
                cx.notify();
            }))
    }

    /// The quality knob: label and value on one line, the slider full-width
    /// under them, Lossless under that. While lossless is on, the slider is
    /// replaced by the word — a live slider would answer a stray drag and
    /// silently leave lossless.
    fn panel_quality(&self, cx: &mut Context<Self>) -> impl IntoElement {
        let lossless = self.quality == Quality::LOSSLESS;
        div()
            .flex()
            .flex_col()
            .gap_1()
            .child(
                div()
                    .flex()
                    .items_center()
                    .justify_between()
                    .child(
                        div()
                            .text_size(px(12.))
                            .text_color(cx.theme().muted_foreground)
                            .child("Quality"),
                    )
                    .child(
                        div()
                            .font_family(cx.theme().mono_font_family.clone())
                            .text_size(px(12.))
                            .text_color(if lossless {
                                cx.theme().muted_foreground
                            } else {
                                cx.theme().foreground
                            })
                            .child(match self.quality.0 {
                                Some(value) => format!("{}", value.round() as u32),
                                None => "lossless".to_string(),
                            }),
                    ),
            )
            .child(
                div()
                    .debug_selector(|| "quality-control".to_string())
                    .when(lossless, |rail| rail.h(px(0.)))
                    .when(!lossless && self.converting, |rail| {
                        rail.child(
                            Progress::new("quality-locked")
                                .value(self.quality.0.unwrap_or(100.))
                                .color(cx.theme().primary)
                                .h(px(6.)),
                        )
                    })
                    .when(!lossless && !self.converting, |rail| {
                        rail.child(Slider::new(&self.quality_slider).horizontal())
                    }),
            )
            // AVIF is lossy-only here, and a switch that lies about that is worse
            // than no switch.
            .children(self.format.supports_lossless().then(|| {
                Switch::new("lossless")
                    .checked(lossless)
                    .label("Lossless")
                    .disabled(self.converting)
                    .on_click(cx.listener(|audit, _, _, cx| {
                        if audit.converting {
                            return;
                        }
                        // A second click on a lit toggle has to turn it off, or
                        // lossless is a one-way door.
                        audit.quality = if audit.quality == Quality::LOSSLESS {
                            Quality::lossy(audit.slider_quality)
                        } else {
                            Quality::LOSSLESS
                        };
                        audit.clear_results();
                        audit.schedule_estimate(cx);
                        cx.notify();
                    }))
            }))
    }

    /// The payoff and the commit, immediately after the knobs that determine it:
    /// what the folder costs now, what it would cost converted, and the button.
    fn output_summary(&self, cx: &mut Context<Self>) -> impl IntoElement {
        let target_count = self.target_count();
        // Source bytes only appear before a conversion. While results stream
        // in, avoid walking thousands of rows on every progress redraw.
        let source = if !self.converting && self.results.is_empty() {
            self.target_bytes()
        } else {
            0
        };

        // Four states, one shape: a headline, its tone, a sentence of detail,
        // the share remaining, and the percent tag.
        let (state, headline, tone, detail, bar, tag) = if self.converting {
            let done = self.results.len() + self.failures.len();
            let total = self.active_target_count.unwrap_or(target_count);
            (
                Some(("CONVERTING", cx.theme().foreground)),
                format!("{done} of {total}"),
                cx.theme().foreground,
                format!(
                    "Converting to {} {}…",
                    self.format.label().to_uppercase(),
                    self.quality.label()
                ),
                Some((1. - done as f32 / total.max(1) as f32, cx.theme().primary)),
                None,
            )
        } else if !self.results.is_empty() {
            let (before, after) = self.converted_totals();
            let growth = after > before;
            let delta = before.abs_diff(after);
            let percent = delta as f32 / before.max(1) as f32 * 100.;
            (
                Some(("COMPLETED · ACTUAL RESULT", cx.theme().green)),
                format!(
                    "{} {}",
                    format_bytes(delta),
                    if growth { "larger" } else { "saved" }
                ),
                if growth {
                    cx.theme().yellow
                } else {
                    cx.theme().green
                },
                format!(
                    "{} files · {} → {}",
                    self.results.len(),
                    format_bytes(before),
                    format_bytes(after)
                ),
                Some((
                    after as f32 / before.max(1) as f32,
                    if growth {
                        cx.theme().yellow
                    } else {
                        cx.theme().green
                    },
                )),
                Some((growth, percent)),
            )
        } else if let Some((projected, sampled)) = self.estimate {
            let growth = projected > source;
            let delta = source.abs_diff(projected);
            let percent = delta as f32 / source.max(1) as f32 * 100.;
            (
                Some(("ESTIMATE", cx.theme().muted_foreground)),
                format!("≈{} output", format_bytes(projected)),
                if growth {
                    cx.theme().yellow
                } else {
                    cx.theme().green
                },
                format!(
                    "from {}{}",
                    format_bytes(source),
                    sampling_note(sampled, target_count),
                ),
                Some((
                    projected as f32 / source.max(1) as f32,
                    if growth {
                        cx.theme().yellow
                    } else {
                        cx.theme().green
                    },
                )),
                Some((growth, percent)),
            )
        } else {
            (
                None,
                "Sizing it up…".to_string(),
                cx.theme().muted_foreground,
                format!("{} on disk", format_bytes(source)),
                None,
                None,
            )
        };

        let (fraction, colour) = bar
            .map(|(remaining, colour)| (1. - remaining, colour))
            .unwrap_or((0., gpui::transparent_black()));

        div()
            .flex()
            .flex_col()
            .gap_1()
            .when(target_count > 0, |summary| {
                summary
                    .children(state.map(|(label, colour)| {
                        div()
                            .debug_selector(|| "output-state".into())
                            .text_size(px(10.))
                            .font_weight(FontWeight::SEMIBOLD)
                            .text_color(colour)
                            .child(label)
                    }))
                    .child(meter("saving", fraction, colour, 3.))
                    .child(
                        div()
                            .flex()
                            .items_center()
                            .gap_2()
                            .child(
                                div()
                                    .font_family("SF Pro Display")
                                    .text_size(px(16.))
                                    .font_weight(FontWeight::SEMIBOLD)
                                    .text_color(tone)
                                    .whitespace_nowrap()
                                    .child(headline),
                            )
                            .children(tag.map(|(growth, percent)| {
                                let tag = if growth {
                                    Tag::warning()
                                } else {
                                    Tag::success()
                                };
                                tag.small().child(if growth {
                                    format!("+{percent:.0}%")
                                } else {
                                    format!("−{percent:.0}%")
                                })
                            })),
                    )
                    .child(
                        div()
                            .font_family(cx.theme().mono_font_family.clone())
                            .text_size(px(11.))
                            .text_color(cx.theme().muted_foreground)
                            .child(detail),
                    )
            })
            .when(!self.selected.is_empty() && !self.converting, |block| {
                block.child(
                    div()
                        .flex()
                        .items_center()
                        .justify_between()
                        .child(
                            div()
                                .text_size(px(11.))
                                .text_color(cx.theme().muted_foreground)
                                .child(format!("{} selected", self.selected.len())),
                        )
                        .child(
                            Button::new("select-none")
                                .ghost()
                                .small()
                                .label("Clear")
                                .on_click(cx.listener(|audit, _, _, cx| {
                                    audit.selected.clear();
                                    audit.selection_changed(cx);
                                })),
                        ),
                )
            })
            .when(
                (!self.results.is_empty()
                    || !self.completed_outputs.is_empty()
                    || self.existing_output > 0)
                    && !self.converting,
                |block| {
                    block.child(
                        Button::new("reveal")
                            .outline()
                            .small()
                            .w_full()
                            .icon(IconName::FolderOpen)
                            .label("Show output")
                            .on_click(cx.listener(|audit, _, _, _| audit.reveal_output())),
                    )
                },
            )
            .child(
                Button::new("convert")
                    .primary()
                    .w_full()
                    .when(self.converting || target_count == 0, |button| {
                        button.ghost()
                    })
                    .label(self.conversion_action_label())
                    .disabled(self.converting || self.scanning.is_some() || target_count == 0)
                    .on_click(cx.listener(|audit, _, _, cx| audit.start_conversion(cx))),
            )
    }
}
