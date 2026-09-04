//! One-at-a-time local AI jobs launched from the comparison view.

use super::*;

pub(super) fn local_ai_landing_applies(
    job: Option<&LocalAiJob>,
    index: usize,
    dataset_generation: u64,
    tool: local_ai::Tool,
) -> bool {
    job.is_some_and(|job| {
        job.index == index
            && job.dataset_generation == dataset_generation
            && job.tool == tool
            && !job.cancelled.load(std::sync::atomic::Ordering::Relaxed)
    })
}

impl LocalAiJob {
    pub(super) fn busy(&self) -> bool {
        matches!(
            self.state,
            LocalAiJobState::SettingUp | LocalAiJobState::Running
        )
    }

    pub(super) fn message(&self, root: &Path) -> String {
        match &self.state {
            LocalAiJobState::SettingUp => match (self.tool, self.first_setup) {
                (local_ai::Tool::RemoveBackground, true) => {
                    "Preparing background removal — first use downloads up to 104 MB…".into()
                }
                (local_ai::Tool::Upscale, true) => {
                    "Preparing 4× upscaling — first use downloads up to 49 MB…".into()
                }
                (local_ai::Tool::RemoveBackground, false) => "Preparing background removal…".into(),
                (local_ai::Tool::Upscale, false) => "Preparing 4× upscaling…".into(),
            },
            LocalAiJobState::Running => match self.tool {
                local_ai::Tool::RemoveBackground => {
                    format!("Removing the background from {}…", self.source_name)
                }
                local_ai::Tool::Upscale => format!("Upscaling {} 4×…", self.source_name),
            },
            LocalAiJobState::Done(path) => format!(
                "Saved to {}",
                path.strip_prefix(root).unwrap_or(path).display()
            ),
            LocalAiJobState::Failed(message) => match self.tool {
                local_ai::Tool::RemoveBackground => format!(
                    "Background removal failed for {}: {message}",
                    self.source_name
                ),
                local_ai::Tool::Upscale => {
                    format!("4× upscaling failed for {}: {message}", self.source_name)
                }
            },
        }
    }
}

impl Audit {
    pub(super) fn local_ai_busy(&self) -> bool {
        self.local_ai_job.as_ref().is_some_and(LocalAiJob::busy)
    }

    pub(super) fn start_local_ai(
        &mut self,
        tool: local_ai::Tool,
        index: usize,
        cx: &mut Context<Self>,
    ) {
        if self.scan_blocks_delivery()
            || self.local_ai_busy()
            || self.studio_busy()
            || self.converting
        {
            return;
        }
        let Some(entry) = self.entries.get(index) else {
            return;
        };
        self.clear_error("local-ai", cx);
        if tool == local_ai::Tool::Upscale
            && let Err(message) = local_ai::upscale_dimensions(entry.width, entry.height)
        {
            let source_name = entry_label(&self.root, self.show_parent(), entry);
            self.local_ai_job = Some(LocalAiJob {
                tool,
                index,
                dataset_generation: self.dataset_generation,
                source_name: source_name.clone(),
                first_setup: false,
                state: LocalAiJobState::Failed(message.clone()),
                cancelled: Arc::new(std::sync::atomic::AtomicBool::new(false)),
            });
            self.notify_error(
                "local-ai",
                "Couldn’t upscale image",
                format!("{source_name}: {message}"),
                cx,
            );
            cx.notify();
            return;
        }

        let source = entry.path.clone();
        let source_name = entry_label(&self.root, self.show_parent(), entry);
        let root = self.root.clone();
        let out_dir = match self.output.context(&self.root) {
            Ok(context) => context.output_root().to_path_buf(),
            Err(message) => {
                self.notify_error(
                    "local-ai",
                    "Couldn’t use the output folder",
                    format!("{}: {message}", self.output.label()),
                    cx,
                );
                return;
            }
        };
        let dataset_generation = self.dataset_generation;
        let cancelled = Arc::new(std::sync::atomic::AtomicBool::new(false));
        self.local_ai_job = Some(LocalAiJob {
            tool,
            index,
            dataset_generation,
            source_name,
            first_setup: !local_ai::installed(tool),
            state: LocalAiJobState::SettingUp,
            cancelled: cancelled.clone(),
        });
        cx.notify();

        cx.spawn(async move |this, cx| {
            let prepare_cancelled = cancelled.clone();
            let prepared = cx
                .background_executor()
                .spawn(async move { local_ai::prepare(tool, &prepare_cancelled) })
                .await;
            let prepared = match prepared {
                Ok(prepared) => prepared,
                Err(message) => {
                    let _ = this.update(cx, |audit, cx| {
                        if local_ai_landing_applies(
                            audit.local_ai_job.as_ref(),
                            index,
                            dataset_generation,
                            tool,
                        ) {
                            let Some(job) = audit.local_ai_job.as_mut() else {
                                return;
                            };
                            job.state = LocalAiJobState::Failed(message);
                            let Some(job) = audit.local_ai_job.as_ref() else {
                                return;
                            };
                            let message = job.message(&audit.root);
                            audit.notify_error("local-ai", "Local AI failed", message, cx);
                            cx.notify();
                        }
                    });
                    return;
                }
            };

            let applies = this
                .update(cx, |audit, cx| {
                    let applies = local_ai_landing_applies(
                        audit.local_ai_job.as_ref(),
                        index,
                        dataset_generation,
                        tool,
                    ) && !audit.scan_blocks_delivery();
                    if applies {
                        let Some(job) = audit.local_ai_job.as_mut() else {
                            return false;
                        };
                        job.state = LocalAiJobState::Running;
                        cx.notify();
                    } else if audit.scan_blocks_delivery() {
                        if let Some(job) = audit.local_ai_job.as_ref() {
                            job.cancelled
                                .store(true, std::sync::atomic::Ordering::Release);
                        }
                        audit.local_ai_job = None;
                        cx.notify();
                    }
                    applies
                })
                .unwrap_or(false);
            if !applies {
                cancelled.store(true, std::sync::atomic::Ordering::Relaxed);
                return;
            }

            let process_cancelled = cancelled.clone();
            let result = cx
                .background_executor()
                .spawn(async move {
                    local_ai::process(prepared, &root, &out_dir, &source, &process_cancelled)
                })
                .await;
            let _ = this.update(cx, |audit, cx| {
                if !local_ai_landing_applies(
                    audit.local_ai_job.as_ref(),
                    index,
                    dataset_generation,
                    tool,
                ) {
                    return;
                }
                match result {
                    Ok(path) => {
                        audit.clear_error("local-ai", cx);
                        audit.existing_output = audit.existing_output.saturating_add(1);
                        let Some(job) = audit.local_ai_job.as_mut() else {
                            return;
                        };
                        job.state = LocalAiJobState::Done(path.clone());
                        let Some(job) = audit.local_ai_job.as_ref() else {
                            return;
                        };
                        let message = job.message(&audit.root);
                        let title = match tool {
                            local_ai::Tool::RemoveBackground => "Background removal finished",
                            local_ai::Tool::Upscale => "Upscale finished",
                        };
                        audit.notify_success("local-ai", title, message, cx);
                        // A model that ran for thirty seconds and answered with a
                        // line of green text was asking you to go and find its
                        // work. Open it instead.
                        audit.open_written(index, path, Some(ProducedBy::Local(tool)), cx);
                    }
                    Err(message) => {
                        let Some(job) = audit.local_ai_job.as_mut() else {
                            return;
                        };
                        job.state = LocalAiJobState::Failed(message);
                        let Some(job) = audit.local_ai_job.as_ref() else {
                            return;
                        };
                        let message = job.message(&audit.root);
                        audit.notify_error("local-ai", "Local AI failed", message, cx);
                    }
                }
                cx.notify();
            });
        })
        .detach();
    }
}
