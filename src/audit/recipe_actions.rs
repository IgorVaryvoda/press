//! Personal recipe actions: apply, save, save as, rename, delete, import and
//! export. Thin over `crate::recipe` storage: the file dialogs stay async
//! like the folder picker, while the byte-level rules live beside the model
//! where the unit tests reach them without a window. The window says
//! "preset"; the files, the flags and this code say recipe.

use super::*;
use crate::recipe::{self, Recipe};

impl Audit {
    pub(super) fn recipe_dir_or_notify(&self, cx: &mut Context<Self>) -> Option<PathBuf> {
        match recipe::dir() {
            Some(dir) => Some(dir),
            None => {
                self.notify_error(
                    "recipes",
                    "Couldn’t use the preset library",
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
        // ambient speed alone, so clicking Keep dimensions never resets a speed
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

    /// The live dials as recipe fields, for a new file and for an update alike.
    fn current_recipe_fields(
        &self,
    ) -> (
        recipe::RecipeFormat,
        recipe::RecipeQuality,
        Option<u32>,
        Option<u8>,
    ) {
        let format = match self.format {
            convert::Format::WebP => recipe::RecipeFormat::WebP,
            convert::Format::Avif => recipe::RecipeFormat::Avif,
            convert::Format::Jpeg => recipe::RecipeFormat::Jpeg,
            convert::Format::Png => recipe::RecipeFormat::Png,
            convert::Format::JpegXl => recipe::RecipeFormat::JpegXl,
            convert::Format::Same => recipe::RecipeFormat::Keep,
        };
        let quality = match self.quality.0 {
            None => recipe::RecipeQuality::Lossless,
            Some(value) => recipe::RecipeQuality::Lossy(value),
        };
        (
            format,
            quality,
            self.max_edge.0,
            crate::avif::configured_speed(),
        )
    }

    /// Open the name prompt under the preset row. Rename starts from the
    /// current name; Save as starts blank. The box takes focus, so the next
    /// keystrokes are the name.
    pub(super) fn open_recipe_prompt(
        &mut self,
        prompt: RecipePrompt,
        window: &mut gpui_kit::Window,
        cx: &mut Context<Self>,
    ) {
        let seed = match prompt {
            RecipePrompt::Rename => self
                .selected_personal()
                .map(|row| row.name)
                .unwrap_or_default(),
            RecipePrompt::SaveAs => String::new(),
        };
        self.recipe_name_input.update(cx, |input, cx| {
            input.set_value(seed, window, cx);
        });
        self.recipe_prompt = Some(prompt);
        window.focus(&self.recipe_name_input.read(cx).focus_handle(cx), cx);
        cx.notify();
    }

    /// The prompt's button: a new file under the typed name, or the selected
    /// file renamed to it. The prompt stays open when the store refuses, so
    /// the name is still there to fix.
    pub(super) fn confirm_recipe_prompt(
        &mut self,
        window: &mut gpui_kit::Window,
        cx: &mut Context<Self>,
    ) {
        let Some(dir) = self.recipe_dir_or_notify(cx) else {
            return;
        };
        let done = match self.recipe_prompt {
            Some(RecipePrompt::SaveAs) => self.save_current_recipe(&dir, window, cx),
            Some(RecipePrompt::Rename) => self.rename_recipe(&dir, window, cx),
            None => return,
        };
        if done {
            self.recipe_prompt = None;
            cx.notify();
        }
    }

    /// Save the live settings under the name in the box, as a new file.
    pub(super) fn save_current_recipe(
        &mut self,
        dir: &Path,
        window: &mut gpui_kit::Window,
        cx: &mut Context<Self>,
    ) -> bool {
        if self.converting {
            return false;
        }
        let name = self.recipe_name(cx);
        if name.is_empty() {
            self.notify_error(
                "recipes",
                "Couldn’t save the preset",
                "name the current settings first",
                cx,
            );
            return false;
        }
        let (format, quality, max_edge, avif_speed) = self.current_recipe_fields();
        let recipe = Recipe {
            schema: recipe::SCHEMA_VERSION,
            id: recipe::suggest_id(dir, &name),
            name,
            revision: 1,
            provenance: recipe::Provenance::Personal,
            format,
            quality,
            max_edge,
            avif_speed,
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
                true
            }
            Err(message) => {
                self.notify_error("recipes", "Couldn’t save the preset", message, cx);
                false
            }
        }
    }

    /// Write the live settings into the selected personal preset and bump its
    /// revision. Built-ins never change: Save as new forks them instead.
    pub(super) fn update_recipe(&mut self, dir: &Path, cx: &mut Context<Self>) {
        if self.converting {
            return;
        }
        let Some(mut recipe) = self.selected_personal() else {
            self.notify_error(
                "recipes",
                "Couldn’t save the preset",
                "select one of your own presets first; a built-in preset forks with Save as new",
                cx,
            );
            return;
        };
        let (format, quality, max_edge, avif_speed) = self.current_recipe_fields();
        recipe.format = format;
        recipe.quality = quality;
        recipe.max_edge = max_edge;
        recipe.avif_speed = avif_speed;
        recipe.revision += 1;
        match recipe::overwrite(dir, &recipe) {
            Ok(()) => {
                self.reload_recipes(dir);
                cx.notify();
            }
            Err(message) => self.notify_error("recipes", "Couldn’t save the preset", message, cx),
        }
    }

    /// Rename the selected personal row to the name in the box. Built-in rows
    /// keep their names; duplicate one first.
    pub(super) fn rename_recipe(
        &mut self,
        dir: &Path,
        window: &mut gpui_kit::Window,
        cx: &mut Context<Self>,
    ) -> bool {
        if self.converting {
            return false;
        }
        let name = self.recipe_name(cx);
        if name.is_empty() {
            self.notify_error(
                "recipes",
                "Couldn’t rename the preset",
                "type the new name first",
                cx,
            );
            return false;
        }
        let Some(mut recipe) = self.selected_personal() else {
            self.notify_error(
                "recipes",
                "Couldn’t rename the preset",
                "select one of your own presets first; built-in presets keep their names",
                cx,
            );
            return false;
        };
        recipe.name = name;
        match recipe::overwrite(dir, &recipe) {
            Ok(()) => {
                self.reload_recipes(dir);
                self.recipe_name_input.update(cx, |input, cx| {
                    input.set_value("", window, cx);
                });
                cx.notify();
                true
            }
            Err(message) => {
                self.notify_error("recipes", "Couldn’t rename the preset", message, cx);
                false
            }
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
                "Couldn’t delete the preset",
                "select one of your own presets first; built-in presets stay",
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
            Err(message) => self.notify_error("recipes", "Couldn’t delete the preset", message, cx),
        }
    }

    /// Import outside bytes after a picker hands them over. The store decides
    /// identity and provenance; the dialog only chose bytes.
    pub(super) fn import_recipe_bytes(&mut self, dir: &Path, bytes: &[u8], cx: &mut Context<Self>) {
        if bytes.len() as u64 > recipe::MAX_FILE_BYTES {
            self.notify_error(
                "recipes",
                "Couldn’t import the preset",
                "that file is larger than any preset",
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
            Err(message) => self.notify_error("recipes", "Couldn’t import the preset", message, cx),
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
                        .add_filter("Preset", &["json"])
                        .pick_file()
                })
                .await;
            let Some(path) = picked else { return };
            let bytes =
                cx.background_executor()
                    .spawn(async move {
                        crate::job::read_bounded(&path, recipe::MAX_FILE_BYTES, "preset")
                    })
                    .await;
            let _ = this.update(cx, |audit, cx| {
                let Some(dir) = audit.recipe_dir_or_notify(cx) else {
                    return;
                };
                match bytes {
                    Ok(bytes) => audit.import_recipe_bytes(&dir, &bytes, cx),
                    Err(message) => {
                        audit.notify_error("recipes", "Couldn’t import the preset", message, cx)
                    }
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
                "Couldn’t export the preset",
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
                        .add_filter("Preset", &["json"])
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
                "Couldn’t export the preset",
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
            Err(message) => self.notify_error("recipes", "Couldn’t export the preset", message, cx),
        }
    }
}
