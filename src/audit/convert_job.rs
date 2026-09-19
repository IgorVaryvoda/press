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

/// The rows a target has no current output for.
///
/// Current means the folder's own record for this source under this target's
/// namespace: the recorded output is the file on disk, it was written with this
/// target's settings, and the source still hashes to what the record says. A
/// record from before fingerprints, or one whose source has been edited, reads
/// as outdated and is converted again — re-encoding a file is cheap beside
/// delivering a stale one.
///
/// This hashes sources and outputs, so it belongs on the background executor.
fn outdated_rows(
    root: &Path,
    out_dir: &Path,
    fingerprint: &str,
    sources: &[(usize, PathBuf)],
    format: Format,
) -> Vec<usize> {
    let recorded = manifest::load(out_dir);
    let paths: Vec<PathBuf> = sources.iter().map(|(_, path)| path.clone()).collect();
    let destination = convert::Destination {
        out_dir,
        backups: None,
        manifest: &recorded,
    };
    let planned = convert::plan_outputs(root, &paths, &paths, &destination, format);
    sources
        .iter()
        .zip(planned)
        .filter_map(|((index, source), written)| {
            let written = written.ok()?;
            let relative_source = source.strip_prefix(root).ok()?;
            let relative_output = written.strip_prefix(out_dir).ok()?;
            let current = recorded
                .latest(relative_source, relative_output)
                .is_some_and(|record| {
                    record.installed(&written)
                        && record.recipe.as_deref() == Some(fingerprint)
                        && record.source_matches(source) == Some(true)
                });
            (!current).then_some(*index)
        })
        .collect()
}

/// One pass of a run: a folder to write into and the settings to write with.
///
/// A folder with no delivery targets makes exactly one of these, which is the
/// run Press has always done. A product-set job with targets makes one per
/// target, and they run one after another inside the same conversion.
#[derive(Clone)]
pub(super) struct Delivery {
    pub(super) id: String,
    /// The namespace under the run's output root. Empty is the root itself.
    pub(super) out: PathBuf,
    pub(super) format: Format,
    pub(super) quality: Quality,
    pub(super) max_edge: MaxEdge,
}

/// What one delivery wrote, for the row that configured it.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub(super) struct DeliveryOutcome {
    pub(super) written: usize,
    pub(super) failed: usize,
}

impl Audit {
    /// The passes this run will make.
    ///
    /// Every target resolves its recipe now, against the library as it stands,
    /// exactly as `press convert --target` does. A target that names a recipe
    /// nobody saved any more stops the run by name rather than silently
    /// delivering the folder's current settings.
    fn delivery_runs(&self) -> Result<Vec<Delivery>, String> {
        if self.work_job.targets.is_empty() {
            return Ok(vec![Delivery {
                id: String::new(),
                out: PathBuf::new(),
                format: self.format,
                quality: self.quality,
                max_edge: self.max_edge,
            }]);
        }
        if self.output == Output::Replace {
            return Err(
                "Delivery targets write into folders of their own, which Replace has none of. \
                 Choose an output folder, or remove the targets."
                    .into(),
            );
        }
        crate::job::validate_target_namespaces(&self.work_job.targets)?;
        // The same library the command line resolves a target against: the
        // personal recipes this window loaded, plus the built-ins, which live in
        // the model rather than on disk.
        let mut library = self.recipes.clone();
        library.extend(crate::recipe::Recipe::builtins());
        crate::job::prepare_targets(&self.work_job.targets, &library)?
            .into_iter()
            .map(|target| {
                let (format, quality, max_edge) = match &target.recipe {
                    Some(recipe) => {
                        let (format, quality, max_edge, _) = recipe.effective();
                        (format, quality, max_edge)
                    }
                    None => (self.format, self.quality, self.max_edge),
                };
                Ok(Delivery {
                    id: target.id,
                    out: target.out,
                    format,
                    quality,
                    max_edge,
                })
            })
            .collect()
    }

    /// Convert the ticked rows, once per delivery target the job names.
    pub(super) fn start_conversion(&mut self, cx: &mut Context<Self>) {
        let deliveries = match self.delivery_runs() {
            Ok(deliveries) => deliveries,
            Err(message) => {
                self.notify_error("conversion", "Couldn’t start the delivery", message, cx);
                return;
            }
        };
        self.start_conversion_with(deliveries, None, cx);
    }

    /// Convert the ticked rows for one target, from its row in the rail.
    ///
    /// The row is the only place that knows which target a person meant, and a
    /// target that failed or arrived late should not need the whole job run
    /// again to catch up.
    pub(super) fn run_delivery_target(&mut self, id: &str, cx: &mut Context<Self>) {
        let deliveries = match self.delivery_runs() {
            Ok(deliveries) => deliveries,
            Err(message) => {
                self.notify_error("conversion", "Couldn’t start the delivery", message, cx);
                return;
            }
        };
        let Some(delivery) = deliveries
            .into_iter()
            .find(|delivery| delivery.id == id)
            .filter(|_| !id.is_empty())
        else {
            return;
        };
        self.start_conversion_with(vec![delivery], None, cx);
    }

    /// `rows` is the selection this run converts. `None` is the ticked rows, and
    /// a regenerate passes the narrower set it worked out from the folder.
    /// Convert only what one target is missing or has outdated.
    ///
    /// The folder is asked, not the screen: a row that was delivered by an
    /// earlier run, at these settings, from these bytes, is left alone. A target
    /// that owes nothing says so instead of re-encoding the folder.
    pub(super) fn regenerate_delivery_target(&mut self, id: &str, cx: &mut Context<Self>) {
        if self.converting || self.plan_busy() {
            return;
        }
        let deliveries = match self.delivery_runs() {
            Ok(deliveries) => deliveries,
            Err(message) => {
                self.notify_error("conversion", "Couldn’t start the delivery", message, cx);
                return;
            }
        };
        let Some(delivery) = deliveries
            .into_iter()
            .find(|delivery| delivery.id == id)
            .filter(|_| !id.is_empty())
        else {
            return;
        };
        let rows = self.targets();
        if rows.is_empty() {
            return;
        }
        let sources: Vec<(usize, PathBuf)> = rows
            .into_iter()
            .filter_map(|index| Some((index, self.entries.get(index)?.path.clone())))
            .collect();
        let root = self.root.clone();
        let output = self.output.clone();
        let dataset = self.dataset_generation;
        // The row's own wording, so a toast names what the person clicked.
        let name = self
            .work_job
            .targets
            .iter()
            .find(|target| target.id == id)
            .map_or_else(|| id.to_string(), |target| target.name.clone());
        cx.spawn(async move |this, cx| {
            let out = delivery.out.clone();
            let format = delivery.format;
            let fingerprint = crate::recipe::fingerprint_settings(
                delivery.format,
                delivery.quality,
                delivery.max_edge,
                Some(crate::avif::speed()),
            );
            let classify_root = root.clone();
            // Hashing sources and outputs is filesystem work: never on the click.
            let outdated = cx
                .background_executor()
                .spawn(async move {
                    let context = output.context(&classify_root)?;
                    let out_dir = context.output_root().join(&out);
                    Ok::<Vec<usize>, String>(outdated_rows(
                        &classify_root,
                        &out_dir,
                        &fingerprint,
                        &sources,
                        format,
                    ))
                })
                .await;
            let _ = this.update(cx, |audit, cx| {
                if audit.dataset_generation != dataset {
                    return;
                }
                match outdated {
                    Ok(rows) if rows.is_empty() => {
                        audit.notify_success(
                            "conversion",
                            "Nothing to regenerate",
                            format!("{name} already delivers every selected image."),
                            cx,
                        );
                    }
                    Ok(rows) => audit.start_conversion_with(vec![delivery], Some(rows), cx),
                    Err(message) => {
                        audit.notify_error("conversion", "Couldn’t read the target", message, cx)
                    }
                }
            });
        })
        .detach();
    }

    fn start_conversion_with(
        &mut self,
        deliveries: Vec<Delivery>,
        rows: Option<Vec<usize>>,
        cx: &mut Context<Self>,
    ) {
        if self.converting
            || self.local_ai_busy()
            || self.studio_busy()
            || self.plan_busy()
            || self.scan_blocks_delivery()
        {
            return;
        }
        let targets = rows.unwrap_or_else(|| self.targets());
        if targets.is_empty() {
            return;
        }
        // Image-menu conversion also needs the live controls and Stop in view.
        self.open_rail(Rail::Convert, cx);
        self.clear_error("conversion", cx);
        let target_count = targets.len();
        let dataset_generation = self.dataset_generation;
        self.converting = true;
        self.active_target_count = Some(target_count);
        let cancel = Arc::new(AtomicBool::new(false));
        self.convert_cancel = Some(cancel.clone());
        // The run wants every byte the samples are sitting on: it holds a decoded
        // image per worker of its own.
        self.estimate_decodes.lock().clear();
        cx.notify();

        let root = self.root.clone();
        let output = self.output.clone();
        // Replace mode is the only run that moves an original, and it moves it
        // into one mirror of the audited tree that the scan steps over.
        let replace = self.output == Output::Replace;
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
        // A whole-job run starts every row's tally again; a single target's
        // re-run only starts its own, so the other rows keep what they wrote.
        if deliveries.len() > 1 {
            self.delivery_progress.clear();
        } else if let Some(delivery) = deliveries.first() {
            self.delivery_progress.remove(&delivery.id);
        }

        let plan_root = root.clone();
        let proof_root = root.clone();
        let proof_output = output.clone();
        cx.spawn(async move |this, cx| {
            // Filesystem proof off the click handler: a slow or revoked
            // destination must not hang the window. Prior results stay on
            // screen until a destination proves itself.
            let proof = cx
                .background_executor()
                .spawn(async move { proof_output.context(&proof_root) })
                .await;
            let context = match proof {
                Ok(context) => context,
                Err(message) => {
                    let _ = this.update(cx, |audit, cx| {
                        if !conversion_landing_applies(
                            audit.dataset_generation,
                            audit.convert_cancel.as_ref(),
                            dataset_generation,
                            &cancel,
                        ) {
                            return;
                        }
                        audit.converting = false;
                        audit.active_target_count = None;
                        audit.convert_cancel = None;
                        audit.notify_error(
                            "conversion",
                            "Couldn’t use the output folder",
                            format!("{}: {message}", audit.output.label()),
                            cx,
                        );
                    });
                    return;
                }
            };
            let current = this
                .update(cx, |audit, cx| {
                    if !conversion_landing_applies(
                        audit.dataset_generation,
                        audit.convert_cancel.as_ref(),
                        dataset_generation,
                        &cancel,
                    ) {
                        return false;
                    }
                    // A stop during proof lands here with nothing planned: the
                    // queue below stays empty and the tail reports it stopped.
                    audit.clear_results();
                    cx.notify();
                    true
                })
                .unwrap_or(false);
            if !current {
                return;
            }
            let root_out_dir = context.output_root().to_path_buf();
            // The destination the results view describes is the first pass. The
            // others report on their own rows: one row of the list has one
            // output here, and a second target's copy of it would take the
            // first one's place in compare, restore and the saved bytes.
            let mut primary_out_dir = None;
            for (position, delivery) in deliveries.into_iter().enumerate() {
                // A stop between passes is a stop: the pass in flight is seen
                // through, and the next target never starts.
                if cancel.load(Ordering::Acquire) {
                    break;
                }
                let primary = position == 0;
                let out_dir = root_out_dir.join(&delivery.out);
                let format = delivery.format;
                let quality = delivery.quality;
                let max_edge = delivery.max_edge;
                let stamp = manifest::Stamp::new(format, quality, max_edge);
                let plan_root = plan_root.clone();
                let sources = sources.clone();
                let audited = audited.clone();
                if primary {
                    primary_out_dir = Some(out_dir.clone());
                }

                let plan_out_dir = out_dir.clone();
                let backups = replace.then(|| manifest::backup_root(&out_dir));
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
                            let recording = convert::Recording::for_source(
                                &root,
                                &out_dir,
                                &stamp,
                                backup.as_ref(),
                            );
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
                                        if primary {
                                            audit.record_result(
                                                index,
                                                format,
                                                converted.bytes,
                                                converted.written,
                                            );
                                        }
                                        audit.record_delivery(&delivery.id, true);
                                    }
                                    Err(error) => {
                                        // Keyed by row, so the badge, the Failed chip and
                                        // the report all read one map. The fallback is the
                                        // word `--json` uses for a failure with no reason.
                                        if primary {
                                            audit.failures.insert(
                                                index,
                                                error.reason().unwrap_or_else(|| {
                                                    "conversion failed".to_string()
                                                }),
                                            );
                                        }
                                        audit.record_delivery(&delivery.id, false);
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
                    // The run cleared results when its destination proved out, so
                    // anything recorded now was written at this destination. An
                    // all-failed run recorded nothing and erases neither field.
                    if !audit.results.is_empty()
                        && let Some(out_dir) = primary_out_dir.as_ref()
                    {
                        audit.conversion_destination = Some((output.clone(), out_dir.clone()));
                        audit.latest_output_root = Some(out_dir.clone());
                    }
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
                    // The list stays. Taking the window to a full-screen
                    // comparison of the first output hid the very report the
                    // run had just produced: the per-file column, the totals in
                    // the panel and the way to the output folder. The panel's
                    // "Compare results" opens the same view, when it is asked
                    // for.
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
