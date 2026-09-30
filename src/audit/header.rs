//! The header line above the table: source, filter box, view switch.

use super::*;

/// A crumb whose parent is the filesystem root, which already draws as `/`.
fn crumbs_after_root(path: &Path) -> bool {
    path.parent()
        .is_some_and(|parent| parent.parent().is_none())
}

impl Audit {
    /// What a scan left out. The status bar owns the folder totals, so this
    /// renders there, muted after the counts; empty when nothing was skipped,
    /// so the common case stays one short line.
    pub(super) fn warning_stats(&self) -> String {
        let mut warnings = String::new();
        if self.skipped_raw > 0 {
            warnings.push_str(&format!(" · {} camera raw skipped", self.skipped_raw));
        }
        // Named as unsupported rather than merely skipped: raw is a deliberate
        // exclusion, HEIC is a gap, and the difference is the user's next move.
        if self.skipped_heic > 0 {
            warnings.push_str(&format!(
                " · {} HEIC skipped (not supported yet)",
                self.skipped_heic
            ));
        }
        if self.skipped_packages > 0 {
            warnings.push_str(&match self.skipped_packages {
                1 => " · 1 macOS package skipped".to_string(),
                many => format!(" · {many} macOS packages skipped"),
            });
        }
        // Information, not a warning: a previous run's output sitting in
        // optimized/ is normal life, and a yellow banner made it look like
        // something had gone wrong. Named only while it is the destination —
        // the count is taken at scan time, and after a switch to replace mode
        // or a chosen folder it described somewhere the next run will not write.
        if self.existing_output > 0 && self.output == Output::Optimized {
            // "2 files in optimized/" read as two files converted this time.
            warnings.push_str(&match self.existing_output {
                1 => format!(" · {}/ already holds 1 file", scan::OUTPUT_DIR),
                many => format!(" · {}/ already holds {many} files", scan::OUTPUT_DIR),
            });
        }
        warnings
    }

    /// Findings available in the status-bar filter menu.
    pub(super) fn available_findings(&self) -> Vec<(Finding, IconName, String)> {
        let mut findings = Vec::new();
        if self.heavy > 0 {
            findings.push((
                Finding::Heavy,
                IconName::TriangleAlert,
                format!("{} heavy", self.heavy),
            ));
        }
        if !self.failures.is_empty() {
            findings.push((
                Finding::Failed,
                IconName::CircleX,
                format!("{} failed", self.failures.len()),
            ));
        }
        if self.mislabelled > 0 {
            findings.push((
                Finding::Mislabelled,
                IconName::TriangleAlert,
                format!("{} mislabelled", self.mislabelled),
            ));
        }
        findings
    }

    /// Keyboard shortcuts for the image list.
    pub(super) fn shortcuts_view(&self, cx: &mut Context<Self>) -> gpui_kit::AnyElement {
        const SHORTCUTS: [(&str, &str); 12] = [
            ("↑ ↓ ← →", "Move between images"),
            ("PgUp PgDn Home End", "Jump through the list"),
            ("Shift + move", "Extend the selection"),
            ("Space", "Tick the row"),
            ("Enter", "Preview image"),
            ("Alt + ← →", "Go back or forward a folder"),
            ("Ctrl/⌘ + A", "Select everything shown"),
            ("Ctrl/⌘ + K", "Focus the filter box"),
            ("Ctrl/⌘ + Enter", "Open the Convert panel"),
            ("Ctrl/⌘ + ,", "Open the Sirv account"),
            ("?", "Show this list"),
            ("Esc", "Close dialogs, then clear the selection"),
        ];
        div()
            .debug_selector(|| "shortcuts-card".into())
            .w(px(400.))
            .flex()
            .flex_col()
            .gap_2()
            .p_4()
            .rounded_lg()
            .bg(cx.theme().secondary)
            .border_1()
            .border_color(cx.theme().border)
            .child(
                div()
                    .font_family("SF Pro Display")
                    .text_size(px(15.))
                    .font_weight(FontWeight::SEMIBOLD)
                    .text_color(cx.theme().foreground)
                    .child("Keyboard shortcuts"),
            )
            .children(SHORTCUTS.iter().map(|(keys, what)| {
                div()
                    .flex()
                    .items_center()
                    .gap_3()
                    .child(
                        div()
                            .w(px(150.))
                            .flex_shrink_0()
                            .font_family(cx.theme().mono_font_family.clone())
                            .text_size(px(11.))
                            .font_weight(FontWeight::SEMIBOLD)
                            .text_color(cx.theme().foreground)
                            .child(*keys),
                    )
                    .child(
                        div()
                            .text_size(px(12.))
                            .text_color(cx.theme().muted_foreground)
                            .child(*what),
                    )
            }))
            .child(
                div()
                    .text_size(px(11.))
                    .text_color(cx.theme().muted_foreground)
                    .child("Press Esc or click outside to close."),
            )
            .into_any_element()
    }

    /// The shortcut list floating over the list, beside the folder overlay. It
    /// lives inside the workspace tree so focus never leaves the list and
    /// Escape reaches the handler that closes it.
    pub(super) fn shortcuts_overlay(&self, cx: &mut Context<Self>) -> gpui_kit::AnyElement {
        div()
            .id("shortcuts-overlay")
            .occlude()
            .absolute()
            .inset_0()
            .flex()
            .items_center()
            .justify_center()
            .child(
                div()
                    .absolute()
                    .inset_0()
                    // The same scrim the settings panel uses. At 0.55 this one
                    // left the table legible under the card, so two dialogs in
                    // one app dimmed the window by two different amounts.
                    .bg(cx.theme().background.opacity(0.82))
                    .debug_selector(|| "shortcuts-backdrop".into())
                    .on_mouse_down(
                        gpui_kit::MouseButton::Left,
                        cx.listener(|audit, _, window, cx| {
                            audit.shortcuts_open = false;
                            window.focus(&audit.focus, cx);
                            cx.notify();
                            cx.stop_propagation();
                        }),
                    ),
            )
            .child(div().relative().occlude().child(self.shortcuts_view(cx)))
            .into_any_element()
    }

    /// Switching list/gallery refills the thumbnail cache: the list needs 96 px
    /// thumbs, the gallery 224 px. One cache, refilled on this rare switch.
    pub(super) fn set_grid(&mut self, grid: bool, cx: &mut Context<Self>) {
        if grid == self.grid {
            return;
        }
        self.grid = grid;
        self.thumbs.clear();
        self.requested.clear();
        self.thumb_queue.clear();
        self.thumb_order.clear();
        self.marquee = None;
        self.selection_bounds.borrow_mut().clear();
        cx.notify();
    }

    /// Which folder this is, how to get to another one, and the two controls
    /// that narrow the list. One row: the second strip was carrying a filter box
    /// and two chips across the whole window, and cost the list forty pixels.
    pub(super) fn header(&self, window: &Window, cx: &mut Context<Self>) -> impl IntoElement {
        let browser_open = self.browser_persistent(window) || self.browser_overlay;
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
        let home = browser::home_dir();
        // A path bar in the GNOME Files manner: one field the width of the
        // header, the open folder in bold at its end, and every folder above it
        // one click away. `/` between crumbs, because a chevron there read as a
        // disclosure arrow.
        let mut crumbs = Vec::new();
        for (index, (label, path)) in breadcrumb_parts.into_iter().enumerate() {
            // The filesystem root is itself spelled `/`; a separator after it
            // read as `/ /`.
            if index > 0 && !crumbs_after_root(&path) {
                crumbs.push(
                    div()
                        .flex_shrink_0()
                        .text_color(cx.theme().muted_foreground)
                        .child("/")
                        .into_any_element(),
                );
            }
            let current = index == last_breadcrumb;
            let is_home = home.as_deref() == Some(path.as_path());
            let source = breadcrumb_source.clone();
            crumbs.push(
                div()
                    .id(("crumb", index))
                    .debug_selector(move || format!("crumb-{index}"))
                    .flex()
                    .items_center()
                    .gap_1p5()
                    .h(px(24.))
                    .px_2()
                    .rounded(px(5.))
                    .min_w_0()
                    // The open folder keeps its name; ancestors give way first.
                    .when(current, |crumb| {
                        crumb
                            .flex_shrink_0()
                            .font_weight(FontWeight::SEMIBOLD)
                            .text_color(cx.theme().foreground)
                    })
                    .when(!current, |crumb| {
                        crumb.text_color(cx.theme().muted_foreground)
                    })
                    .when(is_home, |crumb| {
                        crumb.child(Icon::default().path("icons/house.svg").size_3p5())
                    })
                    .child(
                        div()
                            .min_w_0()
                            .overflow_hidden()
                            .text_ellipsis()
                            .whitespace_nowrap()
                            .child(label),
                    )
                    .when(!current && !self.converting, |crumb| {
                        crumb
                            .cursor_pointer()
                            .hover(|crumb| {
                                crumb
                                    .bg(cx.theme().secondary_hover)
                                    .text_color(cx.theme().foreground)
                            })
                            .on_click(move |_, _, cx| {
                                if let Some(audit) = source.upgrade() {
                                    let path = path.clone();
                                    audit.update(cx, |audit, cx| audit.request_path(path, cx));
                                }
                            })
                    })
                    .into_any_element(),
            );
        }
        let path_bar = div()
            .debug_selector(|| "path-bar".into())
            .flex()
            .items_center()
            .flex_1()
            .min_w_0()
            .h(px(30.))
            .px_1()
            .gap_0p5()
            .overflow_hidden()
            .rounded_md()
            .bg(cx.theme().secondary)
            .border_1()
            .border_color(cx.theme().border)
            .text_sm()
            .children(crumbs);
        let history = |id: &'static str, forward: bool, cx: &mut Context<Self>| {
            div()
                .flex_shrink_0()
                .debug_selector(move || id.into())
                .child(
                    Button::new(id)
                        .small()
                        .ghost()
                        .icon(if forward {
                            IconName::ChevronRight
                        } else {
                            IconName::ChevronLeft
                        })
                        .tooltip(if forward {
                            "Forward (Alt+Right)"
                        } else {
                            "Back (Alt+Left)"
                        })
                        .disabled(!self.can_step_history(forward))
                        .on_click(
                            cx.listener(move |audit, _, _, cx| audit.step_history(forward, cx)),
                        ),
                )
        };
        let back = history("history-back", false, cx);
        let forward = history("history-forward", true, cx);
        let source_menu = cx.entity().downgrade();
        let reveal_source = source_menu.clone();
        let reveal_root = self.root.clone();
        let can_reveal = reveal_root.is_dir();
        let sirv_disabled =
            self.converting || self.batch_folders.is_some() || self.scan_blocks_delivery();
        // Scope rides with opening: it decides what a folder open covers, the
        // way the command line decides it with a flag.
        let open_disabled = self.converting;
        let scope_checked = self.include_subfolders;
        let scope_disabled = self.converting || self.single_file;
        // Paired, the Sirv bar under the header names both folders, so the
        // button stays one short word and the path keeps its room; the
        // selected state and the tooltip say which folder it is.
        let sirv_tip = match &self.sirv_pairing {
            Some(pairing) => format!("Compare with Sirv:{}", pairing.dir),
            None => "Pair a Sirv folder with this local folder".to_string(),
        };
        // Keep pairing visible beside the source path.
        let sirv = div().debug_selector(|| "sirv-pair-header".into()).child(
            Button::new("sirv-pair-header")
                .small()
                .ghost()
                .icon(IconName::Globe)
                .label("Sirv")
                .tooltip(sirv_tip)
                .selected(self.sirv_pairing.is_some())
                .disabled(sirv_disabled)
                // Paired, the button opens what the pairing is for: the two
                // folders compared. Changing the folder stays in the Sirv bar.
                .on_click(cx.listener(|audit, _, _, cx| {
                    if audit.sirv_pairing.is_some() {
                        audit.toggle_sirv_split(cx);
                    } else {
                        audit.open_sirv_browser(cx);
                    }
                })),
        );
        div()
            .debug_selector(|| "audit-header".into())
            .flex()
            .items_center()
            .gap_3()
            .px_3()
            .py_1p5()
            .overflow_hidden()
            .bg(cx.theme().table_head)
            .border_b_1()
            .border_color(cx.theme().border)
            .child(
                div()
                    .flex_shrink_0()
                    .debug_selector(|| "app-menu".into())
                    .child(
                        Button::new("app-menu")
                            .small()
                            .ghost()
                            .icon(IconName::Menu)
                            .tooltip("Menu")
                            .dropdown_menu(move |menu, _, _| {
                                let open_folder = source_menu.clone();
                                let open_images = source_menu.clone();
                                let toggle_scope = source_menu.clone();
                                let pair_sirv = source_menu.clone();
                                let settings = source_menu.clone();
                                let shortcuts = source_menu.clone();
                                #[cfg(feature = "updater")]
                                let updates = source_menu.clone();
                                let reveal_root = reveal_root.clone();
                                let reveal_source = reveal_source.clone();
                                menu.item(
                                    PopupMenuItem::new("Open folder…")
                                        .icon(IconName::Folder)
                                        .disabled(open_disabled)
                                        .on_click(move |_, _, cx| {
                                            if let Some(audit) = open_folder.upgrade() {
                                                audit.update(cx, |audit, cx| audit.pick(true, cx));
                                            }
                                        }),
                                )
                                .item(
                                    PopupMenuItem::new("Open images…")
                                        .icon(IconName::File)
                                        .disabled(open_disabled)
                                        .on_click(move |_, _, cx| {
                                            if let Some(audit) = open_images.upgrade() {
                                                audit.update(cx, |audit, cx| audit.pick(false, cx));
                                            }
                                        }),
                                )
                                .separator()
                                .item(
                                    PopupMenuItem::new("Include subfolders")
                                        .icon(IconName::Network)
                                        .checked(scope_checked)
                                        .disabled(scope_disabled)
                                        .on_click(move |_, _, cx| {
                                            if let Some(audit) = toggle_scope.upgrade() {
                                                audit.update(cx, |audit, cx| {
                                                    audit.toggle_subfolders(cx)
                                                });
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
                                .separator()
                                .item(
                                    PopupMenuItem::new("Pair with Sirv…")
                                        .icon(IconName::Globe)
                                        .disabled(sirv_disabled)
                                        .on_click(move |_, _, cx| {
                                            if let Some(audit) = pair_sirv.upgrade() {
                                                audit.update(cx, |audit, cx| {
                                                    audit.open_sirv_browser(cx)
                                                });
                                            }
                                        }),
                                )
                                .separator()
                                .map(|menu| {
                                    #[cfg(feature = "updater")]
                                    let menu = menu.separator().item(
                                        PopupMenuItem::new("Check for updates…").on_click(
                                            move |_, _, cx| {
                                                if let Some(audit) = updates.upgrade() {
                                                    audit.update(cx, |audit, cx| {
                                                        audit.check_for_updates(true, cx)
                                                    });
                                                }
                                            },
                                        ),
                                    );
                                    menu
                                })
                                .item(
                                    // Named for what it opens. Called "Settings…"
                                    // it promised the output folder, the format
                                    // and the columns, and delivered two Sirv
                                    // credential fields.
                                    PopupMenuItem::new("Sirv account…")
                                        .icon(IconName::Settings)
                                        .on_click(move |_, window, cx| {
                                            if let Some(audit) = settings.upgrade() {
                                                audit.update(cx, |audit, cx| {
                                                    audit.open_settings(window, cx)
                                                });
                                            }
                                        }),
                                )
                                .item(
                                    PopupMenuItem::new("Keyboard shortcuts").on_click(
                                        move |_, window, cx| {
                                            if let Some(audit) = shortcuts.upgrade() {
                                                audit.update(cx, |audit, cx| {
                                                    audit.shortcuts_open = true;
                                                    window.focus(&audit.focus, cx);
                                                    cx.notify();
                                                });
                                            }
                                        },
                                    ),
                                )
                            }),
                    ),
            )
            .child(
                div()
                    .flex_shrink_0()
                    .debug_selector(|| "folder-tree-toggle".into())
                    .child(
                        Button::new("folder-tree-toggle")
                            .small()
                            .ghost()
                            .icon(if browser_open {
                                Icon::default().path("icons/panel-left-filled.svg")
                            } else {
                                Icon::new(IconName::PanelLeft)
                            })
                            .selected(browser_open)
                            .tooltip(if browser_open {
                                "Hide folder sidebar"
                            } else {
                                "Show folder sidebar"
                            })
                            .disabled(self.converting || self.batch_size.is_some())
                            .on_click(
                                cx.listener(|audit, _, window, cx| {
                                    audit.toggle_browser(window, cx)
                                }),
                            ),
                    ),
            )
            .child(
                div()
                    .flex()
                    .items_center()
                    .flex_shrink_0()
                    .child(back)
                    .child(forward),
            )
            .child(path_bar)
            .child(div().flex_shrink_0().child(sirv))
            // The box narrows the list; erasing its text widens it back out.
            // `cleanable` puts the cross inside the field and only while there
            // is text to clear — the objection to the old Clear button was that
            // it sat dimmed for most of the audit's life, not that the list
            // needed no way out of a filter that matches nothing. Narrower
            // below 900px, where the path needs the room more than the filter.
            .child(
                div()
                    .w(px(if width < 900. { 205. } else { 242. }))
                    .flex()
                    .items_center()
                    .flex_shrink_0()
                    .child(
                        div().flex_1().min_w_0().child(
                            Input::new(&self.filter_input)
                                .small()
                                .cleanable(true)
                                .disabled(self.converting)
                                .prefix(IconName::Search),
                        ),
                    ),
            )
            // One segmented control, List | Grid, like the format group in
            // the panel. The single button wore a burger icon in list view,
            // which read as a menu; the icon set has no list or grid glyph.
            .child(
                ButtonGroup::new("view")
                    .small()
                    .outline()
                    .compact()
                    .children([
                        toolbar::segment("view-list", "List", !self.grid)
                            .debug_selector(|| "view-list".into())
                            .tooltip("Show the audit as a list")
                            .disabled(self.converting),
                        toolbar::segment("view-grid", "Grid", self.grid)
                            .debug_selector(|| "view-grid".into())
                            .tooltip("Show the images as a gallery")
                            .disabled(self.converting),
                    ])
                    .on_click(cx.listener(|audit, clicked: &Vec<usize>, _, cx| {
                        if audit.converting {
                            return;
                        }
                        audit.set_grid(clicked.first() == Some(&1), cx);
                    })),
            )
            .child(
                div()
                    .flex_shrink_0()
                    .debug_selector(|| "operations-sidebar-toggle".into())
                    .child(
                        Button::new("operations-sidebar-toggle")
                            .small()
                            .ghost()
                            .icon(if self.sidebar_open {
                                Icon::default().path("icons/panel-right-filled.svg")
                            } else {
                                Icon::new(IconName::PanelRight)
                            })
                            .selected(self.sidebar_open)
                            .tooltip(if self.sidebar_open {
                                "Hide operations sidebar"
                            } else {
                                "Show operations sidebar"
                            })
                            .disabled(self.converting)
                            .on_click(cx.listener(|audit, _, window, cx| {
                                audit.toggle_rail_tab(audit.rail, cx);
                                window.focus(&audit.focus, cx);
                            })),
                    ),
            )
    }
}
