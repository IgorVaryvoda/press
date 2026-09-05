//! Personal recipe actions: apply, save, duplicate, rename, delete, import
//! and export. Thin over `crate::recipe` storage: the file dialogs stay async
//! like the folder picker, while the byte-level rules live beside the model
//! where the unit tests reach them without a window.

use super::*;
use crate::recipe::{self, Recipe};

impl Audit {
    pub(super) fn recipe_dir_or_notify(&self, cx: &mut Context<Self>) -> Option<PathBuf> {
        match recipe::dir() {
            Some(dir) => Some(dir),
            None => {
                self.notify_error(
                    "recipes",
                    "Couldn’t use the recipe library",
                    "no config folder resolves on this machine",
                    cx,
                );
                None
            }
        }
    }

    /// The selected row as a saved recipe, if it names one. Built-in rows
    /// resolve from the model; anything else must still have its file.
    pub(super) fn selected_personal(&self) -> Option<Recipe> {
        let id = self.selected_recipe.as_deref()?;
        self.recipes.iter().find(|recipe| recipe.id == id).cloned()
    }

    pub(super) fn reload_recipes(&mut self, dir: &Path) {
        (self.recipes, self.recipes_skipped) = recipe::list(dir);
        let known = self.selected_recipe.as_deref().is_some_and(|id| {
            Recipe::builtins().iter().any(|row| row.id == id)
                || self.recipes.iter().any(|recipe| recipe.id == id)
        });
        if !known {
            self.selected_recipe = None;
        }
    }

    /// Whether the live settings drifted from what the recipe names. Speed
    /// counts only when the recipe pins one: an unpinned recipe leaves the
    /// global dial alone, so ambient speed never marks its row modified.
    pub(super) fn recipe_modified(&self, recipe: &Recipe) -> bool {
        let (format, quality, max_edge, speed) = recipe.effective();
        format != self.format
            || quality != self.quality
            || max_edge != self.max_edge
            || speed.is_some_and(|pinned| Some(pinned) != crate::avif::configured_speed())
    }

    /// Apply a row's recipe as the live settings and remember the row. Never
    /// writes back: a diverged row reads as modified, and Convert always uses
    /// whatever the controls say right now.
    pub(super) fn apply_recipe(
        &mut self,
        recipe: &Recipe,
        id: &str,
        window: &mut gpui_kit::Window,
        cx: &mut Context<Self>,
    ) {
        if self.converting {
            return;
        }
        let (format, quality, edge, speed) = recipe.effective();
        self.format = format;
        self.quality = quality;
        self.max_edge = edge;
        self.selected_recipe = Some(id.to_string());
        // Only a pinned speed moves the global dial: unpinned recipes leave
        // ambient speed alone, so clicking Recommended never resets a speed
        // the settings file chose.
        if let Some(speed) = speed {
            crate::avif::set_speed(speed);
        }
        if let Some(value) = quality.0 {
            // Keep the slider where the recipe put things, or the knob below
            // would contradict the number in the estimate.
            self.slider_quality = value;
            self.quality_slider
                .update(cx, |slider, cx| slider.set_value(value, window, cx));
        }
        self.clear_results();
        self.schedule_estimate(cx);
        cx.notify();
    }

    fn recipe_name(&self, cx: &App) -> String {
        self.recipe_name_input.read(cx).value().trim().to_string()
    }

    /// Save the live settings under the name in the box.
    pub(super) fn save_current_recipe(
        &mut self,
        dir: &Path,
        window: &mut gpui_kit::Window,
        cx: &mut Context<Self>,
    ) {
        if self.converting {
            return;
        }
        let name = self.recipe_name(cx);
        if name.is_empty() {
            self.notify_error(
                "recipes",
                "Couldn’t save the recipe",
                "name the current settings first",
                cx,
            );
            return;
        }
        let recipe = Recipe {
            schema: recipe::SCHEMA_VERSION,
            id: recipe::suggest_id(dir, &name),
            name,
            revision: 1,
            provenance: recipe::Provenance::Personal,
            format: match self.format {
                convert::Format::WebP => recipe::RecipeFormat::WebP,
                convert::Format::Avif => recipe::RecipeFormat::Avif,
                convert::Format::Jpeg => recipe::RecipeFormat::Jpeg,
                convert::Format::Png => recipe::RecipeFormat::Png,
                convert::Format::JpegXl => recipe::RecipeFormat::JpegXl,
                convert::Format::Same => recipe::RecipeFormat::Keep,
            },
            quality: match self.quality.0 {
                None => recipe::RecipeQuality::Lossless,
                Some(value) => recipe::RecipeQuality::Lossy(value),
            },
            max_edge: self.max_edge.0,
            avif_speed: crate::avif::configured_speed(),
        };
        let id = recipe.id.clone();
        match recipe::save(dir, &recipe) {
            Ok(()) => {
                self.reload_recipes(dir);
                self.selected_recipe = Some(id);
                self.recipe_name_input.update(cx, |input, cx| {
                    input.set_value("", window, cx);
                });
                cx.notify();
            }
            Err(message) => self.notify_error("recipes", "Couldn’t save the recipe", message, cx),
        }
    }

    /// Fork the selected row under a fresh id. Built-ins fork too: the copy
    /// is personal, the template row stays untouched.
    pub(super) fn duplicate_recipe(&mut self, dir: &Path, cx: &mut Context<Self>) {
        if self.converting {
            return;
        }
        let Some(source) = self.selected_recipe.clone().and_then(|id| {
            Recipe::builtins()
                .iter()
                .find(|row| row.id == id)
                .cloned()
                .or_else(|| self.recipes.iter().find(|recipe| recipe.id == id).cloned())
        }) else {
            self.notify_error(
                "recipes",
                "Couldn’t duplicate the recipe",
                "select a preset row first",
                cx,
            );
            return;
        };
        let mut name = format!("{} copy", source.name);
        while name.chars().count() > recipe::MAX_NAME_LEN {
            name.pop();
        }
        let mut fork = source;
        fork.name = name;
        fork.id = recipe::suggest_id(dir, &fork.name);
        fork.revision = 1;
        let id = fork.id.clone();
        match recipe::save(dir, &fork) {
            Ok(()) => {
                self.reload_recipes(dir);
                self.selected_recipe = Some(id);
                cx.notify();
            }
            Err(message) => {
                self.notify_error("recipes", "Couldn’t duplicate the recipe", message, cx)
            }
        }
    }

    /// Rename the selected personal row to the name in the box. Built-in rows
    /// keep their names; duplicate one first.
    pub(super) fn rename_recipe(
        &mut self,
        dir: &Path,
        window: &mut gpui_kit::Window,
        cx: &mut Context<Self>,
    ) {
        if self.converting {
            return;
        }
        let name = self.recipe_name(cx);
        if name.is_empty() {
            self.notify_error(
                "recipes",
                "Couldn’t rename the recipe",
                "type the new name first",
                cx,
            );
            return;
        }
        let Some(mut recipe) = self.selected_personal() else {
            self.notify_error(
                "recipes",
                "Couldn’t rename the recipe",
                "select a personal row first; built-in rows keep their names",
                cx,
            );
            return;
        };
        recipe.name = name;
        match recipe::overwrite(dir, &recipe) {
            Ok(()) => {
                self.reload_recipes(dir);
                self.recipe_name_input.update(cx, |input, cx| {
                    input.set_value("", window, cx);
                });
                cx.notify();
            }
            Err(message) => self.notify_error("recipes", "Couldn’t rename the recipe", message, cx),
        }
    }

    /// Delete the selected personal row. Generated files live elsewhere; only
    /// the recipe file goes.
    pub(super) fn delete_recipe(&mut self, dir: &Path, cx: &mut Context<Self>) {
        if self.converting {
            return;
        }
        let Some(recipe) = self.selected_personal() else {
            self.notify_error(
                "recipes",
                "Couldn’t delete the recipe",
                "select a personal row first; built-in rows stay",
                cx,
            );
            return;
        };
        match recipe::remove(dir, &recipe.id) {
            Ok(()) => {
                self.selected_recipe = None;
                self.reload_recipes(dir);
                cx.notify();
            }
            Err(message) => self.notify_error("recipes", "Couldn’t delete the recipe", message, cx),
        }
    }

    /// Import outside bytes after a picker hands them over. The store decides
    /// identity and provenance; the dialog only chose bytes.
    pub(super) fn import_recipe_bytes(&mut self, dir: &Path, bytes: &[u8], cx: &mut Context<Self>) {
        if bytes.len() as u64 > recipe::MAX_FILE_BYTES {
            self.notify_error(
                "recipes",
                "Couldn’t import the recipe",
                "that file is larger than any recipe",
                cx,
            );
            return;
        }
        match recipe::import_bytes(dir, bytes) {
            Ok(recipe) => {
                let id = recipe.id;
                self.reload_recipes(dir);
                self.selected_recipe = Some(id);
                cx.notify();
            }
            Err(message) => self.notify_error("recipes", "Couldn’t import the recipe", message, cx),
        }
    }

    /// Pick a recipe file off disk. The dialog runs off the update path like
    /// the folder picker; the import still validates strictly at the door.
    pub(super) fn import_recipe_file(&mut self, cx: &mut Context<Self>) {
        if self.converting {
            return;
        }
        cx.spawn(async move |this, cx| {
            let picked = cx
                .background_executor()
                .spawn(async move {
                    rfd::FileDialog::new()
                        .add_filter("Recipe", &["json"])
                        .pick_file()
                })
                .await;
            let Some(path) = picked else { return };
            // Cap the read before it starts: the strict parser caps too, but
            // only after the bytes are already in memory.
            let bytes = std::fs::metadata(&path)
                .ok()
                .filter(|metadata| metadata.len() <= recipe::MAX_FILE_BYTES)
                .and_then(|_| std::fs::read(&path).ok());
            let _ = this.update(cx, |audit, cx| {
                let Some(dir) = audit.recipe_dir_or_notify(cx) else {
                    return;
                };
                match bytes {
                    Some(bytes) => audit.import_recipe_bytes(&dir, &bytes, cx),
                    None => audit.notify_error(
                        "recipes",
                        "Couldn’t import the recipe",
                        "that file cannot be read as a recipe",
                        cx,
                    ),
                }
            });
        })
        .detach();
    }

    /// Write the selected personal row out through a picker.
    pub(super) fn export_recipe_file(&mut self, cx: &mut Context<Self>) {
        if self.converting {
            return;
        }
        let Some(recipe) = self.selected_personal() else {
            self.notify_error(
                "recipes",
                "Couldn’t export the recipe",
                "select a personal row first",
                cx,
            );
            return;
        };
        let id = recipe.id;
        cx.spawn(async move |this, cx| {
            let picked = cx
                .background_executor()
                .spawn(async move {
                    rfd::FileDialog::new()
                        .add_filter("Recipe", &["json"])
                        .set_file_name(format!("{id}.json"))
                        .save_file()
                })
                .await;
            let Some(path) = picked else { return };
            let _ = this.update(cx, |audit, cx| {
                let Some(dir) = audit.recipe_dir_or_notify(cx) else {
                    return;
                };
                audit.export_selected_to(&dir, &path, cx);
            });
        })
        .detach();
    }

    pub(super) fn export_selected_to(&mut self, dir: &Path, path: &Path, cx: &mut Context<Self>) {
        let Some(recipe) = self.selected_personal() else {
            self.notify_error(
                "recipes",
                "Couldn’t export the recipe",
                "select a personal row first",
                cx,
            );
            return;
        };
        match recipe::export_bytes(dir, &recipe.id).and_then(|bytes| {
            std::fs::write(path, &bytes)
                .map_err(|error| format!("{} cannot be written: {error}", path.display()))
        }) {
            Ok(()) => cx.notify(),
            Err(message) => self.notify_error("recipes", "Couldn’t export the recipe", message, cx),
        }
    }
}
