//! Selection, cursor, sort/filter, targets, and estimate scheduling.

use super::*;

impl Audit {
    /// Store settings and schedule the write. The write is debounced: a
    /// delayed save collects the whole drag and stores the size it ended at.
    pub(super) fn remember_settings(
        &mut self,
        settings: settings::Settings,
        cx: &mut Context<Self>,
    ) {
        self.settings = settings;
        if self.settings_save_pending {
            return;
        }
        self.settings_save_pending = true;
        cx.spawn(async move |this, cx| {
            cx.background_executor().timer(SETTINGS_SAVE_DELAY).await;
            let Ok(settings) = this.update(cx, |audit, _| {
                audit.settings_save_pending = false;
                audit.settings.clone()
            }) else {
                return;
            };
            cx.background_executor()
                .spawn(async move { write_settings(&settings) })
                .detach();
        })
        .detach();
    }

    /// The rows a conversion would touch. An empty selection means the whole folder,
    /// so the common case needs no ticking.
    pub(super) fn targets(&self) -> Vec<usize> {
        conversion_targets(&self.visible, &self.selected)
    }

    pub(super) fn target_count(&self) -> usize {
        if self.selected.is_empty() {
            self.visible.len()
        } else {
            self.visible
                .iter()
                .filter(|index| self.selected.contains(index))
                .count()
        }
    }

    pub(super) fn target_bytes(&self) -> u64 {
        self.visible
            .iter()
            .filter(|index| self.selected.is_empty() || self.selected.contains(index))
            .filter_map(|index| self.entries.get(*index))
            .map(|entry| entry.bytes)
            .sum()
    }

    /// Bytes before and after, counting only the files actually converted. Comparing
    /// against the whole folder mid-run would report a fake saving.
    pub(super) fn converted_totals(&self) -> (u64, u64) {
        self.results
            .iter()
            .fold((0, 0), |(before, after), (index, bytes)| {
                let source = self.entries.get(*index).map_or(0, |entry| entry.bytes);
                (before + source, after + bytes)
            })
    }
    /// Rebuild the filtered, sorted view. Nothing keyed by entry index is touched:
    /// a file keeps its thumbnail, its tick and its result through any re-ordering.
    pub(super) fn refresh_visible(&mut self) {
        let needle = self.filter.to_lowercase();
        let finding = self.finding;
        let mut visible: Vec<usize> = self
            .entries
            .iter()
            .enumerate()
            .filter(|(_, entry)| needle.is_empty() || entry.name().to_lowercase().contains(&needle))
            .filter(|(_, entry)| finding.is_none_or(|finding| finding.holds(entry)))
            .map(|(index, _)| index)
            .collect();

        let entries = &self.entries;
        let sort = self.sort;
        visible.sort_by(|a, b| compare_entries(&entries[*a], &entries[*b], sort));

        self.cursor = self.cursor.min(visible.len().saturating_sub(1));
        // Weight bars are drawn against the heaviest file on screen, so filtering
        // down to the small ones still spreads them across the column instead of
        // leaving every bar a stub.
        (self.heaviest, self.visible_bytes) = visible
            .iter()
            .filter_map(|index| self.entries.get(*index))
            .fold((0, 0), |(heaviest, total), entry| {
                (heaviest.max(entry.bytes), total + entry.bytes)
            });
        self.visible = visible;
    }

    pub(super) fn set_sort(&mut self, column: Column, cx: &mut Context<Self>) {
        self.sort = if self.sort.column == column {
            Sort {
                column,
                descending: !self.sort.descending,
            }
        } else {
            // Numbers open largest-first; names open A to Z.
            Sort {
                column,
                descending: !matches!(column, Column::Name | Column::Format),
            }
        };
        self.refresh_visible();
        cx.notify();
    }

    pub(super) fn set_filter(&mut self, filter: String, cx: &mut Context<Self>) {
        if self.converting {
            return;
        }
        self.filter = filter;
        self.refresh_visible();
        self.schedule_estimate(cx);
        cx.notify();
    }

    /// Narrow the list to one finding, or widen it again if that finding already holds.
    /// A second click on a lit control has to turn it off, the way Lossless does.
    pub(super) fn set_finding(&mut self, finding: Finding, cx: &mut Context<Self>) {
        if self.converting {
            return;
        }
        self.finding = (self.finding != Some(finding)).then_some(finding);
        self.refresh_visible();
        self.schedule_estimate(cx);
        cx.notify();
    }

    /// Encode a handful of files in memory to project what a full run would produce.
    /// Nothing is written; this only exists so the quality slider means something
    /// before you commit to it.
    pub(super) fn schedule_estimate(&mut self, cx: &mut Context<Self>) {
        self.estimate_generation += 1;
        self.estimate = None;
        let generation = self.estimate_generation;
        let dataset_generation = self.dataset_generation;

        let targets = self.targets();
        if targets.is_empty() {
            return;
        }

        let (format, quality, max_edge) = (self.format, self.quality, self.max_edge);
        let slices = sample_size(format).min(targets.len());
        // One sample per slice of the list, taken from the middle of it. The list is
        // weight-sorted, so the first file of a slice is its heaviest and the least
        // like the rest of it.
        let strata: Vec<Stratum> = (0..slices)
            .filter_map(|slice| {
                let start = slice * targets.len() / slices;
                let end = (slice + 1) * targets.len() / slices;
                let entry = self.entries.get(*targets.get((start + end) / 2)?)?;
                Some(Stratum {
                    path: entry.path.clone(),
                    bytes: entry.bytes,
                    slice_bytes: targets[start..end]
                        .iter()
                        .filter_map(|index| self.entries.get(*index))
                        .map(|entry| entry.bytes)
                        .sum(),
                })
            })
            .collect();

        cx.spawn(async move |this, cx| {
            cx.background_executor().timer(ESTIMATE_DELAY).await;
            if this
                .read_with(cx, |audit, _| {
                    audit.estimate_generation != generation
                        || audit.dataset_generation != dataset_generation
                })
                .unwrap_or(true)
            {
                return;
            }

            // The samples are independent, so they run together, as many at once as a
            // conversion allows. That is what pays for a sample wide enough to trust:
            // 32 WebP samples of a 3.0GB folder take 0.9s, inside the wait the status
            // bar already shows as "Sizing it up…".
            let concurrency = convert::workers(format);
            let mut inflight: Vec<gpui::Task<(u64, u64, Option<u64>)>> = Vec::new();
            let mut queued = strata.iter();
            let mut sampled = Vec::with_capacity(strata.len());

            loop {
                while inflight.len() < concurrency {
                    let Some(stratum) = queued.next() else {
                        break;
                    };
                    let path = stratum.path.clone();
                    let (slice_bytes, bytes) = (stratum.slice_bytes, stratum.bytes);
                    inflight.push(cx.background_executor().spawn(async move {
                        let encoded = scan::decode(&path)
                            .map(|image| max_edge.apply(image))
                            .and_then(|image| convert::encode(&image, format, quality))
                            .map(|encoded| encoded.len() as u64);
                        (slice_bytes, bytes, encoded)
                    }));
                }
                if inflight.is_empty() {
                    break;
                }
                let ((slice_bytes, bytes, encoded), _, remaining) = select_all(inflight).await;
                inflight = remaining;
                sampled.push((slice_bytes, encoded.map(|encoded| (bytes, encoded))));
            }

            let Some((projected, counted)) = project_total(&sampled) else {
                return;
            };
            let _ = this.update(cx, |audit, cx| {
                // A newer change started while this was encoding.
                if audit.estimate_generation == generation
                    && audit.dataset_generation == dataset_generation
                {
                    audit.estimate = Some((projected, counted));
                    cx.notify();
                }
            });
        })
        .detach();
    }

    /// Move the keyboard cursor, clamped to the list.
    pub(super) fn move_cursor(&mut self, delta: isize, cx: &mut Context<Self>) {
        if self.visible.is_empty() {
            return;
        }
        let last = self.visible.len() - 1;
        self.cursor = (self.cursor as isize + delta).clamp(0, last as isize) as usize;
        if self.grid {
            let columns = self.gallery_columns.unwrap_or(1).max(1);
            self.gallery_scroll
                .scroll_to_item_strict(self.cursor / columns, ScrollStrategy::Nearest);
        } else if let Some(table) = self.table.clone() {
            let visible = table.read(cx).visible_range().rows().clone();
            if !visible.contains(&self.cursor) {
                table.update(cx, |table, cx| table.scroll_to_row(self.cursor, cx));
            }
        }
        cx.notify();
    }

    /// One keyboard step. With shift held it is a selection drag: the run from
    /// the anchor to the new cursor joins the selection, exactly as a
    /// shift-click does.
    pub(super) fn step_cursor(&mut self, delta: isize, extend: bool, cx: &mut Context<Self>) {
        self.move_cursor(delta, cx);
        if extend {
            self.select_through_cursor(cx);
        }
    }

    /// Left and right: one row in the list, one tile across in the gallery.
    pub(super) fn step_cursor_lateral(
        &mut self,
        direction: isize,
        extend: bool,
        cx: &mut Context<Self>,
    ) {
        let columns = if self.grid {
            self.gallery_columns.unwrap_or(1).max(1) as isize
        } else {
            1
        };
        self.step_cursor(direction * columns, extend, cx);
    }

    pub(super) fn select_through_cursor(&mut self, cx: &mut Context<Self>) {
        if self.converting {
            return;
        }
        let (from, to) = if self.anchor <= self.cursor {
            (self.anchor, self.cursor)
        } else {
            (self.cursor, self.anchor)
        };
        let run: Vec<usize> = (from..=to).filter_map(|row| self.entry_at(row)).collect();
        self.selected.extend(run);
        self.schedule_estimate(cx);
        cx.notify();
    }

    /// What a click on a row means, by the rules every file list uses: plain click
    /// selects just that row, the platform modifier adds or removes one, shift takes
    /// the run from the last click, and a second click opens it.
    ///
    /// A plain click used to open the comparison, which made picking a few files to
    /// convert a fight with a full-screen preview.
    pub(super) fn click_row(
        &mut self,
        row: usize,
        event: &gpui::ClickEvent,
        cx: &mut Context<Self>,
    ) {
        let Some(entry) = self.entry_at(row) else {
            return;
        };
        let modifiers = event.modifiers();

        if event.click_count() >= 2 {
            self.cursor = row;
            self.open_compare(entry, cx);
            return;
        }

        if self.converting {
            return;
        }

        if modifiers.platform || modifiers.control {
            if !self.selected.remove(&entry) {
                self.selected.insert(entry);
            }
        } else if modifiers.shift {
            // From wherever the last plain click landed to here, inclusive, so a
            // run of heavy files is two clicks rather than twenty.
            let (from, to) = if self.anchor <= row {
                (self.anchor, row)
            } else {
                (row, self.anchor)
            };
            let run: Vec<usize> = (from..=to).filter_map(|row| self.entry_at(row)).collect();
            self.selected.extend(run);
        } else {
            self.selected.clear();
            self.selected.insert(entry);
            self.anchor = row;
        }

        self.cursor = row;
        self.schedule_estimate(cx);
        cx.notify();
    }

    pub(super) fn toggle_cursor_selection(&mut self, cx: &mut Context<Self>) {
        if self.converting {
            return;
        }
        let Some(entry) = self.entry_at(self.cursor) else {
            return;
        };
        if !self.selected.remove(&entry) {
            self.selected.insert(entry);
        }
        self.schedule_estimate(cx);
        cx.notify();
    }

    /// The entry a visible row points at.
    pub(super) fn entry_at(&self, row: usize) -> Option<usize> {
        self.visible.get(row).copied()
    }

    /// Where an entry currently sits in the view, if the filter has not hidden it.
    pub(super) fn row_of(&self, entry: usize) -> Option<usize> {
        self.visible.iter().position(|index| *index == entry)
    }

    /// Step to the next or previous image while the comparison is open.
    pub(super) fn step_compare(&mut self, delta: isize, cx: &mut Context<Self>) {
        let Some(entry) = self.compare.as_ref().map(|comparison| comparison.index) else {
            return;
        };
        // Step through what is on screen, not through the underlying scan order.
        let Some(row) = self.row_of(entry) else {
            return;
        };
        let next = row as isize + delta;
        if next >= 0
            && let Some(entry) = self.entry_at(next as usize)
        {
            self.cursor = next as usize;
            self.open_compare(entry, cx);
        }
    }
}
