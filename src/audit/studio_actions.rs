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
        matches!(self.state, StudioJobState::Running)
    }

    pub(super) fn message(&self, root: &Path) -> String {
        match &self.state {
            StudioJobState::Running => {
                format!(
                    "Running {} on {} in Studio…",
                    self.tool.label(),
                    self.source_name
                )
            }
            StudioJobState::Done(path) => format!(
                "Studio saved {}",
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

    pub(super) fn save_studio_key(&mut self, cx: &mut Context<Self>) {
        if self.studio_key_checking {
            return;
        }
        let key = self.studio_key_input.read(cx).value().trim().to_string();
        if !key.starts_with("sk_live_") {
            self.studio_status = Some((false, "Studio API keys start with sk_live_".into()));
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
                        audit.studio_status =
                            Some((true, "Studio API key verified and saved".into()));
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
                self.studio_status =
                    Some((true, "Studio API key forgotten on this computer".into()));
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
        if self.studio_busy() || self.local_ai_busy() || self.converting {
            return;
        }
        let Some(key) = self.studio_key.clone() else {
            self.studio_status = Some((false, "Save a Studio API key first".into()));
            cx.notify();
            return;
        };
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
        let root = self.root.clone();
        let out_dir = self.output.root(&self.root);
        let dataset_generation = self.dataset_generation;
        let cancelled = Arc::new(std::sync::atomic::AtomicBool::new(false));
        self.studio_job = Some(StudioJob {
            tool,
            index,
            dataset_generation,
            source_name,
            state: StudioJobState::Running,
            cancelled: cancelled.clone(),
        });
        self.studio_status = None;
        cx.notify();

        cx.spawn(async move |this, cx| {
            let process_cancelled = cancelled.clone();
            let result = cx
                .background_executor()
                .spawn(async move {
                    studio::process(
                        &key,
                        &root,
                        &out_dir,
                        &source,
                        &output_source,
                        tool,
                        &prompt,
                        &process_cancelled,
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
        let busy = self.converting || self.local_ai_busy() || self.studio_busy();

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
                            .child("Runs through the Studio API and brings the image back here."),
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
                            .child(Input::new(&self.studio_prompt).small())
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
                                "Studio API key"
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
                                Input::new(&self.studio_key_input)
                                    .small()
                                    .content_type(InputContentType::Password)
                                    .mask_toggle(),
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
                            .label(format!("Run {}", chosen.label()))
                            .loading(
                                self.studio_job
                                    .as_ref()
                                    .is_some_and(|job| job.busy() && job.tool == chosen),
                            )
                            .disabled(index.is_none() || !has_key || prompt_missing || busy)
                            .on_click(cx.listener(move |audit, _, _, cx| {
                                if let Some(index) = index {
                                    audit.start_studio(index, None, cx);
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
        let prompt_missing = tool.needs_prompt() && self.studio_prompt_text(cx).is_empty();
        let reason = if self.studio_key.is_none() {
            "Open the Studio rail and save an API key".to_string()
        } else if index.is_none() {
            "Select one image to run in Studio".to_string()
        } else if prompt_missing {
            format!("Open the Studio rail and add a prompt for {}", tool.label())
        } else {
            format!("Run {} through the Studio API", tool.label())
        };
        let running = self
            .studio_job
            .as_ref()
            .is_some_and(|job| job.busy() && job.tool == tool && Some(job.index) == index);
        Button::new(id)
            .small()
            .outline()
            .when_some(crate::assets::studio_icon(tool.slug()), |button, path| {
                button.icon(Icon::default().path(path))
            })
            .when(labelled, |button| button.label("Studio"))
            .tooltip(reason)
            .loading(running)
            .disabled(
                self.studio_key.is_none()
                    || index.is_none()
                    || prompt_missing
                    || busy
                    || self.studio_busy(),
            )
            .on_click(cx.listener(move |audit, _, _, cx| {
                if let Some(index) = index {
                    audit.start_studio(index, written.clone(), cx);
                }
            }))
    }
}
