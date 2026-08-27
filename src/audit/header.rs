//! The header line above the table: counts, filter box, view switch.

use super::*;

impl Audit {
    /// Which folder this is, how to get to another one, and the two controls
    /// that narrow the list. One row: the second strip was carrying a filter box
    /// and two chips across the whole window, and cost the list forty pixels.
    pub(super) fn header(&self, count: usize, cx: &mut Context<Self>) -> impl IntoElement {
        let folder = self
            .root
            .file_name()
            .map(|name| name.to_string_lossy().into_owned())
            .unwrap_or_else(|| self.root.display().to_string());

        let mut stats = if count == self.entries.len() {
            format!("{count} images · {}", format_bytes(self.visible_bytes()))
        } else {
            format!(
                "{count} of {} images · {}",
                self.entries.len(),
                format_bytes(self.visible_bytes())
            )
        };
        if self.skipped_raw > 0 {
            stats.push_str(&format!(" · {} camera raw skipped", self.skipped_raw));
        }
        if self.skipped_packages > 0 {
            stats.push_str(&match self.skipped_packages {
                1 => " · 1 macOS package skipped".to_string(),
                many => format!(" · {many} macOS packages skipped"),
            });
        }
        // Information, not a warning: a previous run's output sitting in
        // optimized/ is normal life, and a yellow banner made it look like
        // something had gone wrong.
        if self.existing_output > 0 {
            stats.push_str(&match self.existing_output {
                1 => format!(" · 1 file in {}/", scan::OUTPUT_DIR),
                many => format!(" · {many} files in {}/", scan::OUTPUT_DIR),
            });
        }
        // Three states, three sentences. A pairing whose walk is still running used to
        // read exactly like one whose walk failed: no Sirv text at all.
        match (&self.sirv_pairing, self.sirv_counts) {
            (Some(_), Some((to_push, changed, to_pull))) => stats.push_str(&format!(
                " · Sirv: {to_push} to push · {changed} differ · {to_pull} to pull"
            )),
            (Some(pairing), None) => stats.push_str(match pairing.files {
                Listing::Walking => " · Sirv: listing…",
                Listing::Failed(_) => " · Sirv: listing failed",
                Listing::Ready(_) => "",
            }),
            (None, _) => {}
        }

        div()
            .debug_selector(|| "audit-header".into())
            .flex()
            .flex_wrap()
            .items_center()
            .gap_2()
            .px_3()
            .py_1p5()
            .bg(cx.theme().table_head)
            .border_b_1()
            .border_color(cx.theme().border)
            // Identity and its openers hold the left; the path and the counts
            // run beside them as metadata, on the same line.
            .child(
                div()
                    .flex()
                    .items_center()
                    .gap_2()
                    .flex_1()
                    .min_w_0()
                    .min_w(px(260.))
                    .child(
                        div()
                            .flex()
                            .items_center()
                            .gap_1()
                            .flex_shrink_0()
                            .child(
                                div()
                                    .font_family("SF Pro Display")
                                    .text_size(px(14.))
                                    .font_weight(FontWeight::SEMIBOLD)
                                    .text_color(cx.theme().foreground)
                                    .whitespace_nowrap()
                                    .overflow_hidden()
                                    .text_ellipsis()
                                    .child(folder),
                            )
                            .child(
                                Button::new("open-folder")
                                    .small()
                                    .ghost()
                                    .icon(IconName::Folder)
                                    .tooltip("Open a folder")
                                    .disabled(self.converting)
                                    .on_click(cx.listener(|audit, _, _, cx| audit.pick(true, cx))),
                            )
                            .child(
                                Button::new("open-file")
                                    .small()
                                    .ghost()
                                    .icon(IconName::File)
                                    .tooltip("Open a single image")
                                    .disabled(self.converting)
                                    .on_click(cx.listener(|audit, _, _, cx| audit.pick(false, cx))),
                            )
                            .child(
                                // The sync entry point: opens the remote-folder
                                // browser, which is also where a pairing is undone.
                                {
                                    // A live pairing keeps its name on the
                                    // button: the name IS the state.
                                    let button = Button::new("sirv-browser")
                                        .small()
                                        .ghost()
                                        .icon(IconName::Globe)
                                        .tooltip("Sync with a Sirv folder")
                                        .disabled(self.converting)
                                        .on_click(cx.listener(|audit, _, _, cx| {
                                            audit.open_sirv_browser(cx)
                                        }));
                                    match &self.sirv_pairing {
                                        Some(pairing) => button.label(pairing.dir.clone()),
                                        None => button,
                                    }
                                },
                            ),
                    )
                    .child(
                        div()
                            .font_family(cx.theme().mono_font_family.clone())
                            .text_size(px(11.))
                            .text_color(cx.theme().muted_foreground)
                            .whitespace_nowrap()
                            .overflow_hidden()
                            .text_ellipsis()
                            .min_w_0()
                            .child(format!("{} · {stats}", self.root.display())),
                    ),
            )
            // The two controls that narrow the list, in the same row as the
            // counts they narrow.
            .child(
                div()
                    .w(px(190.))
                    .flex()
                    .items_center()
                    .gap_1()
                    .flex_shrink_0()
                    .child(
                        div().flex_1().min_w_0().child(
                            Input::new(&self.filter_input)
                                .small()
                                .disabled(self.converting)
                                .prefix(IconName::Search),
                        ),
                    )
                    .when(!self.filter.is_empty(), |row| {
                        row.child(
                            div().debug_selector(|| "clear-filter".into()).child(
                                Button::new("clear-filter")
                                    .small()
                                    .ghost()
                                    .label("Clear")
                                    .disabled(self.converting)
                                    .on_click(cx.listener(|audit, _, window, cx| {
                                        let input = audit.filter_input.clone();
                                        input.update(cx, |input, cx| {
                                            input.set_value("", window, cx)
                                        });
                                        audit.set_filter(String::new(), cx);
                                        window.focus(&input.read(cx).focus_handle(cx), cx);
                                    })),
                            ),
                        )
                    }),
            )
            // The audit reads bytes per pixel for every row and then asks you to
            // find the heavy ones yourself. These are that answer, as the control
            // that narrows the list to them.
            .children((self.heavy > 0).then(|| {
                self.finding_button(
                    Finding::Heavy,
                    IconName::TriangleAlert,
                    format!("{} heavy", self.heavy),
                    "Files carrying more bytes per pixel than a photograph \
                     needs. Click to show only them.",
                    cx,
                )
            }))
            .children((self.mislabelled > 0).then(|| {
                self.finding_button(
                    Finding::Mislabelled,
                    IconName::TriangleAlert,
                    format!("{} mislabelled", self.mislabelled),
                    "Files whose bytes are not the format their extension \
                     claims. Click to show only them.",
                    cx,
                )
            }))
            .children((acquisition::SHOW_ACQUISITION_EXTRAS && self.marketplace > 0).then(|| {
                self.finding_button(
                    Finding::Marketplace,
                    IconName::TriangleAlert,
                    format!("{} preflight", self.marketplace),
                    "Marketplace file preflight: 1400×1400, at most 250 KB, and a truthful extension. Review the background visually.",
                    cx,
                )
            }))
            .when(acquisition::SHOW_ACQUISITION_EXTRAS, |header| {
                header.child(
                    Button::new("copy-audit-report")
                        .small()
                        .ghost()
                        .icon(if self.report_copied {
                            IconName::Check
                        } else {
                            IconName::Copy
                        })
                        .label(if self.report_copied {
                            "Copied"
                        } else {
                            "Copy audit"
                        })
                        .tooltip("Copy a shareable Press audit with Sirv and Studio next steps")
                        .disabled(self.converting)
                        .on_click(cx.listener(|audit, _, _, cx| audit.copy_audit_report(cx))),
                )
            })
            // The view toggle sits at the far end of the window: it changes how
            // the list below is drawn and nothing else.
            .child(
                self.toolbar_button(
                    "view-grid",
                    if self.grid { "List" } else { "Grid" },
                    if self.grid {
                        "Show the audit as a list"
                    } else {
                        "Show the images as a gallery"
                    },
                    if self.grid {
                        IconName::Menu
                    } else {
                        IconName::LayoutDashboard
                    },
                    cx,
                    |audit, cx| {
                        audit.grid = !audit.grid;
                        // The list needs 96 px thumbs; the gallery needs 224 px.
                        // Keep one cache and refill it on this rare mode switch.
                        audit.thumbs.clear();
                        audit.requested.clear();
                        audit.thumb_queue.clear();
                        audit.thumb_order.clear();
                        cx.notify();
                    },
                )
                .disabled(self.converting),
            )
            .child(
                // Icon-only: the one global surface, always in the same corner.
                Button::new("open-settings")
                    .small()
                    .ghost()
                    .icon(IconName::Settings)
                    .tooltip("Settings")
                    .on_click(cx.listener(|audit, _, window, cx| audit.open_settings(window, cx))),
            )
    }
}
