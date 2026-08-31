//! Shallow local-folder navigation: places, recent folders, tree, and child rows.

use super::*;
use std::hash::{Hash, Hasher};
use std::sync::OnceLock;

pub(super) const SIDEBAR_WIDTH: f32 = 220.;
pub(super) const SIDEBAR_MIN_WINDOW_WIDTH: f32 = 1040.;
const FOLDER_ROW_HEIGHT: f32 = 34.;
const MAX_VISIBLE_FOLDER_ROWS: usize = 5;

pub(super) fn home_dir() -> Option<PathBuf> {
    static HOME: OnceLock<Option<PathBuf>> = OnceLock::new();
    HOME.get_or_init(|| {
        std::env::var_os(if cfg!(windows) { "USERPROFILE" } else { "HOME" })
            .map(PathBuf::from)
            .map(navigation_path)
    })
    .clone()
}

fn path_label(path: &Path) -> String {
    if home_dir().as_deref() == Some(path) {
        return "Home".into();
    }
    path.file_name()
        .map(|name| name.to_string_lossy().into_owned())
        .filter(|name| !name.is_empty())
        .unwrap_or_else(|| path.display().to_string())
}

fn tree_id(path: &Path) -> String {
    let mut hasher = std::collections::hash_map::DefaultHasher::new();
    path.hash(&mut hasher);
    format!("folder-{:016x}", hasher.finish())
}

fn tree_item(
    path: &Path,
    children: &HashMap<PathBuf, Vec<PathBuf>>,
    loaded: &HashSet<PathBuf>,
    expanded: &HashSet<PathBuf>,
    output: Option<&Path>,
    query: &str,
    paths: &mut HashMap<String, PathBuf>,
) -> Option<TreeItem> {
    if output.is_some_and(|output| path.starts_with(output)) {
        return None;
    }
    let id = tree_id(path);
    let label = path_label(path);
    let mut descendants = children
        .get(path)
        .into_iter()
        .flatten()
        .filter_map(|child| tree_item(child, children, loaded, expanded, output, query, paths))
        .collect::<Vec<_>>();
    if !query.is_empty() && !label.to_lowercase().contains(query) && descendants.is_empty() {
        return None;
    }
    paths.insert(id.clone(), path.to_path_buf());
    if query.is_empty() && !loaded.contains(path) {
        descendants.push(TreeItem::new(format!("{id}-pending"), "Loading…").disabled(true));
    }
    let show_descendants = !query.is_empty() && !descendants.is_empty();
    Some(
        TreeItem::new(id, label)
            .expanded(show_descendants || expanded.contains(path))
            .children(descendants),
    )
}

impl Audit {
    fn browser_output_root(&self) -> PathBuf {
        self.browser_output_root.clone()
    }

    pub(super) fn has_visible_folders(&self) -> bool {
        self.folders
            .iter()
            .any(|path| !path.starts_with(&self.browser_output_root))
    }

    pub(super) fn browser_persistent(&self, window: &Window) -> bool {
        self.batch_size.is_none()
            && self.rail == Rail::None
            && f32::from(window.viewport_size().width) >= SIDEBAR_MIN_WINDOW_WIDTH
    }

    pub(super) fn browser_width(&self, window: &Window) -> f32 {
        if self.browser_persistent(window) {
            SIDEBAR_WIDTH
        } else {
            0.
        }
    }

    pub(super) fn toggle_browser(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        if self.batch_size.is_none() && !self.browser_persistent(window) {
            if self.browser_overlay {
                self.close_browser_overlay(window, cx);
            } else {
                self.browser_overlay = true;
                window.focus(&self.folder_filter_input.read(cx).focus_handle(cx), cx);
                cx.notify();
            }
        }
    }

    pub(super) fn close_browser_overlay(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        self.browser_overlay = false;
        window.focus(&self.focus, cx);
        cx.notify();
    }

    pub(super) fn breadcrumb_parts(&self) -> Vec<(String, PathBuf)> {
        if self.root.as_os_str().is_empty() {
            return Vec::new();
        }
        let mut paths = self
            .root
            .ancestors()
            .map(Path::to_path_buf)
            .collect::<Vec<_>>();
        paths.reverse();
        if let Some(home) = home_dir()
            && self.root.starts_with(&home)
            && let Some(start) = paths.iter().position(|path| path == &home)
        {
            paths.drain(..start);
        }

        if paths.len() > 4 {
            let hidden_parent = paths[paths.len() - 3].clone();
            paths = vec![
                paths[0].clone(),
                hidden_parent.clone(),
                paths[paths.len() - 2].clone(),
                paths[paths.len() - 1].clone(),
            ];
            return paths
                .into_iter()
                .enumerate()
                .map(|(index, path)| {
                    if index == 1 {
                        ("…".into(), path)
                    } else {
                        (path_label(&path), path)
                    }
                })
                .collect();
        }
        paths
            .into_iter()
            .map(|path| (path_label(&path), path))
            .collect()
    }

    pub(super) fn seed_tree_for_current_folder(&mut self, cx: &mut Context<Self>) {
        if self.root.as_os_str().is_empty() {
            let Some(home) = home_dir() else {
                return;
            };
            self.tree_anchor = home.clone();
            self.tree_expanded.insert(home.clone());
            self.rebuild_tree(cx);
            self.load_tree_children(home, cx);
            return;
        }
        self.install_tree_page(self.root.clone(), self.folders.clone(), cx);
    }

    pub(super) fn install_browse(
        &mut self,
        browsed: scan::Browse,
        root: PathBuf,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let folders = browsed.folders;
        self.install_dataset(browsed.scan, root.clone(), false, None, window, cx);
        self.install_browser_page(root, folders, cx);
        self.browser_overlay = false;
        cx.notify();
    }

    pub(super) fn install_browser_page(
        &mut self,
        root: PathBuf,
        folders: Vec<PathBuf>,
        cx: &mut Context<Self>,
    ) {
        self.folders = folders.clone();
        self.folder_scroll
            .scroll_to_item_strict(0, ScrollStrategy::Top);
        self.remember_recent_folder(&root);
        self.install_tree_page(root, folders, cx);
    }

    fn remember_recent_folder(&mut self, root: &Path) {
        self.recent_folders.retain(|path| path != root);
        self.recent_folders.insert(0, root.to_path_buf());
        self.recent_folders.truncate(settings::MAX_RECENT_FOLDERS);
    }

    fn install_tree_page(&mut self, root: PathBuf, folders: Vec<PathBuf>, cx: &mut Context<Self>) {
        if self.tree_anchor.as_os_str().is_empty() || !root.starts_with(&self.tree_anchor) {
            self.tree_anchor = root.clone();
            self.tree_children.clear();
            self.tree_loaded.clear();
            self.tree_loading.clear();
            self.tree_expanded.clear();
        }

        if let Ok(relative) = root.strip_prefix(&self.tree_anchor) {
            let mut parent = self.tree_anchor.clone();
            for component in relative.components() {
                let child = parent.join(component.as_os_str());
                let siblings = self.tree_children.entry(parent.clone()).or_default();
                if !siblings.contains(&child) {
                    siblings.push(child.clone());
                }
                self.tree_expanded.insert(parent);
                parent = child;
            }
        }
        self.tree_children.insert(root.clone(), folders);
        self.tree_loaded.insert(root.clone());
        self.tree_loading.remove(&root);
        self.tree_expanded.insert(root);
        self.rebuild_tree(cx);
    }

    pub(super) fn rebuild_tree(&mut self, cx: &mut Context<Self>) {
        if self.tree_anchor.as_os_str().is_empty() {
            return;
        }
        let query = self
            .folder_filter_input
            .read(cx)
            .value()
            .trim()
            .to_lowercase();
        let mut paths = HashMap::new();
        let output = self.browser_output_root();
        let item = tree_item(
            &self.tree_anchor,
            &self.tree_children,
            &self.tree_loaded,
            &self.tree_expanded,
            Some(&output),
            &query,
            &mut paths,
        );
        let current = tree_id(&self.root);
        self.tree_paths = paths;
        self.tree_state.update(cx, |state, cx| {
            state.set_items(item.into_iter().collect::<Vec<_>>(), cx);
            state.set_selected_index(state.index_of(&current.clone().into()), cx);
        });
    }

    fn rebuild_tree_preserving_selection(&mut self, cx: &mut Context<Self>) {
        let selected = self
            .tree_state
            .read(cx)
            .selected_item()
            .map(|item| item.id.clone());
        self.rebuild_tree(cx);
        if let Some(selected) = selected {
            self.tree_state.update(cx, |state, cx| {
                if let Some(index) = state.index_of(&selected) {
                    state.set_selected_index(Some(index), cx);
                }
            });
        }
    }

    pub(super) fn tree_event(&mut self, event: &TreeEvent, cx: &mut Context<Self>) {
        let (expanded, id) = match event {
            TreeEvent::Expanded(id) => (true, id.as_str()),
            TreeEvent::Collapsed(id) => (false, id.as_str()),
        };
        let Some(path) = self.tree_paths.get(id).cloned() else {
            return;
        };
        self.set_tree_expanded(path, expanded, cx);
    }

    fn set_tree_expanded(&mut self, path: PathBuf, expanded: bool, cx: &mut Context<Self>) {
        if expanded {
            self.tree_expanded.insert(path.clone());
            self.load_tree_children(path, cx);
        } else {
            self.tree_expanded.remove(&path);
        }
    }

    pub(super) fn load_tree_children(&mut self, path: PathBuf, cx: &mut Context<Self>) {
        if self.tree_loaded.contains(&path) || !self.tree_loading.insert(path.clone()) {
            return;
        }
        self.clear_error("folder-tree", cx);
        let anchor = self.tree_anchor.clone();
        cx.spawn(async move |this, cx| {
            let requested = path.clone();
            let folders = cx
                .background_executor()
                .spawn(async move { scan::child_folders(&path) })
                .await;
            let _ = this.update(cx, |audit, cx| {
                if audit.tree_anchor != anchor {
                    return;
                }
                audit.tree_loading.remove(&requested);
                match folders {
                    Ok(folders) => {
                        audit.tree_loaded.insert(requested.clone());
                        audit.tree_children.insert(requested, folders);
                    }
                    Err(error) => {
                        audit.tree_expanded.remove(&requested);
                        audit.notify_error(
                            "folder-tree",
                            "Couldn’t read folder",
                            format!("{}: {error}", requested.display()),
                            cx,
                        );
                    }
                }
                audit.rebuild_tree_preserving_selection(cx);
                cx.notify();
            });
        })
        .detach();
    }

    fn navigation_row(
        &self,
        id: String,
        label: String,
        path: PathBuf,
        icon: IconName,
        cx: &mut Context<Self>,
    ) -> gpui::AnyElement {
        let selected = self.root == path;
        let weak = cx.entity().downgrade();
        let selector = id.clone();
        ListItem::new(id)
            .w_full()
            .h(px(30.))
            .px_2()
            .selected(selected)
            .child(
                div()
                    .debug_selector(move || selector.clone())
                    .flex()
                    .items_center()
                    .gap_2()
                    .min_w_0()
                    .child(Icon::new(icon).size_4())
                    .child(
                        div()
                            .min_w_0()
                            .overflow_hidden()
                            .text_ellipsis()
                            .whitespace_nowrap()
                            .child(label),
                    ),
            )
            .on_click(move |_, window, cx| {
                if let Some(audit) = weak.upgrade() {
                    let path = path.clone();
                    audit.update(cx, |audit, cx| {
                        if !path.is_dir() {
                            audit.recent_folders.retain(|recent| recent != &path);
                            audit.notify_error(
                                "open-image",
                                "Folder is unavailable",
                                format!("{} no longer exists.", path.display()),
                                cx,
                            );
                            cx.notify();
                            return;
                        }
                        audit.close_browser_overlay(window, cx);
                        audit.request_path(path, cx);
                    });
                }
            })
            .into_any_element()
    }

    fn places(&self) -> Vec<(&'static str, PathBuf, IconName)> {
        home_dir()
            .map(|home| vec![("Home", home, IconName::Folder)])
            .unwrap_or_default()
    }

    pub(super) fn folder_sidebar(&self, cx: &mut Context<Self>) -> gpui::AnyElement {
        let output_root = self.browser_output_root();
        let places = self
            .places()
            .into_iter()
            .filter(|(_, path, _)| !path.starts_with(&output_root))
            .collect::<Vec<_>>();
        let place_paths = places
            .iter()
            .map(|(_, path, _)| path.clone())
            .collect::<HashSet<_>>();
        let recents = self
            .recent_folders
            .iter()
            .filter(|path| {
                *path != &self.root
                    && !path.starts_with(&output_root)
                    && !place_paths.contains(*path)
            })
            .cloned()
            .collect::<Vec<_>>();
        let searching = !self.folder_filter_input.read(cx).value().trim().is_empty();
        let no_folder_matches = searching && self.tree_paths.is_empty();
        let weak = cx.entity().downgrade();
        let paths = Arc::new(self.tree_paths.clone());
        let tree = tree(&self.tree_state, move |index, entry, selected, _, _| {
            let path = paths.get(entry.item().id.as_str()).cloned();
            let icon = if entry.is_expanded() {
                IconName::FolderOpen
            } else {
                IconName::Folder
            };
            let disclosure = if entry.is_expanded() {
                IconName::ChevronDown
            } else {
                IconName::ChevronRight
            };
            let disclosure_path = path.clone();
            let disclosure_expanded = entry.is_expanded();
            let disclosure_audit = weak.clone();
            let mut item = ListItem::new(format!("local-tree-{index}"))
                .w_full()
                .h(px(28.))
                .pl(px(8. + entry.depth() as f32 * 12.))
                .pr_2()
                .selected(selected)
                .child(
                    div()
                        .flex()
                        .items_center()
                        .gap_1()
                        .min_w_0()
                        .child(
                            div()
                                .id(format!("folder-disclosure-hit-{index}"))
                                .debug_selector(move || format!("folder-disclosure-{index}"))
                                .size_3()
                                .flex()
                                .items_center()
                                .justify_center()
                                .when(entry.is_folder(), |disclosure| {
                                    disclosure
                                        .on_mouse_down(gpui::MouseButton::Left, |_, _, cx| {
                                            cx.stop_propagation()
                                        })
                                        .on_click(move |_, _, cx| {
                                            cx.stop_propagation();
                                            let Some(path) = disclosure_path.clone() else {
                                                return;
                                            };
                                            if let Some(audit) = disclosure_audit.upgrade() {
                                                audit.update(cx, |audit, cx| {
                                                    audit.set_tree_expanded(
                                                        path,
                                                        !disclosure_expanded,
                                                        cx,
                                                    );
                                                    audit.rebuild_tree_preserving_selection(cx);
                                                    cx.notify();
                                                });
                                            }
                                        })
                                })
                                .children(
                                    entry.is_folder().then(|| Icon::new(disclosure).size_3()),
                                ),
                        )
                        .child(Icon::new(icon).size_4())
                        .child(
                            div()
                                .min_w_0()
                                .overflow_hidden()
                                .text_ellipsis()
                                .whitespace_nowrap()
                                .child(entry.item().label.clone()),
                        ),
                );
            if let Some(path) = path {
                let weak = weak.clone();
                item = item.on_click(move |_, window, cx| {
                    if let Some(audit) = weak.upgrade() {
                        let path = path.clone();
                        audit.update(cx, |audit, cx| {
                            audit.close_browser_overlay(window, cx);
                            audit.request_path(path, cx);
                        });
                    }
                });
            }
            item
        });

        div()
            .debug_selector(|| "folder-sidebar".into())
            .flex()
            .flex_col()
            .w(px(SIDEBAR_WIDTH))
            .h_full()
            .flex_none()
            .overflow_hidden()
            .bg(cx.theme().secondary)
            .border_r_1()
            .border_color(cx.theme().border)
            .child(
                div()
                    .debug_selector(|| "folder-search".into())
                    .px_2()
                    .pt_2()
                    .pb_1()
                    .on_key_down(
                        cx.listener(|audit, event: &gpui::KeyDownEvent, window, cx| {
                            if event.keystroke.key == "down"
                                && event.keystroke.modifiers == gpui::Modifiers::none()
                                && audit
                                    .folder_filter_input
                                    .read(cx)
                                    .focus_handle(cx)
                                    .is_focused(window)
                            {
                                audit
                                    .tree_state
                                    .update(cx, |tree, cx| tree.focus(window, cx));
                                cx.stop_propagation();
                            }
                        }),
                    )
                    .child(
                        Input::new(&self.folder_filter_input)
                            .small()
                            .cleanable(true)
                            .prefix(IconName::Search),
                    ),
            )
            .child(
                div()
                    .px_3()
                    .pt_2()
                    .pb_1()
                    .text_size(px(10.))
                    .font_weight(FontWeight::SEMIBOLD)
                    .text_color(cx.theme().muted_foreground)
                    .child("PLACES"),
            )
            .children(
                places
                    .into_iter()
                    .enumerate()
                    .map(|(index, (label, path, icon))| {
                        self.navigation_row(format!("place-{index}"), label.into(), path, icon, cx)
                    }),
            )
            .when(!recents.is_empty(), |sidebar| {
                sidebar
                    .child(
                        div()
                            .px_3()
                            .pt_3()
                            .pb_1()
                            .text_size(px(10.))
                            .font_weight(FontWeight::SEMIBOLD)
                            .text_color(cx.theme().muted_foreground)
                            .child("RECENT"),
                    )
                    .children(recents.into_iter().enumerate().map(|(index, path)| {
                        self.navigation_row(
                            format!("recent-{index}"),
                            path_label(&path),
                            path,
                            IconName::Folder,
                            cx,
                        )
                    }))
            })
            .child(
                div()
                    .px_3()
                    .pt_3()
                    .pb_1()
                    .text_size(px(10.))
                    .font_weight(FontWeight::SEMIBOLD)
                    .text_color(cx.theme().muted_foreground)
                    .child("FOLDERS"),
            )
            .child(
                div()
                    .flex_1()
                    .min_h_0()
                    .pb_2()
                    .on_key_down(
                        cx.listener(|audit, event: &gpui::KeyDownEvent, window, cx| {
                            let key = event.keystroke.key.as_str();
                            if key == "enter"
                                && event.keystroke.modifiers == gpui::Modifiers::none()
                            {
                                let selected = audit
                                    .tree_state
                                    .read(cx)
                                    .selected_item()
                                    .map(|item| item.id.to_string());
                                if let Some(path) =
                                    selected.and_then(|id| audit.tree_paths.get(&id).cloned())
                                {
                                    audit.close_browser_overlay(window, cx);
                                    audit.request_path(path, cx);
                                }
                            }
                            if key == "escape" && audit.browser_overlay {
                                audit.close_browser_overlay(window, cx);
                            }
                            if matches!(
                                key,
                                "up" | "down"
                                    | "left"
                                    | "right"
                                    | "pagedown"
                                    | "pageup"
                                    | "home"
                                    | "end"
                                    | "escape"
                                    | "space"
                                    | "enter"
                            ) || (key == "a"
                                && (event.keystroke.modifiers.control
                                    || event.keystroke.modifiers.platform))
                            {
                                cx.stop_propagation();
                            }
                        }),
                    )
                    .when(no_folder_matches, |folders| {
                        folders.child(
                            div()
                                .px_3()
                                .py_2()
                                .text_sm()
                                .text_color(cx.theme().muted_foreground)
                                .child("No folders found"),
                        )
                    })
                    .when(!no_folder_matches, |folders| folders.child(tree)),
            )
            .into_any_element()
    }

    pub(super) fn folder_rows(&mut self, cx: &mut Context<Self>) -> gpui::AnyElement {
        let output = self.browser_output_root();
        let folders = Arc::new(
            self.folders
                .iter()
                .filter(|path| !path.starts_with(&output))
                .cloned()
                .collect::<Vec<_>>(),
        );
        let count = folders.len();
        let height = FOLDER_ROW_HEIGHT * count.min(MAX_VISIBLE_FOLDER_ROWS) as f32;
        let rows = uniform_list(
            "folder-rows",
            count,
            cx.processor(move |_audit, range: Range<usize>, _, cx| {
                let folders = folders.clone();
                range
                    .filter_map(|index| {
                        let path = folders.get(index)?.clone();
                        let name = path_label(&path);
                        let selector = format!("child-folder:{name}");
                        Some(
                            div()
                                .debug_selector(move || selector.clone())
                                .child(
                                    ListItem::new(format!("child-folder-{index}"))
                                        .w_full()
                                        .h(px(FOLDER_ROW_HEIGHT))
                                        .px_3()
                                        .child(
                                            div()
                                                .flex()
                                                .items_center()
                                                .gap_2()
                                                .child(Icon::new(IconName::Folder).size_4())
                                                .child(name),
                                        )
                                        .on_click(cx.listener(move |audit, _, _, cx| {
                                            audit.request_path(path.clone(), cx);
                                        })),
                                )
                                .into_any_element(),
                        )
                    })
                    .collect::<Vec<_>>()
            }),
        )
        .track_scroll(&self.folder_scroll)
        .h(px(height));

        div()
            .debug_selector(|| "child-folders".into())
            .flex()
            .flex_col()
            .flex_none()
            .bg(cx.theme().table)
            .border_b_1()
            .border_color(cx.theme().border)
            .child(
                div()
                    .h(px(26.))
                    .flex()
                    .items_center()
                    .px_3()
                    .text_size(px(10.))
                    .font_weight(FontWeight::SEMIBOLD)
                    .text_color(cx.theme().muted_foreground)
                    .child(format!("FOLDERS · {count}")),
            )
            .child(
                div()
                    .relative()
                    .h(px(height))
                    .overflow_hidden()
                    .child(rows)
                    .when(count > MAX_VISIBLE_FOLDER_ROWS, |list| {
                        list.child(
                            div()
                                .absolute()
                                .top_0()
                                .right_0()
                                .bottom_0()
                                .w(Scrollbar::width())
                                .child(
                                    Scrollbar::vertical(&self.folder_scroll)
                                        .mode(ScrollbarMode::Always)
                                        .viewport_from_layout(),
                                ),
                        )
                    }),
            )
            .into_any_element()
    }
}
