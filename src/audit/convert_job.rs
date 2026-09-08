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
    expected: Option<manifest::SourceIdentity>,
    snapshot_failure: Option<convert::Failure>,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum DeliveryRunMode {
    All,
    Selected,
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
            expected: None,
            snapshot_failure: None,
        })
        .collect()
}

fn plan_snapshot_sources(
    root: &Path,
    out_dir: &Path,
    sources: &[(
        usize,
        PathBuf,
        Result<manifest::SourceIdentity, convert::Failure>,
    )],
    audited: &[PathBuf],
    format: Format,
) -> Vec<Planned> {
    let plain: Vec<(usize, PathBuf)> = sources
        .iter()
        .map(|(index, source, _)| (*index, source.clone()))
        .collect();
    let mut planned = plan_sources(root, out_dir, None, plain, audited, format);
    for (plan, (_, _, expected)) in planned.iter_mut().zip(sources) {
        match expected {
            Ok(expected) => plan.expected = Some(expected.clone()),
            Err(error) => plan.snapshot_failure = Some(error.clone()),
        }
    }
    planned
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

fn target_landing_applies(
    audit: &Audit,
    dataset: u64,
    job_id: &str,
    revision: u32,
    request_generation: u64,
    cancel: &Arc<AtomicBool>,
) -> bool {
    conversion_landing_applies(
        audit.dataset_generation,
        audit.convert_cancel.as_ref(),
        dataset,
        cancel,
    ) && audit.owns_job_request(dataset, job_id, revision, request_generation)
}

fn snapshot_failure(error: scan::ConversionDecodeError) -> convert::Failure {
    match error {
        scan::ConversionDecodeError::Failed => convert::Failure::Failed,
        scan::ConversionDecodeError::TooLarge => convert::Failure::TooLarge,
        scan::ConversionDecodeError::SourceChanged => convert::Failure::SourceChanged,
        scan::ConversionDecodeError::AnimatedGif => convert::Failure::AnimatedGif,
        scan::ConversionDecodeError::AnimatedPng => convert::Failure::AnimatedPng,
        scan::ConversionDecodeError::AnimatedWebP => convert::Failure::AnimatedWebP,
        scan::ConversionDecodeError::AnimatedJpegXl => convert::Failure::AnimatedJpegXl,
    }
}

fn capture_source_identity(path: &Path) -> Result<manifest::SourceIdentity, convert::Failure> {
    scan::read_source_bytes(path)
        .map(|bytes| manifest::SourceIdentity::from_bytes(&bytes))
        .map_err(snapshot_failure)
}

fn target_settings(
    target: &crate::job::PreparedTarget,
    current: (Format, Quality, MaxEdge, u8),
) -> (Format, Quality, MaxEdge, u8) {
    target.recipe.as_ref().map_or(current, |recipe| {
        let (format, quality, max_edge, speed) = recipe.effective();
        (format, quality, max_edge, speed.unwrap_or(current.3))
    })
}

fn target_output_root(output: &Output, root: &Path, target: &Path) -> Result<PathBuf, String> {
    if *output == Output::Replace {
        return Err("multi-target conversion does not support replace mode".into());
    }
    let path = output.root(root).join(target);
    Output::Folder(path)
        .context(root)
        .map(|context| context.output_root().to_path_buf())
}

fn target_record_is_current(
    manifest: &manifest::Manifest,
    root: &Path,
    out_dir: &Path,
    source: &Path,
    output: &Path,
    format: Format,
    quality: Quality,
    max_edge: MaxEdge,
    speed: u8,
) -> bool {
    let relative_source = source.strip_prefix(root).unwrap_or(source);
    let relative_output = output.strip_prefix(out_dir).unwrap_or(output);
    let Some(record) = manifest.latest(relative_source, relative_output) else {
        return false;
    };
    let fingerprint = crate::recipe::fingerprint_settings(format, quality, max_edge, Some(speed));
    record.installed(output)
        && record.source_matches(source) == Some(true)
        && record.recipe.as_deref() == Some(fingerprint.as_str())
}

fn classify_target(
    target: &crate::job::JobTarget,
    root: &Path,
    out_dir: &Path,
    planned: &[Planned],
    manifest: &manifest::Manifest,
    journal: &[manifest::TargetStateRecord],
    format: Format,
    quality: Quality,
    max_edge: MaxEdge,
    speed: u8,
) -> TargetProgress {
    let mut progress = TargetProgress::new(target);
    let fingerprint = crate::recipe::fingerprint_settings(format, quality, max_edge, Some(speed));
    for plan in planned {
        let relative_source = plan.source.strip_prefix(root).unwrap_or(&plan.source);
        let state = match (&plan.snapshot_failure, &plan.written) {
            (Some(error), _) | (None, Err(error)) => TargetItemState::Failed(
                error.reason().unwrap_or_else(|| "conversion failed".into()),
            ),
            (None, Ok(output))
                if target_record_is_current(
                    manifest,
                    root,
                    out_dir,
                    &plan.source,
                    output,
                    format,
                    quality,
                    max_edge,
                    speed,
                ) =>
            {
                TargetItemState::Written
            }
            (None, Ok(output)) if output.symlink_metadata().is_ok() => TargetItemState::Outdated,
            (None, Ok(output)) => {
                let relative_output = output.strip_prefix(out_dir).unwrap_or(output);
                journal
                    .iter()
                    .rev()
                    .find(|record| {
                        record.source == relative_source
                            && record.output == relative_output
                            && record.recipe == fingerprint
                    })
                    .map_or(TargetItemState::Unstarted, |record| match record.state {
                        manifest::TargetState::Failed => TargetItemState::Failed(
                            record
                                .reason
                                .clone()
                                .unwrap_or_else(|| "conversion failed".into()),
                        ),
                        manifest::TargetState::Cancelled => TargetItemState::Cancelled,
                        manifest::TargetState::Unstarted => TargetItemState::Unstarted,
                        manifest::TargetState::Written => TargetItemState::Unstarted,
                    })
            }
        };
        progress.items.insert(plan.index, state);
    }
    progress
}

impl Audit {
    /// Rebuild target states from the per-target manifests after a job is
    /// reopened. This is intentionally filesystem work on the executor; the
    /// panel only renders the cached item states.
    pub(super) fn refresh_target_progress(&mut self, cx: &mut Context<Self>) {
        if self.work_job.targets.is_empty() {
            self.target_progress.clear();
            return;
        }
        let recipes: Vec<_> = self
            .recipes
            .iter()
            .cloned()
            .chain(crate::recipe::Recipe::builtins())
            .collect();
        let Ok(prepared) = crate::job::prepare_targets(&self.work_job.targets, &recipes) else {
            self.target_progress.clear();
            return;
        };
        let dataset_generation = self.dataset_generation;
        let job_id = self.work_job.id.clone();
        let revision = self.work_job.revision;
        let request_generation = self.job_request_generation;
        let root = self.root.clone();
        let output = self.output.clone();
        let current_settings = (
            self.format,
            self.quality,
            self.max_edge,
            crate::avif::speed(),
        );
        let sources: Vec<(usize, PathBuf)> = self
            .entries
            .iter()
            .enumerate()
            .map(|(index, entry)| (index, entry.path.clone()))
            .collect();
        let audited: Vec<PathBuf> = self
            .entries
            .iter()
            .map(|entry| entry.path.clone())
            .collect();
        let target_specs = self.work_job.targets.clone();
        cx.spawn(async move |this, cx| {
            let progress = cx
                .background_executor()
                .spawn(async move {
                    let mut rows = Vec::new();
                    for (target_spec, target) in target_specs.iter().zip(&prepared) {
                        let (format, quality, max_edge, speed) =
                            target_settings(target, current_settings);
                        let Ok(out_dir) = target_output_root(&output, &root, &target.out) else {
                            continue;
                        };
                        let planned =
                            plan_sources(&root, &out_dir, None, sources.clone(), &audited, format);
                        let recorded = manifest::load(&out_dir);
                        let journal = manifest::load_target_states(&out_dir);
                        rows.push((
                            target_spec.id.clone(),
                            classify_target(
                                target_spec,
                                &root,
                                &out_dir,
                                &planned,
                                &recorded,
                                &journal,
                                format,
                                quality,
                                max_edge,
                                speed,
                            ),
                        ));
                    }
                    rows
                })
                .await;
            let _ = this.update(cx, |audit, cx| {
                if !audit.owns_job_request(
                    dataset_generation,
                    &job_id,
                    revision,
                    request_generation,
                ) || audit.converting
                {
                    return;
                }
                audit.target_progress = progress.into_iter().collect();
                cx.notify();
            });
        })
        .detach();
    }

    pub(super) fn start_conversion(&mut self, cx: &mut Context<Self>) {
        if self.converting
            || self.local_ai_busy()
            || self.studio_busy()
            || self.scan_blocks_delivery()
        {
            return;
        }
        let targets = self.targets();
        if targets.is_empty() {
            return;
        }
        if !self.work_job.targets.is_empty() {
            self.start_delivery_conversion(targets, DeliveryRunMode::All, None, cx);
            return;
        }
        // Image-menu conversion also needs the live controls and Stop in view.
        self.open_rail(Rail::Convert, cx);
        self.clear_error("conversion", cx);
        let target_count = targets.len();
        let dataset_generation = self.dataset_generation;
        self.converting = true;
        self.active_delivery_items.clear();
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
                        audit.active_delivery_items.clear();
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
            let out_dir = context.output_root().to_path_buf();

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
                    // The run cleared results when its destination proved out, so
                    // anything recorded now was written at this destination. An
                    // all-failed run recorded nothing and erases neither field.
                    if !audit.results.is_empty() {
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

    fn start_delivery_conversion(
        &mut self,
        selected: Vec<usize>,
        mode: DeliveryRunMode,
        only_target: Option<String>,
        cx: &mut Context<Self>,
    ) {
        let recipes: Vec<_> = self
            .recipes
            .iter()
            .cloned()
            .chain(crate::recipe::Recipe::builtins())
            .collect();
        let prepared = match crate::job::prepare_targets(&self.work_job.targets, &recipes) {
            Ok(prepared) => prepared,
            Err(message) => {
                self.notify_error(
                    "conversion",
                    "Couldn’t prepare delivery targets",
                    message,
                    cx,
                );
                return;
            }
        };
        if let Err(message) = crate::job::validate_target_namespaces(&self.work_job.targets) {
            self.notify_error(
                "conversion",
                "Couldn’t prepare delivery targets",
                message,
                cx,
            );
            return;
        }
        if prepared.is_empty() || self.output == Output::Replace {
            self.notify_error(
                "conversion",
                "Couldn’t prepare delivery targets",
                "multi-target conversion needs at least one target and a folder output",
                cx,
            );
            return;
        }
        self.open_rail(Rail::Convert, cx);
        self.clear_error("conversion", cx);
        let dataset_generation = self.dataset_generation;
        let job_id = self.work_job.id.clone();
        let revision = self.work_job.revision;
        let request_generation = self.job_request_generation;
        let cancel = Arc::new(AtomicBool::new(false));
        self.convert_cancel = Some(cancel.clone());
        self.converting = true;
        let run_targets = prepared
            .iter()
            .filter(|target| only_target.as_deref().is_none_or(|id| id == target.id))
            .count();
        self.active_target_count = Some(selected.len().saturating_mul(run_targets));
        self.results.clear();
        self.failures.clear();
        self.active_delivery_items.clear();
        let prior_progress = self.target_progress.clone();
        for target in &self.work_job.targets {
            let target_is_running = only_target
                .as_deref()
                .is_none_or(|only_target| only_target == target.id);
            let mut progress = prior_progress
                .get(&target.id)
                .cloned()
                .unwrap_or_else(|| TargetProgress::new(target));
            if target_is_running {
                for index in &selected {
                    progress.items.insert(*index, TargetItemState::Unstarted);
                    self.active_delivery_items
                        .insert((target.id.clone(), *index));
                }
            }
            self.target_progress.insert(target.id.clone(), progress);
        }
        self.converted_totals = (0, 0);
        self.estimate_decodes.lock().clear();
        cx.notify();

        let root = self.root.clone();
        let output = self.output.clone();
        let current_settings = (
            self.format,
            self.quality,
            self.max_edge,
            crate::avif::speed(),
        );
        let sources: Vec<(usize, PathBuf)> = selected
            .into_iter()
            .filter_map(|index| Some((index, self.entries.get(index)?.path.clone())))
            .collect();
        let audited: Vec<PathBuf> = self
            .entries
            .iter()
            .map(|entry| entry.path.clone())
            .collect();
        let selected_indices = sources.iter().map(|(index, _)| *index).collect::<Vec<_>>();
        let target_specs = self.work_job.targets.clone();
        cx.spawn(async move |this, cx| {
            let snapshots = cx
                .background_executor()
                .spawn({
                    let sources = sources.clone();
                    async move {
                        sources
                            .into_iter()
                            .map(|(index, source)| {
                                (index, source.clone(), capture_source_identity(&source))
                            })
                            .collect::<Vec<_>>()
                    }
                })
                .await;
            let snapshot_lookup: Vec<_> = snapshots
                .iter()
                .map(|(index, source, identity)| (*index, source.clone(), identity.clone()))
                .collect();

            // Resolve and compare every target root before the first target is
            // planned or written. The lexical job check catches ordinary
            // duplicates; this check also catches symlink aliases and an output
            // namespace that resolves inside another one.
            let target_outputs = cx
                .background_executor()
                .spawn({
                    let root = root.clone();
                    let output = output.clone();
                    let targets = target_specs
                        .iter()
                        .map(|target| (target.id.clone(), target.out.clone()))
                        .collect::<Vec<_>>();
                    async move {
                        let mut roots = Vec::with_capacity(targets.len());
                        for (id, out) in targets {
                            let root = target_output_root(&output, &root, &out)
                                .map_err(|message| format!("target {id}: {message}"))?;
                            roots.push((id, root));
                        }
                        for (index, (left_id, left)) in roots.iter().enumerate() {
                            for (right_id, right) in &roots[index + 1..] {
                                if left == right || left.starts_with(right) || right.starts_with(left)
                                {
                                    return Err(format!(
                                        "targets {left_id:?} and {right_id:?} overlap after resolving output aliases"
                                    ));
                                }
                            }
                        }
                        Ok(roots.into_iter().map(|(_, root)| root).collect::<Vec<_>>())
                    }
                })
                .await;
            let target_outputs = match target_outputs {
                Ok(roots) => roots,
                Err(message) => {
                    let _ = this.update(cx, |audit, cx| {
                        if !target_landing_applies(
                            audit,
                            dataset_generation,
                            &job_id,
                            revision,
                            request_generation,
                            &cancel,
                        ) {
                            return;
                        }
                        for target in &target_specs {
                            let progress = audit
                                .target_progress
                                .entry(target.id.clone())
                                .or_insert_with(|| TargetProgress::new(target));
                            for index in &selected_indices {
                                progress.items.insert(
                                    *index,
                                    TargetItemState::Failed(message.clone()),
                                );
                            }
                        }
                        audit.converting = false;
                        audit.active_target_count = None;
                        audit.convert_cancel = None;
                        audit.notify_error(
                            "conversion",
                            "Couldn’t prepare delivery targets",
                            message,
                            cx,
                        );
                        cx.notify();
                    });
                    return;
                }
            };

            for ((target_spec, target), out_dir) in target_specs
                .iter()
                .zip(&prepared)
                .zip(&target_outputs)
            {
                if only_target
                    .as_deref()
                    .is_some_and(|only_target| only_target != target.id)
                {
                    continue;
                }
                let current = this
                    .read_with(cx, |audit, _| {
                        target_landing_applies(
                            audit,
                            dataset_generation,
                            &job_id,
                            revision,
                            request_generation,
                            &cancel,
                        )
                    })
                    .unwrap_or(false);
                if !current {
                    return;
                }
                let (format, quality, max_edge, speed) = target_settings(target, current_settings);
                let out_dir = out_dir.clone();
                let plan_out_dir = out_dir.clone();
                let snapshots_for_plan = snapshot_lookup.clone();
                let audited_for_plan = audited.clone();
                let root_for_plan = root.clone();
                let planned = cx
                    .background_executor()
                    .spawn(async move {
                        plan_snapshot_sources(
                            &root_for_plan,
                            &plan_out_dir,
                            &snapshots_for_plan,
                            &audited_for_plan,
                            format,
                        )
                    })
                    .await;
                let recorded = cx
                    .background_executor()
                    .spawn({
                        let out_dir = out_dir.clone();
                        async move { manifest::load(&out_dir) }
                    })
                    .await;
                let journal = cx
                    .background_executor()
                    .spawn({
                        let out_dir = out_dir.clone();
                        async move { manifest::load_target_states(&out_dir) }
                    })
                    .await;
                let progress = classify_target(
                    target_spec,
                    &root,
                    &out_dir,
                    &planned,
                    &recorded,
                    &journal,
                    format,
                    quality,
                    max_edge,
                    speed,
                );
                let _ = this.update(cx, |audit, cx| {
                    if target_landing_applies(
                        audit,
                        dataset_generation,
                        &job_id,
                        revision,
                        request_generation,
                        &cancel,
                    ) {
                        let existing = audit
                            .target_progress
                            .entry(target.id.clone())
                            .or_insert_with(|| TargetProgress::new(target_spec));
                        existing.items.extend(progress.items.clone());
                        cx.notify();
                    }
                });
                let queued: Vec<Planned> = planned
                    .into_iter()
                    .filter(|plan| {
                        mode == DeliveryRunMode::Selected
                            || !matches!(
                                progress.items.get(&plan.index),
                                Some(TargetItemState::Written)
                            )
                    })
                    .collect();
                let stamp = manifest::Stamp::with_speed(format, quality, max_edge, Some(speed));
                let fingerprint =
                    crate::recipe::fingerprint_settings(format, quality, max_edge, Some(speed));
                for plan in &queued {
                    let Some(written) = plan.written.as_ref().ok() else {
                        continue;
                    };
                    let (Ok(source), Ok(output)) = (
                        plan.source.strip_prefix(&root),
                        written.strip_prefix(&out_dir),
                    ) else {
                        continue;
                    };
                    let _ = manifest::append_target_state(
                        &out_dir,
                        &manifest::TargetStateRecord {
                            run: stamp.run_id(),
                            source: source.to_path_buf(),
                            output: output.to_path_buf(),
                            recipe: fingerprint.clone(),
                            state: manifest::TargetState::Unstarted,
                            reason: None,
                        },
                    );
                }
                let workers = convert::workers(format);
                type Landed = (usize, Result<convert::Converted, convert::Failure>);
                let mut inflight: Vec<gpui_kit::Task<Landed>> = Vec::new();
                let mut completed = Vec::with_capacity(workers);
                let mut queue_position = 0;
                loop {
                    let stopped = cancel.load(Ordering::Acquire);
                    while !stopped && inflight.len() < workers {
                        let Some(plan) = queued.get(queue_position) else {
                            break;
                        };
                        queue_position += 1;
                        let index = plan.index;
                        let source = plan.source.clone();
                        let written = plan.written.clone();
                        let expected = plan.expected.clone();
                        let snapshot_failure = plan.snapshot_failure.clone();
                        let journal_output = plan.written.as_ref().ok().cloned();
                        let out_dir = out_dir.clone();
                        let root = root.clone();
                        let stamp = stamp.clone();
                        let fingerprint = fingerprint.clone();
                        inflight.push(cx.background_executor().spawn(async move {
                            let recording =
                                convert::Recording::for_source(&root, &out_dir, &stamp, None);
                            let result = match snapshot_failure {
                                Some(error) => Err(error),
                                None => match (written, expected) {
                                    (Ok(written), Some(expected)) => {
                                        convert::convert_to_expected_with_avif_speed(
                                            &out_dir,
                                            &source,
                                            &written,
                                            Some(&recording),
                                            format,
                                            quality,
                                            max_edge,
                                            &expected,
                                            speed,
                                        )
                                    }
                                    (Err(error), _) => Err(error),
                                    (Ok(_), None) => Err(convert::Failure::SourceChanged),
                                },
                            };
                            if let Some(output) = journal_output
                                && let (Ok(source), Ok(output)) = (
                                    source.strip_prefix(&root),
                                    output.strip_prefix(&out_dir),
                                )
                            {
                                let (state, reason) = match &result {
                                    Ok(_) => (manifest::TargetState::Written, None),
                                    Err(error) => (
                                        manifest::TargetState::Failed,
                                        error.reason(),
                                    ),
                                };
                                let _ = manifest::append_target_state(
                                    &out_dir,
                                    &manifest::TargetStateRecord {
                                        run: stamp.run_id(),
                                        source: source.to_path_buf(),
                                        output: output.to_path_buf(),
                                        recipe: fingerprint,
                                        state,
                                        reason,
                                    },
                                );
                            }
                            (index, result)
                        }));
                    }
                    if inflight.is_empty() {
                        break;
                    }
                    let ((index, result), _, remaining) = select_all(inflight).await;
                    inflight = remaining;
                    completed.push((index, result));
                    let work_remaining = !inflight.is_empty() || queue_position < queued.len();
                    if !progress_batch_ready(completed.len(), workers, work_remaining) {
                        continue;
                    }
                    let batch = std::mem::take(&mut completed);
                    let _ = this.update(cx, |audit, cx| {
                        if !target_landing_applies(
                            audit,
                            dataset_generation,
                            &job_id,
                            revision,
                            request_generation,
                            &cancel,
                        ) {
                            return;
                        }
                        let progress = audit
                            .target_progress
                            .entry(target.id.clone())
                            .or_insert_with(|| TargetProgress::new(target_spec));
                        for (index, result) in batch {
                            match result {
                                Ok(converted) => {
                                    progress.items.insert(index, TargetItemState::Written);
                                    let source_bytes = audit
                                        .entries
                                        .get(index)
                                        .map_or(0, |entry| entry.bytes);
                                    audit.converted_totals.0 =
                                        audit.converted_totals.0.saturating_add(
                                            source_bytes,
                                        );
                                    audit.converted_totals.1 =
                                        audit.converted_totals.1.saturating_add(converted.bytes);
                                }
                                Err(error) => {
                                    progress.items.insert(
                                        index,
                                        TargetItemState::Failed(
                                            error
                                                .reason()
                                                .unwrap_or_else(|| "conversion failed".into()),
                                        ),
                                    );
                                }
                            }
                        }
                        cx.notify();
                    });
                }
                let stopped = cancel.load(Ordering::Acquire);
                if stopped {
                    let _ = this.update(cx, |audit, cx| {
                        if !target_landing_applies(
                            audit,
                            dataset_generation,
                            &job_id,
                            revision,
                            request_generation,
                            &cancel,
                        ) {
                            return;
                        }
                        let progress = audit
                            .target_progress
                            .entry(target.id.clone())
                            .or_insert_with(|| TargetProgress::new(target_spec));
                        for plan in queued.iter().skip(queue_position) {
                            if !matches!(
                                progress.items.get(&plan.index),
                                Some(TargetItemState::Written | TargetItemState::Failed(_))
                            ) {
                                progress
                                    .items
                                    .insert(plan.index, TargetItemState::Cancelled);
                                if let Some(written) = plan.written.as_ref().ok()
                                    && let (Ok(source), Ok(output)) = (
                                        plan.source.strip_prefix(&root),
                                        written.strip_prefix(&out_dir),
                                    )
                                {
                                    let _ = manifest::append_target_state(
                                        &out_dir,
                                        &manifest::TargetStateRecord {
                                            run: stamp.run_id(),
                                            source: source.to_path_buf(),
                                            output: output.to_path_buf(),
                                            recipe: fingerprint.clone(),
                                            state: manifest::TargetState::Cancelled,
                                            reason: Some("conversion stopped".into()),
                                        },
                                    );
                                }
                            }
                        }
                        cx.notify();
                    });
                }
            }

            let _ = this.update(cx, |audit, cx| {
                if !target_landing_applies(
                    audit,
                    dataset_generation,
                    &job_id,
                    revision,
                    request_generation,
                    &cancel,
                ) {
                    return;
                }
                let stopped = cancel.load(Ordering::Acquire);
                audit.converting = false;
                audit.active_target_count = None;
                audit.active_delivery_items.clear();
                audit.convert_cancel = None;
                audit.stopped_run = stopped.then_some(
                    audit
                        .target_progress
                        .values()
                        .map(|progress| progress.items.len())
                        .sum(),
                );
                let failed = audit
                    .target_progress
                    .values()
                    .map(TargetProgress::failed)
                    .sum::<usize>();
                let written = audit
                    .target_progress
                    .values()
                    .map(TargetProgress::written)
                    .sum::<usize>();
                if failed > 0 {
                    audit.notify_error(
                        "conversion",
                        "Delivery targets incomplete",
                        format!("{written} written, {failed} failed; target rows keep each result"),
                        cx,
                    );
                } else if !stopped {
                    audit.clear_error("conversion", cx);
                    audit.notify_success(
                        "conversion",
                        "Delivery targets complete",
                        format!(
                            "{written} deliverables written across {} targets",
                            audit.target_progress.len()
                        ),
                        cx,
                    );
                }
                cx.notify();
            });
        })
        .detach();
    }

    pub(super) fn retry_delivery_target(&mut self, target_id: &str, cx: &mut Context<Self>) {
        self.run_delivery_status_action(target_id, true, cx);
    }

    pub(super) fn regenerate_delivery_target(&mut self, target_id: &str, cx: &mut Context<Self>) {
        self.run_delivery_status_action(target_id, false, cx);
    }

    fn run_delivery_status_action(
        &mut self,
        target_id: &str,
        failed: bool,
        cx: &mut Context<Self>,
    ) {
        if self.converting || self.job_choice_pending() {
            return;
        }
        let Some(progress) = self.target_progress.get(target_id) else {
            return;
        };
        let selected: Vec<usize> = progress
            .items
            .iter()
            .filter_map(|(index, state)| {
                let match_state = if failed {
                    matches!(state, TargetItemState::Failed(_))
                } else {
                    matches!(state, TargetItemState::Outdated)
                };
                match_state.then_some(*index)
            })
            .collect();
        if selected.is_empty() {
            return;
        }
        self.start_delivery_conversion(
            selected,
            DeliveryRunMode::Selected,
            Some(target_id.to_string()),
            cx,
        );
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

    #[test]
    fn frozen_source_identity_refuses_an_edit_between_targets() {
        let dir = tempfile::tempdir().unwrap();
        let root = dir.path();
        let source = root.join("shot.png");
        let output = root.join("target").join("shot.webp");
        image::ImageBuffer::<image::Rgb<u8>, Vec<u8>>::from_fn(8, 8, |x, y| {
            image::Rgb([x as u8, y as u8, 20])
        })
        .save(&source)
        .unwrap();
        let expected = capture_source_identity(&source).unwrap();
        image::ImageBuffer::<image::Rgb<u8>, Vec<u8>>::from_fn(8, 8, |x, y| {
            image::Rgb([x as u8, y as u8, 200])
        })
        .save(&source)
        .unwrap();
        let result = convert::convert_to_expected_with_avif_speed(
            &root.join("target"),
            &source,
            &output,
            None,
            Format::WebP,
            Quality::lossy(80.),
            MaxEdge::FULL,
            &expected,
            crate::avif::DEFAULT_SPEED,
        );
        assert_eq!(result, Err(convert::Failure::SourceChanged));
        assert!(!output.exists(), "a changed source writes no target output");
    }

    #[test]
    fn target_progress_keeps_success_and_failure_distinct() {
        let target = crate::job::JobTarget {
            id: "web".into(),
            name: "Website".into(),
            recipe: Some("small-files".into()),
            recipe_snapshot: None,
            out: PathBuf::from("website"),
        };
        let mut progress = TargetProgress::new(&target);
        progress.items.insert(1, TargetItemState::Written);
        progress
            .items
            .insert(2, TargetItemState::Failed("source changed".into()));
        progress.items.insert(3, TargetItemState::Cancelled);
        progress.items.insert(4, TargetItemState::Outdated);
        progress.items.insert(5, TargetItemState::Unstarted);
        assert_eq!(progress.written(), 1);
        assert_eq!(progress.failed(), 1);
        assert_eq!(progress.cancelled(), 1);
        assert_eq!(progress.outdated(), 1);
        assert_eq!(progress.unstarted(), 1);
    }
}
