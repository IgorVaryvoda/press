//! Gallery tiles.

use super::toolbar::segment;
use super::*;
use table::finding_chip;

const GALLERY_SORTS: [(Column, &str); 5] = [
    (Column::Name, "Name"),
    (Column::Format, "Format"),
    (Column::Pixels, "Pixels"),
    (Column::Density, "Bytes/pixel"),
    (Column::Weight, "File size"),
];

impl Audit {
    pub(super) fn gallery_sort_bar(&self, cx: &mut Context<Self>) -> impl IntoElement {
        div()
            .debug_selector(|| "gallery-sort".into())
            .flex()
            .items_center()
            .gap_2()
            .px_2()
            .py_1()
            .border_b_1()
            .border_color(cx.theme().border)
            .child(
                div()
                    .text_size(px(11.))
                    .font_weight(FontWeight::SEMIBOLD)
                    .text_color(cx.theme().muted_foreground)
                    .child("SORT"),
            )
            .child(
                ButtonGroup::new("gallery-sort")
                    .small()
                    .compact()
                    .children(GALLERY_SORTS.iter().map(|(column, label)| {
                        let selected = self.sort.column == *column;
                        let label = if selected {
                            format!("{label} {}", if self.sort.descending { "↓" } else { "↑" })
                        } else {
                            (*label).to_string()
                        };
                        segment(format!("gallery-sort-{label}"), label, selected)
                            .disabled(self.converting)
                    }))
                    .on_click(cx.listener(|audit, clicked: &Vec<usize>, _, cx| {
                        if audit.converting {
                            return;
                        }
                        let Some((column, _)) =
                            clicked.first().and_then(|index| GALLERY_SORTS.get(*index))
                        else {
                            return;
                        };
                        audit.set_sort(*column, cx);
                    })),
            )
    }

    /// One gallery tile: the picture, with its name and weight under it.
    pub(super) fn tile(
        &self,
        row: usize,
        index: usize,
        tile_size: f32,
        cx: &mut Context<Self>,
    ) -> gpui::AnyElement {
        let Some(entry) = self.entries.get(index) else {
            return div().id(("tile", row)).into_any_element();
        };
        let thumb = self.thumbs.get(&index).cloned();
        let ticked = self.selected.contains(&index);
        let has_result = self.result_paths.contains_key(&index);
        let busy = self.media_commit_actions_disabled();
        let audit = cx.weak_entity();
        let selection_bounds = self.selection_bounds.clone();

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
                tile.border_color(cx.theme().muted_foreground)
            })
            .on_prepaint(move |bounds, _, _| {
                selection_bounds.borrow_mut().insert(index, bounds);
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
                            .p_0p5()
                            .rounded_sm()
                            .bg(cx.theme().background.opacity(0.9))
                            .border_1()
                            .border_color(cx.theme().border)
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
                    .when(entry.extension_lies(), |slot| {
                        slot.child(
                            div().absolute().bottom(px(4.)).left(px(4.)).child(
                                div()
                                    .px_1()
                                    .rounded_sm()
                                    .bg(cx.theme().background.opacity(0.9))
                                    .text_size(px(10.))
                                    .font_weight(FontWeight::MEDIUM)
                                    .text_color(cx.theme().yellow)
                                    .child(format!(
                                        "actual {} ≠ extension",
                                        format_name(entry.format)
                                    )),
                            ),
                        )
                    })
                    .child(
                        div()
                            .absolute()
                            .bottom(px(4.))
                            .right(px(4.))
                            .debug_selector(move || format!("grid-compare-{index}"))
                            .child(
                                Button::new(("tile-compare", index))
                                    .small()
                                    .label("Compare")
                                    .tooltip("Open the before-and-after comparison")
                                    .disabled(self.converting)
                                    .on_click(cx.listener(move |audit, _, _, cx| {
                                        cx.stop_propagation();
                                        if !audit.converting {
                                            audit.open_compare(index, cx);
                                        }
                                    })),
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
                    .gap_1()
                    .font_family(cx.theme().mono_font_family.clone())
                    .text_size(px(10.))
                    .child(
                        div()
                            .flex_1()
                            .min_w_0()
                            .whitespace_nowrap()
                            .overflow_hidden()
                            .text_ellipsis()
                            .text_color(cx.theme().muted_foreground)
                            .child(format!(
                                "{}×{} · {}",
                                entry.width,
                                entry.height,
                                match self.results.get(&index) {
                                    Some(bytes) => format!(
                                        "{} → {}",
                                        format_bytes(entry.bytes),
                                        format_bytes(*bytes)
                                    ),
                                    None => format_bytes(entry.bytes),
                                }
                            )),
                    )
                    // The same word the list uses. A tile showing `0.14 B/px`
                    // asked you to know the bands by heart, and it was taking
                    // the room the file size needed to print in full.
                    .children(
                        Finding::Heavy
                            .holds(entry)
                            .then(|| finding_chip("heavy", cx)),
                    )
                    .children(
                        entry
                            .extension_lies()
                            .then(|| finding_chip("mislabelled", cx)),
                    ),
            )
            .context_menu(move |menu, _, _| {
                media::image_context_menu(audit.clone(), index, has_result, busy, menu)
            })
            .into_any_element()
    }
}
