//! The conversion job: one sliding window of decodes bounded by worker count.

use super::*;
use crate::manifest;

/// One file's whole plan: which row it is, what to read, where the output goes,
/// and where its original moves first when the run is replacing them.
struct Planned {
    index: usize,
    source: PathBuf,
    written: Result<PathBuf, convert::Failure>,
    backup: Option<convert::Backup>,
}

/// This is filesystem work, even though it does not encode: loading the run
/// record and proving name ownership can stat every recorded output. Call it
/// on the background executor, never from a click handler.
fn plan_sources(
    root: &Path,
    out_dir: &Path,
    backups: Option<&Path>,
    sources: Vec<(usize, PathBuf)>,
    audited: &[PathBuf],
    format: Format,
) -> Vec<Planned> {
    let paths: Vec<PathBuf> = sources.iter().map(|(_, path)| path.clone()).collect();
    let recorded = manifest::load(out_dir);
    let destination = convert::Destination {
        out_dir,
        backups,
        manifest: &recorded,
    };
    let planned = convert::plan_outputs(root, &paths, audited, &destination, format);
    sources
        .into_iter()
        .zip(planned)
        .map(|((index, source), written)| Planned {
            backup: destination.backup(root, &source),
            index,
            source,
            written,
        })
        .collect()
}

/// A dataset may start another job without changing folders. The cancellation
/// token identifies the run as well as the dataset, including during preflight.
fn conversion_landing_applies(
    current_generation: u64,
    current_cancel: Option<&Arc<AtomicBool>>,
    dataset_generation: u64,
    cancel: &Arc<AtomicBool>,
) -> bool {
    current_generation == dataset_generation
        && current_cancel.is_some_and(|current| Arc::ptr_eq(current, cancel))
}

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
        self.clear_error("conversion", cx);
        // Prove the destination before anything moves. A refused output leaves the
        // previous run's results on screen instead of clearing them for a run that
        // was never going to write a file.
        let context = match self.output.context(&self.root) {
            Ok(context) => context,
            Err(message) => {
                self.notify_error(
                    "conversion",
                    "Couldn’t use the output folder",
                    format!("{}: {message}", self.output.label()),
                    cx,
                );
                return;
            }
        };
        let target_count = targets.len();
        let dataset_generation = self.dataset_generation;
        self.converting = true;
        self.active_target_count = Some(target_count);
        let cancel = Arc::new(AtomicBool::new(false));
        self.convert_cancel = Some(cancel.clone());
        // The run wants every byte the samples are sitting on: it holds a decoded
        // image per worker of its own.
        self.estimate_decodes.lock().clear();
        self.clear_results();
        cx.notify();

        let root = self.root.clone();
        let out_dir = context.output_root().to_path_buf();
        // Replace mode is the only run that moves an original, and it moves it
        // into one mirror of the audited tree that the scan steps over.
        let backups = (self.output == Output::Replace).then(|| manifest::backup_root(&out_dir));
        let quality = self.quality;
        let format = self.format;
        let max_edge = self.max_edge;
        let sources: Vec<(usize, PathBuf)> = targets
            .into_iter()
            .filter_map(|index| Some((index, self.entries.get(index)?.path.clone())))
            .collect();
        // Every audited image is protected, not only the ticked ones: writing into a
        // subfolder of the audited tree would otherwise land on an original nobody
        // selected, and this run would never see it.
        let audited: Vec<PathBuf> = self
            .entries
            .iter()
            .map(|entry| entry.path.clone())
            .collect();
        let stamp = manifest::Stamp::new(format, quality, max_edge);

        cx.spawn(async move |this, cx| {
            let plan_root = root.clone();
            let plan_out_dir = out_dir.clone();
            let planning_cancel = cancel.clone();
            let sources = cx
                .background_executor()
                .spawn(async move {
                    if planning_cancel.load(Ordering::Acquire) {
                        return Vec::new();
                    }
                    plan_sources(
                        &plan_root,
                        &plan_out_dir,
                        backups.as_deref(),
                        sources,
                        &audited,
                        format,
                    )
                })
                .await;
            // A replaced dataset or run must not start writing after a slow plan
            // lands. A stopped current run falls through to normal stop reporting;
            // its queue will not start even one encode.
            let current = this
                .read_with(cx, |audit, _| {
                    conversion_landing_applies(
                        audit.dataset_generation,
                        audit.convert_cancel.as_ref(),
                        dataset_generation,
                        &cancel,
                    )
                })
                .unwrap_or(false);
            if !current {
                return;
            }

            // A sliding window rather than batches. Batching waited for all eight of a
            // chunk before starting the ninth, so one 40MB photo held seven workers
            // idle; here a finished file is replaced immediately. The window is what
            // bounds memory: every file in flight holds a fully decoded image.
            let workers = convert::workers(format);
            type Landed = (usize, Result<convert::Converted, convert::Failure>);
            let mut inflight: Vec<gpui_kit::Task<Landed>> = Vec::new();
            let mut queued = sources.iter();
            let mut completed = Vec::with_capacity(workers);

            loop {
                // A stop closes the queue, not the window. Abandoning an encode
                // half way would leave a partial file where the folder expects a
                // whole one, so the files already in flight are seen through and
                // nothing after them is started.
                let stopped = cancel.load(Ordering::Acquire);
                while !stopped && inflight.len() < workers {
                    let Some(planned) = queued.next() else {
                        break;
                    };
                    let index = planned.index;
                    let source = planned.source.clone();
                    let written = planned.written.clone();
                    let backup = planned.backup.clone();
                    let out_dir = out_dir.clone();
                    let root = root.clone();
                    let stamp = stamp.clone();
                    inflight.push(cx.background_executor().spawn(async move {
                        // The record and the backup move belong to the write, one
                        // file at a time: a run killed here has a record for every
                        // original it moved and moved none it has no record for.
                        let recording = convert::Recording {
                            root: &root,
                            out_dir: &out_dir,
                            stamp: &stamp,
                            backup: backup.as_ref(),
                        };
                        let converted = match written {
                            Ok(written) => convert::convert_to(
                                &out_dir,
                                &source,
                                &written,
                                Some(&recording),
                                format,
                                quality,
                                max_edge,
                            ),
                            Err(failure) => Err(failure),
                        };
                        (index, converted)
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
                let work_remaining =
                    !inflight.is_empty() || (!stopped && !queued.as_slice().is_empty());
                if !progress_batch_ready(completed.len(), workers, work_remaining) {
                    continue;
                }
                let batch = std::mem::take(&mut completed);

                if this
                    .update(cx, |audit, cx| {
                        if !conversion_landing_applies(
                            audit.dataset_generation,
                            audit.convert_cancel.as_ref(),
                            dataset_generation,
                            &cancel,
                        ) {
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
                                    // Keyed by row, so the badge, the Failed chip and
                                    // the report all read one map. The fallback is the
                                    // word `--json` uses for a failure with no reason.
                                    audit.failures.insert(
                                        index,
                                        error
                                            .reason()
                                            .unwrap_or_else(|| "conversion failed".to_string()),
                                    );
                                }
                            }
                        }
                        if !audit.failures.is_empty() {
                            audit.failure_summary = named(audit.failure_names().into_iter());
                        }
                        cx.notify();
                    })
                    .is_err()
                {
                    return;
                }
            }

            // Each file recorded itself as it landed, so this only has to read
            // back how many originals the folder can now put back.
            let restorable = cx
                .background_executor()
                .spawn({
                    let root = root.clone();
                    async move { manifest::restorable(&root) }
                })
                .await;

            let _ = this.update(cx, |audit, cx| {
                if conversion_landing_applies(
                    audit.dataset_generation,
                    audit.convert_cancel.as_ref(),
                    dataset_generation,
                    &cancel,
                ) {
                    audit.restorable = restorable;
                    audit.converting = false;
                    audit.active_target_count = None;
                    audit.convert_cancel = None;
                    // A stop clicked as the last file lands converted everything
                    // it was asked to, so it reports like any other finished run.
                    let stopped = cancel.load(Ordering::Acquire)
                        && audit.results.len() + audit.failures.len() < target_count;
                    audit.stopped_run = stopped.then_some(target_count);
                    if !audit.failures.is_empty() {
                        // Counted against what the run actually attempted. A
                        // stopped run never opened the files it did not start,
                        // and they are not failures of anything.
                        let attempted = audit.results.len() + audit.failures.len();
                        // The toast has room for three names. Once there are more
                        // than that, it has to say where the rest of them are.
                        let rest = if audit.failures.len() > 3 {
                            " · the Failed chip shows them all"
                        } else {
                            ""
                        };
                        audit.notify_error(
                            "conversion",
                            "Conversion incomplete",
                            format!(
                                "{} of {attempted} failed: {}{rest}",
                                audit.failures.len(),
                                audit.failure_summary
                            ),
                            cx,
                        );
                    } else {
                        audit.clear_error("conversion", cx);
                        // A clean run toasts the same sentence the lane bar
                        // carries, so the outcome survives the bar scrolling by.
                        // A stopped run stays silent: stopping is a request to
                        // stop, not a completion worth announcing.
                        if !stopped && !audit.results.is_empty() {
                            let summary = audit.conversion_summary();
                            audit.notify_success("conversion", "Conversion complete", summary, cx);
                        }
                    }
                    // A finished run has produced something to look at, and
                    // until now the app said so in a column and left you to
                    // find it. Open it. A stopped run is a request to stop,
                    // not a request to be taken somewhere.
                    if !stopped && let Some(first) = audit.result_rows().first().copied() {
                        audit.open_result(first, cx);
                    }
                }
                cx.notify();
            });
        })
        .detach();
    }

    /// Ask the running conversion to stop. The loop checks between files, so the
    /// files in flight finish and nothing after them starts. The run stays busy
    /// until it acknowledges, which is what keeps the controls it owns disabled
    /// until the last write is on disk.
    pub(super) fn cancel_conversion(&mut self, cx: &mut Context<Self>) {
        if let Some(cancel) = self.convert_cancel.as_ref() {
            cancel.store(true, Ordering::Release);
            cx.notify();
        }
    }

    /// A stop has been asked for and the last files are still landing.
    pub(super) fn convert_stopping(&self) -> bool {
        self.convert_cancel
            .as_ref()
            .is_some_and(|cancel| cancel.load(Ordering::Acquire))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_conversion_completion_belongs_to_its_dataset_and_run() {
        let cancel = Arc::new(AtomicBool::new(false));
        let replacement = Arc::new(AtomicBool::new(false));
        assert!(conversion_landing_applies(7, Some(&cancel), 7, &cancel));
        assert!(!conversion_landing_applies(8, Some(&cancel), 7, &cancel));
        assert!(!conversion_landing_applies(
            7,
            Some(&replacement),
            7,
            &cancel
        ));
        assert!(!conversion_landing_applies(7, None, 7, &cancel));
        // Stop reporting still belongs to the current job after its flag is set.
        cancel.store(true, Ordering::Release);
        assert!(conversion_landing_applies(7, Some(&cancel), 7, &cancel));
    }

    #[test]
    fn background_planning_preserves_row_identity_and_collision_names() {
        let dir = tempfile::tempdir().unwrap();
        let root = dir.path();
        let out_dir = root.join("optimized");
        let png = root.join("shot.png");
        let jpeg = root.join("shot.jpg");
        let audited = vec![png.clone(), jpeg.clone()];
        let planned = plan_sources(
            root,
            &out_dir,
            None,
            vec![(42, png.clone()), (3, jpeg.clone())],
            &audited,
            Format::WebP,
        );
        assert_eq!(planned.len(), 2);
        assert_eq!((planned[0].index, &planned[0].source), (42, &png));
        assert_eq!((planned[1].index, &planned[1].source), (3, &jpeg));
        assert_eq!(planned[0].written, Ok(out_dir.join("shot-png.webp")));
        assert_eq!(planned[1].written, Ok(out_dir.join("shot.webp")));
        assert!(planned.iter().all(|plan| plan.backup.is_none()));
        assert!(
            !out_dir.exists(),
            "planning must not write outputs or a manifest"
        );
    }

    #[test]
    fn background_planning_protects_an_unselected_original() {
        let dir = tempfile::tempdir().unwrap();
        let root = dir.path();
        let source = root.join("shot.png");
        let untouched = root.join("shot.webp");
        let planned = plan_sources(
            root,
            root,
            None,
            vec![(4, source.clone())],
            &[source, untouched],
            Format::WebP,
        );
        assert_eq!(planned[0].written, Err(convert::Failure::OverwritesSource));
    }

    #[test]
    fn background_planning_retains_replace_backups_without_moving_files() {
        let dir = tempfile::tempdir().unwrap();
        let root = dir.path();
        let source = root.join("shot.png");
        let backups = manifest::backup_root(root);
        std::fs::write(&source, b"untouched source").unwrap();
        let planned = plan_sources(
            root,
            root,
            Some(&backups),
            vec![(9, source.clone())],
            std::slice::from_ref(&source),
            Format::WebP,
        );
        assert_eq!(
            planned[0].backup,
            Some(convert::Backup {
                path: backups.join("shot.png"),
                moved: true,
            })
        );
        assert_eq!(std::fs::read(&source).unwrap(), b"untouched source");
        assert!(!backups.exists());
        assert!(!manifest::path(root).exists());
    }
}
