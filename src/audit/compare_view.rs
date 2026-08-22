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
        let entry = self.entries.get(comparison.index);
        let source_bytes = entry.map_or(0, |entry| entry.bytes);
        let name = entry.map(|entry| entry.name()).unwrap_or_default();

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
                    let after = (before * 1.2f32.powf(ticks)).clamp(0.02, 16.);

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
                            .absolute()
                            .top_0()
                            .left(px(divider - 1.))
                            .w(px(2.))
                            .h_full()
                            .bg(rgba(0xffffffcc)),
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

        // Chrome as two full-width bars. These were black boxes pinned at
        // hand-computed offsets, the right-hand one at `view_w - 240` — a number
        // that stopped being the right edge the moment the text or window changed.
        let (saving_text, saving_colour) = match comparison.pair.as_ref() {
            Some(pair) => {
                let saving = pair.saving_percent(source_bytes);
                if saving >= 0. {
                    (format!("−{saving:.0}%"), cx.theme().green)
                } else {
                    (format!("+{:.0}%", -saving), cx.theme().yellow)
                }
            }
            None => (String::new(), cx.theme().green),
        };

        stage
            .child(
                div()
                    .absolute()
                    .left_0()
                    .right_0()
                    .top_0()
                    .flex()
                    .items_center()
                    .gap_3()
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
                    .children(comparison.pair.as_ref().map(|pair| {
                        div()
                            .flex()
                            .items_center()
                            .gap_2()
                            .flex_shrink_0()
                            .text_color(cx.theme().muted_foreground)
                            .whitespace_nowrap()
                            .child(format!(
                                "{} → {} {} · {}",
                                format_bytes(source_bytes),
                                self.format.label().to_uppercase(),
                                self.quality.label(),
                                format_bytes(pair.converted_bytes)
                            ))
                            .child(compare_chip(saving_text, saving_colour, cx))
                    }))
                    .child(
                        Button::new("compare-close")
                            .small()
                            .icon(IconName::Close)
                            .tooltip("Back to the audit")
                            .on_click(cx.listener(|audit, _, _, cx| {
                                audit.compare = None;
                                cx.notify();
                            })),
                    ),
            )
            .child(
                div()
                    .absolute()
                    .left_0()
                    .right_0()
                    .bottom_0()
                    .flex()
                    .flex_wrap()
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
                            .text_color(rgba(0xffffffcc))
                            .child(match (comparison.pair.as_ref(), scale) {
                                (Some(pair), Some(scale)) => {
                                    format!("{}×{} · {:.0}%", pair.width, pair.height, scale * 100.)
                                }
                                (None, _) if comparison.failed => "Preview unavailable".to_string(),
                                _ => "decoding…".to_string(),
                            }),
                    )
                    .child(
                        div()
                            .flex()
                            .items_center()
                            .gap_1()
                            .flex_shrink_0()
                            .child(
                                Button::new("compare-fit")
                                    .ghost()
                                    .small()
                                    .label("Fit")
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
                                    .on_click(cx.listener(|audit, _, _, cx| {
                                        if let Some(comparison) = audit.compare.as_mut() {
                                            comparison.zoom = Some(1.);
                                            comparison.pan = (0., 0.);
                                            cx.notify();
                                        }
                                    })),
                            )
                            .child(
                                Button::new("compare-prev")
                                    .ghost()
                                    .small()
                                    .icon(IconName::ArrowLeft)
                                    .label("Prev")
                                    .on_click(cx.listener(|audit, _, _, cx| {
                                        audit.step_compare(-1, cx);
                                    })),
                            )
                            .child(
                                Button::new("compare-next")
                                    .ghost()
                                    .small()
                                    .icon(IconName::ArrowRight)
                                    .label("Next")
                                    .on_click(cx.listener(|audit, _, _, cx| {
                                        audit.step_compare(1, cx);
                                    })),
                            )
                            .child(
                                div()
                                    .ml_1()
                                    .text_color(rgba(0xffffffcc))
                                    .whitespace_nowrap()
                                    .child("Scroll to zoom · drag to pan"),
                            ),
                    ),
            )
            .into_any_element()
    }
}
