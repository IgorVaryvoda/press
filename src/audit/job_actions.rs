//! Product-set job actions: open, create, map, relink, import, export and
//! refresh. Mutations auto-save into the job library so restart recovery is
//! the normal case, not a mode. Filesystem reads (resolve, manifest, relink
//! targets) stay on background tasks; renders only read cached state.

use super::*;
use crate::job::{self, Job};

/// How many frames the review waits for a laid-out tab map before it gives up
/// on moving the keyboard onto its Export button.
const FOCUS_REVIEW_FRAMES: u8 = 4;

#[derive(Clone, Debug, PartialEq)]
pub(super) struct JobSelection {
    pub job: Job,
    pub choices: Vec<Job>,
}

pub(super) fn job_selection_for(root: &Path) -> JobSelection {
    match job::dir() {
        Some(dir) => job_selection_for_in(&dir, root),
        None => JobSelection {
            job: anonymous_job(root),
            choices: Vec::new(),
        },
    }
}

pub(super) fn job_selection_for_in(dir: &Path, root: &Path) -> JobSelection {
    let (jobs, _) = job::list(dir);
    let choices: Vec<Job> = jobs
        .into_iter()
        .filter(|known| {
            known
                .source_roots
                .iter()
                .any(|stored| roots_match(stored, root))
        })
        .collect();
    if choices.len() > 1 {
        return JobSelection {
            job: anonymous_job_in(dir, root),
            choices,
        };
    }
    JobSelection {
        job: choices
            .into_iter()
            .next()
            .unwrap_or_else(|| anonymous_job_in(dir, root)),
        choices: Vec::new(),
    }
}

/// The same lookup against an explicit library directory, so tests drive it
/// without touching the real config folder.
#[cfg(test)]
pub(super) fn load_job_for_in(dir: &Path, root: &Path) -> Job {
    job_selection_for_in(dir, root).job
}

fn anonymous_job(root: &Path) -> Job {
    Job {
        schema: job::SCHEMA_VERSION,
        id: "job".into(),
        name: "Job".into(),
        revision: 1,
        source_roots: vec![root.to_path_buf()],
        target_recipe: None,
        targets: Vec::new(),
        products: Vec::new(),
    }
}

fn anonymous_job_in(dir: &Path, root: &Path) -> Job {
    let name = root
        .file_name()
        .map(|name| name.to_string_lossy().into_owned())
        .filter(|name| !name.is_empty())
        .unwrap_or_else(|| "Job".into());
    Job::new(job::suggest_id(dir, &name), name, vec![root.to_path_buf()])
        .unwrap_or_else(|_| anonymous_job(root))
}

fn roots_match(known: &Path, root: &Path) -> bool {
    if known == root {
        return true;
    }
    match (std::fs::canonicalize(known), std::fs::canonicalize(root)) {
        (Ok(left), Ok(right)) => left == right,
        _ => false,
    }
}

fn job_request_matches(current: (u64, &str, u32, u64), expected: (u64, &str, u32, u64)) -> bool {
    current == expected
}

impl Audit {
    pub(super) fn job_choice_pending(&self) -> bool {
        !self.job_choices.is_empty()
    }

    pub(super) fn owns_job_request(
        &self,
        dataset: u64,
        id: &str,
        revision: u32,
        request_generation: u64,
    ) -> bool {
        job_request_matches(
            (
                self.dataset_generation,
                &self.work_job.id,
                self.work_job.revision,
                self.job_request_generation,
            ),
            (dataset, id, revision, request_generation),
        )
    }

    fn bump_job_request_generation(&mut self) {
        self.job_request_generation = self.job_request_generation.wrapping_add(1);
    }

    pub(super) fn job_dir_or_notify(&self, cx: &mut Context<Self>) -> Option<PathBuf> {
        match job::dir() {
            Some(dir) => Some(dir),
            None => {
                self.notify_error(
                    "jobs",
                    "Couldn’t use the job library",
                    "no config folder resolves on this machine",
                    cx,
                );
                None
            }
        }
    }

    /// Run a mutation, persist it, and refresh states. Every action below
    /// goes through here so no edit path forgets persistence.
    fn mutate_job(&mut self, dir: &Path, cx: &mut Context<Self>, f: impl FnOnce(&mut Job)) {
        if self.job_choice_pending() {
            self.notify_error(
                "jobs",
                "Choose a job first",
                "select a saved job or start a new one before editing",
                cx,
            );
            return;
        }
        self.job_export_preview = None;
        f(&mut self.work_job);
        self.persist_job(dir, cx);
        self.refresh_job_states(cx);
    }

    /// Bump, save, and report failure. Refresh stays with the caller: some
    /// paths re-resolve, others just failed and have nothing new to show.
    fn persist_job(&mut self, dir: &Path, cx: &mut Context<Self>) {
        self.work_job = self.work_job.bumped();
        if let Err(message) = job::save(dir, &self.work_job) {
            self.notify_error("jobs", "Couldn’t save the job", message, cx);
        }
    }

    /// Delete the open job's file and go anonymous. Sources, outputs and
    /// recipes live elsewhere; only the grouping goes, like deleting a recipe.
    pub(super) fn delete_job(&mut self, dir: &Path, cx: &mut Context<Self>) {
        if self.converting || self.job_choice_pending() {
            return;
        }
        let id = self.work_job.id.clone();
        match job::remove(dir, &id) {
            Ok(()) => {
                let JobSelection { job, choices } = job_selection_for_in(dir, &self.root.clone());
                self.bump_job_request_generation();
                self.work_job = job;
                self.job_choices = choices;
                self.job_export_preview = None;
                self.work_states.clear();
                self.work_stale.clear();
                cx.notify();
            }
            Err(message) => self.notify_error("jobs", "Couldn’t delete the job", message, cx),
        }
    }
    /// Bind the selected personal recipe as the job's target. Existence is
    /// checked at prepare time, not here: a deleted recipe must refuse by
    /// name rather than silently convert with current settings.
    pub(super) fn bind_current_recipe_as_target(&mut self, dir: &Path, cx: &mut Context<Self>) {
        if self.converting || self.job_choice_pending() {
            return;
        }
        let Some(id) = self.selected_recipe.clone() else {
            self.notify_error(
                "jobs",
                "Couldn’t set the target",
                "select a personal recipe first",
                cx,
            );
            return;
        };
        self.mutate_job(dir, cx, move |job| crate::job::bind_target(job, Some(&id)));
    }

    /// Clear the bound target. Conversion keeps using the current settings;
    /// the job just stops naming a recipe.
    pub(super) fn clear_job_target(&mut self, dir: &Path, cx: &mut Context<Self>) {
        if self.converting {
            return;
        }
        self.mutate_job(dir, cx, |job| crate::job::bind_target(job, None));
    }

    /// Re-resolve roles and staleness off the update path. The landing keeps
    /// the newest dataset's results only.
    pub(super) fn refresh_job_states(&mut self, cx: &mut Context<Self>) {
        if self.work_job.products.is_empty() {
            self.work_states.clear();
            self.work_stale.clear();
            return;
        }
        let generation = self.dataset_generation;
        // A newer mutation in the same dataset supersedes this refresh: every
        // mutation bumps the revision before refreshing, so fencing on both
        // drops a late landing from the mapping that no longer exists.
        let job_id = self.work_job.id.clone();
        let revision = self.work_job.revision;
        let request_generation = self.job_request_generation;
        let job = self.work_job.clone();
        let root = self.root.clone();
        let out_dir = self.output.root(&self.root);
        cx.spawn(async move |this, cx| {
            let (states, stale) = cx
                .background_executor()
                .spawn(async move {
                    let states: Vec<_> = job
                        .products
                        .iter()
                        .flat_map(|product| job::resolve_product(product, &root))
                        .collect();
                    let manifest = crate::manifest::load(&out_dir);
                    let stale = job::stale_deliverables(&manifest, &job, &root, &out_dir);
                    (states, stale)
                })
                .await;
            let _ = this.update(cx, |audit, cx| {
                if !audit.owns_job_request(generation, &job_id, revision, request_generation) {
                    return;
                }
                audit.work_states = states;
                audit.work_stale = stale;
                cx.notify();
            });
        })
        .detach();
    }

    fn job_text(&self, cx: &App, input: &gpui_kit::Entity<InputState>) -> String {
        input.read(cx).value().trim().to_string()
    }

    /// Start over with an anonymous job for the current folder. Saved files
    /// stay on disk; nothing is deleted.
    pub(super) fn new_job(&mut self, cx: &mut Context<Self>) {
        self.bump_job_request_generation();
        self.work_job = job::dir()
            .map(|dir| anonymous_job_in(&dir, &self.root.clone()))
            .unwrap_or_else(|| anonymous_job(&self.root.clone()));
        self.job_choices.clear();
        self.job_export_preview = None;
        self.work_states.clear();
        self.work_stale.clear();
        cx.notify();
    }

    pub(super) fn select_saved_job(&mut self, id: &str, cx: &mut Context<Self>) {
        let Some(index) = self.job_choices.iter().position(|job| job.id == id) else {
            return;
        };
        self.bump_job_request_generation();
        self.work_job = self.job_choices.remove(index);
        self.job_choices.clear();
        self.job_export_preview = None;
        self.work_states.clear();
        self.work_stale.clear();
        self.refresh_job_states(cx);
        cx.notify();
    }

    pub(super) fn open_job_export_preview(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        if self.converting || self.job_choice_pending() {
            return;
        }
        match self.work_job.export_draft(&self.root) {
            Ok(draft) => {
                self.job_export_preview = Some(draft);
                // `sets-section` is the last child of the rail settings area.
                // Scroll it to the top before focusing the first review action,
                // so opening Export always gives the user a visible decision.
                let item = 2 + usize::from(!self.recipes_skipped.is_empty());
                self.rail_scroll.scroll_to_top_of_item(item);
                // A frame callback, not a defer: `focus_next` reads the tab
                // stops of a frame that has been laid out, and a defer still
                // runs while the review is unrendered. Waiting for the frame
                // also outlasts the dropdown handing focus back to its trigger
                // as it dismisses.
                cx.on_next_frame(window, |audit, window, cx| {
                    audit.focus_job_export_review(FOCUS_REVIEW_FRAMES, window, cx);
                });
                cx.notify();
            }
            Err(message) => {
                self.notify_error("jobs", "Couldn’t prepare the job export", message, cx)
            }
        }
    }

    /// Put the keyboard on the review's own Export button. The wrapper is a
    /// tab group that is not itself a stop, so anchoring there and stepping
    /// once lands on the real button, which keeps its focus ring and its
    /// native Enter and Space rather than an unstyled div's.
    ///
    /// The step only works off a frame whose tab stops already carry the
    /// review. A window that has not painted it yet steps somewhere else
    /// entirely, so the anchor goes back and the attempt repeats on the next
    /// frame — a bounded number of times, because a review the rail is no
    /// longer showing would otherwise ask for frames forever.
    fn focus_job_export_review(
        &mut self,
        attempts: u8,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        if self.job_export_preview.is_none() {
            return;
        }
        window.focus(&self.job_export_preview_focus, cx);
        window.focus_next(cx);
        let landed = !self.job_export_preview_focus.is_focused(window)
            && self.job_export_preview_focus.contains_focused(window, cx);
        if landed {
            return;
        }
        window.focus(&self.job_export_preview_focus, cx);
        if let Some(left) = attempts.checked_sub(1).filter(|left| *left > 0) {
            cx.on_next_frame(window, move |audit, window, cx| {
                audit.focus_job_export_review(left, window, cx);
            });
        }
    }

    pub(super) fn cancel_job_export_preview(
        &mut self,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        self.job_export_preview = None;
        Self::restore_audit_focus(window, cx);
    }

    /// Add a product with `main` and `detail` roles. Names come from the
    /// product boxes; an empty name refuses instead of inventing one.
    pub(super) fn add_product(&mut self, dir: &Path, cx: &mut Context<Self>) {
        if self.converting {
            return;
        }
        let name = self.job_text(cx, &self.product_name_input.clone());
        let sku = self.job_text(cx, &self.product_sku_input.clone());
        if name.is_empty() {
            self.notify_error(
                "jobs",
                "Couldn’t add the product",
                "name the product first",
                cx,
            );
            return;
        }
        let mut id_base: String = name
            .to_lowercase()
            .chars()
            .map(|cell| {
                if cell.is_ascii_alphanumeric() {
                    cell
                } else {
                    '-'
                }
            })
            .collect::<String>()
            .split('-')
            .filter(|part| !part.is_empty())
            .collect::<Vec<_>>()
            .join("-");
        if id_base.is_empty() {
            id_base = "product".into();
        }
        let mut id = id_base.clone();
        let mut counter = 2;
        while self.work_job.products.iter().any(|known| known.id == id) {
            id = format!("{id_base}-{counter}");
            counter += 1;
        }
        self.mutate_job(dir, cx, move |job| {
            job.products.push(crate::job::ProductSet {
                id,
                name,
                sku_hint: sku,
                roles: vec![
                    crate::job::Role {
                        id: "main".into(),
                        label: "Main".into(),
                        required: true,
                    },
                    crate::job::Role {
                        id: "detail".into(),
                        label: "Detail".into(),
                        required: false,
                    },
                ],
                mappings: Vec::new(),
                binding: None,
            });
        });
    }

    /// Delete a product and all its mappings. Deliverables on disk stay; only
    /// the grouping goes.
    pub(super) fn delete_product(&mut self, dir: &Path, product_id: &str, cx: &mut Context<Self>) {
        if self.converting {
            return;
        }
        let id = product_id.to_string();
        self.mutate_job(dir, cx, move |job| {
            job.products.retain(|product| product.id != id);
        });
    }

    /// Add a role to one product. New roles start optional; `main` keeps the
    /// required flag it was created with.
    pub(super) fn add_role(
        &mut self,
        dir: &Path,
        product_id: &str,
        window: &mut gpui_kit::Window,
        cx: &mut Context<Self>,
    ) {
        if self.converting {
            return;
        }
        let label = self.job_text(cx, &self.role_name_input.clone());
        if label.is_empty() {
            self.notify_error("jobs", "Couldn’t add the role", "label the role first", cx);
            return;
        }
        let product_id = product_id.to_string();
        let mut slug: String = label
            .to_lowercase()
            .chars()
            .map(|cell| {
                if cell.is_ascii_alphanumeric() {
                    cell
                } else {
                    '-'
                }
            })
            .collect::<String>()
            .split('-')
            .filter(|part| !part.is_empty())
            .collect::<Vec<_>>()
            .join("-");
        if slug.is_empty() {
            slug = "role".into();
        }
        let mut id = slug.clone();
        let mut counter = 2;
        let taken = |job: &Job, product_id: &str, id: &str| {
            job.products
                .iter()
                .find(|product| product.id == product_id)
                .is_some_and(|product| product.roles.iter().any(|role| role.id == id))
        };
        while taken(&self.work_job, &product_id, &id) {
            id = format!("{slug}-{counter}");
            counter += 1;
        }
        self.mutate_job(dir, cx, move |job| {
            if let Some(product) = job
                .products
                .iter_mut()
                .find(|product| product.id == product_id)
            {
                product.roles.push(crate::job::Role {
                    id,
                    label,
                    required: false,
                });
            }
        });
        self.role_name_input.update(cx, |input, cx| {
            input.set_value("", window, cx);
        });
    }

    /// Delete an empty role. A role that still maps files refuses: move its
    /// files first, so no mapping evaporates inside a click.
    pub(super) fn delete_role(
        &mut self,
        dir: &Path,
        product_id: &str,
        role_id: &str,
        cx: &mut Context<Self>,
    ) {
        if self.converting {
            return;
        }
        let product_id = product_id.to_string();
        let role_id = role_id.to_string();
        let mapped = self.work_job.products.iter().any(|product| {
            product.id == product_id
                && product
                    .mappings
                    .iter()
                    .any(|mapping| mapping.role_id == role_id)
        });
        if mapped {
            self.notify_error(
                "jobs",
                "Couldn’t delete the role",
                "it still maps files; remove them first",
                cx,
            );
            return;
        }
        self.mutate_job(dir, cx, move |job| {
            if let Some(product) = job
                .products
                .iter_mut()
                .find(|product| product.id == product_id)
            {
                product.roles.retain(|role| role.id != role_id);
            }
        });
    }

    /// Map every selected file to one role. Already-mapped pairs stay put, so
    /// re-clicking a full selection is a no-op rather than duplication.
    pub(super) fn map_selected(
        &mut self,
        dir: &Path,
        product_id: &str,
        role_id: &str,
        cx: &mut Context<Self>,
    ) {
        if self.converting {
            return;
        }
        let product_id = product_id.to_string();
        let role_id = role_id.to_string();
        let known = self.work_job.products.iter().any(|product| {
            product.id == product_id && product.roles.iter().any(|role| role.id == role_id)
        });
        if !known {
            return;
        }
        let mut fresh = Vec::new();
        for index in self.targets() {
            let Some(entry) = self.entries.get(index) else {
                continue;
            };
            let Ok(metadata) = std::fs::metadata(&entry.path) else {
                continue;
            };
            if !metadata.is_file() {
                continue;
            }
            let modified = metadata
                .modified()
                .ok()
                .and_then(|time| time.duration_since(std::time::UNIX_EPOCH).ok())
                .map(|age| age.as_secs());
            fresh.push((entry.path.clone(), metadata.len(), modified));
        }
        if fresh.is_empty() {
            return;
        }
        self.mutate_job(dir, cx, move |job| {
            let Some(product) = job
                .products
                .iter_mut()
                .find(|product| product.id == product_id)
            else {
                return;
            };
            for (path, bytes, modified) in fresh {
                if product
                    .mappings
                    .iter()
                    .any(|mapping| mapping.role_id == role_id && mapping.source.path == path)
                {
                    continue;
                }
                let mut counter = product.mappings.len() + 1;
                while product
                    .mappings
                    .iter()
                    .any(|mapping| mapping.id == format!("m{counter}"))
                {
                    counter += 1;
                }
                product.mappings.push(crate::job::Mapping {
                    id: format!("m{counter}"),
                    role_id: role_id.clone(),
                    source: crate::job::SourceRef {
                        path,
                        bytes,
                        modified,
                    },
                });
            }
        });
    }

    /// Remove one mapped file. The product and role stay for the next mapping.
    pub(super) fn unmap(&mut self, dir: &Path, mapping_id: &str, cx: &mut Context<Self>) {
        if self.converting {
            return;
        }
        let mapping_id = mapping_id.to_string();
        self.mutate_job(dir, cx, move |job| {
            for product in &mut job.products {
                product.mappings.retain(|mapping| mapping.id != mapping_id);
            }
        });
    }

    /// Re-point one mapping through a picker. Identity refreshes from the new
    /// file; the row id stays so history follows the correction. A refusal
    /// names its path and the row keeps pointing where it did.
    pub(super) fn relink_mapping(&mut self, mapping_id: &str, cx: &mut Context<Self>) {
        if self.converting || self.job_choice_pending() {
            return;
        }
        let mapping_id = mapping_id.to_string();
        let dataset = self.dataset_generation;
        let job_id = self.work_job.id.clone();
        let revision = self.work_job.revision;
        let request_generation = self.job_request_generation;
        cx.spawn(async move |this, cx| {
            let picked = cx
                .background_executor()
                .spawn(async move { rfd::FileDialog::new().pick_file() })
                .await;
            let Some(path) = picked else { return };
            let _ = this.update(cx, |audit, cx| {
                if !audit.owns_job_request(dataset, &job_id, revision, request_generation) {
                    return;
                }
                let Some(dir) = audit.job_dir_or_notify(cx) else {
                    return;
                };
                match crate::job::relink(&mut audit.work_job, &mapping_id, &path) {
                    Ok(()) => {
                        audit.job_export_preview = None;
                        audit.persist_job(&dir, cx);
                        audit.refresh_job_states(cx);
                        cx.notify();
                    }
                    Err(message) => {
                        audit.notify_error("jobs", "Couldn’t relink the file", message, cx);
                    }
                }
            });
        })
        .detach();
    }
    /// Select the existing files whose mappings went stale, so Convert
    /// regenerates exactly the outdated deliverables. Missing files have no
    /// rows and stay unselected rather than selecting their neighbors.
    pub(super) fn select_stale_sources(&mut self, cx: &mut Context<Self>) {
        let mut indices = std::collections::HashSet::new();
        for state in &self.work_states {
            for source in &state.sources {
                if source.status != crate::job::SourceStatus::Stale {
                    continue;
                }
                if let Some(index) = self
                    .entries
                    .iter()
                    .position(|entry| entry.path == source.path)
                {
                    indices.insert(index);
                }
            }
        }
        self.selected = indices;
        self.selection_changed(cx);
        cx.notify();
    }

    /// Import a mapping sheet through a picker. Outcomes report per row: what
    /// mapped, what needs a choice, and what names nothing on disk.
    pub(super) fn import_csv_file(&mut self, cx: &mut Context<Self>) {
        if self.converting || self.job_choice_pending() {
            return;
        }
        let dataset = self.dataset_generation;
        let job_id = self.work_job.id.clone();
        let revision = self.work_job.revision;
        let request_generation = self.job_request_generation;
        cx.spawn(async move |this, cx| {
            let picked = cx
                .background_executor()
                .spawn(async move {
                    rfd::FileDialog::new()
                        .add_filter("Mapping sheet", &["csv"])
                        .pick_file()
                })
                .await;
            let Some(path) = picked else { return };
            let bytes = cx
                .background_executor()
                .spawn(async move {
                    crate::job::read_bounded(
                        &path,
                        crate::job::MAX_CSV_BYTES as u64,
                        "mapping sheet",
                    )
                })
                .await;
            let _ = this.update(cx, |audit, cx| {
                if !audit.owns_job_request(dataset, &job_id, revision, request_generation) {
                    return;
                }
                let Some(dir) = audit.job_dir_or_notify(cx) else {
                    return;
                };
                match bytes {
                    Ok(bytes) => audit.import_csv_bytes(&dir, &bytes, cx),
                    Err(message) => {
                        audit.notify_error("jobs", "Couldn’t import the sheet", message, cx)
                    }
                }
            });
        })
        .detach();
    }

    /// Map files from sheet bytes: header-tolerant CSV with sku, role and
    /// filename columns. Reports what mapped and names every row that needs
    /// a human, without writing a single ambiguous mapping.
    pub(super) fn import_csv_bytes(&mut self, dir: &Path, bytes: &[u8], cx: &mut Context<Self>) {
        if self.converting || self.job_choice_pending() {
            return;
        }
        let Ok(text) = std::str::from_utf8(bytes) else {
            self.notify_error(
                "jobs",
                "Couldn’t import the sheet",
                "mapping sheets are UTF-8 text",
                cx,
            );
            return;
        };
        let entries: Vec<crate::scan::Entry> = self
            .entries
            .iter()
            .map(|entry| crate::scan::Entry {
                path: entry.path.clone(),
                format: entry.format,
                width: entry.width,
                height: entry.height,
                bytes: entry.bytes,
            })
            .collect();
        let mut candidate = self.work_job.clone();
        let outcomes = match crate::job::apply_csv_checked(&mut candidate, text, &entries) {
            Ok(outcomes) => outcomes,
            Err(message) => {
                self.notify_error("jobs", "Couldn’t import the sheet", message, cx);
                return;
            }
        };
        self.work_job = candidate;
        self.persist_job(dir, cx);
        self.refresh_job_states(cx);
        let mut report = Vec::new();
        let mut mapped = 0;
        for outcome in outcomes {
            match outcome {
                crate::job::CsvOutcome::Mapped { .. } => mapped += 1,
                crate::job::CsvOutcome::AmbiguousFile { filename, .. } => {
                    report.push(format!("{filename}: several files match"));
                }
                crate::job::CsvOutcome::MissingFile { filename } => {
                    report.push(format!("{filename}: no such file"));
                }
                crate::job::CsvOutcome::UnknownProduct { sku } => {
                    report.push(format!("{sku}: no such product"));
                }
                crate::job::CsvOutcome::UnknownRole { product_id, role } => {
                    report.push(format!("{product_id}: no role {role}"));
                }
                crate::job::CsvOutcome::AmbiguousSku { sku } => {
                    report.push(format!("{sku}: two products share it; use an id"));
                }
            }
        }
        let mut message = format!("mapped {mapped}");
        let problems = !report.is_empty();
        if problems {
            let extra = report.len() > 3;
            let mut shown = report.into_iter().take(3).collect::<Vec<_>>().join("; ");
            if extra {
                shown.push_str("; …");
            }
            message.push_str(&format!(" — {shown}"));
        }
        if mapped == 0 && problems {
            self.notify_error("jobs", "The sheet mapped nothing", message, cx);
        } else {
            self.notify_success("jobs", "Mapping sheet imported", message, cx);
        }
    }

    /// Export the open job beside a picker-chosen path, portable form.
    pub(super) fn export_job_file(&mut self, cx: &mut Context<Self>) {
        let Some(draft) = self.job_export_preview.clone() else {
            return;
        };
        if self.converting || self.job_choice_pending() {
            return;
        }
        let name: String = draft
            .preview
            .job_name
            .chars()
            .map(|cell| match cell {
                '/' | '\\' => '-',
                _ => cell,
            })
            .collect();
        let dataset = self.dataset_generation;
        let job_id = draft.job_id.clone();
        let revision = draft.revision;
        let request_generation = self.job_request_generation;
        let bytes = draft.bytes;
        cx.spawn(async move |this, cx| {
            let picked = cx
                .background_executor()
                .spawn(async move {
                    rfd::FileDialog::new()
                        .add_filter("Job", &["json"])
                        .set_file_name(format!("{name}.press-job.json"))
                        .save_file()
                })
                .await;
            let Some(path) = picked else { return };
            let _ = this.update_in(cx, |audit, window, cx| {
                if !audit.owns_job_request(dataset, &job_id, revision, request_generation) {
                    return;
                }
                if let Err(error) = std::fs::write(&path, &bytes) {
                    audit.notify_error(
                        "jobs",
                        "Couldn’t export the job",
                        format!("{} cannot be written: {error}", path.display()),
                        cx,
                    );
                } else {
                    // The review is gone, so the keyboard has nowhere to sit.
                    // The list takes it back the same way Cancel and Escape
                    // hand it back.
                    audit.job_export_preview = None;
                    Self::restore_audit_focus(window, cx);
                }
            });
        })
        .detach();
    }

    #[cfg(test)]
    pub(super) fn export_job_to(&mut self, path: &Path, cx: &mut Context<Self>) {
        match self.work_job.export_draft(&self.root) {
            Ok(draft) => {
                if let Err(error) = std::fs::write(path, &draft.bytes) {
                    self.notify_error(
                        "jobs",
                        "Couldn’t export the job",
                        format!("{} cannot be written: {error}", path.display()),
                        cx,
                    );
                } else {
                    cx.notify();
                }
            }
            Err(message) => self.notify_error("jobs", "Couldn’t export the job", message, cx),
        }
    }

    /// Adopt outside job bytes under the current folder, rebinding every path
    /// beneath it. Files that do not resolve arrive as relink states.
    pub(super) fn import_job_bytes(&mut self, dir: &Path, bytes: &[u8], cx: &mut Context<Self>) {
        if self.converting {
            return;
        }
        if bytes.len() as u64 > crate::job::MAX_FILE_BYTES {
            self.notify_error(
                "jobs",
                "Couldn’t import the job",
                "that file is larger than any job",
                cx,
            );
            return;
        }
        let root = self.root.clone();
        let parsed: Result<crate::job::PortableJob, String> =
            serde_json::from_slice(bytes).map_err(|error| format!("job does not parse: {error}"));
        match parsed.and_then(|portable| crate::job::Job::from_portable(&portable, &root)) {
            Ok(job) if crate::job::exists(dir, &job.id) => {
                self.notify_error(
                    "jobs",
                    "Couldn’t import the job",
                    format!("a job named {:?} already exists", job.id),
                    cx,
                );
            }
            Ok(job) => {
                self.bump_job_request_generation();
                self.work_job = job;
                self.job_choices.clear();
                self.job_export_preview = None;
                if let Err(message) = job::save(dir, &self.work_job) {
                    self.notify_error("jobs", "Couldn’t import the job", message, cx);
                }
                self.refresh_job_states(cx);
                cx.notify();
            }
            Err(message) => self.notify_error("jobs", "Couldn’t import the job", message, cx),
        }
    }

    /// Pick a job file off disk. The dialog runs off the update path; the
    /// bytes still validate strictly at the door.
    pub(super) fn import_job_file(&mut self, cx: &mut Context<Self>) {
        if self.converting {
            return;
        }
        let dataset = self.dataset_generation;
        let job_id = self.work_job.id.clone();
        let revision = self.work_job.revision;
        let request_generation = self.job_request_generation;
        cx.spawn(async move |this, cx| {
            let picked = cx
                .background_executor()
                .spawn(async move {
                    rfd::FileDialog::new()
                        .add_filter("Job", &["json"])
                        .pick_file()
                })
                .await;
            let Some(path) = picked else { return };
            let bytes =
                cx.background_executor()
                    .spawn(async move {
                        crate::job::read_bounded(&path, crate::job::MAX_FILE_BYTES, "job")
                    })
                    .await;
            let _ = this.update(cx, |audit, cx| {
                if !audit.owns_job_request(dataset, &job_id, revision, request_generation) {
                    return;
                }
                let Some(dir) = audit.job_dir_or_notify(cx) else {
                    return;
                };
                match bytes {
                    Ok(bytes) => audit.import_job_bytes(&dir, &bytes, cx),
                    Err(message) => {
                        audit.notify_error("jobs", "Couldn’t import the job", message, cx)
                    }
                }
            });
        })
        .detach();
    }
}
#[cfg(test)]
mod tests {
    use super::*;

    fn library(tag: &str) -> PathBuf {
        let dir = std::env::temp_dir().join(format!("press-job-load-{tag}-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).expect("the job library is created");
        dir
    }

    #[test]
    fn same_job_identity_needs_the_current_request_generation() {
        assert!(job_request_matches((4, "job", 7, 8), (4, "job", 7, 8)));
        assert!(!job_request_matches((4, "job", 7, 8), (4, "job", 7, 9)));
    }

    /// Two jobs claiming one root stay as explicit choices on every lookup.
    #[test]
    fn shared_roots_load_the_same_job_every_time() {
        let dir = library("shared");
        let root = dir.join("photos");
        std::fs::create_dir_all(&root).expect("the root exists");
        let mut zebra = Job::new("zebra".into(), "Zebra".into(), vec![root.clone()]).unwrap();
        zebra.revision = 2;
        job::save(&dir, &zebra).unwrap();
        let alpha = Job::new("alpha".into(), "Alpha".into(), vec![root.clone()]).unwrap();
        job::save(&dir, &alpha).unwrap();
        for _ in 0..2 {
            let selection = job_selection_for_in(&dir, &root);
            assert_eq!(selection.choices.len(), 2);
            assert!(selection.choices.iter().any(|job| job.id == "alpha"));
            assert!(selection.choices.iter().any(|job| job.id == "zebra"));
            assert!(selection.job.products.is_empty());
        }
        std::fs::remove_dir_all(&dir).unwrap();
    }

    #[test]
    fn loading_ambiguous_job_never_picks_one() {
        let dir = library("ambiguous-loader");
        let root = dir.join("photos");
        std::fs::create_dir_all(&root).expect("the root exists");
        let alpha = Job::new("alpha".into(), "Alpha".into(), vec![root.clone()]).unwrap();
        job::save(&dir, &alpha).unwrap();
        let zebra = Job::new("zebra".into(), "Zebra".into(), vec![root.clone()]).unwrap();
        job::save(&dir, &zebra).unwrap();
        let loaded = load_job_for_in(&dir, &root);
        assert!(loaded.products.is_empty());
        assert!(!matches!(loaded.id.as_str(), "alpha" | "zebra"));
        std::fs::remove_dir_all(&dir).unwrap();
    }

    /// Canonical matches also remain explicit choices, including when the
    /// queried spelling is a symlink.
    #[cfg(unix)]
    #[test]
    fn canonical_matches_are_explicit_choices() {
        let dir = library("canonical");
        let root = dir.join("photos");
        std::fs::create_dir_all(&root).expect("the root exists");
        let link = dir.join("linked");
        std::os::unix::fs::symlink(&root, &link).unwrap();
        let zebra = Job::new("zebra".into(), "Zebra".into(), vec![root.clone()]).unwrap();
        job::save(&dir, &zebra).unwrap();
        let alpha = Job::new("alpha".into(), "Alpha".into(), vec![root]).unwrap();
        job::save(&dir, &alpha).unwrap();
        let selection = job_selection_for_in(&dir, &link);
        assert_eq!(selection.choices.len(), 2);
        assert!(selection.choices.iter().any(|job| job.id == "alpha"));
        assert!(selection.choices.iter().any(|job| job.id == "zebra"));
        assert!(selection.job.products.is_empty());
        std::fs::remove_dir_all(&dir).unwrap();
    }

    /// An exact stored spelling does not hide canonical claimants.
    #[cfg(unix)]
    #[test]
    fn exact_matches_do_not_hide_canonical_claimants() {
        let dir = library("canonical-exact");
        let root = dir.join("photos");
        std::fs::create_dir_all(&root).expect("the root exists");
        let link = dir.join("linked");
        std::os::unix::fs::symlink(&root, &link).unwrap();
        for (id, name, path) in [
            ("alpha", "Alpha", root.clone()),
            ("zebra", "Zebra", root),
            ("zed", "Zed", link.clone()),
        ] {
            job::save(&dir, &Job::new(id.into(), name.into(), vec![path]).unwrap()).unwrap();
        }
        let selection = job_selection_for_in(&dir, &link);
        assert_eq!(selection.choices.len(), 3);
        assert!(selection.choices.iter().any(|job| job.id == "zed"));
        std::fs::remove_dir_all(&dir).unwrap();
    }
}
