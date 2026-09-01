//! The header line above the table: counts, filter box, view switch.

use super::*;

impl Audit {
    /// The counts beside the breadcrumb. A string rather than inline elements: what
    /// a scan left out is as much a fact as what it found, and both are asserted on.
    pub(super) fn stats_line(&self, count: usize) -> String {
        let mut stats = if self.sirv_scope == Some(SirvScope::OnlyRemote) {
            format!("{count} files only on Sirv")
        } else if let Some(folders) = self.batch_folders {
            format!(
                "{folders} folders · {count} images · {}",
                format_bytes(self.visible_bytes())
            )
        } else if self.batch_size.is_some() && count == self.entries.len() {
            format_bytes(self.visible_bytes())
        } else if count == self.entries.len() {
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
        // Named as unsupported rather than merely skipped: raw is a deliberate
        // exclusion, HEIC is a gap, and the difference is the user's next move.
        if self.skipped_heic > 0 {
            stats.push_str(&format!(
                " · {} HEIC skipped (not supported yet)",
                self.skipped_heic
            ));
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
        stats
    }

    /// Which folder this is, how to get to another one, and the two controls
    /// that narrow the list. One row: the second strip was carrying a filter box
    /// and two chips across the whole window, and cost the list forty pixels.
    pub(super) fn header(
        &self,
        count: usize,
        window: &Window,
        cx: &mut Context<Self>,
    ) -> impl IntoElement {
        let source_icon = if self.batch_folders.is_some() {
            IconName::Folder
        } else if self.batch_size.is_some() {
            IconName::File
        } else {
            IconName::Folder
        };
        let tree_collapsed = self.batch_size.is_none() && !self.browser_persistent(window);
        let breadcrumb_source = cx.entity().downgrade();
        let mut breadcrumb_parts = self.breadcrumb_parts();
        let width = f32::from(window.viewport_size().width);
        let breadcrumb_limit = if width < 820. {
            1
        } else if width < browser::SIDEBAR_MIN_WINDOW_WIDTH {
            2
        } else {
            4
        };
        if breadcrumb_parts.len() > breadcrumb_limit {
            breadcrumb_parts.drain(..breadcrumb_parts.len() - breadcrumb_limit);
        }
        let last_breadcrumb = breadcrumb_parts.len().saturating_sub(1);
        let breadcrumbs = Breadcrumb::new().children(breadcrumb_parts.into_iter().enumerate().map(
            |(index, (label, path))| {
                let disabled = index == last_breadcrumb || self.converting;
                let mut item = BreadcrumbItem::new(label).disabled(disabled);
                if !disabled {
                    let source = breadcrumb_source.clone();
                    item = item.on_click(move |_, _, cx| {
                        if let Some(audit) = source.upgrade() {
                            let path = path.clone();
                            audit.update(cx, |audit, cx| audit.request_path(path, cx));
                        }
                    });
                }
                item
            },
        ));
        let source_menu = cx.entity().downgrade();
        let reveal_source = source_menu.clone();
        let reveal_root = self.root.clone();
        let can_reveal = reveal_root.is_dir();

        let stats = self.stats_line(count);
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
            // One named source control replaces three adjacent icon-only openers.
            // Counts remain metadata beside it, on the same line.
            .child(
                div()
                    .flex()
                    .items_center()
                    .gap_2()
                    .flex_1()
                    .min_w_0()
                    .min_w(px(260.))
                    .children(tree_collapsed.then(|| {
                        div()
                            .debug_selector(|| "folder-tree-toggle".into())
                            .child(
                                Button::new("folder-tree-toggle")
                                    .small()
                                    .ghost()
                                    .icon(IconName::PanelLeft)
                                    .selected(self.browser_overlay)
                                    .tooltip("Browse folders")
                                    .disabled(self.converting)
                                    .on_click(cx.listener(|audit, _, window, cx| {
                                        audit.toggle_browser(window, cx)
                                    })),
                            )
                    }))
                    .child(
                        div()
                            .min_w_0()
                            .overflow_hidden()
                            .child(breadcrumbs),
                    )
                    .child(
                        Button::new("source-picker")
                            .small()
                            .ghost()
                            .icon(source_icon)
                            .tooltip("Open a folder or choose images")
                            .dropdown_caret(true)
                            .disabled(self.converting)
                            .dropdown_menu(move |menu, _, _| {
                                let open_folder = source_menu.clone();
                                let open_images = source_menu.clone();
                                let reveal_root = reveal_root.clone();
                                let reveal_source = reveal_source.clone();
                                menu.item(
                                    PopupMenuItem::new("Open folder…")
                                        .icon(IconName::Folder)
                                        .on_click(move |_, _, cx| {
                                            if let Some(audit) = open_folder.upgrade() {
                                                audit.update(cx, |audit, cx| audit.pick(true, cx));
                                            }
                                        }),
                                )
                                .item(
                                    PopupMenuItem::new("Open images…")
                                        .icon(IconName::File)
                                        .on_click(move |_, _, cx| {
                                            if let Some(audit) = open_images.upgrade() {
                                                audit.update(cx, |audit, cx| audit.pick(false, cx));
                                            }
                                        }),
                                )
                                .separator()
                                .item(
                                    PopupMenuItem::new("Reveal in file manager")
                                        .icon(IconName::FolderOpen)
                                        .disabled(!can_reveal)
                                        .on_click(move |_, _, cx| {
                                            if let Some(audit) = reveal_source.upgrade() {
                                                let path = reveal_root.clone();
                                                audit.update(cx, |audit, cx| {
                                                    audit.reveal_path(
                                                        &path,
                                                        "Couldn’t show source folder",
                                                        cx,
                                                    );
                                                });
                                            }
                                        }),
                                )
                            }),
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
                            .child(stats),
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
                        .tooltip("Copy a shareable Press audit with Sirv and AI next steps")
                        .disabled(self.converting || self.scan_blocks_delivery())
                        .on_click(cx.listener(|audit, _, _, cx| audit.copy_audit_report(cx))),
                )
            })
            // Sirv is a separate remote pairing, not another kind of local source.
            // Once paired, the reconciliation strip below owns its status and actions.
            .when(self.sirv_pairing.is_none(), |header| {
                header.child(
                    Button::new("sirv-browser")
                        .small()
                        .ghost()
                        .icon(IconName::Globe)
                        .label("Pair with Sirv")
                        .disabled(
                            self.converting
                                || self.batch_folders.is_some()
                                || self.scan_blocks_delivery(),
                        )
                        .on_click(
                            cx.listener(|audit, _, _, cx| audit.open_sirv_browser(cx)),
                        ),
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
                        audit.marquee = None;
                        audit.selection_bounds.borrow_mut().clear();
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
