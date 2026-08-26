use super::*;

/// Rows are for scanning a folder of thousands, so they are sized to fit as many
/// as possible while still showing a thumbnail you can recognise.
const ROW_HEIGHT: f32 = 40.;
const THUMB_SLOT: f32 = 34.;

// ── Column widths ───────────────────────────────────────────────────────────
// One constant per column, shared by the header and every row. They used to be
// written twice and had already drifted; a header that sits over the wrong column
// is worse than no header.
const W_TICK: f32 = 34.;
const W_FORMAT: f32 = 82.;
const W_PIXELS: f32 = 96.;
const W_DENSITY: f32 = 74.;
const W_WEIGHT: f32 = 100.;
const W_RESULT: f32 = 112.;
const W_RESULT_NARROW: f32 = 152.;
/// The Sirv diff column. Wide windows only: below 900px the name needs the
/// room more than the status does.
const W_SYNC: f32 = 86.;
const W_FORMAT_COMPACT: f32 = 70.;
const W_PIXELS_COMPACT: f32 = 88.;
const W_DENSITY_COMPACT: f32 = 60.;
const W_WEIGHT_COMPACT: f32 = 86.;
pub(super) const W_NAME_MIN: f32 = 140.;

pub(super) fn result_size_text(before: u64, after: u64, narrow: bool) -> String {
    if narrow {
        format!("{} → {}", format_bytes(before), format_bytes(after))
    } else {
        format_bytes(after)
    }
}

/// The audit list, as the component library's virtualised table.
///
/// It was a `uniform_list` with the header, the column widths, the sort arrows and
/// the hit testing all written by hand and kept in step by hand. The delegate hands
/// all of that to the library, which is also where column resizing and dragging come
/// from for free.
pub(super) struct AuditTable {
    /// Weak, because the audit owns the table state, which owns this.
    audit: gpui::WeakEntity<Audit>,
    columns: Vec<TableColumn>,
    /// Width for the name column, recomputed from the window so the fixed columns
    /// do not leave an empty strip on the right. Columns here take a width, not a
    /// share, so somebody has to do the arithmetic.
    name_width: f32,
    compact: bool,
}

/// The columns, in display order. `Column` is what the audit sorts by; this adds the
/// ones that carry no sortable value of their own.
#[derive(Clone, Copy, PartialEq, Eq)]
pub(super) enum TableColumn {
    Tick,
    Thumb,
    Name,
    Format,
    Pixels,
    Density,
    Weight,
    Sync,
    Result,
}

impl TableColumn {
    /// The audit column this sorts by, if it sorts.
    fn sorts_by(&self) -> Option<Column> {
        match self {
            TableColumn::Name => Some(Column::Name),
            TableColumn::Format => Some(Column::Format),
            TableColumn::Pixels => Some(Column::Pixels),
            TableColumn::Density => Some(Column::Density),
            TableColumn::Weight => Some(Column::Weight),
            _ => None,
        }
    }

    fn spec(&self, name_width: f32, compact: bool, narrow: bool) -> TableCol {
        match self {
            TableColumn::Tick => TableCol::new("tick", "").width(px(W_TICK)),
            TableColumn::Thumb => TableCol::new("thumb", "").width(px(THUMB_SLOT + 12.)).p_0(),
            // Name takes whatever the other columns leave, so the window has no dead
            // strip down its right-hand side.
            TableColumn::Name => TableCol::new("name", "Name")
                .width(px(name_width))
                .min_width(px(W_NAME_MIN))
                .sortable()
                .resizable(true),
            TableColumn::Format => TableCol::new("format", "Format")
                .width(px(if compact { W_FORMAT_COMPACT } else { W_FORMAT }))
                .sortable(),
            TableColumn::Pixels => TableCol::new("pixels", "Size")
                .width(px(if compact { W_PIXELS_COMPACT } else { W_PIXELS }))
                .text_right()
                .sortable(),
            TableColumn::Density => TableCol::new("density", "B/px")
                .width(px(if compact {
                    W_DENSITY_COMPACT
                } else {
                    W_DENSITY
                }))
                .text_right()
                .sortable(),
            TableColumn::Weight => TableCol::new("weight", "File size")
                .width(px(if compact { W_WEIGHT_COMPACT } else { W_WEIGHT }))
                .text_right()
                .sortable(),
            TableColumn::Sync => TableCol::new("sirv", "Sirv").width(px(W_SYNC)),
            TableColumn::Result => {
                TableCol::new("result", if narrow { "Before → after" } else { "Result" })
                    .width(px(if narrow { W_RESULT_NARROW } else { W_RESULT }))
                    .text_right()
            }
        }
    }
}

impl AuditTable {
    /// Chrome the table spends on gaps, cell padding and its own border.
    const CHROME: f32 = 30.;

    fn fixed_width(compact: bool, show_result: bool, show_sync: bool) -> f32 {
        W_TICK
            + THUMB_SLOT
            + 12.
            + if compact { W_FORMAT_COMPACT } else { W_FORMAT }
            + if compact { W_PIXELS_COMPACT } else { W_PIXELS }
            + if compact {
                W_DENSITY_COMPACT
            } else {
                W_DENSITY
            }
            + if compact { W_WEIGHT_COMPACT } else { W_WEIGHT }
            + if compact || !show_sync { 0. } else { W_SYNC }
            + if show_result { W_RESULT } else { 0. }
    }

    pub(super) fn layout(
        width: f32,
        show_result: bool,
        show_sync: bool,
    ) -> (bool, f32, Vec<TableColumn>) {
        let compact = width < 900.;
        let narrow =
            width < Self::fixed_width(compact, show_result, show_sync) + Self::CHROME + W_NAME_MIN;
        let mut columns = if narrow {
            let mut columns = vec![
                TableColumn::Tick,
                TableColumn::Thumb,
                TableColumn::Name,
                TableColumn::Density,
            ];
            columns.push(if show_result {
                TableColumn::Result
            } else {
                TableColumn::Weight
            });
            columns
        } else {
            vec![
                TableColumn::Tick,
                TableColumn::Thumb,
                TableColumn::Name,
                TableColumn::Format,
                TableColumn::Pixels,
                TableColumn::Density,
                TableColumn::Weight,
            ]
        };
        if show_sync && !compact && !narrow {
            columns.push(TableColumn::Sync);
        }
        if show_result && !narrow {
            columns.push(TableColumn::Result);
        }
        let fixed_width = if narrow {
            W_TICK
                + THUMB_SLOT
                + 12.
                + W_DENSITY_COMPACT
                + if show_result {
                    W_RESULT_NARROW
                } else {
                    W_WEIGHT_COMPACT
                }
        } else {
            Self::fixed_width(compact, show_result, show_sync)
        };
        let name_width = (width - fixed_width - Self::CHROME).max(W_NAME_MIN);
        (compact, name_width, columns)
    }

    pub(super) fn new(audit: gpui::WeakEntity<Audit>, window: &Window) -> Self {
        let mut table = Self {
            audit,
            name_width: W_NAME_MIN,
            compact: false,
            columns: Vec::new(),
        };
        table.set_viewport_width(f32::from(window.viewport_size().width), false, false);
        table
    }

    pub(super) fn set_viewport_width(&mut self, width: f32, show_result: bool, show_sync: bool) {
        (self.compact, self.name_width, self.columns) = Self::layout(width, show_result, show_sync);
    }
}

impl TableDelegate for AuditTable {
    fn columns_count(&self, _cx: &App) -> usize {
        self.columns.len()
    }

    fn rows_count(&self, cx: &App) -> usize {
        self.audit
            .upgrade()
            .map_or(0, |audit| audit.read(cx).visible.len())
    }

    fn column(&self, col_ix: usize, cx: &App) -> TableCol {
        let Some(column) = self.columns.get(col_ix) else {
            return TableCol::new("none", "");
        };
        let narrow = !self.columns.contains(&TableColumn::Format);
        let mut spec = column.spec(self.name_width, self.compact, narrow);
        // Show the arrow on whichever column the audit is actually ordered by.
        if let Some(sort) = column.sorts_by()
            && let Some(audit) = self.audit.upgrade()
        {
            let audit = audit.read(cx);
            if audit.sort.column == sort {
                spec = if audit.sort.descending {
                    spec.descending()
                } else {
                    spec.ascending()
                };
            }
        }
        spec
    }

    fn perform_sort(
        &mut self,
        col_ix: usize,
        _sort: ColumnSort,
        _window: &mut Window,
        cx: &mut Context<TableState<Self>>,
    ) {
        let Some(column) = self.columns.get(col_ix).and_then(TableColumn::sorts_by) else {
            return;
        };
        let Some(audit) = self.audit.upgrade() else {
            return;
        };
        audit.update(cx, |audit, cx| audit.set_sort(column, cx));
    }

    /// The tick header selects everything the filter is showing. Everything else
    /// is its column's name.
    fn render_th(
        &mut self,
        col_ix: usize,
        _window: &mut Window,
        cx: &mut Context<TableState<Self>>,
    ) -> impl IntoElement {
        let column = self.columns.get(col_ix).copied();
        let Some(audit) = self.audit.upgrade() else {
            return div().into_any_element();
        };
        match column {
            Some(TableColumn::Tick) => {
                let state = audit.read(cx).selection_state();
                div()
                    .h_full()
                    .flex()
                    .items_center()
                    .debug_selector(|| "table-select-all".to_string())
                    .on_key_down(cx.listener(|_, event, _, cx| {
                        if is_checkbox_activation_key(event) {
                            cx.stop_propagation();
                        }
                    }))
                    .child(
                        Checkbox::new("select-all")
                            .checked(state == SelectionState::All)
                            .tooltip("Select every image the filter is showing")
                            .on_click(cx.listener(|table, _: &bool, _, cx| {
                                cx.stop_propagation();
                                let Some(audit) = table.delegate().audit.upgrade() else {
                                    return;
                                };
                                audit.update(cx, |audit, cx| audit.toggle_select_all(cx));
                            })),
                    )
                    .into_any_element()
            }
            _ => div()
                .size_full()
                .child(self.column(col_ix, cx).name.clone())
                .into_any_element(),
        }
    }

    fn render_tr(
        &mut self,
        row_ix: usize,
        _window: &mut Window,
        cx: &mut Context<TableState<Self>>,
    ) -> gpui::Stateful<gpui::Div> {
        let row = div().id(("row", row_ix));
        let Some(audit) = self.audit.upgrade() else {
            return row;
        };
        let audit_state = audit.read(cx);
        let ticked = audit_state
            .entry_at(row_ix)
            .is_some_and(|entry| audit_state.selected.contains(&entry));
        let cursor = audit_state.cursor;

        row.h(px(ROW_HEIGHT))
            .relative()
            .border_1()
            .border_color(gpui::transparent_black())
            .when(ticked, |row| row.bg(cx.theme().list_active))
            .when(row_ix == cursor, |row| {
                row.border_color(cx.theme().muted_foreground)
            })
            .on_click(cx.listener(move |table, event: &gpui::ClickEvent, _, cx| {
                let Some(audit) = table.delegate().audit.upgrade() else {
                    return;
                };
                audit.update(cx, |audit, cx| audit.click_row(row_ix, event, cx));
            }))
    }

    fn render_td(
        &mut self,
        row_ix: usize,
        col_ix: usize,
        _window: &mut Window,
        cx: &mut Context<TableState<Self>>,
    ) -> impl IntoElement {
        let Some(column) = self.columns.get(col_ix).copied() else {
            return div().into_any_element();
        };
        let narrow_result =
            column == TableColumn::Result && !self.columns.contains(&TableColumn::Weight);
        let Some(handle) = self.audit.upgrade() else {
            return div().into_any_element();
        };

        // Ask once per row. This delegate is called once per visible cell, so
        // putting the request above the match entered the same mutable path for
        // every column even though only this column can draw the image.
        if column == TableColumn::Thumb {
            handle.update(cx, |audit, cx| {
                if let Some(entry) = audit.entry_at(row_ix) {
                    audit.request_thumb(entry, cx);
                }
            });
        }

        let audit = handle.read(cx);
        let Some(index) = audit.entry_at(row_ix) else {
            return div().into_any_element();
        };
        let Some(entry) = audit.entries.get(index) else {
            return div().into_any_element();
        };

        match column {
            TableColumn::Tick => {
                let ticked = audit.selected.contains(&index);
                div()
                    .h_full()
                    .flex()
                    .items_center()
                    .debug_selector(move || format!("table-checkbox-{index}"))
                    .on_key_down(cx.listener(|_, event, _, cx| {
                        if is_checkbox_activation_key(event) {
                            cx.stop_propagation();
                        }
                    }))
                    .child(
                        Checkbox::new(("tick", index))
                            .checked(ticked)
                            .on_click(cx.listener(move |table, _: &bool, _, cx| {
                                cx.stop_propagation();
                                let Some(audit) = table.delegate().audit.upgrade() else {
                                    return;
                                };
                                audit.update(cx, |audit, cx| {
                                    if audit.converting {
                                        return;
                                    }
                                    if !audit.selected.remove(&index) {
                                        audit.selected.insert(index);
                                    }
                                    audit.selection_changed(cx);
                                });
                            })),
                    )
                    .into_any_element()
            }
            TableColumn::Thumb => div()
                .w(px(THUMB_SLOT))
                .h(px(THUMB_SLOT))
                .flex()
                .items_center()
                .justify_center()
                .rounded_sm()
                .bg(cx.theme().background)
                // A fixed slot, so rows do not jump as thumbnails arrive.
                .when_some(audit.thumbs.get(&index).cloned(), |slot, image| {
                    slot.child(img(image).max_w(px(THUMB_SLOT)).max_h(px(THUMB_SLOT)))
                })
                .into_any_element(),
            TableColumn::Name => {
                // The finding, in the row that has it. `heavy` used to be a number
                // in a column you had to know how to read; a file can be both
                // heavy and mislabelled, and both are worth saying.
                let heavy = entry.bytes_per_pixel() > DENSITY_HEAVY;
                let lies = entry.extension_lies();
                div()
                    .w_full()
                    .flex()
                    .items_center()
                    .gap_2()
                    .min_w_0()
                    .child(
                        div()
                            .flex_shrink(1.)
                            .overflow_hidden()
                            .text_ellipsis()
                            .whitespace_nowrap()
                            .text_color(cx.theme().foreground)
                            .child(entry.name()),
                    )
                    .children(heavy.then(|| finding_chip("heavy", cx)))
                    .children(lies.then(|| finding_chip("mislabelled", cx)))
                    .into_any_element()
            }
            TableColumn::Format => {
                let lies = entry.extension_lies();
                div()
                    .flex()
                    .items_center()
                    .gap_1()
                    .whitespace_nowrap()
                    .font_weight(FontWeight::MEDIUM)
                    .text_color(if lies {
                        cx.theme().yellow
                    } else {
                        cx.theme().muted_foreground
                    })
                    .child(format_name(entry.format))
                    // The extension disagrees with the bytes. The mark is small
                    // because the count in the notice above is what raises it.
                    .when(lies, |cell| cell.child(div().text_size(px(11.)).child("≠")))
                    .into_any_element()
            }
            TableColumn::Pixels => div()
                .w_full()
                .flex()
                .justify_end()
                .font_family(cx.theme().mono_font_family.clone())
                .text_size(px(12.))
                .text_color(cx.theme().muted_foreground)
                .whitespace_nowrap()
                .child(format!("{}×{}", entry.width, entry.height))
                .into_any_element(),
            TableColumn::Density => {
                let density = entry.bytes_per_pixel();
                div()
                    .w_full()
                    .flex()
                    .justify_end()
                    .font_family(cx.theme().mono_font_family.clone())
                    .text_size(px(12.))
                    .font_weight(FontWeight::MEDIUM)
                    .whitespace_nowrap()
                    .text_color(cx.theme().muted_foreground)
                    .child(format!("{density:.2}"))
                    .into_any_element()
            }
            // Just the number. The list is ordered by this column, so a bar
            // under every figure drew the sort order a second time.
            TableColumn::Weight => div()
                .w_full()
                .flex()
                .justify_end()
                .items_center()
                .font_family(cx.theme().mono_font_family.clone())
                .text_size(px(12.))
                .font_weight(FontWeight::MEDIUM)
                .whitespace_nowrap()
                .text_color(cx.theme().foreground)
                .child(format_bytes(entry.bytes))
                .into_any_element(),
            TableColumn::Sync => {
                // The row's file against the paired Sirv folder. No pairing,
                // no status: the column exists only when it can know.
                let Some((label, colour)) = audit.sync_label(entry, cx) else {
                    return div().into_any_element();
                };
                div()
                    .flex()
                    .items_center()
                    .font_family(cx.theme().mono_font_family.clone())
                    .text_size(px(11.))
                    .text_color(colour)
                    .child(label)
                    .into_any_element()
            }
            TableColumn::Result => div()
                .flex()
                .when(narrow_result, |result| {
                    result.flex_col().items_end().gap_0p5()
                })
                .when(!narrow_result, |result| result.items_center().gap_2())
                .justify_end()
                .whitespace_nowrap()
                .when_some(audit.results.get(&index), |slot, converted| {
                    let saved = entry.bytes.saturating_sub(*converted);
                    let percent = if entry.bytes == 0 {
                        0.
                    } else {
                        saved as f32 / entry.bytes as f32 * 100.
                    };
                    // A file that grew is a real outcome, not a rounding error:
                    // re-encoding an already-optimal JPEG usually costs bytes.
                    let grew = *converted > entry.bytes;
                    slot.child(
                        div()
                            .font_family(cx.theme().mono_font_family.clone())
                            .text_size(px(if narrow_result { 11. } else { 12. }))
                            .text_color(cx.theme().muted_foreground)
                            .child(result_size_text(entry.bytes, *converted, narrow_result)),
                    )
                    .child(if grew {
                        Tag::warning().small().child("larger")
                    } else {
                        Tag::success().small().child(format!("−{percent:.0}%"))
                    })
                })
                .into_any_element(),
        }
    }

    fn render_empty(
        &mut self,
        _window: &mut Window,
        cx: &mut Context<TableState<Self>>,
    ) -> impl IntoElement {
        let (filter, total) = self.audit.upgrade().map_or_else(
            || (String::new(), 0),
            |audit| {
                let audit = audit.read(cx);
                (audit.filter.clone(), audit.entries.len())
            },
        );
        div()
            .debug_selector(|| "filter-empty-result".into())
            .py_8()
            .px_4()
            .w_full()
            .flex()
            .flex_col()
            .items_center()
            .justify_center()
            .gap_1()
            .child(
                div()
                    .text_size(px(14.))
                    .font_weight(FontWeight::SEMIBOLD)
                    .text_color(cx.theme().foreground)
                    .child(if filter.is_empty() {
                        "No images to show".to_string()
                    } else {
                        format!("No images match “{filter}”")
                    }),
            )
            .child(
                div()
                    .text_size(px(12.))
                    .text_color(cx.theme().muted_foreground)
                    .child(if filter.is_empty() {
                        String::new()
                    } else {
                        format!("Clear the filter to show all {total} images.")
                    }),
            )
    }
}

#[derive(Clone, Copy, PartialEq, Eq)]
pub(super) enum SelectionState {
    None,
    Some,
    All,
}

impl Audit {
    pub(super) fn selection_state(&self) -> SelectionState {
        let selected = self
            .visible
            .iter()
            .filter(|index| self.selected.contains(index))
            .count();
        match selected {
            0 => SelectionState::None,
            count if count == self.visible.len() => SelectionState::All,
            _ => SelectionState::Some,
        }
    }

    pub(super) fn toggle_select_all(&mut self, cx: &mut Context<Self>) {
        if self.converting {
            return;
        }
        if self.selection_state() == SelectionState::All {
            for index in &self.visible {
                self.selected.remove(index);
            }
        } else {
            self.selected.extend(self.visible.iter().copied());
        }
        self.selection_changed(cx);
    }
}

fn finding_chip(label: &'static str, cx: &App) -> impl IntoElement {
    div()
        .flex_shrink_0()
        .text_size(px(10.))
        .text_color(cx.theme().yellow)
        .child(label)
}
