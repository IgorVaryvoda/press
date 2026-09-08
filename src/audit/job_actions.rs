//! Product-set job actions: open, create, map, relink, import, export and
//! refresh. Mutations auto-save into the job library so restart recovery is
//! the normal case, not a mode. Filesystem reads (resolve, manifest, relink
//! targets) stay on background tasks; renders only read cached state.

use super::*;
use crate::job::{self, Job};

/// Open the saved job covering `root`, or an anonymous one for it. A saved
/// job matches by exact or canonical root; the first match wins and the rest
/// stay listed nowhere, because two jobs claiming one folder is itself a
/// conflict to resolve rather than guess at.
pub(super) fn load_job_for(root: &Path) -> Job {
    if let Some(dir) = job::dir() {
        return load_job_for_in(&dir, root);
    }
    anonymous_job(root)
}

/// The same lookup against an explicit library directory, so tests drive it
/// without touching the real config folder.
pub(super) fn load_job_for_in(dir: &Path, root: &Path) -> Job {
    {
        let (jobs, _) = job::list(dir);
        if let Some(known) = jobs.into_iter().find(|job| {
            job.source_roots
                .iter()
                .any(|known| roots_match(known, root))
        }) {
            return known;
        }
        let name = root
            .file_name()
            .map(|name| name.to_string_lossy().into_owned())
            .filter(|name| !name.is_empty())
            .unwrap_or_else(|| "Job".into());
        if let Ok(job) = Job::new(job::suggest_id(dir, &name), name, vec![root.to_path_buf()]) {
            return job;
        }
    }
    anonymous_job(root)
}

fn anonymous_job(root: &Path) -> Job {
    Job {
        schema: job::SCHEMA_VERSION,
        id: "job".into(),
        name: "Job".into(),
        revision: 1,
        source_roots: vec![root.to_path_buf()],
        target_recipe: None,
        products: Vec::new(),
    }
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

impl Audit {
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
        if self.converting {
            return;
        }
        let id = self.work_job.id.clone();
        match job::remove(dir, &id) {
            Ok(()) => {
                self.work_job = load_job_for_in(dir, &self.root.clone());
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
        if self.converting {
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
                if audit.dataset_generation != generation {
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
        self.work_job = load_job_for(&self.root.clone());
        self.work_states.clear();
        self.work_stale.clear();
        cx.notify();
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
        if self.converting {
            return;
        }
        let mapping_id = mapping_id.to_string();
        cx.spawn(async move |this, cx| {
            let picked = cx
                .background_executor()
                .spawn(async move { rfd::FileDialog::new().pick_file() })
                .await;
            let Some(path) = picked else { return };
            let _ = this.update(cx, |audit, cx| {
                let Some(dir) = audit.job_dir_or_notify(cx) else {
                    return;
                };
                match crate::job::relink(&mut audit.work_job, &mapping_id, &path) {
                    Ok(()) => {
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
        if self.converting {
            return;
        }
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
            let bytes = std::fs::metadata(&path)
                .ok()
                .filter(|metadata| metadata.len() <= 1 << 20)
                .and_then(|_| std::fs::read(&path).ok());
            let _ = this.update(cx, |audit, cx| {
                let Some(dir) = audit.job_dir_or_notify(cx) else {
                    return;
                };
                match bytes {
                    Some(bytes) => audit.import_csv_bytes(&dir, &bytes, cx),
                    None => audit.notify_error(
                        "jobs",
                        "Couldn’t import the sheet",
                        "that file cannot be read as a mapping sheet",
                        cx,
                    ),
                }
            });
        })
        .detach();
    }

    /// Map files from sheet bytes: header-tolerant CSV with sku, role and
    /// filename columns. Reports what mapped and names every row that needs
    /// a human, without writing a single ambiguous mapping.
    pub(super) fn import_csv_bytes(&mut self, dir: &Path, bytes: &[u8], cx: &mut Context<Self>) {
        if self.converting {
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
        let mut report = Vec::new();
        let mut mapped = 0;
        self.mutate_job(dir, cx, |job| {
            for outcome in crate::job::apply_csv(job, text, &entries) {
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
        });
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
        if self.converting {
            return;
        }
        let name: String = self
            .work_job
            .name
            .chars()
            .map(|cell| match cell {
                '/' | '\\' => '-',
                _ => cell,
            })
            .collect();
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
            let _ = this.update(cx, |audit, cx| {
                audit.export_job_to(&path, cx);
            });
        })
        .detach();
    }

    pub(super) fn export_job_to(&mut self, path: &Path, cx: &mut Context<Self>) {
        let root = self.root.clone();
        match self.work_job.to_portable(&root).and_then(|portable| {
            serde_json::to_string_pretty(&portable)
                .map_err(|error| format!("the job does not serialize: {error}"))
        }) {
            Ok(json) => {
                if let Err(error) = std::fs::write(path, json.as_bytes()) {
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
                self.work_job = job;
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
            let bytes = std::fs::metadata(&path)
                .ok()
                .filter(|metadata| metadata.len() <= crate::job::MAX_FILE_BYTES)
                .and_then(|_| std::fs::read(&path).ok());
            let _ = this.update(cx, |audit, cx| {
                let Some(dir) = audit.job_dir_or_notify(cx) else {
                    return;
                };
                match bytes {
                    Some(bytes) => audit.import_job_bytes(&dir, &bytes, cx),
                    None => audit.notify_error(
                        "jobs",
                        "Couldn’t import the job",
                        "that file cannot be read as a job",
                        cx,
                    ),
                }
            });
        })
        .detach();
    }
}
