//! Media actions: opening the comparison and driving the thumbnail cache.

use super::*;

impl Audit {
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
                    audit
                        .compare
                        .as_ref()
                        .is_some_and(|open| open.index == index && open.key == key)
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
                if let Some(pair) = built.as_ref() {
                    audit.cached = Some((key.clone(), pair.clone()));
                }
                // Ignore a result the user already navigated away from.
                if let Some(comparison) = audit.compare.as_mut()
                    && comparison.index == index
                    && comparison.dataset_generation == dataset_generation
                    && comparison.key == key
                {
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
        let dataset_generation = self.dataset_generation;
        let Some(path) = self.entries.get(index).map(|entry| entry.path.clone()) else {
            return;
        };

        cx.spawn(async move |this, cx| {
            let loaded = cx
                .background_executor()
                .spawn(async move { thumbs::load(&path, thumbs::THUMB_EDGE) })
                .await;

            if let Some(image) = loaded {
                let _ = this.update(cx, |audit, cx| {
                    if audit.dataset_generation == dataset_generation {
                        audit.thumbs.insert(index, image);
                        audit.thumb_order.push_back(index);
                        audit.trim_thumbs();
                        cx.notify();
                    }
                });
            }
        })
        .detach();
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
