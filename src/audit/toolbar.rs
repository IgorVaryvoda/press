//! Toolbar control groups: format, resize, generic buttons.

use super::*;

/// One option in a segmented control.
///
pub(super) fn segment(
    id: impl Into<gpui_kit::ElementId>,
    label: impl Into<gpui_kit::SharedString>,
    selected: bool,
) -> Button {
    // The group's neutral selected state keeps `primary` for the conversion commit.
    Button::new(id).label(label).selected(selected)
}

impl Audit {
    /// The resize presets, as one segmented control. `ButtonGroup` reports the index
    /// that was clicked, so the options are listed once and read back by position.
    pub(super) fn resize_group(&self, cx: &mut Context<Self>) -> ButtonGroup {
        let options = MaxEdge::PRESETS;
        ButtonGroup::new("resize")
            .children(options.iter().map(|edge| {
                // "full" is the CLI's word and stays there; next to pixel
                // sizes under a "Max size" heading, the window's word for
                // no-limit is the source itself.
                let display = match edge.0 {
                    None => "Original".to_string(),
                    Some(value) => value.to_string(),
                };
                segment(
                    gpui_kit::SharedString::from(edge.label()),
                    display,
                    self.max_edge == *edge,
                )
                .disabled(self.converting)
            }))
            .on_click(cx.listener(move |audit, clicked: &Vec<usize>, window, cx| {
                if audit.converting {
                    return;
                }
                let Some(edge) = clicked.first().and_then(|index| options.get(*index)) else {
                    return;
                };
                audit.max_edge = *edge;
                audit.clear_custom_max_edge(window, cx);
                audit.clear_results();
                audit.schedule_estimate(cx);
                cx.notify();
            }))
    }

    /// The presets and, under them, the box for any other size: a theme that wants
    /// 1200px used to need the CLI.
    pub(super) fn resize_control(&self, cx: &mut Context<Self>) -> gpui_kit::AnyElement {
        div()
            .flex()
            .flex_col()
            .gap_1()
            .child(self.resize_group(cx).small().compact())
            .child(
                div()
                    .debug_selector(|| "max-edge-input".into())
                    .w(px(120.))
                    .child(
                        Input::new(&self.max_edge_input)
                            .small()
                            .disabled(self.converting)
                            .suffix(
                                div()
                                    .text_size(px(11.))
                                    .text_color(cx.theme().muted_foreground)
                                    .child("px"),
                            ),
                    ),
            )
            .into_any_element()
    }

    /// A typed size. Junk leaves the current size alone: a box half-way through
    /// "1200" reads 12, and 12px is not a size anyone asked for, but 0 or "abc" is
    /// not a reason to snap back to the source size either.
    pub(super) fn apply_custom_max_edge(&mut self, text: &str, cx: &mut Context<Self>) {
        if self.converting {
            return;
        }
        let Some(edge) = MaxEdge::parse(text) else {
            return;
        };
        if edge == self.max_edge {
            return;
        }
        self.max_edge = edge;
        self.clear_results();
        self.schedule_estimate(cx);
        cx.notify();
    }

    /// A preset click empties the box, or the number left behind would contradict
    /// the lit button.
    pub(super) fn clear_custom_max_edge(
        &mut self,
        window: &mut gpui_kit::Window,
        cx: &mut Context<Self>,
    ) {
        self.max_edge_input
            .update(cx, |input, cx| input.set_value("", window, cx));
    }

    pub(super) fn format_group(&self, cx: &mut Context<Self>) -> ButtonGroup {
        let options = [
            Format::WebP,
            Format::Avif,
            Format::JpegXl,
            Format::Jpeg,
            Format::Same,
        ];
        ButtonGroup::new("format")
            .children(options.iter().map(|format| {
                segment(format.label(), format.display(), self.format == *format)
                    .disabled(self.converting)
            }))
            .on_click(cx.listener(move |audit, clicked: &Vec<usize>, _, cx| {
                if audit.converting {
                    return;
                }
                let Some(format) = clicked.first().and_then(|index| options.get(*index)) else {
                    return;
                };
                audit.apply_format(*format, cx);
            }))
    }

    pub(super) fn apply_format(&mut self, format: Format, cx: &mut Context<Self>) {
        if self.converting || self.format == format {
            return;
        }
        self.format = format;
        // There is no lossless AVIF here: carrying the flag across would convert
        // lossy while every label says lossless.
        if !format.supports_lossless() && self.quality == Quality::LOSSLESS {
            self.quality = Quality::lossy(self.slider_quality);
        }
        self.clear_results();
        self.schedule_estimate(cx);
        cx.notify();
    }
}
