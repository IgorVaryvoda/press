//! Direct Studio API connection, rail, and one-at-a-time image jobs.

use super::*;

pub(super) fn studio_landing_applies(
    job: Option<&StudioJob>,
    index: usize,
    dataset_generation: u64,
    tool: studio::Tool,
) -> bool {
    job.is_some_and(|job| {
        job.index == index
            && job.dataset_generation == dataset_generation
            && job.tool == tool
            && !job.cancelled.load(std::sync::atomic::Ordering::Relaxed)
    })
}

impl StudioJob {
    pub(super) fn busy(&self) -> bool {
        matches!(
            self.state,
            StudioJobState::Preparing | StudioJobState::Running
        )
    }

    pub(super) fn message(&self, root: &Path) -> String {
        match &self.state {
            StudioJobState::Preparing => {
                format!("Preparing {} for Studio…", self.source_name)
            }
            StudioJobState::AwaitingConfirmation(upload) => format!(
                "Press will make a temporary {}×{} WebP upload copy of {}. It may reduce visible quality; the original stays untouched.",
                upload.width, upload.height, self.source_name
            ),
            StudioJobState::Running => {
                format!(
                    "Running {} on {} with AI operations…",
                    self.tool.label(),
                    self.source_name
                )
            }
            StudioJobState::Done(path) => format!(
                "AI result saved {}",
                path.strip_prefix(root).unwrap_or(path).display()
            ),
            StudioJobState::Failed(message) => {
                format!(
                    "{} failed for {}: {message}",
                    self.tool.label(),
                    self.source_name
                )
            }
        }
    }
}

impl Audit {
    pub(super) fn studio_busy(&self) -> bool {
        self.studio_key_checking || self.studio_job.as_ref().is_some_and(StudioJob::busy)
    }

    fn studio_prompt_text(&self, cx: &App) -> String {
        self.studio_prompt.read(cx).value().trim().to_string()
    }

    pub(super) fn studio_input_focused(&self, window: &Window, cx: &App) -> bool {
        self.studio_prompt
            .read(cx)
            .focus_handle(cx)
            .is_focused(window)
            || self
                .studio_key_input
                .read(cx)
                .focus_handle(cx)
                .is_focused(window)
    }

    pub(super) fn save_studio_key(&mut self, cx: &mut Context<Self>) {
        if self.studio_key_checking {
            return;
        }
        let key = self.studio_key_input.read(cx).value().trim().to_string();
        if !key.starts_with("sk_live_") {
            self.studio_status = Some((false, "AI API keys start with sk_live_".into()));
            cx.notify();
            return;
        }
        self.studio_key_checking = true;
        self.studio_status = None;
        cx.notify();

        cx.spawn(async move |this, cx| {
            let checked = cx
                .background_executor()
                .spawn({
                    let key = key.clone();
                    async move {
                        studio::verify_key(&key)?;
                        studio::save_key(&key)?;
                        Ok::<_, String>(key)
                    }
                })
                .await;
            let _ = this.update(cx, |audit, cx| {
                audit.studio_key_checking = false;
                match checked {
                    Ok(key) => {
                        audit.studio_key = Some(key);
                        audit.studio_status = Some((true, "AI API key verified and saved".into()));
                    }
                    Err(message) => audit.studio_status = Some((false, message)),
                }
                cx.notify();
            });
        })
        .detach();
    }

    fn forget_studio_key(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        match studio::forget_key() {
            Ok(()) => {
                self.studio_key = None;
                self.studio_status = Some((true, "AI API key forgotten on this computer".into()));
                self.studio_key_input.update(cx, |input, cx| {
                    input.set_value("", window, cx);
                });
            }
            Err(message) => self.studio_status = Some((false, message)),
        }
        cx.notify();
    }

    pub(super) fn start_studio(
        &mut self,
        index: usize,
        written: Option<PathBuf>,
        cx: &mut Context<Self>,
    ) {
        if self.scan_blocks_delivery()
            || self.studio_busy()
            || self.local_ai_busy()
            || self.converting
        {
            return;
        }
        if self.studio_key.is_none() {
            self.studio_status = Some((false, "Save an AI API key first".into()));
            cx.notify();
            return;
        }
        let Some(entry) = self.entries.get(index) else {
            return;
        };
        let tool = self.studio_tool;
        let prompt = self.studio_prompt_text(cx);
        if tool.needs_prompt() && prompt.is_empty() {
            self.studio_status = Some((false, format!("{} needs a prompt", tool.label())));
            cx.notify();
            return;
        }

        let source = written.unwrap_or_else(|| entry.path.clone());
        let output_source = entry.path.clone();
        let source_name = source
            .file_name()
            .map(|name| name.to_string_lossy().into_owned())
            .unwrap_or_else(|| entry.name());
        let dataset_generation = self.dataset_generation;
        let cancelled = Arc::new(std::sync::atomic::AtomicBool::new(false));
        self.studio_job = Some(StudioJob {
            tool,
            index,
            dataset_generation,
            source_name,
            output_source,
            prompt,
            state: StudioJobState::Preparing,
            cancelled: cancelled.clone(),
        });
        self.studio_status = None;
        cx.notify();

        cx.spawn(async move |this, cx| {
            let result = cx
                .background_executor()
                .spawn(async move { studio::prepare_upload(&source, &cancelled) })
                .await;
            let _ = this.update(cx, |audit, cx| {
                if !studio_landing_applies(
                    audit.studio_job.as_ref(),
                    index,
                    dataset_generation,
                    tool,
                ) {
                    return;
                }
                if audit.scan_blocks_delivery() {
                    audit.retire_studio_for_scan(cx);
                    return;
                }
                match result {
                    Ok(studio::Preflight::Ready(upload)) => {
                        audit.run_prepared_studio(upload, cx);
                    }
                    Ok(studio::Preflight::NeedsConfirmation(upload)) => {
                        audit.studio_job.as_mut().unwrap().state =
                            StudioJobState::AwaitingConfirmation(upload);
                        audit.rail = Rail::Studio;
                    }
                    Err(message) => {
                        audit.studio_job.as_mut().unwrap().state = StudioJobState::Failed(message);
                    }
                }
                cx.notify();
            });
        })
        .detach();
    }

    fn confirm_studio(&mut self, cx: &mut Context<Self>) {
        if self.scan_blocks_delivery() {
            self.retire_studio_for_scan(cx);
            return;
        }
        let upload = {
            let Some(job) = self.studio_job.as_mut() else {
                return;
            };
            match std::mem::replace(&mut job.state, StudioJobState::Preparing) {
                StudioJobState::AwaitingConfirmation(upload) => upload,
                state => {
                    job.state = state;
                    return;
                }
            }
        };
        self.run_prepared_studio(upload, cx);
    }

    fn run_prepared_studio(&mut self, upload: studio::PreparedUpload, cx: &mut Context<Self>) {
        if self.scan_blocks_delivery() {
            self.retire_studio_for_scan(cx);
            return;
        }
        let Some(key) = self.studio_key.clone() else {
            return;
        };
        let Some(job) = self.studio_job.as_mut() else {
            return;
        };
        job.state = StudioJobState::Running;
        let index = job.index;
        let dataset_generation = job.dataset_generation;
        let tool = job.tool;
        let prompt = job.prompt.clone();
        let output_source = job.output_source.clone();
        let cancelled = job.cancelled.clone();
        let root = self.root.clone();
        let out_dir = self.output.root(&self.root);
        cx.notify();

        cx.spawn(async move |this, cx| {
            let result = cx
                .background_executor()
                .spawn(async move {
                    studio::process_prepared(
                        &key,
                        &root,
                        &out_dir,
                        &upload,
                        &output_source,
                        tool,
                        &prompt,
                        &cancelled,
                    )
                })
                .await;
            let _ = this.update(cx, |audit, cx| {
                if !studio_landing_applies(
                    audit.studio_job.as_ref(),
                    index,
                    dataset_generation,
                    tool,
                ) {
                    return;
                }
                match result {
                    Ok(path) => {
                        audit.existing_output = audit.existing_output.saturating_add(1);
                        audit.studio_job.as_mut().unwrap().state =
                            StudioJobState::Done(path.clone());
                        audit.open_written(index, path, Some(ProducedBy::Studio(tool)), cx);
                    }
                    Err(message) => {
                        audit.studio_job.as_mut().unwrap().state = StudioJobState::Failed(message);
                    }
                }
                cx.notify();
            });
        })
        .detach();
    }

    fn retire_studio_for_scan(&mut self, cx: &mut Context<Self>) {
        if let Some(job) = self.studio_job.take() {
            job.cancelled
                .store(true, std::sync::atomic::Ordering::Release);
        }
        cx.notify();
    }

    pub(super) fn studio_rail(&self, cx: &mut Context<Self>) -> gpui::AnyElement {
        let index = self.single_target();
        let target = index
            .and_then(|index| self.entries.get(index))
            .map(Entry::name)
            .unwrap_or_else(|| "Select one image to run".into());
        let chosen = self.studio_tool;
        let prompt = self.studio_prompt_text(cx);
        let prompt_missing = chosen.needs_prompt() && prompt.is_empty();
        let has_key = self.studio_key.is_some();
        let busy = self.converting
            || self.local_ai_busy()
            || self.studio_busy()
            || self.scan_blocks_delivery();
        let awaiting_confirmation = self.studio_job.as_ref().is_some_and(|job| {
            job.index == index.unwrap_or(usize::MAX)
                && job.tool == chosen
                && matches!(job.state, StudioJobState::AwaitingConfirmation(_))
        });
        let written = self
            .studio_source
            .as_ref()
            .filter(|(source_index, _)| Some(*source_index) == index)
            .map(|(_, path)| path.clone());

        div()
            .flex()
            .flex_col()
            .flex_1()
            .min_h_0()
            .child(
                div()
                    .id("studio-tools")
                    .flex()
                    .flex_col()
                    .flex_1()
                    .min_h_0()
                    .overflow_y_scroll()
                    .gap_1()
                    .px_3()
                    .py_3()
                    .child(
                        div()
                            .text_size(px(11.))
                            .text_color(cx.theme().muted_foreground)
                            .pb_1()
                            .child("Uses hosted AI and brings the image back here."),
                    )
                    .children(studio::TOOLS.iter().copied().map(|tool| {
                        Button::new(gpui::SharedString::from(format!(
                            "studio-tool-{}",
                            tool.slug()
                        )))
                        .small()
                        .ghost()
                        .w_full()
                        .label(tool.label())
                        .when_some(crate::assets::studio_icon(tool.slug()), |button, path| {
                            button.icon(Icon::default().path(path))
                        })
                        .selected(chosen == tool)
                        .child(div().flex_1())
                        .on_click(cx.listener(move |audit, _, _, cx| {
                            audit.studio_tool = tool;
                            audit.studio_status = None;
                            cx.notify();
                        }))
                    }))
                    .when(chosen.needs_prompt(), |panel| {
                        panel
                            .child(
                                div()
                                    .pt_2()
                                    .text_size(px(11.))
                                    .text_color(cx.theme().muted_foreground)
                                    .child(chosen.prompt_placeholder()),
                            )
                            .child(
                                div()
                                    .debug_selector(|| "studio-prompt-input".into())
                                    .child(Input::new(&self.studio_prompt).small()),
                            )
                    })
                    .child(div().h(px(8.)))
                    .child(
                        div()
                            .flex()
                            .items_center()
                            .justify_between()
                            .text_size(px(11.))
                            .text_color(cx.theme().muted_foreground)
                            .child(if has_key {
                                "API key saved on this computer"
                            } else {
                                "AI API key"
                            })
                            .when(has_key, |row| {
                                row.child(
                                    Button::new("studio-forget-key")
                                        .xsmall()
                                        .ghost()
                                        .label("Forget")
                                        .disabled(busy)
                                        .on_click(cx.listener(|audit, _, window, cx| {
                                            audit.forget_studio_key(window, cx);
                                        })),
                                )
                            }),
                    )
                    .when(!has_key, |panel| {
                        panel
                            .child(
                                div().debug_selector(|| "studio-key-input".into()).child(
                                    Input::new(&self.studio_key_input)
                                        .small()
                                        .content_type(InputContentType::Password)
                                        .mask_toggle(),
                                ),
                            )
                            .child(
                                div()
                                    .flex()
                                    .gap_1()
                                    .child(
                                        Button::new("studio-get-key")
                                            .small()
                                            .ghost()
                                            .icon(IconName::ExternalLink)
                                            .label("Get API key")
                                            .on_click(|_, _, cx| cx.open_url(studio::API_KEYS_URL)),
                                    )
                                    .child(
                                        Button::new("studio-save-key")
                                            .small()
                                            .outline()
                                            .label("Verify & save")
                                            .loading(self.studio_key_checking)
                                            .disabled(
                                                self.studio_key_input
                                                    .read(cx)
                                                    .value()
                                                    .trim()
                                                    .is_empty()
                                                    || self.studio_key_checking,
                                            )
                                            .on_click(cx.listener(|audit, _, _, cx| {
                                                audit.save_studio_key(cx);
                                            })),
                                    ),
                            )
                    })
                    .when_some(self.studio_status.clone(), |panel, (ok, message)| {
                        panel.child(
                            div()
                                .pt_1()
                                .text_size(px(11.))
                                .text_color(if ok { cx.theme().green } else { cx.theme().red })
                                .child(message),
                        )
                    })
                    .when_some(self.studio_job.as_ref(), |panel, job| {
                        panel.child(
                            div()
                                .pt_1()
                                .text_size(px(11.))
                                .text_color(cx.theme().muted_foreground)
                                .child(job.message(&self.root)),
                        )
                    }),
            )
            .child(
                div()
                    .flex_shrink_0()
                    .flex()
                    .flex_col()
                    .gap_1()
                    .px_3()
                    .py_3()
                    .border_t_1()
                    .border_color(cx.theme().border)
                    .child(
                        div()
                            .text_size(px(11.))
                            .text_color(cx.theme().muted_foreground)
                            .overflow_hidden()
                            .text_ellipsis()
                            .whitespace_nowrap()
                            .child(target),
                    )
                    .child(
                        Button::new("studio-commit")
                            .primary()
                            .w_full()
                            .label(if awaiting_confirmation {
                                "Prepare upload copy & run".to_string()
                            } else {
                                format!("Run {}", chosen.label())
                            })
                            .loading(
                                self.studio_job
                                    .as_ref()
                                    .is_some_and(|job| job.busy() && job.tool == chosen),
                            )
                            .disabled(
                                index.is_none()
                                    || !has_key
                                    || !awaiting_confirmation && (prompt_missing || busy),
                            )
                            .on_click(cx.listener(move |audit, _, _, cx| {
                                if awaiting_confirmation {
                                    audit.confirm_studio(cx);
                                } else if let Some(index) = index {
                                    audit.start_studio(index, written.clone(), cx);
                                }
                            })),
                    ),
            )
            .into_any_element()
    }

    pub(super) fn studio_button(
        &self,
        id: &'static str,
        index: Option<usize>,
        written: Option<PathBuf>,
        labelled: bool,
        busy: bool,
        cx: &mut Context<Self>,
    ) -> Button {
        let tool = self.studio_tool;
        Button::new(id)
            .small()
            .outline()
            .when_some(crate::assets::studio_icon(tool.slug()), |button, path| {
                button.icon(Icon::default().path(path))
            })
            .when(labelled, |button| button.label("AI operations"))
            .tooltip("Open hosted AI operations for this image")
            .disabled(index.is_none() || busy || self.studio_busy())
            .on_click(cx.listener(move |audit, _, _, cx| {
                if let Some(index) = index {
                    audit.open_ai_operations(index, written.clone(), cx);
                }
            }))
    }
}
