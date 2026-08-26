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
        let position = match self.row_of(comparison.index) {
            Some(row) => format!("{} of {}", row + 1, self.visible.len()),
            None => String::new(),
        };
        let can_previous = self.compare_target_from(comparison.index, -1).is_some();
        let can_next = self.compare_target_from(comparison.index, 1).is_some();

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
                    let Some(pair) = comparison.pair.as_ref() else {
                        return;
                    };

                    let ticks = match event.delta {
                        gpui::ScrollDelta::Lines(delta) => delta.y,
                        gpui::ScrollDelta::Pixels(delta) => f32::from(delta.y) / 40.,
                    };
                    if ticks == 0. {
                        return;
                    }

                    let fit = (view_w / pair.width as f32)
                        .min(view_h / pair.height as f32)
                        .min(1.);
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
                        // Free: the divider tracks the pointer.
                        None => comparison.split = (at.0 / view_w).clamp(0., 1.),
                    }
                    cx.notify();
                }),
            );

        // Fit never scales up: a 400px thumbnail blown across a 4K window is just a
        // blurry 400px thumbnail. Computed before the branch because the chrome
        // reports the zoom as well as the image using it.
        let scale = comparison.pair.as_ref().map(|pair| {
            let fit = (view_w / pair.width as f32)
                .min(view_h / pair.height as f32)
                .min(1.);
            comparison.zoom.unwrap_or(fit)
        });

        if let (Some(pair), Some(scale)) = (comparison.pair.as_ref(), scale) {
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

            stage =
                stage
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
                    .child(div().absolute().top(px(48.)).left(px(divider + 12.)).child(
                        compare_chip(self.format.label().to_uppercase(), cx.theme().green, cx),
                    ));
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
                            "Could not build a comparison preview for this image.",
                        )
                        .max_w(px(420.)),
                    ),
            );
        }

        // The pair decodes and encodes in the background; until it lands the
        // stage used to be a black void with six grey letters in the footer.
        // A build in progress should look like one.
        if comparison.pair.is_none() && !comparison.failed {
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
                            .child(format!(
                                "Building preview — encoding to {} {}…",
                                self.format.label().to_uppercase(),
                                self.quality.label()
                            )),
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
            // Zoom, in its own cluster out of the way. It changes how you look
            // at the image and nothing about the file.
            .child(
                div()
                    .absolute()
                    .right(px(16.))
                    .bottom(px(16.))
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
            .child(self.compare_bar(comparison, entry, cx))
            .into_any_element()
    }

    /// The comparison's action bar: what this image costs now and converted,
    /// then the same four verbs the audit's bar carries. They act on the image
    /// you are looking at, so none of them needs a selection rule.
    fn compare_bar(
        &self,
        comparison: &Comparison,
        entry: Option<&Entry>,
        cx: &mut Context<Self>,
    ) -> impl IntoElement {
        let index = comparison.index;
        let source_bytes = entry.map_or(0, |entry| entry.bytes);
        let busy = self.local_ai_busy() || self.converting;
        let studio = entry.map_or_else(
            || Err("This image is no longer in the audit".to_string()),
            |entry| self.studio_url_for(entry),
        );
        let upscale_error =
            entry.and_then(|entry| local_ai::upscale_dimensions(entry.width, entry.height).err());

        // One readout, in this order of usefulness: a running job, then the
        // encoded result, then why there is not one yet.
        let (text, colour) = match (self.local_ai_job.as_ref(), comparison.pair.as_ref()) {
            (Some(job), _) if job.busy() => (job.message(&self.root), cx.theme().muted_foreground),
            (_, Some(pair)) => {
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
            (_, None) if comparison.failed => (
                "Preview unavailable".to_string(),
                cx.theme().muted_foreground,
            ),
            (_, None) => ("Building preview…".to_string(), cx.theme().muted_foreground),
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
                    .child(
                        div()
                            .pl_2()
                            .pr_1()
                            .font_family(cx.theme().mono_font_family.clone())
                            .text_size(px(12.))
                            .font_weight(FontWeight::MEDIUM)
                            .text_color(colour)
                            .whitespace_nowrap()
                            .child(text),
                    )
                    .child(div().w(px(1.)).h(px(20.)).bg(cx.theme().border))
                    .child(
                        Button::new("compare-convert")
                            .small()
                            .outline()
                            .icon(IconName::Replace)
                            .label("Convert this")
                            .tooltip("Convert this image with the current settings")
                            .disabled(busy || entry.is_none())
                            .on_click(cx.listener(move |audit, _, _, cx| {
                                // The run's progress lives in the list, and the
                                // list is where the result lands.
                                audit.compare = None;
                                audit.selected.clear();
                                if let Some(entry) = audit.entries.get(index).map(|_| index) {
                                    audit.selected.insert(entry);
                                }
                                audit.selection_changed(cx);
                                audit.start_conversion(cx);
                            })),
                    )
                    // Not every platform ships the local models. A verb that
                    // cannot run anywhere on this machine is not disabled, it
                    // is absent.
                    .children(local_ai::available().then(|| {
                        self.compare_local_ai(
                            "compare-remove-background",
                            IconName::Frame,
                            "Remove background",
                            local_ai::Tool::RemoveBackground,
                            index,
                            entry.is_none(),
                            busy,
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
                            cx,
                        )
                    }))
                    .child({
                        let url = studio.as_ref().ok().cloned();
                        Button::new("compare-edit-studio")
                            .small()
                            .outline()
                            .icon(IconName::ExternalLink)
                            .label("Edit in Studio")
                            .tooltip(match &studio {
                                Ok(_) => "Open this synced image in Sirv AI Studio".to_string(),
                                Err(reason) => reason.clone(),
                            })
                            .disabled(url.is_none() || busy)
                            .on_click(cx.listener(move |_, _, _, cx| {
                                if let Some(url) = &url {
                                    cx.open_url(url);
                                }
                            }))
                    }),
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
                .unwrap_or_else(|| "Local AI is running…".into())
        } else {
            format!("{label} on this computer; the first run downloads the model")
        };
        Button::new(id)
            .small()
            .outline()
            .icon(icon)
            .label(label)
            .tooltip(tooltip)
            .loading(running)
            .disabled(blocked || (busy && !running))
            .on_click(cx.listener(move |audit, _, _, cx| {
                audit.start_local_ai(tool, index, cx);
            }))
    }
}
