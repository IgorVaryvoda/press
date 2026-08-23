//! The header line above the table: counts, filter box, view switch.

use super::*;

impl Audit {
    /// Which folder this is, and how to get to another one.
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
            .flex()
            .flex_wrap()
            .items_center()
            .gap_2()
            .px_3()
            .py_2()
            .border_b_1()
            .border_color(cx.theme().border)
            // Identity and its two actions share the top-left corner: the name,
            // then the openers that replace it. The path and the count sit
            // underneath as metadata.
            .child(
                div()
                    .flex()
                    .flex_col()
                    .flex_1()
                    .min_w_0()
                    .min_w(px(220.))
                    .child(
                        div()
                            .flex()
                            .items_center()
                            .gap_2()
                            .min_w_0()
                            .child(
                                div()
                                    .font_family("SF Pro Display")
                                    .text_size(px(15.))
                                    .font_weight(FontWeight::SEMIBOLD)
                                    .text_color(cx.theme().foreground)
                                    .whitespace_nowrap()
                                    .overflow_hidden()
                                    .text_ellipsis()
                                    .child(folder),
                            )
                            // Icon-only: three ellipsised text buttons crowding the
                            // title read as clutter, and the tooltips say the same
                            // words on demand.
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
                                // A live pairing keeps its name on the button; the
                                // name IS the state.
                                {
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
                            .flex()
                            .items_baseline()
                            .gap_2()
                            .min_w_0()
                            .child(
                                div()
                                    .text_size(px(11.))
                                    .text_color(cx.theme().muted_foreground)
                                    .whitespace_nowrap()
                                    .overflow_hidden()
                                    .text_ellipsis()
                                    .child(self.root.display().to_string()),
                            )
                            .child(
                                div()
                                    .font_family(cx.theme().mono_font_family.clone())
                                    .text_size(px(11.))
                                    .text_color(cx.theme().muted_foreground)
                                    .whitespace_nowrap()
                                    .flex_shrink_0()
                                    .child(stats),
                            ),
                    ),
            )
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

    /// The audit's working row: the filter and the findings you can act on.
    /// Conversion settings live in the output panel on the right, next to the
    /// estimate and the button they drive.
    pub(super) fn controls(&self, cx: &mut Context<Self>) -> impl IntoElement {
        let heavy = self
            .entries
            .iter()
            .filter(|entry| Finding::Heavy.holds(entry))
            .count();

        div()
            .flex()
            .flex_wrap()
            .items_center()
            .gap_2()
            .px_3()
            .py_2()
            .border_b_1()
            .border_color(cx.theme().border)
            .child(
                div().w(px(220.)).flex_shrink_0().child(
                    Input::new(&self.filter_input)
                        .small()
                        .cleanable(true)
                        .disabled(self.converting)
                        .prefix(IconName::Search),
                ),
            )
            // The audit colours every row by weight per pixel and then asks
            // you to find the heavy ones yourself.
            .children((heavy > 0).then(|| {
                self.finding_button(
                    Finding::Heavy,
                    IconName::TriangleAlert,
                    format!("{heavy} heavy"),
                    "Files carrying more bytes per pixel than a photograph \
                     needs. Click to show only them.",
                    cx,
                )
            }))
            // Same chip family as heavy: a finding you can act on, not a
            // banner shouting a sentence across the window.
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
    }
}
