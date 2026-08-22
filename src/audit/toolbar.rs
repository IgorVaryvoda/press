//! Toolbar control groups: format, resize, generic buttons.

use super::*;

/// One option in a segmented control.
///
pub(super) fn segment(
    id: impl Into<gpui::ElementId>,
    label: impl Into<gpui::SharedString>,
    selected: bool,
) -> Button {
    // The group's neutral selected state keeps `primary` for the conversion commit.
    Button::new(id).label(label).selected(selected)
}

impl Audit {
    /// Several exclusive options as one control, under the word for what they choose.
    /// The old toolbar was thirteen identical ghost buttons in a row with a 12px gap
    /// standing in for grouping, and nothing said which was which.
    pub(super) fn control_group(
        &self,
        label: &'static str,
        group: ButtonGroup,
        cx: &mut Context<Self>,
    ) -> impl IntoElement {
        // The word sits left of its control, in the same muted voice as the
        // rest of the strip's metadata.
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
                    .child(label),
            )
            .child(group.small().compact())
    }

    /// The resize presets, as one segmented control. `ButtonGroup` reports the index
    /// that was clicked, so the options are listed once and read back by position.
    pub(super) fn resize_group(&self, cx: &mut Context<Self>) -> ButtonGroup {
        let options = MaxEdge::PRESETS;
        ButtonGroup::new("resize")
            .children(options.iter().map(|edge| {
                segment(
                    gpui::SharedString::from(edge.label()),
                    edge.label(),
                    self.max_edge == *edge,
                )
                .disabled(self.converting)
            }))
            .on_click(cx.listener(move |audit, clicked: &Vec<usize>, _, cx| {
                if audit.converting {
                    return;
                }
                let Some(edge) = clicked.first().and_then(|index| options.get(*index)) else {
                    return;
                };
                audit.max_edge = *edge;
                audit.results.clear();
                audit.schedule_estimate(cx);
                cx.notify();
            }))
    }

    pub(super) fn format_group(&self, cx: &mut Context<Self>) -> ButtonGroup {
        let options = [Format::WebP, Format::Avif];
        ButtonGroup::new("format")
            .children(options.iter().map(|format| {
                segment(
                    format.label(),
                    format.label().to_uppercase(),
                    self.format == *format,
                )
                .disabled(self.converting)
            }))
            .on_click(cx.listener(move |audit, clicked: &Vec<usize>, _, cx| {
                if audit.converting {
                    return;
                }
                let Some(format) = clicked.first().and_then(|index| options.get(*index)) else {
                    return;
                };
                audit.format = *format;
                // Results describe the old format; keeping them would mislabel them.
                audit.results.clear();
                audit.schedule_estimate(cx);
                cx.notify();
            }))
    }

    pub(super) fn toolbar_button(
        &self,
        id: &'static str,
        text: &'static str,
        tooltip: &'static str,
        icon: IconName,
        cx: &mut Context<Self>,
        on_click: impl Fn(&mut Self, &mut Context<Self>) + 'static,
    ) -> Button {
        Button::new(id)
            .small()
            .icon(icon)
            .label(text)
            .tooltip(tooltip)
            .on_click(cx.listener(move |audit, _, _, cx| on_click(audit, cx)))
    }
}
