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
        if self.local_ai_busy() {
            return;
        }
        let Some(entry) = self.entries.get(index) else {
            return;
        };
        if tool == local_ai::Tool::Upscale
            && let Err(message) = local_ai::upscale_dimensions(entry.width, entry.height)
        {
            self.local_ai_job = Some(LocalAiJob {
                tool,
                index,
                dataset_generation: self.dataset_generation,
                source_name: entry.name(),
                first_setup: false,
                state: LocalAiJobState::Failed(message),
                cancelled: Arc::new(std::sync::atomic::AtomicBool::new(false)),
            });
            cx.notify();
            return;
        }

        let source = entry.path.clone();
        let source_name = entry.name();
        let root = self.root.clone();
        let out_dir = self.output.root(&self.root);
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
                            audit.local_ai_job.as_mut().unwrap().state =
                                LocalAiJobState::Failed(message);
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
                    );
                    if applies {
                        audit.local_ai_job.as_mut().unwrap().state = LocalAiJobState::Running;
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
                        audit.existing_output = audit.existing_output.saturating_add(1);
                        audit.local_ai_job.as_mut().unwrap().state =
                            LocalAiJobState::Done(path.clone());
                        // A model that ran for thirty seconds and answered with a
                        // line of green text was asking you to go and find its
                        // work. Open it instead.
                        audit.open_written(index, path, Some(tool), cx);
                    }
                    Err(message) => {
                        audit.local_ai_job.as_mut().unwrap().state =
                            LocalAiJobState::Failed(message);
                    }
                }
                cx.notify();
            });
        })
        .detach();
    }
}
