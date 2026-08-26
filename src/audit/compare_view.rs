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
        let compact = view_w < 900.;
        let entry = self.entries.get(comparison.index);
        let source_bytes = entry.map_or(0, |entry| entry.bytes);
        let name = entry.map(|entry| entry.name()).unwrap_or_default();
        let local_ai_available = local_ai::available();
        let can_previous = self.compare_target_from(comparison.index, -1).is_some();
        let can_next = self.compare_target_from(comparison.index, 1).is_some();
        let local_status = self.local_ai_job.as_ref().filter(|job| {
            job.index == comparison.index && job.dataset_generation == comparison.dataset_generation
        });

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
                    .when(!compact, |bar| {
                        bar.children(comparison.pair.as_ref().map(|pair| {
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
                                .child(compare_chip(saving_text.clone(), saving_colour, cx))
                        }))
                    })
                    .child({
                        // The preview exists to answer "is this good enough?" —
                        // yes should not require finding your way back to a
                        // button on another screen.
                        let target_count = self.target_count();
                        Button::new("compare-convert")
                            .primary()
                            .small()
                            .label(self.conversion_action_label())
                            .disabled(self.converting || target_count == 0)
                            .on_click(cx.listener(|audit, _, _, cx| {
                                // Back to the list, where the run's progress lives.
                                audit.compare = None;
                                audit.start_conversion(cx);
                            }))
                    })
                    .when(local_ai_available, |bar| bar.child({
                        let index = comparison.index;
                        let busy = self.local_ai_busy();
                        let running = self.local_ai_job.as_ref().is_some_and(|job| {
                            job.busy() && job.tool == local_ai::Tool::RemoveBackground
                        });
                        let tooltip = if busy {
                            self.local_ai_job
                                .as_ref()
                                .map(|job| job.message(&self.root))
                                .unwrap_or_else(|| "Local AI is running…".into())
                        } else {
                            "Remove the background locally with BiRefNet Lite; first use downloads the model".into()
                        };
                        Button::new("compare-remove-background")
                            .small()
                            .label(if compact {
                                "Remove BG"
                            } else {
                                "Remove background"
                            })
                            .tooltip(tooltip)
                            .disabled(entry.is_none() || (busy && !running))
                            .when(running, |button| button.icon(IconName::Loader))
                            .loading(running)
                            .on_click(cx.listener(move |audit, _, _, cx| {
                                audit.start_local_ai(
                                    local_ai::Tool::RemoveBackground,
                                    index,
                                    cx,
                                );
                            }))
                    }))
                    .when(local_ai_available, |bar| bar.child({
                        let index = comparison.index;
                        let busy = self.local_ai_busy();
                        let running = self
                            .local_ai_job
                            .as_ref()
                            .is_some_and(|job| job.busy() && job.tool == local_ai::Tool::Upscale);
                        let upscale_error = entry.and_then(|entry| {
                            local_ai::upscale_dimensions(entry.width, entry.height).err()
                        });
                        let tooltip = if busy {
                            self.local_ai_job
                                .as_ref()
                                .map(|job| job.message(&self.root))
                                .unwrap_or_else(|| "Local AI is running…".into())
                        } else if let Some(message) = upscale_error.as_ref() {
                            format!("{message}; use Sirv Studio for this image")
                        } else {
                            "Upscale 4× locally with Remacri ESRGAN; first use downloads the model".into()
                        };
                        Button::new("compare-upscale")
                            .small()
                            .label("Upscale 4×")
                            .tooltip(tooltip)
                            .disabled(entry.is_none() || upscale_error.is_some() || (busy && !running))
                            .when(running, |button| button.icon(IconName::Loader))
                            .loading(running)
                            .on_click(cx.listener(move |audit, _, _, cx| {
                                audit.start_local_ai(local_ai::Tool::Upscale, index, cx);
                            }))
                    }))
                    .child({
                        let studio = entry.map_or_else(
                            || Err("This image is no longer in the audit".to_string()),
                            |entry| self.studio_url_for(entry),
                        );
                        let (url, tooltip) = match studio {
                            Ok(url) => (
                                Some(url),
                                "Open this synced image in Sirv AI Studio".to_string(),
                            ),
                            Err(reason) => (None, reason),
                        };
                        let disabled = url.is_none();
                        Button::new("compare-edit-studio")
                            .small()
                            .icon(IconName::ExternalLink)
                            .label(if compact { "Studio" } else { "Edit in Studio" })
                            .tooltip(tooltip)
                            .disabled(disabled)
                            .on_click(cx.listener(move |_, _, _, cx| {
                                if let Some(url) = &url {
                                    cx.open_url(url);
                                }
                            }))
                    })
                    .child(
                        div().debug_selector(|| "compare-tools".into()).child(
                            Button::new("compare-tools")
                                .small()
                                .ghost()
                                .icon(IconName::Ellipsis)
                                .label("Tools")
                                .tooltip("Background removal, upscaling, and Sirv Studio")
                                .selected(comparison.tools_open)
                                .on_click(cx.listener(|audit, _, _, cx| {
                                    if let Some(comparison) = audit.compare.as_mut() {
                                        comparison.tools_open = !comparison.tools_open;
                                        cx.notify();
                                    }
                                })),
                        ),
                    )
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
            .when(comparison.tools_open, |stage| {
                stage.child(self.compare_tools_panel(comparison.index, entry, cx))
            })
            .child(
                div()
                    .absolute()
                    .left_0()
                    .right_0()
                    .bottom_0()
                    .flex()
                    .items_center()
                    .gap_2()
                    .px_3()
                    .py_2()
                    .bg(rgba(0x000000bf))
                    .text_size(px(12.))
                    .child({
                        let (text, colour, busy) = match local_status {
                            Some(job) => {
                                let colour = match job.state {
                                    LocalAiJobState::Done(_) => cx.theme().green,
                                    LocalAiJobState::Failed(_) => cx.theme().red,
                                    LocalAiJobState::SettingUp | LocalAiJobState::Running => {
                                        gpui::Hsla::from(rgba(0xffffffcc))
                                    }
                                };
                                (job.message(&self.root), colour, job.busy())
                            }
                            None => (
                                match (comparison.pair.as_ref(), scale) {
                                    (Some(pair), _) if compact => format!(
                                        "{} → {} · {}",
                                        format_bytes(source_bytes),
                                        format_bytes(pair.converted_bytes),
                                        saving_text
                                    ),
                                    (Some(pair), Some(_)) => format!(
                                        "{}×{} px",
                                        pair.width, pair.height
                                    ),
                                    (None, _) if comparison.failed => {
                                        "Preview unavailable".to_string()
                                    }
                                    _ => "Building preview…".to_string(),
                                },
                                gpui::Hsla::from(rgba(0xffffffcc)),
                                false,
                            ),
                        };
                        let tooltip = local_status.map(|_| text.clone());
                        div()
                            .flex_1()
                            .min_w_0()
                            .flex()
                            .items_center()
                            .gap_1()
                            .when(busy, |status| {
                                status.child(
                                    gpui_component::spinner::Spinner::new()
                                        .xsmall()
                                        .color(colour),
                                )
                            })
                            .child(
                                div()
                                    .id("compare-status-text")
                                    .flex_1()
                                    .min_w_0()
                                    .whitespace_nowrap()
                                    .overflow_hidden()
                                    .text_ellipsis()
                                    .text_color(colour)
                                    .when_some(tooltip, |status, tooltip| {
                                        status.tooltip(move |window, cx| {
                                            let tooltip = tooltip.clone();
                                            gpui_component::tooltip::Tooltip::element(move |_, _| {
                                                div()
                                                    .w(px(560.))
                                                    .whitespace_normal()
                                                    .child(tooltip.clone())
                                            })
                                            .build(window, cx)
                                        })
                                    })
                                    .child(text),
                            )
                    })
                    .when(
                        local_status
                            .is_some_and(|job| matches!(job.state, LocalAiJobState::Done(_))),
                        |bar| {
                            bar.child(
                                Button::new("compare-show-output")
                                    .ghost()
                                    .small()
                                    .icon(IconName::FolderOpen)
                                    .label("Show output")
                                    .tooltip("Open optimized/ in the file manager")
                                    .on_click(cx.listener(|audit, _, _, _| audit.reveal_output())),
                            )
                        },
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
                            )
                            .child(
                                Button::new("compare-prev")
                                    .ghost()
                                    .small()
                                    .icon(IconName::ArrowLeft)
                                    .label("Prev")
                                    .disabled(!can_previous)
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
                                    .disabled(!can_next)
                                    .on_click(cx.listener(|audit, _, _, cx| {
                                        audit.step_compare(1, cx);
                                    })),
                            )
                            .when(!compact && local_status.is_none(), |controls| {
                                controls.child(
                                    div()
                                        .ml_1()
                                        .text_color(rgba(0xffffffcc))
                                        .whitespace_nowrap()
                                        .child("Move divider to compare · scroll to zoom · drag to pan"),
                                )
                            }),
                    ),
            )
            .into_any_element()
    }

    fn compare_tools_panel(
        &self,
        index: usize,
        entry: Option<&Entry>,
        cx: &mut Context<Self>,
    ) -> impl IntoElement {
        let busy = self.local_ai_busy();
        let remove_running = self
            .local_ai_job
            .as_ref()
            .is_some_and(|job| job.busy() && job.tool == local_ai::Tool::RemoveBackground);
        let remove_tooltip = if busy {
            self.local_ai_job
                .as_ref()
                .map(|job| job.message(&self.root))
                .unwrap_or_else(|| "Local AI is running…".into())
        } else {
            "Remove the background locally with BiRefNet Lite; first use downloads the engine and model".into()
        };

        let upscale_running = self
            .local_ai_job
            .as_ref()
            .is_some_and(|job| job.busy() && job.tool == local_ai::Tool::Upscale);
        let upscale_error =
            entry.and_then(|entry| local_ai::upscale_dimensions(entry.width, entry.height).err());
        let upscale_tooltip = if busy {
            self.local_ai_job
                .as_ref()
                .map(|job| job.message(&self.root))
                .unwrap_or_else(|| "Local AI is running…".into())
        } else if let Some(message) = upscale_error.as_ref() {
            format!("{message}; use Sirv Studio for this image")
        } else {
            "Upscale 4× locally with Remacri ESRGAN; first use downloads the engine and model"
                .into()
        };

        let studio = entry.map_or_else(
            || Err("This image is no longer in the audit".to_string()),
            |entry| self.studio_url_for(entry),
        );
        let (studio_url, studio_tooltip) = match studio {
            Ok(url) => (
                Some(url),
                "Open this synced image in Sirv AI Studio".to_string(),
            ),
            Err(reason) => (None, reason),
        };
        let studio_disabled = studio_url.is_none();

        div()
            .id("compare-tools-panel")
            .debug_selector(|| "compare-tools-panel".into())
            .absolute()
            .top(px(44.))
            .right(px(44.))
            .w(px(240.))
            .flex()
            .flex_col()
            .gap_1()
            .p_2()
            .rounded_lg()
            .border_1()
            .border_color(cx.theme().border)
            .bg(cx.theme().secondary)
            .child(
                div()
                    .px_1()
                    .pb_1()
                    .text_size(px(11.))
                    .font_weight(FontWeight::SEMIBOLD)
                    .text_color(cx.theme().muted_foreground)
                    .child("IMAGE TOOLS"),
            )
            .child(
                Button::new("compare-remove-background")
                    .small()
                    .w_full()
                    .label("Remove background")
                    .tooltip(remove_tooltip)
                    .disabled(entry.is_none() || (busy && !remove_running))
                    .when(remove_running, |button| button.icon(IconName::Loader))
                    .loading(remove_running)
                    .on_click(cx.listener(move |audit, _, _, cx| {
                        if let Some(comparison) = audit.compare.as_mut() {
                            comparison.tools_open = false;
                        }
                        audit.start_local_ai(local_ai::Tool::RemoveBackground, index, cx);
                    })),
            )
            .child(
                Button::new("compare-upscale")
                    .small()
                    .w_full()
                    .label("Upscale 4×")
                    .tooltip(upscale_tooltip)
                    .disabled(
                        entry.is_none() || upscale_error.is_some() || (busy && !upscale_running),
                    )
                    .when(upscale_running, |button| button.icon(IconName::Loader))
                    .loading(upscale_running)
                    .on_click(cx.listener(move |audit, _, _, cx| {
                        if let Some(comparison) = audit.compare.as_mut() {
                            comparison.tools_open = false;
                        }
                        audit.start_local_ai(local_ai::Tool::Upscale, index, cx);
                    })),
            )
            .child(
                Button::new("compare-edit-studio")
                    .small()
                    .w_full()
                    .icon(IconName::ExternalLink)
                    .label("Edit in Sirv Studio")
                    .tooltip(studio_tooltip)
                    .disabled(studio_disabled)
                    .on_click(cx.listener(move |audit, _, _, cx| {
                        if let Some(comparison) = audit.compare.as_mut() {
                            comparison.tools_open = false;
                        }
                        if let Some(url) = &studio_url {
                            cx.open_url(url);
                        }
                    })),
            )
    }
}
