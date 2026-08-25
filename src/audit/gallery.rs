//! Gallery tiles.

use super::*;

impl Audit {
    /// One gallery tile: the picture, with its name and weight under it.
    pub(super) fn tile(
        &self,
        row: usize,
        index: usize,
        tile_size: f32,
        cx: &mut Context<Self>,
    ) -> gpui::Stateful<gpui::Div> {
        let Some(entry) = self.entries.get(index) else {
            return div().id(("tile", row));
        };
        let thumb = self.thumbs.get(&index).cloned();
        let ticked = self.selected.contains(&index);

        let density = entry.bytes_per_pixel();

        div()
            .id(("tile", row))
            .w(px(tile_size))
            .flex()
            .flex_col()
            .gap_2()
            .p_2()
            .rounded_lg()
            .bg(cx.theme().secondary)
            // Always bordered, in nothing, so arrowing onto a tile does not shunt
            // its contents a pixel down and right.
            .border_1()
            .border_color(gpui::transparent_black())
            .when(ticked, |tile| {
                tile.bg(cx.theme().list_active)
                    .border_color(cx.theme().list_active_border)
            })
            .when(row == self.cursor, |tile| {
                tile.border_color(cx.theme().ring)
            })
            .hover(|style| style.bg(cx.theme().list_hover))
            .on_click(cx.listener(move |audit, event: &gpui::ClickEvent, _, cx| {
                if let Some(position) = audit.row_of(index) {
                    audit.click_row(position, event, cx);
                }
            }))
            .child(
                div()
                    .relative()
                    .w_full()
                    .h(px(tile_size - 68.))
                    .flex()
                    .items_center()
                    .justify_center()
                    .rounded_md()
                    .overflow_hidden()
                    .bg(cx.theme().background)
                    .when_some(thumb, |slot, image| {
                        slot.child(
                            img(image)
                                .max_w(px(tile_size - 16.))
                                .max_h(px(tile_size - 68.)),
                        )
                    })
                    // The grid had no way to tick anything; the keyboard was the
                    // only route to a selection you could see in the list.
                    .child(
                        div()
                            .absolute()
                            .top(px(4.))
                            .left(px(4.))
                            .debug_selector(move || format!("grid-checkbox-{index}"))
                            .on_key_down(cx.listener(|_, event, _, cx| {
                                if is_checkbox_activation_key(event) {
                                    cx.stop_propagation();
                                }
                            }))
                            .child(
                                Checkbox::new(("tile-tick", index))
                                    .checked(ticked)
                                    .on_click(cx.listener(move |audit, _: &bool, _, cx| {
                                        cx.stop_propagation();
                                        if audit.converting {
                                            return;
                                        }
                                        if !audit.selected.remove(&index) {
                                            audit.selected.insert(index);
                                        }
                                        audit.selection_changed(cx);
                                    })),
                            ),
                    )
                    .child(
                        div().absolute().bottom(px(4.)).right(px(4.)).child(
                            div()
                                .px_1()
                                .rounded_sm()
                                .bg(cx.theme().background.opacity(0.8))
                                .text_size(px(10.))
                                .font_weight(FontWeight::MEDIUM)
                                .text_color(if entry.extension_lies() {
                                    cx.theme().yellow
                                } else {
                                    cx.theme().muted_foreground
                                })
                                .child(format_name(entry.format))
                                .when(entry.extension_lies(), |label| label.child(" ≠")),
                        ),
                    )
                    // The same word the table's Sirv column uses. The gallery used to
                    // show nothing at all, so switching to grid lost the diff.
                    .children(self.sync_label(entry, cx).map(|(label, colour)| {
                        div().absolute().top(px(4.)).right(px(4.)).child(
                            div()
                                .px_1()
                                .rounded_sm()
                                .bg(cx.theme().background.opacity(0.8))
                                .font_family(cx.theme().mono_font_family.clone())
                                .text_size(px(10.))
                                .text_color(colour)
                                .child(label),
                        )
                    })),
            )
            .child(
                div()
                    .w_full()
                    .overflow_hidden()
                    .text_ellipsis()
                    .whitespace_nowrap()
                    .text_size(px(12.))
                    .text_color(cx.theme().foreground)
                    .child(entry.name()),
            )
            .child(
                div()
                    .w_full()
                    .flex()
                    .items_center()
                    .justify_between()
                    .font_family(cx.theme().mono_font_family.clone())
                    .text_size(px(10.))
                    .child(div().text_color(cx.theme().muted_foreground).child(
                        match self.results.get(&index) {
                            Some(bytes) => {
                                format!("{} → {}", format_bytes(entry.bytes), format_bytes(*bytes))
                            }
                            None => format_bytes(entry.bytes),
                        },
                    ))
                    .child(
                        div()
                            .text_color(density_colour(density, cx))
                            .child(format!("{density:.2}")),
                    ),
            )
    }
}
