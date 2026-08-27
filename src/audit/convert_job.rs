//! The conversion job: one sliding window of decodes bounded by worker count.

use super::*;

impl Audit {
    pub(super) fn start_conversion(&mut self, cx: &mut Context<Self>) {
        if self.converting || self.local_ai_busy() || self.studio_busy() || self.scanning.is_some()
        {
            return;
        }
        let targets = self.targets();
        if targets.is_empty() {
            return;
        }
        let target_count = targets.len();
        let dataset_generation = self.dataset_generation;
        self.converting = true;
        self.active_target_count = Some(target_count);
        self.clear_results();
        self.failures.clear();
        cx.notify();

        let root = self.root.clone();
        let out_dir = self.output.root(&self.root);
        let quality = self.quality;
        let format = self.format;
        let max_edge = self.max_edge;
        let sources: Vec<(usize, PathBuf)> = targets
            .into_iter()
            .filter_map(|index| Some((index, self.entries.get(index)?.path.clone())))
            .collect();
        // Two sources can want one output name, so the whole run picks its names
        // together before any of it writes.
        let paths: Vec<PathBuf> = sources.iter().map(|(_, path)| path.clone()).collect();
        let planned = convert::plan_outputs(&root, &paths, &out_dir, format);
        let sources: Vec<(usize, PathBuf, PathBuf)> = sources
            .into_iter()
            .zip(planned)
            .map(|((index, source), written)| (index, source, written))
            .collect();

        cx.spawn(async move |this, cx| {
            // A sliding window rather than batches. Batching waited for all eight of a
            // chunk before starting the ninth, so one 40MB photo held seven workers
            // idle; here a finished file is replaced immediately. The window is what
            // bounds memory: every file in flight holds a fully decoded image.
            let workers = convert::workers(format);
            let mut inflight: Vec<
                gpui::Task<(usize, Result<convert::Converted, convert::Failure>)>,
            > = Vec::new();
            let mut queued = sources.iter();
            let mut completed = Vec::with_capacity(workers);

            loop {
                while inflight.len() < workers {
                    let Some((index, source, written)) = queued.next() else {
                        break;
                    };
                    let (index, source, written) = (*index, source.clone(), written.clone());
                    let root = root.clone();
                    inflight.push(cx.background_executor().spawn(async move {
                        (
                            index,
                            convert::convert_to(
                                &root, &source, &written, format, quality, max_edge,
                            ),
                        )
                    }));
                }
                if inflight.is_empty() {
                    break;
                }
                // Take whichever file finishes first. Waiting for source order here
                // quietly turns one slow image back into a batch barrier.
                let ((index, result), _, remaining) = select_all(inflight).await;
                inflight = remaining;
                completed.push((index, result));

                // Publishing once per file made a 6,000-image conversion rebuild the
                // same window 6,000 times. One worker-window keeps progress live while
                // cutting UI invalidations by 87.5% for WebP.
                let work_remaining = !inflight.is_empty() || !queued.as_slice().is_empty();
                if !progress_batch_ready(completed.len(), workers, work_remaining) {
                    continue;
                }
                let batch = std::mem::take(&mut completed);

                if this
                    .update(cx, |audit, cx| {
                        if audit.dataset_generation != dataset_generation {
                            return;
                        }
                        for (index, result) in batch {
                            match result {
                                Ok(converted) => {
                                    audit.record_result(
                                        index,
                                        format,
                                        converted.bytes,
                                        converted.written,
                                    );
                                }
                                Err(error) => {
                                    let name = audit
                                        .entries
                                        .get(index)
                                        .map(|entry| entry.name())
                                        .unwrap_or_default();
                                    audit.failures.push(match error.reason() {
                                        Some(reason) => format!("{name} ({reason})"),
                                        None => name,
                                    });
                                }
                            }
                        }
                        cx.notify();
                    })
                    .is_err()
                {
                    return;
                }
            }

            let _ = this.update(cx, |audit, cx| {
                if audit.dataset_generation == dataset_generation {
                    audit.converting = false;
                    audit.active_target_count = None;
                    // A finished run has produced something to look at, and
                    // until now the app said so in a column and left you to
                    // find it. Open it.
                    if let Some(first) = audit.result_rows().first().copied() {
                        audit.open_result(first, cx);
                    }
                }
                cx.notify();
            });
        })
        .detach();
    }
}
