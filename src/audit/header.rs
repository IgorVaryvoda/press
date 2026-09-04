//! The header line above the table: source, filter box, view switch.

use super::*;

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
        // something had gone wrong.
        if self.existing_output > 0 {
            warnings.push_str(&match self.existing_output {
                1 => format!(" · 1 file in {}/", scan::OUTPUT_DIR),
                many => format!(" · {many} files in {}/", scan::OUTPUT_DIR),
            });
        }
        warnings
    }

    /// The findings the list can narrow to right now, with the label and
    /// tooltip each chip would carry. Listed once so the inline chips and the
    /// narrow-window menu cannot disagree.
    pub(super) fn available_findings(&self) -> Vec<(Finding, IconName, String, &'static str)> {
        let mut findings = Vec::new();
        // The audit reads bytes per pixel for every row and then asks you to
        // find the heavy ones yourself. These are that answer, as the control
        // that narrows the list to them.
        if self.heavy > 0 {
            findings.push((
                Finding::Heavy,
                IconName::TriangleAlert,
                format!("{} heavy", self.heavy),
                "Files carrying more bytes per pixel than a photograph \
                 needs. Click to show only them.",
            ));
        }
        // What the last run could not convert. Only here while there is
        // something to show: a chip that is always present would be a filter
        // for an empty list on every folder that converted cleanly.
        if !self.failures.is_empty() {
            findings.push((
                Finding::Failed,
                IconName::CircleX,
                format!("{} failed", self.failures.len()),
                "Files the last run could not convert. Click to show only \
                 them, then convert them again once the cause is fixed.",
            ));
        }
        if self.mislabelled > 0 {
            findings.push((
                Finding::Mislabelled,
                IconName::TriangleAlert,
                format!("{} mislabelled", self.mislabelled),
                "Files whose bytes are not the format their extension \
                 claims. Click to show only them.",
            ));
        }
        if acquisition::SHOW_ACQUISITION_EXTRAS && self.marketplace > 0 {
            findings.push((
                Finding::Marketplace,
                IconName::TriangleAlert,
                format!("{} preflight", self.marketplace),
                "Marketplace file preflight: 1400×1400, at most 250 KB, and a truthful extension. Review the background visually.",
            ));
        }
        findings
    }

    /// Every finding under one icon beside the filter. The header used to carry
    /// a chip per finding; chips plus the filter no longer fit the header's
    /// job, so one menu carries them all. `None` when there is nothing to
    /// narrow to, so the common case stays quiet. Lit while one is in force,
    /// so the count and the list below it never disagree.
    fn findings_menu(&self, cx: &mut Context<Self>) -> Option<gpui_kit::AnyElement> {
        let findings = self.available_findings();
        if findings.is_empty() {
            return None;
        }
        let total: usize = findings
            .iter()
            .map(|(finding, _, _, _)| match finding {
                Finding::Failed => self.failures.len(),
                Finding::Heavy => self.heavy,
                Finding::Mislabelled => self.mislabelled,
                Finding::Marketplace => self.marketplace,
            })
            .sum();
        let summary = findings
            .iter()
            .map(|(_, _, label, _)| label.clone())
            .collect::<Vec<_>>()
            .join(" · ");
        let active = self.finding;
        let source = cx.entity().downgrade();
        let has_failed = findings
            .iter()
            .any(|(finding, _, _, _)| *finding == Finding::Failed);
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
                    findings
                        .iter()
                        .fold(menu, |menu, (finding, icon, label, _)| {
                            let finding = *finding;
                            let label = label.clone();
                            let source = source.clone();
                            menu.item(
                                PopupMenuItem::new(label)
                                    .icon(icon.clone())
                                    .checked(active == Some(finding))
                                    .on_click(move |_, _, cx| {
                                        if let Some(audit) = source.upgrade() {
                                            audit.update(cx, |audit, cx| {
                                                audit.set_finding(finding, cx)
                                            });
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

    /// Every shortcut the list answers to, in one place. The window already
    /// moves this way, but nothing said so, and a shortcut nobody names is one
    /// nobody finds. The dialog is state, not a rail: it changes nothing.
    pub(super) fn shortcuts_view(&self, cx: &mut Context<Self>) -> gpui_kit::AnyElement {
        const SHORTCUTS: [(&str, &str); 9] = [
            ("↑ ↓ ← →", "Move between images"),
            ("PgUp PgDn Home End", "Jump through the list"),
            ("Shift + move", "Extend the selection"),
            ("Space", "Tick the row"),
            ("Enter", "Compare original and output"),
            ("Ctrl/⌘ + A", "Select everything shown"),
            ("Ctrl/⌘ + K", "Focus the filter box"),
            ("Ctrl/⌘ + ,", "Open settings"),
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
            .absolute()
            .inset_0()
            .flex()
            .items_center()
            .justify_center()
            .child(
                div()
                    .absolute()
                    .inset_0()
                    .bg(cx.theme().background.opacity(0.55))
                    .debug_selector(|| "shortcuts-backdrop".into())
                    .on_mouse_down(
                        gpui_kit::MouseButton::Left,
                        cx.listener(|audit, _, _, cx| {
                            audit.shortcuts_open = false;
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
        let sirv_disabled =
            self.converting || self.batch_folders.is_some() || self.scan_blocks_delivery();
        // Scope rides with opening: it decides what a folder open covers, the
        // way the command line decides it with a flag.
        let scope_checked = self.include_subfolders;
        let scope_disabled = self.converting || self.single_file;
        div()
            .debug_selector(|| "audit-header".into())
            .flex()
            .items_center()
            .gap_2()
            .px_3()
            .py_1p5()
            .overflow_hidden()
            .bg(cx.theme().table_head)
            .border_b_1()
            .border_color(cx.theme().border)
            // One named source control replaces three adjacent icon-only openers.
            .child(
                div()
                    .flex()
                    .items_center()
                    .gap_2()
                    .flex_1()
                    .min_w_0()
                    .min_w(px(260.))
                    .children(tree_collapsed.then(|| {
                        div().debug_selector(|| "folder-tree-toggle".into()).child(
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
                    .child(div().min_w_0().overflow_hidden().child(breadcrumbs))
                    .child(
                        Button::new("source-picker")
                            .small()
                            .ghost()
                            .icon(source_icon)
                            .label("Open")
                            .tooltip("Open a folder or choose images")
                            .dropdown_caret(true)
                            .disabled(self.converting)
                            .dropdown_menu(move |menu, _, _| {
                                let open_folder = source_menu.clone();
                                let open_images = source_menu.clone();
                                let toggle_scope = source_menu.clone();
                                let pair_sirv = source_menu.clone();
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
                            }),
                    ),
            )
            // The box narrows the list; erasing its text widens it back out.
            // There is no Clear button: it sat dimmed for most of the audit's
            // life and duplicated what backspace already does.
            .child(
                div()
                    .w(px(242.))
                    .flex()
                    .items_center()
                    .flex_shrink_0()
                    .child(
                        div().flex_1().min_w_0().child(
                            Input::new(&self.filter_input)
                                .small()
                                .disabled(self.converting)
                                .prefix(IconName::Search),
                        ),
                    ),
            )
            // Findings narrow the list too, hidden under one icon beside the
            // filter now that the per-finding chips are gone.
            .children(self.findings_menu(cx))
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
            // Sirv pairing lives in the Open menu; the reconciliation strip below
            // owns its status and actions once paired.
            // Persistent beside the view toggle so pairing is discoverable:
            // the menu buried it and users never found it. Selected when
            // paired, so the paired state stays visible above the strip.
            .child(
                div().debug_selector(|| "sirv-pair-header".into()).child(
                    Button::new("sirv-pair-header")
                        .small()
                        .ghost()
                        .icon(IconName::Globe)
                        .label("Sirv")
                        .tooltip(if self.sirv_pairing.is_some() {
                            "Paired with Sirv — see status below the header"
                        } else {
                            "Pair a Sirv folder with this local folder"
                        })
                        .selected(self.sirv_pairing.is_some())
                        .disabled(sirv_disabled)
                        .on_click(cx.listener(|audit, _, _, cx| audit.open_sirv_browser(cx))),
                ),
            )
            // One button, not a pair: the views exclude each other. The icon
            // mirrors the layout on screen, so the grid never wears a burger
            // menu, and the tooltip names the destination a click reaches.
            .child(
                Button::new("view-grid")
                    .small()
                    .outline()
                    .icon(if self.grid {
                        IconName::LayoutDashboard
                    } else {
                        IconName::Menu
                    })
                    .tooltip(if self.grid {
                        "Show the audit as a list"
                    } else {
                        "Show the images as a gallery"
                    })
                    .disabled(self.converting)
                    .on_click(cx.listener(|audit, _, _, cx| {
                        if audit.converting {
                            return;
                        }
                        audit.set_grid(!audit.grid, cx);
                    })),
            )
            .child(
                // The operations sidebar, always a click away. Collapsed, the
                // list takes the room; the verbs in the action bar reopen it
                // on the tab they need.
                Button::new("toggle-sidebar")
                    .small()
                    .ghost()
                    .icon(IconName::PanelRight)
                    .tooltip("Toggle operations sidebar")
                    .selected(self.sidebar_open)
                    .on_click(cx.listener(|audit, _, _, cx| {
                        audit.sidebar_open = !audit.sidebar_open;
                        cx.notify();
                    })),
            )
            .child(
                // The full shortcut list, one key away. Text, not an icon: no
                // icon in the set says "keyboard" on its own.
                Button::new("open-shortcuts")
                    .small()
                    .ghost()
                    .label("?")
                    .tooltip("Keyboard shortcuts (?)")
                    .on_click(cx.listener(|audit, _, _, cx| {
                        audit.shortcuts_open = true;
                        cx.notify();
                    })),
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
