//! The side-by-side comparison view: zoom, pan, chips, footer.

use super::*;

fn compare_chip(text: impl Into<gpui::SharedString>, colour: gpui::Hsla, _cx: &App) -> gpui::Div {
    div()
        .h(px(18.))
        .px_2()
        .flex()
        .items_center()
        .flex_shrink_0()
        .rounded_md()
        .bg(rgba(0x000000b8))
        .text_size(px(11.))
        .font_weight(FontWeight::MEDIUM)
        .text_color(colour)
        .child(text.into())
}

impl Audit {
    /// Full-window comparison. It opens fitted, because you cannot judge a crop of an
    /// image you have not seen yet, and zooms to 1:1 and beyond — fitting a 5568px
    /// photo into a 900px window hides exactly the artefacts this view exists to show.
    pub(super) fn compare_view(
        &self,
        comparison: &Comparison,
        window: &Window,
        cx: &mut Context<Self>,
    ) -> gpui::AnyElement {
        let viewport = window.viewport_size();
        let (view_w, view_h) = (f32::from(viewport.width), f32::from(viewport.height));
        // At the minimum width the conversion button already names the target format.
        // Keep the image name readable and drop the duplicate byte summary first.
        let entry = self.entries.get(comparison.index);
        let name = entry.map(|entry| entry.name()).unwrap_or_default();
        // Where this image sits in the folder, so stepping through it has a
        // sense of distance rather than just a pair of arrows.
        let position = match (
            comparison.written.is_some(),
            self.result_position(comparison.index),
            self.row_of(comparison.index),
        ) {
            (true, Some((at, total)), _) => format!("{at} of {total} outputs"),
            (_, _, Some(row)) => format!("{} of {}", row + 1, self.visible.len()),
            _ => String::new(),
        };
        let (can_previous, can_next) = if comparison.written.is_some() {
            self.result_position(comparison.index)
                .map_or((false, false), |(at, total)| (at > 1, at < total))
        } else {
            (
                self.compare_target_from(comparison.index, -1).is_some(),
                self.compare_target_from(comparison.index, 1).is_some(),
            )
        };

        let mut stage = div()
            .id("compare-stage")
            .absolute()
            .inset_0()
            .overflow_hidden()
            .bg(rgb(0x0b0d10))
            .on_mouse_down(
                gpui::MouseButton::Left,
                cx.listener(|audit, event: &gpui::MouseDownEvent, _, cx| {
                    if let Some(comparison) = audit.compare.as_mut() {
                        let at = (f32::from(event.position.x), f32::from(event.position.y));
                        comparison.drag = Some((at, comparison.pan));
                        cx.notify();
                    }
                }),
            )
            .on_mouse_up(
                gpui::MouseButton::Left,
                cx.listener(|audit, _: &gpui::MouseUpEvent, _, cx| {
                    if let Some(comparison) = audit.compare.as_mut() {
                        comparison.drag = None;
                        cx.notify();
                    }
                }),
            )
            .on_scroll_wheel(
                cx.listener(move |audit, event: &gpui::ScrollWheelEvent, _, cx| {
                    let Some(comparison) = audit.compare.as_mut() else {
                        return;
                    };
                    let Some((width, height)) = comparison.dimensions() else {
                        return;
                    };

                    let ticks = match event.delta {
                        gpui::ScrollDelta::Lines(delta) => delta.y,
                        gpui::ScrollDelta::Pixels(delta) => f32::from(delta.y) / 40.,
                    };
                    if ticks == 0. {
                        return;
                    }

                    let fit = (view_w / width as f32).min(view_h / height as f32).min(1.);
                    let before = comparison.zoom.unwrap_or(fit);
                    // Fit is the floor: below it the image is a stamp adrift in
                    // a black stage, and there is nothing smaller-than-everything
                    // could ever show about compression.
                    let after = (before * 1.2f32.powf(ticks)).clamp(fit, 16.);

                    if after <= fit {
                        // Landing back at fit snaps to the centred fitted state,
                        // so zooming out is always a way home — `None` keeps
                        // tracking the window if it resizes.
                        comparison.zoom = None;
                        comparison.pan = (0., 0.);
                        cx.notify();
                        return;
                    }

                    // Keep whatever is under the pointer under the pointer. Without this
                    // zooming walks the image off screen.
                    let pointer = (
                        f32::from(event.position.x) - view_w / 2.,
                        f32::from(event.position.y) - view_h / 2.,
                    );
                    let ratio = after / before;
                    comparison.pan = (
                        pointer.0 - (pointer.0 - comparison.pan.0) * ratio,
                        pointer.1 - (pointer.1 - comparison.pan.1) * ratio,
                    );
                    comparison.zoom = Some(after);
                    cx.notify();
                }),
            )
            .on_mouse_move(
                cx.listener(move |audit, event: &gpui::MouseMoveEvent, _, cx| {
                    let Some(comparison) = audit.compare.as_mut() else {
                        return;
                    };
                    let at = (f32::from(event.position.x), f32::from(event.position.y));

                    match comparison.drag {
                        // Held: pan both sides together, so they stay in register.
                        Some((from, start_pan)) => {
                            comparison.pan =
                                (start_pan.0 + at.0 - from.0, start_pan.1 + at.1 - from.1);
                        }
                        // Free: the divider tracks the pointer only when there
                        // are two sides to divide.
                        None if comparison.mode == MediaMode::Compare => {
                            comparison.split = (at.0 / view_w).clamp(0., 1.)
                        }
                        None => return,
                    }
                    cx.notify();
                }),
            );

        // Fit never scales up: a 400px thumbnail blown across a 4K window is just a
        // blurry 400px thumbnail. Computed before the branch because the chrome
        // reports the zoom as well as the image using it.
        let scale = comparison.dimensions().map(|(width, height)| {
            let fit = (view_w / width as f32).min(view_h / height as f32).min(1.);
            comparison.zoom.unwrap_or(fit)
        });

        if comparison.mode == MediaMode::Preview {
            if let (Some(preview), Some(scale)) = (comparison.preview.as_ref(), scale) {
                let image_w = preview.width as f32 * scale;
                let image_h = preview.height as f32 * scale;
                let left = (view_w - image_w) / 2. + comparison.pan.0;
                let top = (view_h - image_h) / 2. + comparison.pan.1;
                stage = stage.child(
                    div()
                        .absolute()
                        .left(px(left))
                        .top(px(top))
                        .w(px(image_w))
                        .h(px(image_h))
                        .child(img(preview.image.clone()).w(px(image_w)).h(px(image_h))),
                );
            }
        } else if let (Some(pair), Some(scale)) = (comparison.pair.as_ref(), scale) {
            let natural = (pair.width as f32, pair.height as f32);
            let (image_w, image_h) = (natural.0 * scale, natural.1 * scale);
            // Negative when the image is larger than the window: that is the crop.
            let left = (view_w - image_w) / 2. + comparison.pan.0;
            let top = (view_h - image_h) / 2. + comparison.pan.1;
            let divider = view_w * comparison.split;

            let placed = |image: &Arc<gpui::RenderImage>| {
                div()
                    .absolute()
                    .left(px(left))
                    .top(px(top))
                    .w(px(image_w))
                    .h(px(image_h))
                    .child(img(image.clone()).w(px(image_w)).h(px(image_h)))
            };

            stage = stage
                .child(placed(&pair.converted))
                .child(
                    // The original, clipped to everything left of the divider. Its
                    // child keeps full width so both sides stay in register.
                    div()
                        .absolute()
                        .left_0()
                        .top_0()
                        .h_full()
                        .w(px(divider))
                        .overflow_hidden()
                        .child(
                            div()
                                .absolute()
                                .left_0()
                                .top_0()
                                .w(px(view_w))
                                .h(px(view_h))
                                .child(placed(&pair.original)),
                        ),
                )
                .child(
                    div()
                        .debug_selector(|| "compare-divider-handle".into())
                        .absolute()
                        .top_0()
                        .left(px(divider - 1.))
                        .w(px(2.))
                        .h_full()
                        .bg(rgba(0xffffffcc)),
                )
                .child(
                    div()
                        .absolute()
                        .top(px(view_h / 2. - 18.))
                        .left(px(divider - 16.))
                        .w(px(32.))
                        .h(px(36.))
                        .flex()
                        .items_center()
                        .justify_center()
                        .rounded_full()
                        .border_1()
                        .border_color(rgba(0xffffffcc))
                        .bg(rgba(0x000000cc))
                        .text_size(px(16.))
                        .text_color(gpui::white())
                        .cursor_ew_resize()
                        .child("↔"),
                )
                .child(
                    // Which side is which, pinned to the divider rather than to the
                    // window, so it stays true as the divider moves.
                    div()
                        .absolute()
                        .top(px(48.))
                        .left(px(divider - 76.))
                        .w(px(64.))
                        .flex()
                        .justify_end()
                        .child(compare_chip("original", cx.theme().foreground, cx)),
                )
                .child(
                    div()
                        .absolute()
                        .top(px(48.))
                        .left(px(divider + 12.))
                        .child(compare_chip(
                            match comparison.written.as_ref() {
                                // The file, not the format: in result mode this
                                // side is a thing on disk with a name.
                                Some(written) => match comparison.produced_by {
                                    Some(producer) => producer.result_label().to_string(),
                                    None => written
                                        .file_name()
                                        .map(|name| name.to_string_lossy().into_owned())
                                        .unwrap_or_else(|| self.format.label().to_uppercase()),
                                },
                                None => self.format.label().to_uppercase(),
                            },
                            cx.theme().green,
                            cx,
                        )),
                );
        }

        if comparison.failed {
            stage = stage.child(
                div()
                    .size_full()
                    .flex()
                    .items_center()
                    .justify_center()
                    .child(
                        Alert::error(
                            "compare-error",
                            if comparison.mode == MediaMode::Preview {
                                "Could not decode a preview for this image."
                            } else {
                                "Could not build a comparison for this image."
                            },
                        )
                        .max_w(px(420.)),
                    ),
            );
        }

        // The pair decodes and encodes in the background; until it lands the
        // stage used to be a black void with six grey letters in the footer.
        // A build in progress should look like one.
        if comparison.dimensions().is_none() && !comparison.failed {
            let message = match (comparison.mode, comparison.written.is_some()) {
                (MediaMode::Preview, _) => "Loading preview…".to_string(),
                (MediaMode::Compare, true) => {
                    "Loading the original and converted output…".to_string()
                }
                (MediaMode::Compare, false) => format!(
                    "Building comparison — encoding to {} {}…",
                    self.format.label().to_uppercase(),
                    self.quality.label()
                ),
            };
            stage = stage.child(
                div()
                    .size_full()
                    .flex()
                    .flex_col()
                    .items_center()
                    .justify_center()
                    .gap_2()
                    .child(
                        gpui_component::spinner::Spinner::new()
                            .large()
                            .color(cx.theme().muted_foreground),
                    )
                    .child(
                        div()
                            .text_size(px(12.))
                            .text_color(cx.theme().muted_foreground)
                            .child(message),
                    ),
            );
        }

        // Chrome as two full-width bars. These were black boxes pinned at
        // hand-computed offsets, the right-hand one at `view_w - 240` — a number
        // that stopped being the right edge the moment the text or window changed.
        stage
            // The top line: which file, where it sits in the folder, and the
            // way out. The verbs moved to the bar below, where the audit keeps
            // the same four — one bar meaning one thing in both screens.
            .child(
                div()
                    .absolute()
                    .left_0()
                    .right_0()
                    .top_0()
                    .flex()
                    .items_center()
                    .gap_2()
                    .px_3()
                    .py_2()
                    .bg(rgba(0x000000bf))
                    .text_size(px(12.))
                    .child(
                        div()
                            .flex_1()
                            .min_w_0()
                            .whitespace_nowrap()
                            .overflow_hidden()
                            .text_ellipsis()
                            .text_color(cx.theme().foreground)
                            .font_weight(FontWeight::MEDIUM)
                            .child(name.clone()),
                    )
                    .child(
                        div()
                            .flex_shrink_0()
                            .font_family(cx.theme().mono_font_family.clone())
                            .text_size(px(11.))
                            .text_color(rgba(0xffffffcc))
                            .whitespace_nowrap()
                            .child(position.clone()),
                    )
                    .child(
                        Button::new("compare-prev")
                            .small()
                            .ghost()
                            .icon(IconName::ArrowLeft)
                            .tooltip("Previous image")
                            .disabled(!can_previous)
                            .on_click(cx.listener(|audit, _, _, cx| audit.step_compare(-1, cx))),
                    )
                    .child(
                        Button::new("compare-next")
                            .small()
                            .ghost()
                            .icon(IconName::ArrowRight)
                            .tooltip("Next image")
                            .disabled(!can_next)
                            .on_click(cx.listener(|audit, _, _, cx| audit.step_compare(1, cx))),
                    )
                    .child(
                        Button::new("compare-close")
                            .small()
                            .ghost()
                            .icon(IconName::Close)
                            .tooltip("Back to the audit")
                            .on_click(cx.listener(|audit, _, _, cx| {
                                audit.compare = None;
                                cx.notify();
                            })),
                    ),
            )
            // Zoom, in its own cluster out of the way — top right, under the
            // header, because the bar's width changes with what it is offering
            // and the two used to collide at the minimum width.
            .child(
                div()
                    .absolute()
                    .right(px(12.))
                    .top(px(48.))
                    .flex()
                    .items_center()
                    .gap_1()
                    .px_1()
                    .py_1()
                    .rounded_lg()
                    .bg(cx.theme().secondary)
                    .border_1()
                    .border_color(cx.theme().border)
                    .child(
                        Button::new("compare-fit")
                            .ghost()
                            .small()
                            .label("Fit")
                            .selected(comparison.zoom.is_none())
                            .on_click(cx.listener(|audit, _, _, cx| {
                                if let Some(comparison) = audit.compare.as_mut() {
                                    comparison.zoom = None;
                                    comparison.pan = (0., 0.);
                                    cx.notify();
                                }
                            })),
                    )
                    .child(
                        Button::new("compare-actual")
                            .ghost()
                            .small()
                            .label("100%")
                            .selected(comparison.zoom == Some(1.))
                            .on_click(cx.listener(|audit, _, _, cx| {
                                if let Some(comparison) = audit.compare.as_mut() {
                                    comparison.zoom = Some(1.);
                                    comparison.pan = (0., 0.);
                                    cx.notify();
                                }
                            })),
                    ),
            )
            .children(
                comparison
                    .written
                    .is_some()
                    .then(|| self.result_strip(comparison, cx)),
            )
            .child(self.compare_bar(comparison, entry, view_w, cx))
            .into_any_element()
    }

    /// Every output the run wrote, along the foot. This is the part the app
    /// never had: a finished run reported a number and left you to go and find
    /// the files it was talking about.
    fn result_strip(&self, comparison: &Comparison, cx: &mut Context<Self>) -> impl IntoElement {
        let current = comparison.index;
        let rows = self.strip_rows(current);
        div()
            .debug_selector(|| "result-strip".into())
            .absolute()
            .left_0()
            .right_0()
            .bottom(px(74.))
            .flex()
            .justify_center()
            .child(
                div()
                    .id("result-strip-scroll")
                    .flex()
                    .items_center()
                    .gap_1()
                    .max_w(px(980.))
                    .px_2()
                    .py_2()
                    .rounded_lg()
                    .bg(cx.theme().secondary.opacity(0.92))
                    .border_1()
                    .border_color(cx.theme().border)
                    .overflow_x_scroll()
                    .children(rows.into_iter().map(|index| {
                        let selected = index == current;
                        let name = self
                            .entries
                            .get(index)
                            .map(|entry| entry.name())
                            .unwrap_or_default();
                        let saving = match (
                            self.entries.get(index).map(|entry| entry.bytes),
                            self.results.get(&index),
                        ) {
                            (Some(before), Some(after)) if before > 0 => {
                                let delta = before as f32 - *after as f32;
                                Some(delta / before as f32 * 100.)
                            }
                            _ => None,
                        };
                        div()
                            .id(("result-tile", index))
                            .flex()
                            .flex_col()
                            .gap_1()
                            .w(px(86.))
                            .flex_shrink_0()
                            .p_1()
                            .rounded_md()
                            .cursor_pointer()
                            .when(selected, |tile| {
                                tile.bg(cx.theme().list_active)
                                    .border_1()
                                    .border_color(cx.theme().list_active_border)
                            })
                            .when(!selected, |tile| {
                                tile.border_1()
                                    .border_color(gpui::transparent_black())
                                    .hover(|tile| tile.bg(cx.theme().list_hover))
                            })
                            .child(
                                div()
                                    .w_full()
                                    .h(px(48.))
                                    .flex()
                                    .items_center()
                                    .justify_center()
                                    .rounded_sm()
                                    .bg(cx.theme().background)
                                    .overflow_hidden()
                                    .when_some(self.thumbs.get(&index).cloned(), |slot, image| {
                                        slot.child(img(image).max_w_full().max_h(px(48.)))
                                    }),
                            )
                            .child(
                                div()
                                    .text_size(px(10.))
                                    .text_color(cx.theme().muted_foreground)
                                    .whitespace_nowrap()
                                    .overflow_hidden()
                                    .text_ellipsis()
                                    .child(name),
                            )
                            .children(saving.map(|percent| {
                                div()
                                    .font_family(cx.theme().mono_font_family.clone())
                                    .text_size(px(10.))
                                    .text_color(if percent >= 0. {
                                        cx.theme().green
                                    } else {
                                        cx.theme().yellow
                                    })
                                    .child(if percent >= 0. {
                                        format!("−{percent:.0}%")
                                    } else {
                                        format!("+{:.0}%", -percent)
                                    })
                            }))
                            .on_click(cx.listener(move |audit, _, _, cx| {
                                audit.open_result(index, cx);
                            }))
                    })),
            )
    }

    /// The media action bar. A source preview keeps the image actions close;
    /// a before/after comparison stays focused on reviewing and converting.
    fn compare_bar(
        &self,
        comparison: &Comparison,
        entry: Option<&Entry>,
        width: f32,
        cx: &mut Context<Self>,
    ) -> impl IntoElement {
        // The same thresholds the audit's bar uses: the bar has to fit the
        // window it floats over, and at the minimum width it does not.
        let labelled = width >= 900.;
        let index = comparison.index;
        let source_bytes = entry.map_or(0, |entry| entry.bytes);
        let busy = self.local_ai_busy() || self.studio_busy() || self.converting;
        let previewing = comparison.mode == MediaMode::Preview;
        let showing_result = comparison.written.is_some();
        let upscale_error =
            entry.and_then(|entry| local_ai::upscale_dimensions(entry.width, entry.height).err());

        // One readout, in this order of usefulness: a running job, then the
        // encoded result, then why there is not one yet.
        let running = if previewing {
            self.local_ai_job
                .as_ref()
                .filter(|job| job.busy())
                .map(|job| job.message(&self.root))
                .or_else(|| {
                    self.studio_job
                        .as_ref()
                        .filter(|job| job.busy())
                        .map(|job| job.message(&self.root))
                })
        } else {
            None
        };
        let (text, colour) = if let Some(message) = running {
            (message, cx.theme().muted_foreground)
        } else if previewing {
            match comparison.preview.as_ref() {
                Some(preview) => (
                    format!(
                        "{}×{} · {}",
                        preview.width,
                        preview.height,
                        format_bytes(source_bytes)
                    ),
                    cx.theme().foreground,
                ),
                None if comparison.failed => (
                    "Preview unavailable".to_string(),
                    cx.theme().muted_foreground,
                ),
                None => ("Loading preview…".to_string(), cx.theme().muted_foreground),
            }
        } else {
            match comparison.pair.as_ref() {
                Some(pair) => {
                    let saving = pair.saving_percent(source_bytes);
                    (
                        format!(
                            "{} → {} · {}",
                            format_bytes(source_bytes),
                            format_bytes(pair.converted_bytes),
                            if saving >= 0. {
                                format!("−{saving:.0}%")
                            } else {
                                format!("+{:.0}%", -saving)
                            }
                        ),
                        if saving >= 0. {
                            cx.theme().green
                        } else {
                            cx.theme().yellow
                        },
                    )
                }
                None if comparison.failed => (
                    "Preview unavailable".to_string(),
                    cx.theme().muted_foreground,
                ),
                None => ("Building preview…".to_string(), cx.theme().muted_foreground),
            }
        };

        div()
            .absolute()
            .left_0()
            .right_0()
            .bottom(px(16.))
            .flex()
            .justify_center()
            .child(
                div()
                    .debug_selector(|| "compare-bar".into())
                    .flex()
                    .items_center()
                    .gap_2()
                    .h(px(46.))
                    .px_2()
                    .rounded_lg()
                    .bg(cx.theme().secondary)
                    .border_1()
                    .border_color(cx.theme().border)
                    .shadow_lg()
                    .children((width >= 700.).then(|| {
                        div()
                            .pl_2()
                            .pr_1()
                            .font_family(cx.theme().mono_font_family.clone())
                            .text_size(px(12.))
                            .font_weight(FontWeight::MEDIUM)
                            .text_color(colour)
                            .whitespace_nowrap()
                            .child(text)
                    }))
                    .children(
                        (width >= 700.).then(|| div().w(px(1.)).h(px(20.)).bg(cx.theme().border)),
                    )
                    .children(previewing.then(|| {
                        Button::new("preview-compare")
                            .small()
                            .primary()
                            .label("Compare")
                            .tooltip("Build a before-and-after comparison")
                            .disabled(entry.is_none())
                            .on_click(cx.listener(move |audit, _, _, cx| {
                                audit.open_compare(index, cx);
                            }))
                    }))
                    .children(
                        (previewing && self.result_paths.contains_key(&index)).then(|| {
                            Button::new("preview-result")
                                .small()
                                .outline()
                                .label("See result")
                                .tooltip("Compare the existing output with its original")
                                .on_click(cx.listener(move |audit, _, _, cx| {
                                    audit.open_result(index, cx);
                                }))
                        }),
                    )
                    // A file that already exists wants opening, not converting
                    // again — and the folder is the thing you actually go to.
                    .children(comparison.produced_by.map(|_| {
                        Button::new("result-keep")
                            .small()
                            .primary()
                            .icon(IconName::Check)
                            .when(labelled, |button| button.label("Keep"))
                            .tooltip("Keep this file and go back to the audit")
                            .on_click(cx.listener(|audit, _, _, cx| {
                                audit.compare = None;
                                cx.notify();
                            }))
                    }))
                    .children(comparison.produced_by.map(|_| {
                        Button::new("result-discard")
                            .small()
                            .outline()
                            .icon(IconName::Close)
                            .when(labelled, |button| button.label("Discard"))
                            .tooltip("Delete this file and go back to the audit")
                            .on_click(cx.listener(|audit, _, _, cx| audit.discard_written(cx)))
                    }))
                    .children(
                        (showing_result && comparison.produced_by.is_none()).then(|| {
                            Button::new("result-show")
                                .small()
                                .primary()
                                .icon(IconName::FolderOpen)
                                .when(labelled, |button| button.label("Show in folder"))
                                .tooltip("Open the output folder in the file manager")
                                .on_click(cx.listener(|audit, _, _, _| audit.reveal_output()))
                        }),
                    )
                    .children(comparison.produced_by.is_none().then(|| {
                        Button::new("compare-convert")
                            .small()
                            .outline()
                            .icon(IconName::Replace)
                            .when(labelled, |button| {
                                button.label(if showing_result {
                                    "Convert again"
                                } else {
                                    "Convert this"
                                })
                            })
                            .tooltip("Convert this image with the current settings")
                            .disabled(busy || entry.is_none())
                            .on_click(cx.listener(move |audit, _, _, cx| {
                                audit.convert_one(index, cx);
                            }))
                    }))
                    .children(previewing.then(|| {
                        div()
                            .debug_selector(|| "preview-ai-actions".into())
                            .flex()
                            .items_center()
                            .gap_2()
                            // Not every platform ships the local models. A verb
                            // that cannot run here is absent, not disabled.
                            .children(local_ai::available().then(|| {
                                self.compare_local_ai(
                                    "compare-remove-background",
                                    IconName::Frame,
                                    "Remove background",
                                    local_ai::Tool::RemoveBackground,
                                    index,
                                    entry.is_none(),
                                    busy,
                                    labelled,
                                    cx,
                                )
                            }))
                            .children(local_ai::available().then(|| {
                                self.compare_local_ai(
                                    "compare-upscale",
                                    IconName::Maximize,
                                    "Upscale 4×",
                                    local_ai::Tool::Upscale,
                                    index,
                                    entry.is_none() || upscale_error.is_some(),
                                    busy,
                                    labelled,
                                    cx,
                                )
                            }))
                            .child(self.studio_button(
                                "compare-edit-studio",
                                entry.map(|_| index),
                                comparison.written.clone(),
                                labelled,
                                busy,
                                cx,
                            ))
                    })),
            )
    }

    /// One local model in the comparison bar. Same shape as the audit's, minus
    /// the selection rule: the image you are looking at is the target.
    #[allow(clippy::too_many_arguments)]
    fn compare_local_ai(
        &self,
        id: &'static str,
        icon: IconName,
        label: &'static str,
        tool: local_ai::Tool,
        index: usize,
        blocked: bool,
        busy: bool,
        labelled: bool,
        cx: &mut Context<Self>,
    ) -> impl IntoElement {
        let running = self
            .local_ai_job
            .as_ref()
            .is_some_and(|job| job.busy() && job.tool == tool);
        let tooltip = if busy && !running {
            self.local_ai_job
                .as_ref()
                .map(|job| job.message(&self.root))
                .or_else(|| self.studio_job.as_ref().map(|job| job.message(&self.root)))
                .unwrap_or_else(|| "Local AI is running…".into())
        } else {
            format!("{label} on this computer; the first run downloads the model")
        };
        Button::new(id)
            .small()
            .outline()
            .icon(icon)
            .when(labelled, |button| button.label(label))
            .tooltip(tooltip)
            .loading(running)
            .disabled(blocked || (busy && !running))
            .on_click(cx.listener(move |audit, _, _, cx| {
                audit.start_local_ai(tool, index, cx);
            }))
    }
}
