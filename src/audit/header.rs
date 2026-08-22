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
                            .child(
                                Button::new("open-folder")
                                    .small()
                                    .ghost()
                                    .icon(IconName::Folder)
                                    .label("Open folder…")
                                    .disabled(self.converting)
                                    .on_click(cx.listener(|audit, _, _, cx| audit.pick(true, cx))),
                            )
                            .child(
                                Button::new("open-file")
                                    .small()
                                    .ghost()
                                    .icon(IconName::File)
                                    .label("Open image…")
                                    .disabled(self.converting)
                                    .on_click(cx.listener(|audit, _, _, cx| audit.pick(false, cx))),
                            )
                            .child(
                                // The sync entry point: opens the remote-folder
                                // browser, which is also where a pairing is undone.
                                Button::new("sirv-browser")
                                    .small()
                                    .ghost()
                                    .icon(IconName::Globe)
                                    .label(match &self.sirv_pairing {
                                        Some(pairing) => pairing.dir.clone(),
                                        None => "Sirv…".into(),
                                    })
                                    .disabled(self.converting)
                                    .on_click(
                                        cx.listener(|audit, _, _, cx| audit.open_sirv_browser(cx)),
                                    ),
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

    /// The three knobs that decide what a conversion produces, each under its own
    /// name and drawn as one control rather than a run of loose buttons.
    pub(super) fn controls(&self, window: &Window, cx: &mut Context<Self>) -> impl IntoElement {
        let lossless = self.quality == Quality::LOSSLESS;
        let heavy = self
            .entries
            .iter()
            .filter(|entry| Finding::Heavy.holds(entry))
            .count();
        // Below this width the audit controls and conversion settings are two
        // intentional bands. Letting one long flex row break wherever it ran out of
        // room left Quality orphaned on a line of its own at the default window size.
        let stacked = f32::from(window.viewport_size().width) < 1060.;

        div()
            .flex()
            .flex_wrap()
            .when(stacked, |strip| strip.flex_col().items_start())
            .when(!stacked, |strip| strip.items_center())
            .gap_2()
            .px_3()
            .py_2()
            .border_b_1()
            .border_color(cx.theme().border)
            // What the list shows, then what a conversion would do: the reading
            // order of the strip follows the order you use it.
            .child(
                div()
                    .flex()
                    .items_center()
                    .gap_2()
                    .flex_shrink_0()
                    .child(
                        div().w(px(150.)).flex_shrink_0().child(
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
                            cx,
                        )
                    })),
            )
            .child(
                div()
                    .flex()
                    .flex_wrap()
                    .items_center()
                    .gap_3()
                    .child(self.control_group("Resize", self.resize_group(cx), cx))
                    .child(self.control_group("Format", self.format_group(cx), cx))
                    .child(
                        div()
                            .flex()
                            .items_center()
                            .gap_2()
                            .flex_shrink_0()
                            .child(
                                div()
                                    .text_size(px(12.))
                                    .text_color(cx.theme().muted_foreground)
                                    .whitespace_nowrap()
                                    .child("Quality"),
                            )
                            .child(
                                div()
                                    .w(px(110.))
                                    .debug_selector(|| "quality-control".to_string())
                                    .when(self.converting, |rail| {
                                        rail.child(
                                            Progress::new("quality-locked")
                                                .value(self.quality.0.unwrap_or(100.))
                                                .color(cx.theme().primary)
                                                .h(px(6.)),
                                        )
                                    })
                                    .when(!self.converting, |slider| {
                                        slider.child(Slider::new(&self.quality_slider).horizontal())
                                    }),
                            )
                            .child(
                                div()
                                    .w(px(26.))
                                    .font_family(cx.theme().mono_font_family.clone())
                                    .text_size(px(12.))
                                    .whitespace_nowrap()
                                    .text_color(if lossless {
                                        cx.theme().muted_foreground
                                    } else {
                                        cx.theme().foreground
                                    })
                                    .child(match self.quality.0 {
                                        Some(value) => format!("{}", value.round() as u32),
                                        None => "—".to_string(),
                                    }),
                            )
                            .child(
                                Switch::new("lossless")
                                    .checked(lossless)
                                    .label("Lossless")
                                    .disabled(self.converting)
                                    .on_click(cx.listener(|audit, _, _, cx| {
                                        if audit.converting {
                                            return;
                                        }
                                        // A second click on a lit toggle has to turn it off,
                                        // or lossless is a one-way door.
                                        audit.quality = if audit.quality == Quality::LOSSLESS {
                                            Quality::lossy(audit.slider_quality)
                                        } else {
                                            Quality::LOSSLESS
                                        };
                                        audit.results.clear();
                                        audit.schedule_estimate(cx);
                                        cx.notify();
                                    })),
                            ),
                    ),
            )
    }
}
