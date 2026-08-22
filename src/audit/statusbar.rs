//! The status bar: summary counts, notices row, finding buttons.

use super::*;

impl Audit {
    /// The payoff, said once and out loud: what the folder costs now, what it would
    /// cost converted, and the button that does it. This used to be 11px of grey
    /// wedged between the button and the window edge — the wrong volume for the only
    /// number the app exists to produce.
    pub(super) fn summary(&self, cx: &mut Context<Self>) -> impl IntoElement {
        let target_count = self.target_count();
        // Source bytes only appear before a conversion. While results stream in,
        // avoid walking thousands of rows on every progress redraw.
        let source = if !self.converting && self.results.is_empty() {
            self.target_bytes()
        } else {
            0
        };

        // Four states, one shape: a headline, the share it leaves behind, and a
        // sentence of detail.
        let (headline, tone, detail, bar, tag) = if self.converting {
            let done = self.results.len() + self.failures.len();
            let total = self.active_target_count.unwrap_or(target_count);
            (
                format!("{done} of {total}"),
                cx.theme().foreground,
                format!(
                    "Converting to {} {}…",
                    self.format.label().to_uppercase(),
                    self.quality.label()
                ),
                Some((done as f32 / total.max(1) as f32, cx.theme().primary)),
                None,
            )
        } else if !self.results.is_empty() {
            let (before, after) = self.converted_totals();
            let growth = after > before;
            let delta = before.abs_diff(after);
            let percent = delta as f32 / before.max(1) as f32 * 100.;
            (
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
                    "{} converted · {} → {}",
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
                // A projection from a few dozen encodes, said as one. Unqualified it
                // read as a measurement, and the reader had no way to tell it from the
                // completed total above, which is one.
                format!(
                    "≈{} to {}",
                    format_bytes(delta),
                    if growth { "grow" } else { "save" }
                ),
                if growth {
                    cx.theme().yellow
                } else {
                    cx.theme().green
                },
                format!(
                    "{} now → ≈{} as {} {} · sampled {sampled}",
                    format_bytes(source),
                    format_bytes(projected),
                    self.format.label().to_uppercase(),
                    self.quality.label()
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
                "Sizing it up…".to_string(),
                cx.theme().muted_foreground,
                format!("{} on disk", format_bytes(source)),
                None,
                None,
            )
        };

        // A status bar: fixed at the bottom, one height in every state, so the
        // list above it never jumps when the numbers arrive.
        let (fraction, colour) = bar
            .map(|(remaining, colour)| (1. - remaining, colour))
            .unwrap_or((0., gpui::transparent_black()));

        div()
            .flex()
            .flex_col()
            .px_3()
            .pt_1()
            .pb_2()
            // The one strip allowed colour: washed in the tone of the headline,
            // so the state of the job reads before any word does.
            .bg(tone.opacity(0.08))
            .border_t_1()
            .border_color(cx.theme().border)
            .child(meter("saving", fraction, colour, 3.))
            .child(
                div()
                    .flex()
                    .flex_wrap()
                    .items_center()
                    .gap_2()
                    .child(
                        div()
                            .font_family("SF Pro Display")
                            .text_size(px(18.))
                            .font_weight(FontWeight::SEMIBOLD)
                            .text_color(tone)
                            .whitespace_nowrap()
                            .flex_shrink_0()
                            .child(headline),
                    )
                    // The share saved, which is the number people actually quote.
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
                    }))
                    .child(
                        div()
                            .flex_1()
                            .min_w_0()
                            .font_family(cx.theme().mono_font_family.clone())
                            .text_size(px(11.))
                            .text_color(cx.theme().muted_foreground)
                            .whitespace_nowrap()
                            .overflow_hidden()
                            .text_ellipsis()
                            .child(detail),
                    )
                    .when(!self.selected.is_empty() && !self.converting, |row| {
                        row.child(
                            Button::new("select-none")
                                .ghost()
                                .small()
                                .label(format!("Clear {}", self.selected.len()))
                                .on_click(cx.listener(|audit, _, _, cx| {
                                    audit.selected.clear();
                                    audit.schedule_estimate(cx);
                                    cx.notify();
                                })),
                        )
                    })
                    .when(!self.results.is_empty() && !self.converting, |row| {
                        row.child(
                            Button::new("reveal")
                                .outline()
                                .small()
                                .icon(IconName::FolderOpen)
                                .label("Show output")
                                .on_click(cx.listener(|audit, _, _, _| audit.reveal_output())),
                        )
                    })
                    .child(
                        Button::new("convert")
                            .primary()
                            .when(self.converting || target_count == 0, |button| {
                                button.ghost()
                            })
                            .label(if self.converting {
                                "Converting…".to_string()
                            } else if self.selected.is_empty() {
                                format!("Convert all to {}", self.format.label().to_uppercase())
                            } else {
                                format!(
                                    "Convert {} to {}",
                                    target_count,
                                    self.format.label().to_uppercase()
                                )
                            })
                            .disabled(
                                self.converting || self.scanning.is_some() || target_count == 0,
                            )
                            .on_click(cx.listener(|audit, _, _, cx| audit.start_conversion(cx))),
                    ),
            )
    }

    /// Everything the scan could not take at face value, in one line rather than
    /// three scattered ones. The mislabelled count is a button: it is the audit's best
    /// finding, and a number you cannot act on is a dead end.
    pub(super) fn notices(&self, cx: &mut Context<Self>) -> Option<gpui::AnyElement> {
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
        if self.existing_output > 0 {
            parts.push(match self.existing_output {
                1 => format!("{}/ already holds 1 file", scan::OUTPUT_DIR),
                many => format!("{}/ already holds {many} files", scan::OUTPUT_DIR),
            });
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
        if let Some(job) = &self.sirv_job {
            let verb = match job.kind {
                SirvJobKind::Pull => "Sirv pull",
                SirvJobKind::PullChanged => "Sirv pull (overwrite)",
                SirvJobKind::Push => "Sirv push",
                SirvJobKind::PushChanged => "Sirv push (overwrite)",
            };
            let failures = if job.failures.is_empty() {
                String::new()
            } else {
                format!(
                    ", {} failed: {}",
                    job.failures.len(),
                    job.failures
                        .iter()
                        .take(3)
                        .cloned()
                        .collect::<Vec<_>>()
                        .join(", ")
                )
            };
            parts.push(format!("{verb}: {} of {}{failures}", job.done, job.total));
        }
        if parts.is_empty() && self.mislabelled == 0 {
            return None;
        }

        // Left-aligned and only as wide as its text. A full-bleed box for six words
        // was a bigger shape on screen than the finding it was reporting.
        Some(
            div()
                .flex()
                .flex_wrap()
                .items_center()
                .gap_2()
                .children((self.mislabelled > 0).then(|| {
                    self.finding_button(
                        Finding::Mislabelled,
                        IconName::TriangleAlert,
                        match self.mislabelled {
                            1 => "1 file is not the format its extension claims".to_string(),
                            many => {
                                format!("{many} files are not the format their extension claims")
                            }
                        },
                        cx,
                    )
                }))
                .children((!parts.is_empty()).then(|| {
                    Alert::warning("notices", parts.join("  ·  "))
                        .icon(IconName::TriangleAlert)
                        .py_1()
                }))
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
        cx: &mut Context<Self>,
    ) -> impl IntoElement {
        let active = self.finding == Some(finding);
        Button::new(("finding", finding as usize))
            .small()
            .icon(icon)
            .label(label)
            .selected(active)
            .when(!active, |button| button.ghost())
            .when(active, |button| button.warning())
            .on_click(cx.listener(move |audit, _, _, cx| audit.set_finding(finding, cx)))
    }
}
