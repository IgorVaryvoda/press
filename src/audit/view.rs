use super::*;

/// A label for the comparison view, which floats over the picture rather than over
/// a theme surface, so it carries its own dark backing.
/// A proportional bar. The audit is a ranking and a column of numbers does not
/// rank — 632 KB and 104 KB were set in the same size and colour, so the shape of
/// the folder was invisible in a list sorted by exactly that.
pub(super) fn meter(
    id: impl Into<gpui::ElementId>,
    fraction: f32,
    colour: gpui::Hsla,
    height: f32,
) -> Progress {
    let fraction = if fraction.is_finite() {
        fraction.clamp(0., 1.)
    } else {
        0.
    };
    Progress::new(id)
        .value(fraction * 100.)
        .color(colour)
        .h(px(height))
}

impl Audit {
    /// The remote-folder browser: a small panel over the window. Walk folders
    /// down, pair the folder you land on, or undo a pairing.
    fn sirv_browser_view(&self, browser: &SirvBrowser, cx: &mut Context<Self>) -> gpui::AnyElement {
        let paired = self
            .sirv_pairing
            .as_ref()
            .map(|pairing| pairing.dir.clone());
        let at_root = browser.path.trim_end_matches('/').is_empty();

        let body: gpui::AnyElement = match browser.nodes.as_ref() {
            None => div()
                .flex()
                .items_center()
                .gap_2()
                .text_size(px(12.))
                .text_color(cx.theme().muted_foreground)
                .child(IconName::LoaderCircle)
                .child(format!("Listing {}…", browser.path))
                .into_any_element(),
            Some(Err(message)) => div()
                .text_size(px(12.))
                .text_color(cx.theme().yellow)
                .child(message.clone())
                .into_any_element(),
            Some(Ok(nodes)) => {
                let mut rows: Vec<gpui::AnyElement> = Vec::new();
                if browser.path != "/" {
                    rows.push(
                        Button::new("sirv-up")
                            .ghost()
                            .small()
                            .icon(IconName::ArrowUp)
                            .label("..")
                            .on_click(cx.listener(|audit, _, _, cx| audit.ascend_sirv(cx)))
                            .into_any_element(),
                    );
                }
                for (ix, node) in nodes.iter().filter(|node| node.is_folder()).enumerate() {
                    let name = node
                        .filename
                        .rsplit('/')
                        .next()
                        .unwrap_or(&node.filename)
                        .to_string();
                    let descend_to = name.clone();
                    rows.push(
                        Button::new(("sirv-dir", ix))
                            .ghost()
                            .small()
                            .icon(IconName::FolderOpen)
                            .label(name)
                            .on_click(cx.listener(move |audit, _, _, cx| {
                                audit.descend_sirv(descend_to.clone(), cx);
                            }))
                            .into_any_element(),
                    );
                }
                if rows.is_empty() {
                    rows.push(
                        div()
                            .text_size(px(12.))
                            .text_color(cx.theme().muted_foreground)
                            .child("No subfolders.")
                            .into_any_element(),
                    );
                }
                div()
                    .id("sirv-list")
                    .flex()
                    .flex_col()
                    .items_start()
                    .gap_0p5()
                    .max_h(px(280.))
                    .overflow_y_scroll()
                    .children(rows)
                    .into_any_element()
            }
        };

        div()
            .w(px(440.))
            .flex()
            .flex_col()
            .gap_3()
            .p_4()
            .rounded_lg()
            .bg(cx.theme().secondary)
            .border_1()
            .border_color(cx.theme().border)
            .child(
                div()
                    .flex()
                    .flex_col()
                    .gap_0p5()
                    .child(
                        div()
                            .font_family("SF Pro Display")
                            .text_size(px(15.))
                            .font_weight(FontWeight::SEMIBOLD)
                            .text_color(cx.theme().foreground)
                            .child("Sync with Sirv"),
                    )
                    .child(
                        div()
                            .font_family(cx.theme().mono_font_family.clone())
                            .text_size(px(11.))
                            .text_color(cx.theme().muted_foreground)
                            .whitespace_nowrap()
                            .overflow_hidden()
                            .text_ellipsis()
                            .child(browser.path.clone()),
                    ),
            )
            .child(body)
            .when_some(self.sirv_job.as_ref(), |panel, job| {
                panel.child(
                    div()
                        .text_size(px(11.))
                        .font_family(cx.theme().mono_font_family.clone())
                        .text_color(if job.failed == 0 {
                            cx.theme().muted_foreground
                        } else {
                            cx.theme().yellow
                        })
                        .child(match (job.finished, job.kind) {
                            (false, SirvJobKind::Pull) => {
                                format!("Pulling {} of {}…", job.done, job.total)
                            }
                            (false, SirvJobKind::PullChanged) => {
                                format!("Taking from Sirv {} of {}…", job.done, job.total)
                            }
                            (false, SirvJobKind::Push) => {
                                format!("Pushing {} of {}…", job.done, job.total)
                            }
                            (false, SirvJobKind::PushChanged) => {
                                format!("Overwriting on Sirv {} of {}…", job.done, job.total)
                            }
                            (true, kind) => {
                                let verb = match kind {
                                    SirvJobKind::Pull => "Pulled",
                                    SirvJobKind::PullChanged => "Took from Sirv",
                                    SirvJobKind::Push => "Pushed",
                                    SirvJobKind::PushChanged => "Overwrote on Sirv",
                                };
                                let failures = if job.failed == 0 {
                                    String::new()
                                } else {
                                    let rest = job.failed.saturating_sub(job.failures.len());
                                    format!(
                                        ", {} failed: {}{}",
                                        job.failed,
                                        job.failures.join(", "),
                                        if rest == 0 {
                                            String::new()
                                        } else {
                                            format!(" and {rest} more")
                                        },
                                    )
                                };
                                format!("{verb} {} of {}{failures}", job.done, job.total)
                            }
                        }),
                )
            })
            .child(
                div().flex().items_center().justify_between().child(
                    div()
                        .flex()
                        .flex_wrap()
                        .gap_2()
                        .when_some(paired, |row, dir| {
                            let busy = self.sirv_busy();
                            let stopping = self.sirv_job.as_ref().is_some_and(|job| job.stopping);
                            let push_changed_confirmed =
                                self.sirv_confirm == Some(SirvJobKind::PushChanged);
                            let pull_changed_confirmed =
                                self.sirv_confirm == Some(SirvJobKind::PullChanged);
                            let (to_push, changed, to_pull) = self.sirv_counts.unwrap_or((0, 0, 0));
                            row.when(busy, |row| {
                                row.child(
                                    Button::new("sirv-stop")
                                        .outline()
                                        .small()
                                        .label(if stopping { "Stopping…" } else { "Stop" })
                                        .disabled(stopping)
                                        .on_click(cx.listener(|audit, _, _, cx| {
                                            audit.cancel_sirv_transfer();
                                            cx.notify();
                                        })),
                                )
                            })
                            .child(
                                Button::new("sirv-pull")
                                    .outline()
                                    .small()
                                    .icon(IconName::ArrowDown)
                                    .label(format!("Pull {to_pull} missing"))
                                    .disabled(busy || to_pull == 0)
                                    .on_click(cx.listener(|audit, _, _, cx| audit.start_pull(cx))),
                            )
                            .child(
                                Button::new("sirv-push")
                                    .outline()
                                    .small()
                                    .icon(IconName::ArrowUp)
                                    .label(format!("Push {to_push} new"))
                                    .disabled(busy || to_push == 0)
                                    .on_click(cx.listener(|audit, _, _, cx| audit.start_push(cx))),
                            )
                            .when(changed > 0, |row| {
                                row.child(
                                    Button::new("sirv-push-changed")
                                        .when(push_changed_confirmed, |button| button.primary())
                                        .when(!push_changed_confirmed, |button| button.ghost())
                                        .small()
                                        .label(if push_changed_confirmed {
                                            format!("Really overwrite {changed} on Sirv?")
                                        } else {
                                            format!("Overwrite {changed} on Sirv")
                                        })
                                        .disabled(busy)
                                        .on_click(cx.listener(|audit, _, _, cx| {
                                            if audit.sirv_confirm == Some(SirvJobKind::PushChanged)
                                            {
                                                audit.sirv_confirm = None;
                                                audit.start_push_changed(cx);
                                            } else {
                                                audit.sirv_confirm = Some(SirvJobKind::PushChanged);
                                                cx.notify();
                                            }
                                        })),
                                )
                                .child(
                                    Button::new("sirv-pull-changed")
                                        .when(pull_changed_confirmed, |button| button.primary())
                                        .when(!pull_changed_confirmed, |button| button.ghost())
                                        .small()
                                        .label(if pull_changed_confirmed {
                                            format!("Really replace {changed} local files?")
                                        } else {
                                            format!("Take {changed} from Sirv")
                                        })
                                        .disabled(busy)
                                        .on_click(cx.listener(|audit, _, _, cx| {
                                            if audit.sirv_confirm == Some(SirvJobKind::PullChanged)
                                            {
                                                audit.sirv_confirm = None;
                                                audit.start_pull_changed(cx);
                                            } else {
                                                audit.sirv_confirm = Some(SirvJobKind::PullChanged);
                                                cx.notify();
                                            }
                                        })),
                                )
                            })
                            .child(
                                Button::new("sirv-unpair")
                                    .ghost()
                                    .small()
                                    .label(format!("Unpair {dir}"))
                                    .disabled(busy)
                                    .on_click(cx.listener(|audit, _, window, cx| {
                                        audit.unpair_sirv(cx);
                                        Self::restore_audit_focus(window, cx);
                                    })),
                            )
                        }),
                ),
            )
            .child(
                div()
                    .flex()
                    .gap_2()
                    .child(
                        Button::new("sirv-close")
                            .ghost()
                            .small()
                            .label("Close")
                            .on_click(cx.listener(|audit, _, window, cx| {
                                audit.close_sirv_browser(window, cx);
                            })),
                    )
                    .child(
                        Button::new("sirv-pair")
                            .primary()
                            .small()
                            .label(if at_root {
                                "Open a folder to pair it"
                            } else {
                                "Pair this folder"
                            })
                            .disabled(at_root || !matches!(browser.nodes, Some(Ok(_))))
                            .on_click(cx.listener(|audit, _, window, cx| {
                                audit.pair_sirv(cx);
                                Self::restore_audit_focus(window, cx);
                            })),
                    ),
            )
            .into_any_element()
    }

    /// Bytes of what is on screen. With a filter active the folder total would be
    /// describing files the list is not showing.
    pub(super) fn visible_bytes(&self) -> u64 {
        self.visible_bytes
    }
}

impl Render for Audit {
    // Three shapes share this method — empty state, comparison, and the list — so it
    // erases to one type rather than making the caller's `impl Trait` pick a winner.
    #[allow(refining_impl_trait)]
    fn render(&mut self, window: &mut Window, cx: &mut Context<Self>) -> gpui::AnyElement {
        let count = self.visible.len();

        let title = match self.root.file_name() {
            Some(name) => format!("{} — Press", name.to_string_lossy()),
            None => "Press".to_string(),
        };
        if title != self.titled {
            window.set_window_title(&title);
            self.titled = title;
        }

        // Cheap enough to compare every frame, and it means a crash still leaves the
        // last good size and folder on disk. The write itself is delayed.
        let viewport = window.viewport_size();
        let current = settings::Settings {
            width: Some(f32::from(viewport.width)),
            height: Some(f32::from(viewport.height)),
            folder: self.root.is_dir().then(|| self.root.clone()),
        };
        if current != self.settings {
            self.remember_settings(current, cx);
        }

        if let Some(table) = self.table.clone() {
            // The table lives left of the output panel; handing it the full
            // viewport would make every column calculation 264px too wide.
            let (root_left, root_right) = root_horizontal_chrome(window);
            let width =
                f32::from(viewport.width) - panel::OUTPUT_PANEL_WIDTH - root_left - root_right;
            let show_result = !self.results.is_empty();
            let signature = (width.round().max(0.) as u32, show_result);
            if self.table_signature != Some(signature) {
                self.table_signature = Some(signature);
                cx.defer(move |cx| {
                    table.update(cx, |table, cx| {
                        table.delegate_mut().set_viewport_width(width, show_result);
                        table.refresh(cx);
                        cx.notify();
                    });
                });
            }
        }

        if let Some(scanning) = self.scanning.as_ref() {
            let label = scanning.clone();
            return div()
                .size_full()
                .flex()
                .items_center()
                .justify_center()
                .p_4()
                .bg(cx.theme().background)
                .child(
                    div()
                        .flex()
                        .flex_col()
                        .items_center()
                        .gap_2()
                        .w(px(420.))
                        .px_4()
                        .py_4()
                        .rounded_lg()
                        .bg(cx.theme().secondary)
                        .border_1()
                        .border_color(cx.theme().border)
                        .child(
                            div()
                                .font_family("SF Pro Display")
                                .text_size(px(18.))
                                .font_weight(FontWeight::SEMIBOLD)
                                .text_color(cx.theme().foreground)
                                .child(format!("Scanning {label}…")),
                        )
                        .child(
                            div()
                                .text_size(px(12.))
                                .text_color(cx.theme().muted_foreground)
                                .child(
                                    "The current folder stays untouched until the scan finishes.",
                                ),
                        ),
                )
                .on_drop(cx.listener(|audit, paths: &gpui::ExternalPaths, _, cx| {
                    if let Some(path) = paths.paths().first() {
                        audit.request_path(path.clone(), cx);
                    }
                }))
                .into_any_element();
        }

        if self.entries.is_empty() {
            let empty_folder = self.root.is_dir();
            return div()
                .size_full()
                .flex()
                .items_center()
                .justify_center()
                .p_4()
                .bg(cx.theme().background)
                .child(
                    // A panel rather than loose text, so the window has something in
                    // it and the drop target has an edge you can see.
                    div()
                        .flex()
                        .flex_col()
                        .items_center()
                        .gap_2()
                        .w(px(400.))
                        .px_4()
                        .py_6()
                        .border_dashed()
                        .border_1()
                        .border_color(if self.drag_over {
                            cx.theme().drag_border
                        } else {
                            cx.theme().border
                        })
                        .child(
                            div()
                                .font_family("SF Pro Display")
                                .text_size(px(19.))
                                .font_weight(FontWeight::SEMIBOLD)
                                .text_color(cx.theme().foreground)
                                .child(if empty_folder {
                                    "No supported images found"
                                } else {
                                    "Audit a folder of images"
                                }),
                        )
                        .child(
                            div()
                                .text_size(px(12.))
                                .text_color(cx.theme().muted_foreground)
                                .text_center()
                                .child(if empty_folder {
                                    "This folder has no supported images. Choose another folder \
                                     or drop an image here."
                                } else {
                                    "Nothing is uploaded. Every file is read, resized and \
                                     re-encoded on this machine."
                                }),
                        )
                        .child(
                            div()
                                .flex()
                                .gap_2()
                                .pt_2()
                                .child(
                                    Button::new("empty-folder")
                                        .primary()
                                        .icon(IconName::Folder)
                                        .label("Open folder…")
                                        .on_click(
                                            cx.listener(|audit, _, _, cx| audit.pick(true, cx)),
                                        ),
                                )
                                .child(
                                    Button::new("empty-file")
                                        .outline()
                                        .icon(IconName::File)
                                        .label("Open image…")
                                        .on_click(
                                            cx.listener(|audit, _, _, cx| audit.pick(false, cx)),
                                        ),
                                ),
                        )
                        .child(
                            div()
                                .pt_2()
                                .text_size(px(12.))
                                .text_color(cx.theme().muted_foreground.opacity(0.7))
                                .child("or drop one anywhere in this window"),
                        ),
                )
                .on_drag_move(cx.listener(
                    |audit, _: &gpui::DragMoveEvent<gpui::ExternalPaths>, _, cx| {
                        if !audit.drag_over {
                            audit.drag_over = true;
                            cx.notify();
                        }
                    },
                ))
                .on_drop(cx.listener(|audit, paths: &gpui::ExternalPaths, _, cx| {
                    audit.drag_over = false;
                    if let Some(path) = paths.paths().first() {
                        audit.request_path(path.clone(), cx);
                    }
                }))
                .into_any_element();
        }

        if self.settings_panel.is_some() {
            let view = self.settings_panel_view(cx);
            // The click that opened the panel left focus on the button it
            // replaced; take focus next frame so typing lands in the first
            // field. Once only: after that the field with focus is whichever
            // one Tab or a click chose. Nothing else in the framework moves Tab
            // between inputs, so this panel cycles them itself.
            cx.defer_in(window, |audit, window, cx| {
                if let Some(panel) = audit.settings_panel.as_mut()
                    && !panel.focused
                {
                    panel.focused = true;
                    let handle = panel.client_id.read(cx).focus_handle(cx);
                    window.focus(&handle, cx);
                }
            });
            return div()
                .size_full()
                .flex()
                .items_center()
                .justify_center()
                .bg(cx.theme().background)
                .on_key_down(
                    cx.listener(|audit, event: &gpui::KeyDownEvent, window, cx| {
                        match event.keystroke.key.as_str() {
                            "escape" => {
                                audit.close_settings(window, cx);
                            }
                            "tab" => {
                                const FIELDS: usize = 2;
                                let direction = if event.keystroke.modifiers.shift {
                                    FIELDS - 1
                                } else {
                                    1
                                };
                                if let Some(panel) = audit.settings_panel.as_mut() {
                                    panel.focus_ix = (panel.focus_ix + direction) % FIELDS;
                                    let handle = [
                                        panel.client_id.read(cx).focus_handle(cx),
                                        panel.client_secret.read(cx).focus_handle(cx),
                                    ][panel.focus_ix]
                                        .clone();
                                    window.focus(&handle, cx);
                                }
                            }
                            _ => {}
                        }
                    }),
                )
                .child(view)
                .into_any_element();
        }

        if let Some(browser) = self.sirv_browser.take() {
            let view = self.sirv_browser_view(&browser, cx);
            self.sirv_browser = Some(browser);
            // The click that opened the browser left focus on the header
            // button it replaced, so Escape had nowhere to land. Same fix as
            // the comparison: take focus next frame, once this tree exists.
            cx.defer_in(window, |audit, window, cx| {
                if let Some(browser) = audit.sirv_browser.as_mut()
                    && !browser.focused
                {
                    browser.focused = true;
                    window.focus(&browser.focus, cx);
                }
            });
            return div()
                .size_full()
                .flex()
                .items_center()
                .justify_center()
                .bg(cx.theme().background)
                .track_focus(&self.sirv_browser.as_ref().unwrap().focus)
                .on_key_down(
                    cx.listener(|audit, event: &gpui::KeyDownEvent, window, cx| {
                        if event.keystroke.key == "escape" {
                            audit.close_sirv_browser(window, cx);
                        }
                    }),
                )
                .child(view)
                .into_any_element();
        }

        if let Some(comparison) = self.compare.take() {
            // Taken and put back so the view can borrow `self` immutably while the
            // listeners it builds hold a mutable handle to the same entity.
            let view = self.compare_view(&comparison, window, cx);
            self.compare = Some(comparison);
            // The click or Enter that opened this view left focus inside the list
            // it replaced, so Escape had nowhere to land. Take focus back next
            // frame, after the compare tree exists.
            cx.defer_in(window, |audit, window, cx| window.focus(&audit.focus, cx));
            return div()
                .size_full()
                .relative()
                .track_focus(&self.focus)
                .on_key_down(cx.listener(|audit, event: &gpui::KeyDownEvent, _, cx| {
                    match event.keystroke.key.as_str() {
                        "escape" => {
                            audit.compare = None;
                            cx.notify();
                        }
                        "right" | "down" => audit.step_compare(1, cx),
                        "left" | "up" => audit.step_compare(-1, cx),
                        "f" => {
                            if let Some(comparison) = audit.compare.as_mut() {
                                comparison.zoom = None;
                                comparison.pan = (0., 0.);
                                cx.notify();
                            }
                        }
                        "1" => {
                            if let Some(comparison) = audit.compare.as_mut() {
                                comparison.zoom = Some(1.);
                                comparison.pan = (0., 0.);
                                cx.notify();
                            }
                        }
                        _ => {}
                    }
                }))
                .child(view)
                .into_any_element();
        }

        div()
            .size_full()
            .flex()
            .flex_col()
            .bg(cx.theme().background)
            .track_focus(&self.focus)
            // Always bordered, so a hovering drag recolours the frame instead of
            // shifting the whole window's contents inward by two pixels.
            .border_2()
            .border_color(if self.drag_over {
                cx.theme().drag_border
            } else {
                gpui::transparent_black()
            })
            .on_drag_move(cx.listener(
                |audit, _: &gpui::DragMoveEvent<gpui::ExternalPaths>, _, cx| {
                    if !audit.drag_over {
                        audit.drag_over = true;
                        cx.notify();
                    }
                },
            ))
            .on_key_down(
                cx.listener(|audit, event: &gpui::KeyDownEvent, window, cx| {
                    // The filter box swallows its own keys, so these only fire when the
                    // list itself has focus. Shift turns any move into a selection
                    // drag from the anchor.
                    let extend = event.keystroke.modifiers.shift;
                    match event.keystroke.key.as_str() {
                        "down" => audit.step_cursor(1, extend, cx),
                        "up" => audit.step_cursor(-1, extend, cx),
                        "left" => audit.step_cursor_lateral(-1, extend, cx),
                        "right" => audit.step_cursor_lateral(1, extend, cx),
                        "pagedown" => audit.step_cursor(10, extend, cx),
                        "pageup" => audit.step_cursor(-10, extend, cx),
                        "home" => audit.step_cursor(isize::MIN / 2, extend, cx),
                        "end" => audit.step_cursor(isize::MAX / 2, extend, cx),
                        "escape" => {
                            // Nothing is open here, so escape means "put the list down":
                            // the ticked set clears, the way it does in every file manager.
                            if !audit.selected.is_empty() && !audit.converting {
                                audit.selected.clear();
                                audit.schedule_estimate(cx);
                                cx.notify();
                            }
                        }
                        "a" if event.keystroke.modifiers.control
                            || event.keystroke.modifiers.platform =>
                        {
                            // Select what the list shows, not what the folder holds:
                            // a filter that hides files from the list must hide them
                            // from Convert too.
                            if !audit.converting {
                                audit.selected.extend(audit.visible.iter().copied());
                                audit.schedule_estimate(cx);
                                cx.notify();
                            }
                        }
                        "," if event.keystroke.modifiers.control
                            || event.keystroke.modifiers.platform =>
                        {
                            audit.open_settings(window, cx);
                        }
                        "space" => audit.toggle_cursor_selection(cx),
                        "enter" => {
                            if !audit.converting
                                && let Some(entry) = audit.entry_at(audit.cursor)
                            {
                                audit.open_compare(entry, cx);
                            }
                        }
                        _ => {}
                    }
                }),
            )
            .on_drop(cx.listener(|audit, paths: &gpui::ExternalPaths, _, cx| {
                audit.drag_over = false;
                if let Some(path) = paths.paths().first() {
                    audit.request_path(path.clone(), cx);
                }
            }))
            .child(self.header(count, cx))
            // Audit on the left, the output panel on the right: the working
            // area and the settings column split below one shared header.
            .child(
                div()
                    .flex()
                    .flex_1()
                    .overflow_hidden()
                    .child(
                        div()
                            .flex()
                            .flex_col()
                            .flex_1()
                            .min_w_0()
                            .overflow_hidden()
                            .child(self.controls(cx))
                            .children(self.notices(cx))
                            .child(self.audit_content(count, window, cx)),
                    )
                    .child(self.output_panel(cx)),
            )
            .into_any_element()
    }
}

impl Audit {
    /// The list or the gallery, filling the space left of the panel.
    fn audit_content(
        &mut self,
        count: usize,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> gpui::AnyElement {
        // The list runs to the window edge; hairlines above it, not a
        // card floating in padding.
        div()
            .flex()
            .flex_col()
            .flex_1()
            .overflow_hidden()
            .bg(cx.theme().table)
            // Columns take a width, not a share, so the remainder after the
            // fixed ones has to be handed to the name column by hand.
            .child(if self.grid {
                // One virtualised band is one row of fixed-size tiles.
                let (root_left, root_right) = root_horizontal_chrome(window);
                let layout = gallery_layout(
                    f32::from(window.viewport_size().width) - panel::OUTPUT_PANEL_WIDTH,
                    root_left,
                    root_right,
                    count,
                );
                if let Some(previous) = self.gallery_columns
                    && previous != layout.columns
                {
                    self.gallery_scroll
                        .scroll_to_item_strict(0, ScrollStrategy::Top);
                }
                self.gallery_columns = Some(layout.columns);
                let gallery = uniform_list(
                    "gallery",
                    layout.rows,
                    cx.processor(|audit, range: std::ops::Range<usize>, _window, cx| {
                        range
                            .map(|band| {
                                // A plain loop: the closure form borrows `audit`
                                // mutably for `request_thumb` and immutably for
                                // `tile`, which nested closures cannot express.
                                let mut tiles = Vec::new();
                                let (root_left, root_right) = root_horizontal_chrome(_window);
                                let layout = gallery_layout(
                                    f32::from(_window.viewport_size().width)
                                        - panel::OUTPUT_PANEL_WIDTH,
                                    root_left,
                                    root_right,
                                    audit.visible.len(),
                                );
                                for row in layout.band_range(band) {
                                    let Some(entry) = audit.entry_at(row) else {
                                        continue;
                                    };
                                    audit.request_thumb(entry, cx);
                                    tiles.push(audit.tile(row, entry, layout.tile, cx));
                                }
                                div().flex().gap_2().children(tiles)
                            })
                            .collect::<Vec<_>>()
                    }),
                )
                .track_scroll(&self.gallery_scroll)
                .size_full()
                .p_2();
                div()
                    .relative()
                    .flex_1()
                    .overflow_hidden()
                    .child(gallery)
                    .child(
                        div()
                            .absolute()
                            .top_0()
                            .right_0()
                            .bottom_0()
                            .w(Scrollbar::width())
                            .debug_selector(|| "gallery-scrollbar".into())
                            .child(
                                Scrollbar::vertical(&self.gallery_scroll)
                                    .id("gallery-scrollbar")
                                    .mode(ScrollbarMode::Always)
                                    .viewport_from_layout(),
                            ),
                    )
                    .into_any_element()
            } else if let Some(table) = self.table.as_ref() {
                DataTable::new(table)
                    .stripe(false)
                    .bordered(false)
                    .into_any_element()
            } else {
                div().into_any_element()
            })
            .into_any_element()
    }
}
