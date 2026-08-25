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
                    Some(_) => edge.label(),
                };
                segment(
                    gpui::SharedString::from(edge.label()),
                    display,
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
                audit.clear_results();
                audit.schedule_estimate(cx);
                cx.notify();
            }))
    }

    pub(super) fn format_picker(&self) -> Select<Vec<Format>> {
        Select::new(&self.format_select)
            .small()
            .w_full()
            .disabled(self.converting)
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
