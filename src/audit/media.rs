//! Media actions: opening the comparison and driving the thumbnail cache.

use super::*;

pub(super) fn comparison_landing_applies(
    open: Option<&Comparison>,
    index: usize,
    dataset_generation: u64,
    key: &compare::Key,
) -> bool {
    open.is_some_and(|comparison| {
        comparison.index == index
            && comparison.dataset_generation == dataset_generation
            && comparison.key == *key
    })
}

impl Audit {
    fn thumb_edge(&self) -> u32 {
        if self.grid {
            thumbs::THUMB_EDGE
        } else {
            thumbs::TABLE_THUMB_EDGE
        }
    }

    fn thumb_request_is_current(&self, request: &ThumbRequest) -> bool {
        self.dataset_generation == request.dataset_generation && self.thumb_edge() == request.edge
    }

    pub(super) fn thumb_is_visible(&self, index: usize, cx: &App) -> bool {
        let Some(row) = self.row_of(index) else {
            return false;
        };
        if self.grid {
            let columns = self.gallery_columns.unwrap_or(1).max(1);
            let band = row / columns;
            self.gallery_visible.contains(&band)
        } else {
            self.table
                .as_ref()
                .is_some_and(|table| table.read(cx).visible_range().rows().contains(&row))
        }
    }

    fn notify_thumbs(&self, cx: &mut Context<Self>) {
        if self.grid {
            cx.notify();
        } else if let Some(table) = self.table.clone() {
            table.update(cx, |_, cx| cx.notify());
        }
    }

    /// Open the side-by-side view for a row and start building both sides.
    pub(super) fn open_compare(&mut self, index: usize, cx: &mut Context<Self>) {
        let Some(path) = self.entries.get(index).map(|entry| entry.path.clone()) else {
            return;
        };
        let dataset_generation = self.dataset_generation;
        let key = compare::Key::new(&path, self.format, self.quality, self.max_edge);
        self.compare = Some(Comparison {
            index,
            dataset_generation,
            key: key.clone(),
            pair: None,
            failed: false,
            split: 0.5,
            pan: (0., 0.),
            // Open fitted: you cannot judge a crop of an image you have not seen.
            zoom: None,
            drag: None,
        });
        cx.notify();

        let quality = self.quality;
        let format = self.format;
        let max_edge = self.max_edge;
        // Same image, same settings: skip the encoder entirely.
        if let Some((cached_key, pair)) = self.cached.as_ref()
            && *cached_key == key
        {
            if let Some(comparison) = self.compare.as_mut() {
                comparison.pair = Some(pair.clone());
            }
            cx.notify();
            return;
        }

        cx.spawn(async move |this, cx| {
            // Building a pair is a full decode, encode and second decode. Arrowing
            // through a folder used to start one per keypress and leave every one of
            // them running; wait for the arrow key to stop first.
            cx.background_executor().timer(COMPARE_DELAY).await;
            let still_open = this
                .read_with(cx, |audit, _| {
                    comparison_landing_applies(
                        audit.compare.as_ref(),
                        index,
                        dataset_generation,
                        &key,
                    )
                })
                .unwrap_or(false);
            if !still_open {
                return;
            }

            let built = cx
                .background_executor()
                .spawn(async move { compare::build(&path, format, quality, max_edge) })
                .await
                .map(Arc::new);

            let _ = this.update(cx, |audit, cx| {
                // Ignore a result the user already navigated away from.
                let applies = comparison_landing_applies(
                    audit.compare.as_ref(),
                    index,
                    dataset_generation,
                    &key,
                );
                if applies {
                    if let Some(pair) = built.as_ref() {
                        audit.cached = Some((key.clone(), pair.clone()));
                    }
                    let comparison = audit.compare.as_mut().unwrap();
                    comparison.failed = built.is_none();
                    comparison.pair = built;
                    cx.notify();
                }
            });
        })
        .detach();
    }

    /// Kick off decoding for a row, unless it is already loaded or in flight.
    pub(super) fn request_thumb(&mut self, index: usize, cx: &mut Context<Self>) {
        if self.thumbs.contains_key(&index) || !self.requested.insert(index) {
            return;
        }
        let Some(path) = self.entries.get(index).map(|entry| entry.path.clone()) else {
            return;
        };
        let request = ThumbRequest {
            index,
            dataset_generation: self.dataset_generation,
            edge: self.thumb_edge(),
            path,
        };

        cx.spawn(async move |this, cx| {
            // Rows crossed during a fast scroll are never seen long enough to
            // justify a decode or a texture upload. Once the list settles, only
            // the rows still in the viewport continue.
            cx.background_executor().timer(THUMB_SETTLE).await;
            let _ = this.update(cx, |audit, cx| {
                // A folder or view-mode change already cleared `requested`; do not
                // remove a new request for the same row when this old timer wakes.
                if !audit.thumb_request_is_current(&request) {
                    return;
                }
                if !audit.thumb_is_visible(request.index, cx) {
                    audit.requested.remove(&request.index);
                    return;
                }
                audit.thumb_queue.push_back(request);
                audit.start_thumb_jobs(cx);
            });
        })
        .detach();
    }

    pub(super) fn start_thumb_jobs(&mut self, cx: &mut Context<Self>) {
        while self.thumb_inflight < THUMB_WORKERS {
            let Some(request) = self.thumb_queue.pop_front() else {
                return;
            };
            if !self.thumb_request_is_current(&request) {
                continue;
            }
            if self.thumbs.contains_key(&request.index) {
                continue;
            }
            if !self.thumb_is_visible(request.index, cx) {
                self.requested.remove(&request.index);
                continue;
            }

            self.thumb_inflight += 1;
            let ThumbRequest {
                index,
                dataset_generation,
                edge,
                path,
            } = request;
            cx.spawn(async move |this, cx| {
                let loaded = cx
                    .background_executor()
                    .spawn(async move { thumbs::load(&path, edge) })
                    .await;

                let _ = this.update(cx, |audit, cx| {
                    audit.thumb_inflight = audit.thumb_inflight.saturating_sub(1);
                    if audit.dataset_generation == dataset_generation
                        && audit.thumb_edge() == edge
                        && let Some(image) = loaded
                    {
                        audit.thumbs.insert(index, image);
                        audit.thumb_order.push_back(index);
                        audit.trim_thumbs();
                        if audit.thumb_is_visible(index, cx) {
                            audit.notify_thumbs(cx);
                        }
                    }
                    audit.start_thumb_jobs(cx);
                });
            })
            .detach();
        }
    }

    /// How one file stands against the paired Sirv folder, as the word and colour the
    /// window says it in. `None` when there is no pairing or its listing is not ready:
    /// the state exists only when it can be known.
    ///
    /// One place for it, so the table and the gallery cannot drift into two
    /// vocabularies for one fact — the gallery had none at all, which is the widest
    /// two vocabularies can drift.
    pub(super) fn sync_label(&self, entry: &Entry, cx: &App) -> Option<(&'static str, gpui::Hsla)> {
        let Listing::Ready(files) = &self.sirv_pairing.as_ref()?.files else {
            return None;
        };
        let key = sirv::relative_key(&self.root, &entry.path)?;
        Some(match sirv::classify(entry.bytes, files.get(&key)) {
            sirv::SyncState::Same => ("synced", cx.theme().muted_foreground),
            sirv::SyncState::Changed => ("changed", cx.theme().yellow),
            sirv::SyncState::OnlyLocal => ("new", cx.theme().blue),
        })
    }

    /// Drop the oldest thumbnails once the cache is over its bound. `requested` has to
    /// forget them too, or scrolling back to a dropped row would show a permanent gap.
    pub(super) fn trim_thumbs(&mut self) {
        while self.thumb_order.len() > THUMB_CACHE {
            let Some(oldest) = self.thumb_order.pop_front() else {
                return;
            };
            self.thumbs.remove(&oldest);
            self.requested.remove(&oldest);
        }
    }
}
