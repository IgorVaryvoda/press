//! Media actions: opening the comparison and driving the thumbnail cache.

use super::*;

pub(super) fn comparison_landing_applies(
    open: Option<&Comparison>,
    index: usize,
    dataset_generation: u64,
    mode: MediaMode,
    key: &compare::Key,
) -> bool {
    open.is_some_and(|comparison| {
        comparison.index == index
            && comparison.dataset_generation == dataset_generation
            && comparison.mode == mode
            && comparison.key == *key
    })
}

pub(super) fn image_context_menu(
    audit: gpui_kit::WeakEntity<Audit>,
    index: usize,
    has_result: bool,
    busy: bool,
    menu: PopupMenu,
) -> PopupMenu {
    let preview = audit.clone();
    let compare = audit.clone();
    let result = audit.clone();
    let convert = audit.clone();
    let ai_operations = audit;
    let menu = menu
        .item(PopupMenuItem::new("Preview").on_click(move |_, _, cx| {
            if let Some(audit) = preview.upgrade() {
                audit.update(cx, |audit, cx| audit.open_preview(index, cx));
            }
        }))
        .item(PopupMenuItem::new("Compare").on_click(move |_, _, cx| {
            if let Some(audit) = compare.upgrade() {
                audit.update(cx, |audit, cx| audit.open_compare(index, cx));
            }
        }));
    let menu = if has_result {
        menu.item(
            PopupMenuItem::new("See converted result").on_click(move |_, _, cx| {
                if let Some(audit) = result.upgrade() {
                    audit.update(cx, |audit, cx| audit.open_result(index, cx));
                }
            }),
        )
    } else {
        menu
    };
    menu.separator()
        .item(
            PopupMenuItem::new("Convert this image")
                .disabled(busy)
                .on_click(move |_, _, cx| {
                    if let Some(audit) = convert.upgrade() {
                        audit.update(cx, |audit, cx| audit.convert_one(index, cx));
                    }
                }),
        )
        .item(
            PopupMenuItem::new("AI operations…")
                .disabled(busy)
                .on_click(move |_, _, cx| {
                    if let Some(audit) = ai_operations.upgrade() {
                        audit.update(cx, |audit, cx| audit.open_ai_operations(index, None, cx));
                    }
                }),
        )
}

impl Audit {
    fn notify_media_error(
        &self,
        index: usize,
        title: &'static str,
        detail: &'static str,
        cx: &mut Context<Self>,
    ) {
        let name = self
            .entries
            .get(index)
            .map(Entry::name)
            .unwrap_or_else(|| "This image".into());
        self.notify_error("media", title, format!("{name} {detail}"), cx);
    }

    pub(super) fn media_commit_actions_disabled(&self) -> bool {
        self.converting || self.local_ai_busy() || self.studio_busy() || self.scan_blocks_delivery()
    }

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

    fn thumb_visible_rows(&self, cx: &App) -> std::ops::Range<usize> {
        if self.grid {
            let columns = self.gallery_columns.unwrap_or(1).max(1);
            self.gallery_visible.start.saturating_mul(columns)
                ..self
                    .gallery_visible
                    .end
                    .saturating_mul(columns)
                    .min(self.visible.len())
        } else {
            self.table
                .as_ref()
                .map_or(0..0, |table| table.read(cx).visible_range().rows().clone())
        }
    }

    pub(super) fn thumb_is_visible(&self, index: usize, cx: &App) -> bool {
        // The results strip is a viewport too. Without this every thumbnail it
        // asked for was dropped on arrival for not being in the list, and the
        // strip stayed a row of empty boxes.
        if self
            .compare
            .as_ref()
            .is_some_and(|comparison| comparison.written.is_some())
            && self.result_paths.contains_key(&index)
        {
            return true;
        }
        let Some(row) = self.row_of(index) else {
            return false;
        };
        self.thumb_visible_rows(cx).contains(&row)
    }

    fn thumb_is_wanted(&self, index: usize, cx: &App) -> bool {
        if self.thumb_is_visible(index, cx) {
            return true;
        }
        let Some(row) = self.row_of(index) else {
            return false;
        };
        thumb_overscan_rows(
            self.thumb_visible_rows(cx),
            self.visible.len(),
            thumb_cache_limit(self.thumb_edge()),
        )
        .contains(&row)
    }

    fn notify_thumbs(&mut self, cx: &mut Context<Self>) {
        if self.thumb_notify_pending {
            return;
        }
        self.thumb_notify_pending = true;
        cx.spawn(async move |this, cx| {
            cx.background_executor().timer(THUMB_REDRAW_DELAY).await;
            let _ = this.update(cx, |audit, cx| {
                audit.thumb_notify_pending = false;
                if audit.grid {
                    cx.notify();
                } else if let Some(table) = audit.table.clone() {
                    table.update(cx, |_, cx| cx.notify());
                }
            });
        })
        .detach();
    }

    /// Open the side-by-side view for a row and start building both sides.
    /// Open a finished output beside the file it came from. Unlike the preview,
    /// nothing is encoded here: both sides are read off disk, so the bytes on
    /// screen are the bytes in the folder.
    pub(super) fn open_result(&mut self, index: usize, cx: &mut Context<Self>) {
        let Some(written) = self.result_paths.get(&index).cloned() else {
            return;
        };
        self.open_written(index, written, None, cx);
    }

    /// The same view for any file this app has written next to a source —
    /// a conversion, a cutout, an upscale. Whatever produced it, the thing to
    /// do next is look at it.
    pub(super) fn open_written(
        &mut self,
        index: usize,
        written: PathBuf,
        produced_by: Option<ProducedBy>,
        cx: &mut Context<Self>,
    ) {
        let Some(source) = self.entries.get(index).map(|entry| entry.path.clone()) else {
            return;
        };
        self.clear_error("media", cx);
        let dataset_generation = self.dataset_generation;
        let key = compare::Key::new(&written, self.format, self.quality, self.max_edge);
        self.compare = Some(Comparison {
            index,
            dataset_generation,
            mode: MediaMode::Compare,
            focused: false,
            key: key.clone(),
            preview: None,
            pair: None,
            failed: false,
            split: 0.5,
            pan: (0., 0.),
            zoom: None,
            drag: None,
            written: Some(written.clone()),
            produced_by,
        });
        // The strip is about to show these; ask for their thumbnails now rather
        // than when each tile paints, which is too late to be useful.
        for row in self.strip_rows(index) {
            self.request_thumb(row, cx);
        }
        if let Some(pair) = self.take_cached_pair(&key) {
            if let Some(comparison) = self.compare.as_mut() {
                comparison.pair = Some(pair);
            }
            self.prefetch_media(cx);
            cx.notify();
            return;
        }
        if self.is_prefetching(&key, MediaMode::Compare) {
            cx.notify();
            return;
        }
        cx.notify();

        cx.spawn(async move |this, cx| {
            let built = cx
                .background_executor()
                .spawn(async move { compare::build_written(&source, &written) })
                .await
                .map(Arc::new);
            let _ = this.update(cx, |audit, cx| {
                if !comparison_landing_applies(
                    audit.compare.as_ref(),
                    index,
                    dataset_generation,
                    MediaMode::Compare,
                    &key,
                ) {
                    return;
                }
                let comparison = audit.compare.as_mut().unwrap();
                comparison.failed = built.is_none();
                comparison.pair = built;
                if comparison.failed {
                    audit.notify_media_error(
                        index,
                        "Couldn’t compare result",
                        "or its written output is missing, damaged, or unsupported.",
                        cx,
                    );
                } else {
                    audit.clear_error("media", cx);
                }
                audit.prefetch_media(cx);
                cx.notify();
            });
        })
        .detach();
    }

    pub(super) fn open_preview(&mut self, index: usize, cx: &mut Context<Self>) {
        let Some((path, width, height)) = self
            .entries
            .get(index)
            .map(|entry| (entry.path.clone(), entry.width, entry.height))
        else {
            return;
        };
        self.clear_error("media", cx);
        let dataset_generation = self.dataset_generation;
        let key = compare::Key::new(&path, self.format, self.quality, self.max_edge);
        let cached = self.take_cached_preview(&key);
        let full_resolution = cached.is_some();
        let preview = cached.or_else(|| {
            self.thumbs.get(&index).cloned().map(|image| {
                Arc::new(Preview {
                    image,
                    width,
                    height,
                    profile: None,
                    // A thumbnail standing in until the decode lands. It came out of
                    // the cache's lossy WebP, so it is a picture, not a source.
                    decoded: false,
                })
            })
        });
        self.compare = Some(Comparison {
            index,
            dataset_generation,
            mode: MediaMode::Preview,
            focused: false,
            key: key.clone(),
            preview,
            pair: None,
            failed: false,
            split: 0.5,
            pan: (0., 0.),
            zoom: None,
            drag: None,
            written: None,
            produced_by: None,
        });
        let awaiting_prefetch = !full_resolution && self.is_prefetching(&key, MediaMode::Preview);
        if !awaiting_prefetch {
            self.prefetch_media(cx);
        }
        cx.notify();

        if full_resolution || awaiting_prefetch {
            return;
        }

        cx.spawn(async move |this, cx| {
            // A thumbnail is already on screen. Give repeated arrows one short
            // window to settle before committing memory to the full-size decode.
            cx.background_executor().timer(PREVIEW_DELAY).await;
            let still_open = this
                .read_with(cx, |audit, _| {
                    comparison_landing_applies(
                        audit.compare.as_ref(),
                        index,
                        dataset_generation,
                        MediaMode::Preview,
                        &key,
                    )
                })
                .unwrap_or(false);
            if !still_open {
                return;
            }

            let built = cx
                .background_executor()
                .spawn(async move { compare::preview(&path) })
                .await
                .map(Arc::new);
            let _ = this.update(cx, |audit, cx| {
                if !comparison_landing_applies(
                    audit.compare.as_ref(),
                    index,
                    dataset_generation,
                    MediaMode::Preview,
                    &key,
                ) {
                    return;
                }
                if let Some(preview) = built.as_ref() {
                    audit.cached = Some((key.clone(), CachedMedia::Preview(preview.clone())));
                }
                let comparison = audit.compare.as_mut().unwrap();
                comparison.failed = built.is_none();
                comparison.preview = built;
                if comparison.failed {
                    audit.notify_media_error(
                        index,
                        "Couldn’t open preview",
                        "is damaged or uses an unsupported image feature.",
                        cx,
                    );
                } else {
                    audit.clear_error("media", cx);
                }
                audit.prefetch_media(cx);
                cx.notify();
            });
        })
        .detach();
    }

    pub(super) fn convert_one(&mut self, index: usize, cx: &mut Context<Self>) {
        if self.media_commit_actions_disabled() {
            return;
        }
        self.prefetch = None;
        self.prefetch_key = None;
        self.compare = None;
        self.selected.clear();
        if index < self.entries.len() {
            self.selected.insert(index);
        }
        self.selection_changed(cx);
        self.start_conversion(cx);
    }

    pub(super) fn open_compare(&mut self, index: usize, cx: &mut Context<Self>) {
        let Some(path) = self.entries.get(index).map(|entry| entry.path.clone()) else {
            return;
        };
        self.clear_error("media", cx);
        let dataset_generation = self.dataset_generation;
        let key = compare::Key::new(&path, self.format, self.quality, self.max_edge);
        // Read before the comparison replaces it: the preview of this same file, at
        // these same settings, has already decoded it.
        let previewed = self.previewed_source(&key);
        self.compare = Some(Comparison {
            index,
            dataset_generation,
            mode: MediaMode::Compare,
            focused: false,
            key: key.clone(),
            preview: None,
            pair: None,
            failed: false,
            split: 0.5,
            pan: (0., 0.),
            // Open fitted: you cannot judge a crop of an image you have not seen.
            zoom: None,
            drag: None,
            written: None,
            produced_by: None,
        });
        cx.notify();

        let quality = self.quality;
        let format = self.format;
        let max_edge = self.max_edge;
        // Same image, same settings: skip the encoder entirely. Arrowing through a
        // folder lands here every step, on the pair built while you looked at the
        // one before it.
        if let Some(pair) = self.take_cached_pair(&key) {
            if let Some(comparison) = self.compare.as_mut() {
                comparison.pair = Some(pair);
            }
            self.prefetch_media(cx);
            cx.notify();
            return;
        }
        if self.is_prefetching(&key, MediaMode::Compare) {
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
                        MediaMode::Compare,
                        &key,
                    )
                })
                .unwrap_or(false);
            if !still_open {
                return;
            }

            let built = cx
                .background_executor()
                .spawn(async move {
                    compare::build(&path, format, quality, max_edge, previewed.as_deref())
                })
                .await
                .map(Arc::new);

            let _ = this.update(cx, |audit, cx| {
                // Ignore a result the user already navigated away from.
                let applies = comparison_landing_applies(
                    audit.compare.as_ref(),
                    index,
                    dataset_generation,
                    MediaMode::Compare,
                    &key,
                );
                if applies {
                    if let Some(pair) = built.as_ref() {
                        audit.cached = Some((key.clone(), CachedMedia::Pair(pair.clone())));
                    }
                    let comparison = audit.compare.as_mut().unwrap();
                    comparison.failed = built.is_none();
                    comparison.pair = built;
                    if comparison.failed {
                        audit.notify_media_error(
                            index,
                            "Couldn’t build comparison",
                            "could not be decoded or encoded with these settings.",
                            cx,
                        );
                    } else {
                        audit.clear_error("media", cx);
                    }
                    audit.prefetch_media(cx);
                    cx.notify();
                }
            });
        })
        .detach();
    }

    /// Return the current or prebuilt media for `key`. Media built ahead becomes
    /// the current cache entry when navigation reaches it.
    fn take_cached_media(&mut self, key: &compare::Key, mode: MediaMode) -> Option<CachedMedia> {
        if let Some((cached, media)) = self.cached.as_ref()
            && cached == key
            && media.mode() == mode
        {
            return Some(media.clone());
        }
        match self.ahead.take() {
            Some((ahead, media)) if ahead == *key && media.mode() == mode => {
                self.cached = Some((ahead, media.clone()));
                Some(media)
            }
            _ => None,
        }
    }

    /// The preview of `key` that is on screen or held in the media cache, when it
    /// holds the file's own decoded pixels. A comparison built from it reads nothing.
    fn previewed_source(&self, key: &compare::Key) -> Option<Arc<Preview>> {
        let on_screen = self
            .compare
            .as_ref()
            .filter(|comparison| comparison.key == *key)
            .and_then(|comparison| comparison.preview.clone());
        let held = match self.cached.as_ref() {
            Some((cached, CachedMedia::Preview(preview))) if cached == key => Some(preview.clone()),
            _ => None,
        };
        on_screen.or(held).filter(|preview| preview.decoded)
    }

    fn take_cached_preview(&mut self, key: &compare::Key) -> Option<Arc<Preview>> {
        match self.take_cached_media(key, MediaMode::Preview) {
            Some(CachedMedia::Preview(preview)) => Some(preview),
            Some(CachedMedia::Pair(_)) | None => None,
        }
    }

    fn take_cached_pair(&mut self, key: &compare::Key) -> Option<Arc<Pair>> {
        match self.take_cached_media(key, MediaMode::Compare) {
            Some(CachedMedia::Pair(pair)) => Some(pair),
            Some(CachedMedia::Preview(_)) | None => None,
        }
    }

    fn is_prefetching(&self, key: &compare::Key, mode: MediaMode) -> bool {
        self.prefetch_key
            .as_ref()
            .is_some_and(|(loading, loading_mode)| loading == key && *loading_mode == mode)
    }

    /// Build the media the next arrow step will request while the current one is on
    /// screen. Only one speculative build exists and large images stay demand-driven.
    fn prefetch_media(&mut self, cx: &mut Context<Self>) {
        if self.converting || self.local_ai_busy() || self.studio_busy() {
            return;
        }
        let Some(comparison) = self.compare.as_ref() else {
            return;
        };
        let mode = comparison.mode;
        let looking_at_results = comparison.written.is_some();
        if looking_at_results && comparison.produced_by.is_some() {
            return;
        }
        let target = if looking_at_results {
            let rows = self.result_rows();
            rows.iter()
                .position(|row| *row == comparison.index)
                .and_then(|at| at.checked_add_signed(self.compare_step))
                .and_then(|next| rows.get(next).copied())
        } else {
            self.compare_target_from(comparison.index, self.compare_step)
                .map(|(_, target)| target)
        };
        let Some(target) = target else {
            self.prefetch = None;
            self.prefetch_key = None;
            return;
        };
        let Some(entry) = self.entries.get(target) else {
            return;
        };
        let Some(written) = (if looking_at_results {
            self.result_paths.get(&target).cloned().map(Some)
        } else {
            Some(None)
        }) else {
            return;
        };
        let (edge, bytes_per_pixel) = match mode {
            MediaMode::Preview => (u32::MAX, 4),
            MediaMode::Compare if written.is_some() => (u32::MAX, 8),
            MediaMode::Compare => (self.max_edge.0.unwrap_or(u32::MAX), 8),
        };
        let (width, height) = thumbs::fit(entry.width, entry.height, edge);
        if u64::from(width) * u64::from(height) * bytes_per_pixel > PREFETCH_BUDGET {
            self.ahead = None;
            self.prefetch = None;
            self.prefetch_key = None;
            return;
        }
        let path = entry.path.clone();
        let key = compare::Key::new(
            written.as_deref().unwrap_or(&path),
            self.format,
            self.quality,
            self.max_edge,
        );
        if self
            .cached
            .as_ref()
            .is_some_and(|(held, media)| *held == key && media.mode() == mode)
            || self
                .ahead
                .as_ref()
                .is_some_and(|(held, media)| *held == key && media.mode() == mode)
            || self.is_prefetching(&key, mode)
        {
            return;
        }

        let dataset_generation = self.dataset_generation;
        let (format, quality, max_edge) = (self.format, self.quality, self.max_edge);
        self.ahead = None;
        self.prefetch_key = Some((key.clone(), mode));
        self.prefetch = Some(cx.spawn(async move |this, cx| {
            cx.background_executor()
                .timer(match mode {
                    MediaMode::Preview => PREVIEW_DELAY,
                    MediaMode::Compare => COMPARE_DELAY,
                })
                .await;
            let built = cx
                .background_executor()
                .spawn(async move {
                    match mode {
                        MediaMode::Preview => compare::preview(&path)
                            .map(Arc::new)
                            .map(CachedMedia::Preview),
                        MediaMode::Compare => match written {
                            Some(written) => compare::build_written(&path, &written),
                            None => compare::build(&path, format, quality, max_edge, None),
                        }
                        .map(Arc::new)
                        .map(CachedMedia::Pair),
                    }
                })
                .await;
            let _ = this.update(cx, |audit, cx| {
                if audit.dataset_generation != dataset_generation
                    || !audit.is_prefetching(&key, mode)
                {
                    return;
                }
                audit.prefetch_key = None;

                if comparison_landing_applies(
                    audit.compare.as_ref(),
                    target,
                    dataset_generation,
                    mode,
                    &key,
                ) {
                    let failed = built.is_none();
                    if let Some(media) = built.as_ref() {
                        audit.cached = Some((key.clone(), media.clone()));
                    }
                    let comparison = audit.compare.as_mut().unwrap();
                    comparison.failed = built.is_none();
                    match built {
                        Some(CachedMedia::Preview(preview)) => {
                            comparison.preview = Some(preview);
                        }
                        Some(CachedMedia::Pair(pair)) => comparison.pair = Some(pair),
                        None => match mode {
                            MediaMode::Preview => comparison.preview = None,
                            MediaMode::Compare => comparison.pair = None,
                        },
                    }
                    if failed {
                        audit.notify_media_error(
                            target,
                            if mode == MediaMode::Preview {
                                "Couldn’t open preview"
                            } else {
                                "Couldn’t build comparison"
                            },
                            "is damaged, unsupported, or could not be encoded.",
                            cx,
                        );
                    } else {
                        audit.clear_error("media", cx);
                    }
                    audit.prefetch_media(cx);
                    cx.notify();
                } else if let Some(media) = built {
                    audit.ahead = Some((key, media));
                }
            });
        }));
    }

    /// Kick off decoding for a row, unless it is already loaded or in flight.
    pub(super) fn request_thumb(&mut self, index: usize, cx: &mut Context<Self>) {
        self.promote_thumb(index);
        self.queue_thumb(index, true);
        if self.thumb_prefetch_pending {
            return;
        }
        self.thumb_prefetch_pending = true;
        cx.spawn(async move |this, cx| {
            let _ = this.update(cx, |audit, cx| {
                audit.thumb_prefetch_pending = false;
                if audit.compare.is_none() {
                    let visible = audit.thumb_visible_rows(cx);
                    let wanted = thumb_overscan_rows(
                        visible.clone(),
                        audit.visible.len(),
                        thumb_cache_limit(audit.thumb_edge()),
                    );
                    let visible_indices = visible
                        .clone()
                        .filter_map(|row| audit.entry_at(row))
                        .collect::<Vec<_>>();
                    let wanted_indices = wanted
                        .filter(|row| !visible.contains(row))
                        .filter_map(|row| audit.entry_at(row))
                        .collect::<Vec<_>>();
                    // Fast decoders first: JPEG and WebP fill the viewport
                    // while PNG and AVIF wait behind them, not beside them.
                    let (fast, slow): (Vec<usize>, Vec<usize>) =
                        visible_indices.into_iter().partition(|index| {
                            audit.entries.get(*index).is_some_and(|entry| {
                                matches!(
                                    entry.format,
                                    scan::FileFormat::Image(
                                        image::ImageFormat::Jpeg | image::ImageFormat::WebP
                                    )
                                )
                            })
                        });
                    for index in fast {
                        audit.promote_thumb(index);
                        audit.queue_thumb(index, true);
                    }
                    for index in slow {
                        audit.promote_thumb(index);
                        audit.queue_thumb(index, true);
                    }
                    for index in wanted_indices {
                        audit.queue_thumb(index, false);
                    }
                }
                audit.start_thumb_jobs(cx);
            });
        })
        .detach();
    }

    pub(super) fn promote_thumb(&mut self, index: usize) -> bool {
        if let Some(position) = self
            .thumb_queue
            .iter()
            .position(|request| request.index == index)
            && position > 0
        {
            let request = self.thumb_queue.remove(position).unwrap();
            self.thumb_queue.push_front(request);
            true
        } else {
            false
        }
    }

    fn queue_thumb(&mut self, index: usize, visible: bool) {
        if self.thumbs.contains_key(&index) || !self.requested.insert(index) {
            return;
        }
        let Some(entry) = self.entries.get(index) else {
            self.requested.remove(&index);
            return;
        };
        let request = ThumbRequest {
            index,
            dataset_generation: self.dataset_generation,
            edge: self.thumb_edge(),
            path: entry.path.clone(),
            native_scaled: matches!(
                entry.format,
                scan::FileFormat::Image(image::ImageFormat::Jpeg | image::ImageFormat::WebP)
            ),
            fallback: false,
        };
        if visible {
            self.thumb_queue.push_front(request);
        } else {
            self.thumb_queue.push_back(request);
        }
    }

    pub(super) fn start_thumb_jobs(&mut self, cx: &mut Context<Self>) {
        // A fast scroll queues rows the view has already left. Drop them here,
        // before they take a worker slot, and forget them in `requested` so a
        // scroll back requeues rather than finding a permanent gap.
        let wanted: std::collections::HashSet<usize> = self
            .thumb_queue
            .iter()
            .map(|request| request.index)
            .filter(|index| self.thumb_is_wanted(*index, cx))
            .collect();
        if wanted.len() != self.thumb_queue.len() {
            let mut kept = std::collections::VecDeque::with_capacity(wanted.len());
            while let Some(request) = self.thumb_queue.pop_front() {
                if wanted.contains(&request.index) {
                    kept.push_back(request);
                } else {
                    self.requested.remove(&request.index);
                }
            }
            self.thumb_queue = kept;
        }
        while self.thumb_inflight < THUMB_WORKERS && !self.thumb_queue.is_empty() {
            let Some(position) = self.thumb_queue.iter().position(|request| {
                !request.fallback || self.thumb_slow_inflight < THUMB_SLOW_WORKERS
            }) else {
                return;
            };
            let request = self.thumb_queue.remove(position).unwrap();
            if !self.thumb_request_is_current(&request) {
                continue;
            }
            if self.thumbs.contains_key(&request.index) {
                continue;
            }
            if !self.thumb_is_wanted(request.index, cx) {
                self.requested.remove(&request.index);
                continue;
            }

            self.thumb_inflight += 1;
            self.thumb_slow_inflight += usize::from(request.fallback);
            let worker = request.clone();
            cx.spawn(async move |this, cx| {
                let loaded = cx
                    .background_executor()
                    .spawn(async move {
                        if worker.fallback {
                            Ok(thumbs::load_fallback(&worker.path, worker.edge))
                        } else {
                            match thumbs::load_fast(&worker.path, worker.edge, worker.native_scaled)
                            {
                                thumbs::FastLoad::Ready(loaded) => Ok(Some(loaded)),
                                thumbs::FastLoad::Fallback => Err(()),
                            }
                        }
                    })
                    .await;

                let _ = this.update(cx, |audit, cx| {
                    let mut pending_cache = None;
                    if audit.thumb_request_is_current(&request) {
                        match loaded {
                            Ok(Some(loaded)) => {
                                let thumbs::LoadedThumb { image, cache } = loaded;
                                audit.thumbs.insert(request.index, image);
                                audit.thumb_order.push_back(request.index);
                                audit.trim_thumbs();
                                if audit.thumb_is_visible(request.index, cx) {
                                    audit.notify_thumbs(cx);
                                }
                                pending_cache = cache;
                            }
                            Err(()) if audit.thumb_is_wanted(request.index, cx) => {
                                let mut request = request.clone();
                                request.fallback = true;
                                cx.spawn(async move |this, cx| {
                                    cx.background_executor().timer(THUMB_SLOW_SETTLE).await;
                                    let _ = this.update(cx, |audit, cx| {
                                        if !audit.thumb_request_is_current(&request)
                                            || !audit.thumb_is_wanted(request.index, cx)
                                        {
                                            audit.requested.remove(&request.index);
                                            return;
                                        }
                                        if audit.thumb_is_visible(request.index, cx) {
                                            audit.thumb_queue.push_front(request);
                                        } else {
                                            audit.thumb_queue.push_back(request);
                                        }
                                        audit.start_thumb_jobs(cx);
                                    });
                                })
                                .detach();
                            }
                            Err(()) => {
                                audit.requested.remove(&request.index);
                            }
                            Ok(None) => {}
                        }
                    }
                    if let Some(cache) = pending_cache {
                        cx.spawn(async move |this, cx| {
                            cx.background_executor()
                                .spawn(async move { thumbs::persist(cache) })
                                .await;
                            let _ = this.update(cx, |audit, cx| {
                                audit.thumb_inflight = audit.thumb_inflight.saturating_sub(1);
                                audit.thumb_slow_inflight = audit
                                    .thumb_slow_inflight
                                    .saturating_sub(usize::from(request.fallback));
                                audit.start_thumb_jobs(cx);
                            });
                        })
                        .detach();
                        return;
                    }
                    audit.thumb_inflight = audit.thumb_inflight.saturating_sub(1);
                    audit.thumb_slow_inflight = audit
                        .thumb_slow_inflight
                        .saturating_sub(usize::from(request.fallback));
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
    pub(super) fn sync_label(
        &self,
        entry: &Entry,
        cx: &App,
    ) -> Option<(&'static str, gpui_kit::Hsla)> {
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
        let limit = thumb_cache_limit(self.thumb_edge());
        while self.thumb_order.len() > limit {
            let Some(oldest) = self.thumb_order.pop_front() else {
                return;
            };
            self.thumbs.remove(&oldest);
            self.requested.remove(&oldest);
        }
    }
}
