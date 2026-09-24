//! Saved plans in the window: write one, open one back, run it, continue it.
//!
//! A saved plan is reviewed before it converts anything, so the window's part is
//! small and explicit. Creating a plan re-reads the folder rather than trusting
//! the list on screen, because the plan records the bytes it hashed. Running one
//! goes through the same `saved_plan` engine the command line uses, on the
//! background executor, with Stop between files. Rendering only reads what a
//! landed task left behind.

use super::*;
use crate::saved_plan::{self, ExecutionMode};

/// How often a running plan's counter reaches the window. The engine writes its
/// run state after every file, so this only paces the number people read.
const PLAN_TICK: Duration = Duration::from_millis(250);

/// The plan the window has in hand.
pub(super) struct PlanWork {
    pub(super) path: PathBuf,
    /// Items the plan holds: sources times targets. A run counts against this
    /// before its first result lands.
    pub(super) items: usize,
    /// Set when the plan was reviewed against a requirements snapshot. The
    /// window has no picker for that document and must not run the plan without
    /// it, so those plans stay a command-line job.
    pub(super) requirements: Option<String>,
}

impl Audit {
    /// A run is on the executor. Conversion and a second plan command wait.
    pub(super) fn plan_busy(&self) -> bool {
        self.plan_cancel.is_some()
    }

    /// The plan file's own name, which is how people asked for it in the picker.
    fn plan_label(path: &Path) -> String {
        path.file_name()
            .map(|name| name.to_string_lossy().to_string())
            .unwrap_or_else(|| path.display().to_string())
    }

    /// The folder and destination a saved-plan command binds. `None` means the
    /// window said why it cannot.
    fn plan_binding(&mut self, cx: &mut Context<Self>) -> Option<(PathBuf, Output)> {
        if self.batch_size.is_some() {
            self.notify_error(
                "plans",
                "A saved plan needs a folder",
                "This list is an explicit batch of files. Open the folder itself to plan it.",
                cx,
            );
            return None;
        }
        if self.output == Output::Replace {
            self.notify_error(
                "plans",
                "A saved plan needs an output folder",
                "Replace rewrites the source folder, which a reviewed plan never does. \
                 Choose an output folder first.",
                cx,
            );
            return None;
        }
        Some((self.root.clone(), self.output.clone()))
    }

    /// The ticked rows, as names relative to the folder on screen. Only plan
    /// creation reads the selection: a saved plan carries its own list, and a
    /// run that asked the window again would run something else.
    fn plan_selection(&mut self, cx: &mut Context<Self>) -> Option<Vec<PathBuf>> {
        let targets = self.targets();
        if targets.is_empty() {
            self.notify_error(
                "plans",
                "Nothing is selected",
                "Tick the images the plan should hold.",
                cx,
            );
            return None;
        }
        let mut relative = Vec::with_capacity(targets.len());
        for index in targets {
            let Some(entry) = self.entries.get(index) else {
                continue;
            };
            match entry.path.strip_prefix(&self.root) {
                Ok(name) => relative.push(name.to_path_buf()),
                Err(_) => {
                    self.notify_error(
                        "plans",
                        "A saved plan needs one source folder",
                        format!("{} sits outside this folder.", entry.path.display()),
                        cx,
                    );
                    return None;
                }
            }
        }
        Some(relative)
    }

    /// Write the ticked rows and the current settings out as a plan.
    ///
    /// The folder is read again here rather than reused from the list: the plan
    /// records a hash and the measurements taken from those same bytes, and a
    /// row on screen may be minutes old. A file that has gone since the audit is
    /// named instead of quietly dropped from the plan.
    pub(super) fn save_plan_file(&mut self, cx: &mut Context<Self>) {
        if self.converting || self.plan_busy() {
            return;
        }
        let Some((root, output)) = self.plan_binding(cx) else {
            return;
        };
        let Some(wanted) = self.plan_selection(cx) else {
            return;
        };
        let subfolders = self.dataset_subfolders;
        let target = saved_plan::TargetInput {
            id: "default".into(),
            out: String::new(),
            recipe: saved_plan::EffectiveRecipe::from_settings(
                self.format,
                self.quality,
                self.max_edge,
                Some(crate::avif::speed()),
            ),
        };
        let dataset = self.dataset_generation;
        cx.spawn(async move |this, cx| {
            let picked = cx
                .background_executor()
                .spawn(async move {
                    rfd::FileDialog::new()
                        .set_file_name("plan.json")
                        .add_filter("Saved plan", &["json"])
                        .save_file()
                })
                .await;
            let Some(plan_path) = picked else { return };
            let written = plan_path.clone();
            let built = cx
                .background_executor()
                .spawn(async move {
                    let (source, output_root) = plan_roots(&root, &output)?;
                    let scan = plan_scan(&source, &output_root, subfolders)?;
                    let errors = scan.unreadable.len() + scan.walk_errors.len();
                    let wanted: std::collections::HashSet<PathBuf> = wanted.into_iter().collect();
                    let entries: Vec<Entry> = scan
                        .entries
                        .into_iter()
                        .filter(|entry| {
                            entry
                                .path
                                .strip_prefix(&source)
                                .is_ok_and(|name| wanted.contains(name))
                        })
                        .collect();
                    if entries.len() != wanted.len() {
                        return Err(format!(
                            "the folder changed since the audit: {} of {} selected files are still there. \
                             Rescan the folder and save the plan again.",
                            entries.len(),
                            wanted.len()
                        ));
                    }
                    let plan = saved_plan::build(
                        &source,
                        &output_root,
                        &entries,
                        errors,
                        vec![target],
                        None,
                    )?;
                    saved_plan::save_new(&written, &plan)?;
                    Ok::<usize, String>(plan.sources.len())
                })
                .await;
            let _ = this.update(cx, |audit, cx| {
                if audit.dataset_generation != dataset {
                    return;
                }
                match built {
                    Ok(sources) => {
                        audit.plan_open = true;
                        audit.plan_done = 0;
                        audit.plan_status = Some(format!(
                            "{sources} files planned. Nothing is converted yet."
                        ));
                        audit.plan_work = Some(PlanWork {
                            path: plan_path.clone(),
                            items: sources,
                            requirements: None,
                        });
                        audit.notify_success(
                            "plans",
                            "Saved the plan",
                            Self::plan_label(&plan_path),
                            cx,
                        );
                        cx.notify();
                    }
                    Err(message) => {
                        audit.notify_error("plans", "Couldn’t save the plan", message, cx);
                    }
                }
            });
        })
        .detach();
    }

    /// Open a plan file for this folder. Loading proves the document; it never
    /// touches the sources or the output.
    pub(super) fn open_plan_file(&mut self, cx: &mut Context<Self>) {
        if self.converting || self.plan_busy() {
            return;
        }
        let dataset = self.dataset_generation;
        cx.spawn(async move |this, cx| {
            let picked = cx
                .background_executor()
                .spawn(async move {
                    rfd::FileDialog::new()
                        .add_filter("Saved plan", &["json"])
                        .pick_file()
                })
                .await;
            let Some(plan_path) = picked else { return };
            let read = plan_path.clone();
            let loaded = cx
                .background_executor()
                .spawn(async move { saved_plan::load(&read) })
                .await;
            let _ = this.update(cx, |audit, cx| {
                if audit.dataset_generation != dataset {
                    return;
                }
                match loaded {
                    Ok(plan) => {
                        let items = plan.sources.len() * plan.targets.len();
                        let requirements = plan
                            .requirements
                            .as_ref()
                            .map(|rules| format!("{} rev {}", rules.id, rules.revision));
                        audit.plan_open = true;
                        audit.plan_done = 0;
                        audit.plan_status = Some(match requirements.as_deref() {
                            Some(rules) => format!(
                                "{items} items, reviewed against {rules}. Run it from the command line with --requirements-file."
                            ),
                            None => format!("{items} items. Run it to convert them."),
                        });
                        audit.plan_work = Some(PlanWork {
                            path: plan_path,
                            items,
                            requirements,
                        });
                        cx.notify();
                    }
                    Err(message) => {
                        audit.notify_error("plans", "Couldn’t open the plan", message, cx);
                    }
                }
            });
        })
        .detach();
    }

    /// Run, continue or repair the open plan. Every mode binds the same two
    /// roots the plan was saved against and reports what the engine recorded.
    pub(super) fn run_plan(&mut self, mode: Option<ExecutionMode>, cx: &mut Context<Self>) {
        if self.converting || self.plan_busy() || self.restoring || self.sirv_busy() {
            return;
        }
        let Some(work) = self.plan_work.as_ref() else {
            return;
        };
        if let Some(rules) = work.requirements.as_deref() {
            self.notify_error(
                "plans",
                "This plan needs its requirements file",
                format!(
                    "It was reviewed against {rules}, and the window has nowhere to supply that \
                     document. Run `press execute … --requirements-file` instead."
                ),
                cx,
            );
            return;
        }
        let plan_path = work.path.clone();
        let total = work.items;
        let Some((root, output)) = self.plan_binding(cx) else {
            return;
        };
        let cancel = Arc::new(AtomicBool::new(false));
        let done = Arc::new(std::sync::atomic::AtomicUsize::new(0));
        self.plan_cancel = Some(cancel.clone());
        self.plan_done = 0;
        self.plan_status = Some(match mode {
            Some(_) => format!("Running: 0 of {total}."),
            None => "Checking the outputs this plan describes…".to_string(),
        });
        // Stop lives inside this section, so a run never starts behind a fold
        // the person would have to remember to open to reach it.
        self.plan_open = true;
        self.open_rail(Rail::Convert, cx);
        cx.notify();

        let dataset = self.dataset_generation;
        let ticking = (cancel.clone(), done.clone());
        cx.spawn(async move |this, cx| {
            // The counter reaches the window on its own task: the run itself is
            // one blocking call on the executor and cannot report from there.
            let (token, counter) = ticking;
            loop {
                cx.background_executor().timer(PLAN_TICK).await;
                let stop = this
                    .update(cx, |audit, cx| {
                        if !audit.plan_run_applies(dataset, &token) {
                            return true;
                        }
                        let done = counter.load(Ordering::Acquire);
                        if done != audit.plan_done {
                            audit.plan_done = done;
                            audit.plan_status = Some(format!("Running: {done} of {total}."));
                            cx.notify();
                        }
                        false
                    })
                    .unwrap_or(true);
                if stop {
                    break;
                }
            }
        })
        .detach();

        cx.spawn(async move |this, cx| {
            let watched = cancel.clone();
            let counted = done.clone();
            let outcome = cx
                .background_executor()
                .spawn(async move {
                    let (source, output_root) = plan_roots(&root, &output)?;
                    match mode {
                        Some(mode) => saved_plan::execute(
                            &plan_path,
                            &source,
                            &output_root,
                            mode,
                            None,
                            &mut |_| {
                                counted.fetch_add(1, Ordering::Release);
                                !watched.load(Ordering::Acquire)
                            },
                        ),
                        None => saved_plan::reconcile(&plan_path, &source, &output_root, None),
                    }
                })
                .await;
            let _ = this.update(cx, |audit, cx| {
                if !audit.plan_run_applies(dataset, &cancel) {
                    return;
                }
                let stopped = cancel.load(Ordering::Acquire);
                audit.plan_cancel = None;
                match outcome {
                    Ok(result) => {
                        let counts = &result.report.counts;
                        let mut line = format!(
                            "{} written · {} failed · {} left",
                            counts.written, counts.failed, counts.unstarted
                        );
                        if counts.cancelled != 0 {
                            line.push_str(&format!(" · {} cancelled", counts.cancelled));
                        }
                        if counts.requirements_failed != 0 {
                            line.push_str(&format!(
                                " · {} missed their requirements",
                                counts.requirements_failed
                            ));
                        }
                        if stopped {
                            line.push_str(". Stopped; run it again to continue.");
                        }
                        audit.plan_done = counts.written + counts.failed + counts.cancelled;
                        audit.plan_status = Some(line);
                        audit.refresh_after_plan(cx);
                    }
                    Err(message) => {
                        audit.plan_status = Some(message.clone());
                        audit.notify_error("plans", "The plan did not run", message, cx);
                    }
                }
                cx.notify();
            });
        })
        .detach();
    }

    /// Ask a running plan to stop. The engine checks between files, so the run
    /// state and the folder still describe the same outputs when it lands.
    pub(super) fn stop_plan(&mut self, cx: &mut Context<Self>) {
        if let Some(cancel) = self.plan_cancel.as_ref() {
            cancel.store(true, Ordering::Release);
            self.plan_status = Some("Stopping after this file…".to_string());
            cx.notify();
        }
    }

    /// This landing belongs to the run the window is still showing.
    fn plan_run_applies(&self, dataset: u64, cancel: &Arc<AtomicBool>) -> bool {
        self.dataset_generation == dataset
            && self
                .plan_cancel
                .as_ref()
                .is_some_and(|current| Arc::ptr_eq(current, cancel))
    }

    /// A plan writes into the folder the list is showing results from, so the
    /// same refresh an ordinary conversion ends with applies here.
    fn refresh_after_plan(&mut self, cx: &mut Context<Self>) {
        self.refresh_job_states(cx);
    }

    pub(super) fn plan_section(&self, cx: &Context<Self>) -> impl IntoElement + use<> {
        let open = self.plan_open;
        let busy = self.converting || self.plan_busy();
        let running = self.plan_busy();
        let has_plan = self.plan_work.is_some();
        let needs_file = self
            .plan_work
            .as_ref()
            .is_some_and(|work| work.requirements.is_some());
        let name = self
            .plan_work
            .as_ref()
            .map(|work| Self::plan_label(&work.path));
        let status = self.plan_status.clone();
        let menu = cx.entity().downgrade();
        div()
            .debug_selector(|| "plan-section".into())
            .flex()
            .flex_col()
            .flex_shrink_0()
            .min_w_0()
            .gap_1()
            .child(
                div()
                    .flex()
                    .items_center()
                    .justify_between()
                    .child(
                        Button::new("plan-toggle")
                            .debug_selector(|| "plan-toggle".into())
                            .small()
                            .ghost()
                            .icon(if open {
                                IconName::ChevronDown
                            } else {
                                IconName::ChevronRight
                            })
                            .label("Saved plan")
                            .on_click(cx.listener(|audit, _, _, cx| {
                                audit.plan_open = !audit.plan_open;
                                cx.notify();
                            })),
                    )
                    .child(
                        div().debug_selector(|| "plan-menu".into()).child(
                            Button::new("plan-actions")
                                .small()
                                .ghost()
                                .label(name.unwrap_or_else(|| "No plan".to_string()))
                                .dropdown_caret(true)
                                .disabled(busy)
                                .dropdown_menu(move |menu_builder, _, _| {
                                    let save = menu.clone();
                                    let open_plan = menu.clone();
                                    let retry = menu.clone();
                                    let check = menu.clone();
                                    let act = |audit: &gpui_kit::WeakEntity<Audit>,
                                               cx: &mut App,
                                               act: fn(&mut Audit, &mut Context<Audit>)| {
                                        if let Some(audit) = audit.upgrade() {
                                            audit.update(cx, act);
                                        }
                                    };
                                    menu_builder
                                        .item(PopupMenuItem::new("Save plan…").on_click(
                                            move |_, _, cx| act(&save, cx, Audit::save_plan_file),
                                        ))
                                        .item(PopupMenuItem::new("Open plan…").on_click(
                                            move |_, _, cx| {
                                                act(&open_plan, cx, Audit::open_plan_file)
                                            },
                                        ))
                                        .separator()
                                        .item(PopupMenuItem::new("Retry failed items").on_click(
                                            move |_, _, cx| {
                                                if let Some(audit) = retry.upgrade() {
                                                    audit.update(cx, |audit, cx| {
                                                        audit.run_plan(
                                                            Some(ExecutionMode::RetryFailed),
                                                            cx,
                                                        );
                                                    });
                                                }
                                            },
                                        ))
                                        .item(PopupMenuItem::new("Check written outputs").on_click(
                                            move |_, _, cx| {
                                                if let Some(audit) = check.upgrade() {
                                                    audit.update(cx, |audit, cx| {
                                                        audit.run_plan(None, cx);
                                                    });
                                                }
                                            },
                                        ))
                                }),
                        ),
                    ),
            )
            .children(open.then(|| {
                div()
                    .debug_selector(|| "plan-body".into())
                    .flex()
                    .flex_col()
                    .gap_1()
                    .child(
                        div()
                            .debug_selector(|| "plan-status".into())
                            .text_size(px(11.))
                            .text_color(cx.theme().muted_foreground)
                            .child(status.unwrap_or_else(|| {
                                "Save the ticked images and these settings as a plan, review it, \
                                 then run it here or from the command line."
                                    .to_string()
                            })),
                    )
                    .children((has_plan && !needs_file).then(|| {
                        div().debug_selector(|| "plan-run".into()).child(
                            Button::new("plan-run")
                                .small()
                                .w_full()
                                .when(!running, |button| button.primary())
                                .label(if running { "Stop" } else { "Run plan" })
                                .disabled(self.converting)
                                .on_click(cx.listener(|audit, _, _, cx| {
                                    if audit.plan_busy() {
                                        audit.stop_plan(cx);
                                    } else {
                                        audit.run_plan(Some(ExecutionMode::ContinueUnstarted), cx);
                                    }
                                })),
                        )
                    }))
            }))
    }
}

/// The two roots a saved-plan command binds, resolved the way the command line
/// resolves them: the source folder as the kernel opens it, the output through
/// the same boundary check the window's Output setting uses everywhere else.
fn plan_roots(root: &Path, output: &Output) -> Result<(PathBuf, PathBuf), String> {
    let source = std::fs::canonicalize(root)
        .map_err(|error| format!("this folder cannot be established: {error}"))?;
    if !source.is_dir() {
        return Err(format!("{} is not a folder", source.display()));
    }
    let context = output.context(&source)?;
    Ok((source, context.output_root().to_path_buf()))
}

fn plan_scan(source: &Path, output_root: &Path, subfolders: bool) -> Result<scan::Scan, String> {
    if subfolders {
        Ok(scan::scan(source, output_root))
    } else {
        scan::browse(source, output_root)
            .map(|browse| browse.scan)
            .map_err(|error| format!("this folder cannot be read: {error}"))
    }
}
