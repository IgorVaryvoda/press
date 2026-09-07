//! The action bar and its rails.
//!
//! The bar floats over the list and holds verbs only: Convert, the two local
//! models, and hosted AI operations. Choosing one opens its rail on the
//! right, carrying that operation's settings and the button that commits it.
//! No operation borrows another's controls, and the bar never has to explain
//! itself — which is what the old right-hand inspector column was doing for
//! conversion alone.

use super::*;

/// The open panel's built-in width, and the range its grab edge allows. The
/// table and the gallery lay themselves out against the viewport minus the
/// rail, so a panel never silently squeezes their column math.
pub(super) const RAIL_WIDTH: f32 = 320.;
pub(super) const RAIL_MIN: f32 = 280.;
pub(super) const RAIL_MAX: f32 = 520.;
/// How far past the minimum a drag goes before the panel collapses.
pub(super) const RAIL_SNAP: f32 = 60.;
/// Room the list leaves under itself for the floating bar: the bar's 18px
/// offset and 46px height, plus a gap. Without the room the bar covers the
/// last row; without the gap the list's cut-off bottom row meets the bar's
/// top edge and reads as hidden under it rather than as the viewport's end.
pub(super) const BAR_CLEARANCE: f32 = 72.;

/// Below this much room, the three secondary verbs drop to icons. The bar has
/// to fit the list it floats over, and at the minimum window with the panel
/// open there are about 400px to fit into.
const BAR_LABELS_WIDTH: f32 = 720.;
/// Below this, the readout goes too: the verbs are what the bar is for.
const BAR_READOUT_WIDTH: f32 = 560.;

/// The named outputs most runs want, through the recipe model personal rows
/// share: the rows and the settings they apply cannot disagree.
fn builtin_recipes() -> [crate::recipe::Recipe; 4] {
    crate::recipe::Recipe::builtins()
}

pub(super) fn active_preset(format: Format, quality: Quality, edge: MaxEdge) -> Option<usize> {
    builtin_recipes().iter().position(|row| {
        let (row_format, row_quality, row_edge, _) = row.effective();
        format == row_format && quality == row_quality && edge == row_edge
    })
}

/// What the summary calls a run that has ended. A run the user stopped kept every
/// file it wrote, so it says how far it got: the files it never started are not
/// failures, and the ones it finished are as real as any other result.
pub(super) fn conversion_result_state(stopped_total: Option<usize>, converted: usize) -> String {
    match stopped_total {
        Some(total) => format!("STOPPED · {converted} OF {total} CONVERTED"),
        None => "COMPLETED · ACTUAL RESULT".into(),
    }
}

pub(super) fn sampling_note(sampled: usize, total: usize) -> String {
    if sampled < total {
        format!(" · {sampled}\u{a0}of\u{a0}{total}\u{a0}sampled")
    } else {
        String::new()
    }
}

impl Audit {
    /// The bar itself: what the folder stands to save, then the verbs. It floats
    /// over the list rather than reserving a column, because it is four words
    /// wide and the list is the thing you came to read.
    pub(super) fn action_bar(&self, width: f32, cx: &mut Context<Self>) -> impl IntoElement {
        let labelled = width >= BAR_LABELS_WIDTH;
        let target_count = self.target_count();
        let single = self.single_target();
        let busy = self.converting
            || self.local_ai_busy()
            || self.studio_busy()
            || self.scan_blocks_delivery();

        div()
            .absolute()
            .left_0()
            .right_0()
            .bottom(px(18.))
            .flex()
            .justify_center()
            .child(
                div()
                    .debug_selector(|| "action-bar".into())
                    .flex()
                    .items_center()
                    .gap_2()
                    .h(px(46.))
                    .px_2()
                    .rounded_lg()
                    .bg(cx.theme().secondary)
                    .border_1()
                    .border_color(cx.theme().border)
                    .shadow_lg()
                    .block_mouse_except_scroll()
                    .children((width >= BAR_READOUT_WIDTH).then(|| self.bar_readout(cx)))
                    .children(
                        (width >= BAR_READOUT_WIDTH)
                            .then(|| div().w(px(1.)).h(px(20.)).bg(cx.theme().border)),
                    )
                    .child(
                        Button::new("rail-convert")
                            .small()
                            .icon(IconName::Replace)
                            .label("Convert")
                            .tooltip("Choose a format and quality, then convert")
                            .outline()
                            .selected(self.rail == Rail::Convert)
                            .disabled(busy || target_count == 0)
                            .on_click(
                                cx.listener(|audit, _, _, cx| audit.open_rail(Rail::Convert, cx)),
                            ),
                    )
                    // Absent rather than disabled where the models cannot run.
                    .children(local_ai::available().then(|| {
                        self.local_ai_action(
                            "rail-remove-background",
                            Rail::RemoveBackground,
                            IconName::Frame,
                            "Remove background",
                            local_ai::Tool::RemoveBackground,
                            single,
                            busy,
                            labelled,
                            cx,
                        )
                    }))
                    .children(local_ai::available().then(|| {
                        self.local_ai_action(
                            "rail-upscale",
                            Rail::Upscale,
                            IconName::Maximize,
                            "Upscale 4×",
                            local_ai::Tool::Upscale,
                            single,
                            busy,
                            labelled,
                            cx,
                        )
                    })),
            )
    }

    /// The projected result beside the selection it describes. The Convert rail
    /// carries the same numbers, but the decision happens here, next to the
    /// verbs, so the readout earns its width by answering it.
    pub(super) fn savings_note(&self) -> Option<String> {
        let (projected, counted, refused) = self.estimate?;
        if counted == 0 && refused > 0 {
            return Some(format!("· {refused} refused at these settings"));
        }
        let source = self.selected_target_bytes;
        if source == 0 {
            return None;
        }
        let growth = projected > source;
        let percent = source.abs_diff(projected) as f32 / source as f32 * 100.;
        Some(format!(
            "· ≈{} output, {:.0}% {}{}",
            format_bytes(projected),
            percent,
            if growth { "larger" } else { "saved" },
            if refused > 0 {
                format!(" · {refused} refused")
            } else {
                String::new()
            },
        ))
    }

    /// Left of the divider: the projected saving, or the selection once there is
    /// one. Both are facts about what the verbs beside them would act on.
    fn bar_readout(&self, cx: &Context<Self>) -> impl IntoElement {
        let row = div().flex().items_center().gap_2().pl_2().pr_1();
        // A run in progress outranks both the estimate and the selection: the
        // bar has to stand on its own with every rail closed, and while work is
        // happening the only fact worth its width is how far along it is.
        if self.converting {
            let done = self.results.len() + self.failures.len();
            let total = self
                .active_target_count
                .unwrap_or_else(|| self.target_count());
            return row
                .child(
                    div()
                        .font_family(cx.theme().mono_font_family.clone())
                        .text_size(px(13.))
                        .font_weight(FontWeight::MEDIUM)
                        .text_color(cx.theme().foreground)
                        .whitespace_nowrap()
                        .child(format!("{done} of {total}")),
                )
                .child(
                    div()
                        .text_size(px(11.))
                        .text_color(cx.theme().muted_foreground)
                        .whitespace_nowrap()
                        .child("converting"),
                );
        }
        if self.selected.is_empty() {
            return row
                .child(
                    div()
                        .text_size(px(12.))
                        .font_weight(FontWeight::MEDIUM)
                        .text_color(cx.theme().foreground)
                        .whitespace_nowrap()
                        .child("Select images"),
                )
                .child(
                    div()
                        .text_size(px(11.))
                        .text_color(cx.theme().muted_foreground)
                        .whitespace_nowrap()
                        .child("to analyse"),
                )
                .child(
                    div().debug_selector(|| "bar-select-all".into()).child(
                        Button::new("bar-select-all")
                            .small()
                            .ghost()
                            .label("Select all")
                            .disabled(self.converting)
                            .on_click(cx.listener(|audit, _, _, cx| audit.toggle_select_all(cx))),
                    ),
                );
        }
        row.child(
            div()
                .text_size(px(12.))
                .text_color(cx.theme().foreground)
                .whitespace_nowrap()
                .child(format!(
                    "{} of {}",
                    self.selected_target_count,
                    self.visible.len()
                )),
        )
        .children(self.savings_note().map(|note| {
            div()
                .text_size(px(11.))
                .text_color(cx.theme().muted_foreground)
                .whitespace_nowrap()
                .child(note)
        }))
        .child(
            Button::new("bar-select-all")
                .small()
                .ghost()
                .label(if self.selection_state() == table::SelectionState::All {
                    "Clear"
                } else {
                    "Select all"
                })
                .disabled(self.converting)
                .on_click(cx.listener(|audit, _, _, cx| audit.toggle_select_all(cx))),
        )
    }

    /// Single images use the local model; batches open the Studio handoff.
    #[allow(clippy::too_many_arguments)]
    fn local_ai_action(
        &self,
        id: &'static str,
        rail: Rail,
        icon: IconName,
        label: &'static str,
        tool: local_ai::Tool,
        single: Option<usize>,
        busy: bool,
        labelled: bool,
        cx: &mut Context<Self>,
    ) -> impl IntoElement {
        let batch = self.target_count() > 1;
        let blocked = match single {
            None if batch => None,
            None => Some("Select one image to run this on".to_string()),
            Some(index) => {
                let entry = self.entries.get(index);
                match (tool, entry) {
                    (local_ai::Tool::Upscale, Some(entry)) => {
                        local_ai::upscale_dimensions(entry.width, entry.height)
                            .err()
                            .map(|message| format!("{message}; use AI operations for this image"))
                    }
                    _ => None,
                }
            }
        };
        let running = self
            .local_ai_job
            .as_ref()
            .is_some_and(|job| job.busy() && job.tool == tool);
        let tooltip = match (&blocked, busy) {
            (Some(reason), _) => reason.clone(),
            (None, true) => self
                .local_ai_job
                .as_ref()
                .map(|job| job.message(&self.root))
                .or_else(|| self.studio_job.as_ref().map(|job| job.message(&self.root)))
                .unwrap_or_else(|| "Local AI is running…".into()),
            (None, false) if batch => format!("{label} for multiple images in Sirv AI Studio"),
            (None, false) => format!("{label} on this computer; the first run downloads the model"),
        };
        Button::new(id)
            .small()
            .outline()
            .icon(icon)
            .when(labelled, |button| button.label(label))
            .tooltip(tooltip)
            .selected(self.rail == rail)
            .loading(running)
            .disabled(blocked.is_some() || busy)
            .on_click(cx.listener(move |audit, _, _, cx| audit.open_rail(rail, cx)))
            .map(|button| div().debug_selector(move || id.into()).child(button))
    }

    pub(super) fn rail_view(&self, cx: &mut Context<Self>) -> impl IntoElement {
        div()
            .flex()
            .flex_shrink_0()
            .h_full()
            .children(self.sidebar_open.then(|| self.rail_panel(cx)))
    }

    fn tool_chooser(&self, cx: &mut Context<Self>) -> impl IntoElement {
        div()
            .debug_selector(|| "tool-chooser".into())
            .flex()
            .flex_col()
            .flex_1()
            .justify_center()
            .gap_2()
            .p_4()
            .children(
                [
                    (Rail::Convert, "Convert", Icon::new(IconName::Replace)),
                    (
                        Rail::RemoveBackground,
                        "Remove background",
                        Icon::default().path("icons/studio/background-removal.svg"),
                    ),
                    (
                        Rail::Upscale,
                        "Upscale",
                        Icon::default().path("icons/studio/upscale.svg"),
                    ),
                    (Rail::Studio, "AI operations", Icon::new(IconName::Bot)),
                ]
                .into_iter()
                .map(|(rail, label, icon)| {
                    div()
                        .debug_selector(move || format!("tool-{}", rail.slug()))
                        .child(
                            Button::new(("tool", rail as usize))
                                .outline()
                                .w_full()
                                .h_9()
                                .rounded_lg()
                                .accessibility_label(label)
                                .child(
                                    div()
                                        .w_full()
                                        .flex()
                                        .items_center()
                                        .gap_2()
                                        .text_size(px(13.))
                                        .child(icon.size_4())
                                        .child(label),
                                )
                                .on_click(
                                    cx.listener(move |audit, _, _, cx| audit.open_rail(rail, cx)),
                                ),
                        )
                }),
            )
    }

    /// The open panel: the lit operation's title, its settings, and its commit
    /// at the foot — the same shape whichever operation it belongs to. Its
    /// left edge is the grab edge that sizes it.
    fn rail_panel(&self, cx: &mut Context<Self>) -> impl IntoElement {
        let tab = self.rail;
        let title = tab.title();
        let body = match tab {
            Rail::Convert => self.convert_rail(cx).into_any_element(),
            Rail::RemoveBackground | Rail::Upscale => self.local_ai_rail(tab, cx),
            Rail::Studio => self.studio_rail(cx),
            Rail::None => self.tool_chooser(cx).into_any_element(),
        };
        let accent = cx.theme().primary;
        div()
            .debug_selector(|| "rail".into())
            .relative()
            .w(px(self.rail_size))
            .flex_shrink_0()
            .flex()
            .flex_col()
            .border_l_1()
            .border_color(cx.theme().border)
            .bg(cx.theme().background)
            .children((tab != Rail::None).then(|| {
                div()
                    .flex()
                    .items_center()
                    .gap_2()
                    .flex_shrink_0()
                    .h(px(48.))
                    .px_3()
                    .child(
                        div().debug_selector(|| "tools-back".into()).child(
                            Button::new("tools-back")
                                .small()
                                .ghost()
                                .icon(IconName::ArrowLeft)
                                .tooltip("Back to tools")
                                .disabled(self.converting)
                                .on_click(cx.listener(|audit, _, window, cx| {
                                    if !audit.converting {
                                        audit.rail = Rail::None;
                                        window.focus(&audit.focus, cx);
                                        cx.notify();
                                    }
                                })),
                        ),
                    )
                    .child(
                        div()
                            .font_family("SF Pro Display")
                            .text_size(px(14.))
                            .font_weight(FontWeight::SEMIBOLD)
                            .text_color(cx.theme().foreground)
                            .child(title),
                    )
            }))
            .child(body)
            .child(
                // The grab edge. Drag it to size the panel; drag it past the
                // minimum and the panel collapses.
                div()
                    .id("rail-resize")
                    .debug_selector(|| "rail-resize".into())
                    .absolute()
                    .left_0()
                    .top_0()
                    .bottom_0()
                    .w(px(6.))
                    .cursor_ew_resize()
                    .hover(move |edge| edge.bg(accent.opacity(0.5)))
                    .when(self.rail_drag, |edge| edge.bg(accent))
                    .on_mouse_down(
                        gpui_kit::MouseButton::Left,
                        cx.listener(|audit, _, _, cx| {
                            audit.rail_drag = true;
                            cx.notify();
                        }),
                    ),
            )
    }

    fn convert_rail(&self, cx: &mut Context<Self>) -> impl IntoElement {
        div()
            .flex()
            .flex_col()
            .flex_1()
            .min_h_0()
            .child(
                div()
                    .id("rail-settings")
                    .debug_selector(|| "rail-settings".into())
                    .flex()
                    .flex_col()
                    .flex_1()
                    .min_h_0()
                    .overflow_y_scroll()
                    .gap_3()
                    .px_4()
                    .py_2()
                    // Where the output lands, said once and always. Every other
                    // number in this rail is about size; this one is about the
                    // question people actually ask before pressing Convert.
                    .child(self.destination_row(cx))
                    // Preset and controls share the sidebar's inset.
                    .child(
                        div()
                            .debug_selector(|| "transform-card".into())
                            .flex()
                            .flex_col()
                            .gap_2()
                            .child(self.preset_row(cx))
                            .child(div().debug_selector(|| "format-setting".into()).child(
                                self.panel_setting(
                                    "Format",
                                    self.format_group(cx).small().outline().compact().w_full(),
                                    cx,
                                ),
                            ))
                            .child(self.panel_quality(cx))
                            .child(div().debug_selector(|| "max-size-setting".into()).child(
                                self.panel_setting("Max size", self.resize_control(cx), cx),
                            )),
                    )
                    .children((!self.recipes_skipped.is_empty()).then(|| {
                        div()
                            .debug_selector(|| "recipes-skipped".into())
                            .text_size(px(11.))
                            .text_color(cx.theme().muted_foreground)
                            .child(format!(
                                "{} preset files unreadable",
                                self.recipes_skipped.len()
                            ))
                    }))
                    .child(self.sets_section(cx)),
            )
            .child(
                div()
                    .flex_shrink_0()
                    .flex()
                    .flex_col()
                    .gap_1()
                    .px_4()
                    .py_3()
                    .border_t_1()
                    .border_color(cx.theme().border)
                    .child(self.output_summary(cx)),
            )
    }

    /// Both local models take no settings: what the rail owes you is what it is
    /// about to do, to which file, and the download the first run costs.
    fn local_ai_rail(&self, rail: Rail, cx: &mut Context<Self>) -> gpui_kit::AnyElement {
        let tool = match rail {
            Rail::Upscale => local_ai::Tool::Upscale,
            _ => local_ai::Tool::RemoveBackground,
        };
        if self.target_count() > 1 {
            let (operation, url) = match tool {
                local_ai::Tool::Upscale => ("upscaling", studio::BATCH_UPSCALE_URL),
                local_ai::Tool::RemoveBackground => {
                    ("background removal", studio::BATCH_BACKGROUND_REMOVAL_URL)
                }
            };
            return div()
                .debug_selector(|| "local-ai-batch-handoff".into())
                .flex()
                .flex_col()
                .flex_1()
                .justify_center()
                .gap_3()
                .p_4()
                .text_sm()
                .child(
                    div()
                        .font_weight(FontWeight::SEMIBOLD)
                        .child(format!("{} images selected", self.target_count())),
                )
                .child(format!(
                    "Press runs {operation} on one image at a time. \
                     Use Sirv AI Studio to process multiple images together."
                ))
                .child(
                    div()
                        .text_xs()
                        .text_color(cx.theme().muted_foreground)
                        .child("Opens in your browser. Sign in and add your images there; Press does not upload this selection."),
                )
                .child(
                    div()
                        .debug_selector(|| "open-studio-batch".into())
                        .child(
                            Button::new("open-studio-batch")
                                .primary()
                                .w_full()
                                .icon(IconName::ExternalLink)
                                .label("Open in Sirv AI Studio")
                                .on_click(move |_, _, cx| cx.open_url(url)),
                        ),
                )
                .child(
                    div()
                        .text_xs()
                        .text_color(cx.theme().muted_foreground)
                        .child("Or select one image to process it locally."),
                )
                .into_any_element();
        }
        let index = self.single_target();
        let entry = index.and_then(|index| self.entries.get(index));
        // The thumbnail the list already decoded. A rail that names a file and
        // then leaves 400px of nothing under it reads as unfinished, and the
        // picture answers "which image" better than the name does.
        let preview = index.and_then(|index| self.thumbs.get(&index).cloned());
        let installed = local_ai::installed(tool);
        // The commit names the outcome rather than repeating the rail's own
        // title, which the bar is already saying a third time.
        let mut commit = rail.title().to_string();
        let detail = match (tool, entry) {
            (local_ai::Tool::Upscale, Some(entry)) => {
                match local_ai::upscale_dimensions(entry.width, entry.height) {
                    Ok((width, height)) => {
                        commit = format!("Upscale to {width}×{height}");
                        format!("{}×{} → {width}×{height}", entry.width, entry.height)
                    }
                    Err(message) => message,
                }
            }
            (local_ai::Tool::RemoveBackground, Some(_)) => {
                "Writes a transparent PNG; the original is untouched".to_string()
            }
            (_, None) => "Select one image first".to_string(),
        };
        let note = match (tool, installed) {
            (_, true) => "Runs on this computer. Nothing is uploaded.".to_string(),
            (local_ai::Tool::RemoveBackground, false) => {
                "Runs on this computer. The first run downloads BiRefNet, up to 104 MB.".to_string()
            }
            (local_ai::Tool::Upscale, false) => {
                "Runs on this computer. The first run downloads Remacri ESRGAN, up to 49 MB."
                    .to_string()
            }
        };

        div()
            .debug_selector(|| "local-ai-single".into())
            .flex()
            .flex_col()
            .flex_1()
            .min_h_0()
            .child(
                div()
                    .flex()
                    .flex_col()
                    .flex_1()
                    .min_h_0()
                    .gap_2()
                    .px_3()
                    .py_3()
                    .children(preview.map(|image| {
                        // Hugs the thumbnail the list decoded rather than
                        // sitting it in a fixed box: blowing a 96px thumb up to
                        // fill 260px would only show you the mush.
                        div()
                            .w_full()
                            .flex()
                            .items_center()
                            .justify_center()
                            .py_2()
                            .rounded_md()
                            .bg(cx.theme().background)
                            .overflow_hidden()
                            .child(img(image).max_w_full().max_h(px(160.)))
                    }))
                    .child(
                        div()
                            .text_size(px(13.))
                            .text_color(cx.theme().foreground)
                            .overflow_hidden()
                            .text_ellipsis()
                            .whitespace_nowrap()
                            .child(
                                entry.map_or_else(|| "No image selected".to_string(), Entry::name),
                            ),
                    )
                    .child(
                        div()
                            .font_family(cx.theme().mono_font_family.clone())
                            .text_size(px(11.))
                            .text_color(cx.theme().muted_foreground)
                            .child(detail),
                    )
                    .child(
                        div()
                            .mt_1()
                            .px_2()
                            .py_2()
                            .rounded_md()
                            .bg(cx.theme().background)
                            .text_size(px(11.))
                            .text_color(cx.theme().muted_foreground)
                            .child(note),
                    ),
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
                    .children(
                        self.local_ai_job
                            .as_ref()
                            .map(|job| job.message(&self.root))
                            .map(|message| {
                                div()
                                    .text_size(px(11.))
                                    .text_color(cx.theme().muted_foreground)
                                    .child(message)
                            }),
                    )
                    .child(
                        Button::new("run-local-ai")
                            .primary()
                            .w_full()
                            .label(commit)
                            .disabled(
                                entry.is_none()
                                    || self.local_ai_busy()
                                    || self.studio_busy()
                                    || self.converting
                                    || self.scan_blocks_delivery(),
                            )
                            .on_click(cx.listener(move |audit, _, _, cx| {
                                if let Some(index) = audit.single_target() {
                                    audit.start_local_ai(tool, index, cx);
                                }
                            })),
                    ),
            )
            .into_any_element()
    }

    /// The gutter's header control: which optional columns are on. Sirv and
    /// Result are not listed — they appear when a pairing or a conversion
    /// exists, which is not a preference to hold an opinion about.
    pub(super) fn column_picker(&mut self, cx: &mut Context<Self>) -> gpui_kit::AnyElement {
        let audit = cx.entity();
        let prefs = self.column_prefs;
        div()
            .size_full()
            .flex()
            .items_center()
            .justify_end()
            .child(
                Popover::new("column-picker")
                    .anchor(gpui_kit::Anchor::TopRight)
                    .trigger(
                        Button::new("column-picker-trigger")
                            .xsmall()
                            .ghost()
                            .icon(IconName::Settings2)
                            .tooltip("Choose columns"),
                    )
                    .content(move |_, _, _| {
                        let audit = audit.clone();
                        let reset = audit.clone();
                        div()
                            .debug_selector(|| "column-picker".into())
                            .w(px(200.))
                            .flex()
                            .flex_col()
                            .gap_0p5()
                            .p_1()
                            .children(table::OPTIONAL_COLUMNS.iter().enumerate().map(
                                |(index, (label, shown, _))| {
                                    let audit = audit.clone();
                                    div().px_1p5().py_1().child(
                                        Checkbox::new(("column", index))
                                            .label(*label)
                                            .checked(shown(&prefs))
                                            .on_click(move |_: &bool, _, cx| {
                                                audit.update(cx, |audit, cx| {
                                                    audit.toggle_column(index, cx)
                                                });
                                            }),
                                    )
                                },
                            ))
                            .child(
                                Button::new("columns-reset")
                                    .small()
                                    .ghost()
                                    .w_full()
                                    .justify_start()
                                    .label("Reset to defaults")
                                    .on_click(move |_, _, cx| {
                                        reset.update(cx, |audit, cx| audit.reset_columns(cx));
                                    }),
                            )
                    }),
            )
            .into_any_element()
    }

    /// Where the output lands, said once and always — and changeable, because
    /// `optimized/` beside the originals is the right default and the wrong
    /// answer for anyone whose output belongs in a staging folder or a build
    /// tree. The originals never move either way.
    fn destination_row(&self, cx: &mut Context<Self>) -> impl IntoElement {
        let custom = matches!(self.output, Output::Folder(_));
        let replacing = self.output == Output::Replace;
        div()
            .debug_selector(|| "output-destination".into())
            .flex()
            .flex_col()
            .gap_1()
            .child(
                div()
                    .flex()
                    .items_center()
                    .gap_1()
                    .child(
                        div()
                            .flex()
                            .items_center()
                            .gap_1()
                            .flex_1()
                            .min_w_0()
                            .text_size(px(12.))
                            .text_color(cx.theme().foreground)
                            .child(IconName::FolderOpen)
                            .child(
                                div()
                                    .flex_1()
                                    .min_w_0()
                                    .whitespace_nowrap()
                                    .overflow_hidden()
                                    .text_ellipsis()
                                    .child(self.output.label()),
                            ),
                    )
                    .children(custom.then(|| {
                        Button::new("output-default")
                            .xsmall()
                            .ghost()
                            .label("Reset")
                            .tooltip("Write into optimized/ beside the originals again")
                            .disabled(
                                self.converting
                                    || root_needs_custom_output(&self.root, &Output::Optimized),
                            )
                            .on_click(cx.listener(|audit, _, _, cx| audit.reset_output(cx)))
                    }))
                    .children((!replacing).then(|| {
                        Button::new("output-choose")
                            .xsmall()
                            .ghost()
                            .label("Change")
                            .tooltip("Choose the folder converted files are written to")
                            .disabled(self.converting)
                            .on_click(cx.listener(|audit, _, _, cx| audit.pick_output(cx)))
                    })),
            )
            // A mode, so a switch: "Replace" beside "Change" read as two
            // folder links, and one of them rewrote the folder you audited.
            .child(
                div()
                    .debug_selector(|| "output-replace".into())
                    .flex()
                    .items_center()
                    .justify_between()
                    .h_6()
                    .text_size(px(13.))
                    .child("Replace originals in place")
                    .child(
                        Switch::new("output-replace")
                            .small()
                            .checked(replacing)
                            .accessibility_label("Replace originals in place")
                            .disabled(self.converting)
                            .on_click(cx.listener(|audit, _, _, cx| {
                                if audit.output == Output::Replace {
                                    audit.reset_output(cx);
                                } else {
                                    audit.use_replace_output(cx);
                                }
                            })),
                    ),
            )
            // The promise the whole app rests on, kept in view rather than
            // discovered afterwards — and said differently when the destination
            // is the folder itself, because there the promise is the backup.
            .child(
                div()
                    .debug_selector(|| "output-promise".into())
                    .text_size(px(11.))
                    .text_color(cx.theme().muted_foreground)
                    .child(if replacing {
                        format!(
                            "Originals move to {}/ and can be restored",
                            crate::scan::BACKUP_DIR
                        )
                    } else {
                        "Originals are never touched".to_string()
                    }),
            )
    }

    /// A label over its control, each on its own line: the column is narrow on
    /// purpose, and side-by-side labels were what made the old strip cramped.
    fn panel_setting(
        &self,
        label: &'static str,
        control: impl IntoElement,
        cx: &Context<Self>,
    ) -> impl IntoElement {
        div()
            .flex()
            .flex_col()
            .gap_1()
            .child(
                div()
                    .text_size(px(12.))
                    .text_color(cx.theme().muted_foreground)
                    .child(label),
            )
            .child(control)
    }

    /// The row the dials currently name: the applied one, or failing that the
    /// first row the dials still match.
    fn selected_preset_row(&self) -> Option<crate::recipe::Recipe> {
        let rows: Vec<_> = builtin_recipes()
            .into_iter()
            .chain(self.recipes.iter().cloned())
            .collect();
        self.selected_recipe
            .as_deref()
            .and_then(|id| rows.iter().find(|row| row.id == id))
            .or_else(|| rows.iter().find(|row| !self.recipe_modified(row)))
            .cloned()
    }

    /// What the estimate says will run: the preset's name, marked when the
    /// dials left it, or "custom settings" when no row matches.
    pub(super) fn preset_label(&self) -> String {
        match self.selected_preset_row() {
            Some(row) if self.recipe_modified(&row) => format!("{} (edited)", row.name),
            Some(row) => row.name,
            None => "custom settings".to_string(),
        }
    }

    /// The preset row: its label and edited mark, the chooser, the actions
    /// menu, and the name prompt while Save as or Rename asks for one. The
    /// chooser lists rows only; the verbs live behind the dots, so the list
    /// stays a list.
    fn preset_row(&self, cx: &Context<Self>) -> impl IntoElement + use<> {
        let rows: Vec<_> = builtin_recipes()
            .into_iter()
            .chain(self.recipes.iter().cloned())
            .collect();
        let selected = self.selected_preset_row();
        let selected_id = selected.as_ref().map(|row| row.id.clone());
        let modified = selected
            .as_ref()
            .is_some_and(|row| self.recipe_modified(row));
        let label = selected
            .as_ref()
            .map_or_else(|| "Custom settings".to_string(), |row| row.name.clone());
        let personal = self.selected_personal();
        let has_personal = personal.is_some();
        let can_update = modified && has_personal;
        let update_label = personal.as_ref().map_or_else(
            || "Save changes".to_string(),
            |row| format!("Save changes to {}", row.name),
        );
        let builtin_count = builtin_recipes().len();
        let chooser = cx.entity().downgrade();
        let actions = cx.entity().downgrade();
        let busy = self.converting;
        div()
            .when(
                active_preset(self.format, self.quality, self.max_edge).is_none(),
                |row| row.debug_selector(|| "custom-settings-active".into()),
            )
            .flex()
            .flex_col()
            .gap_1()
            .child(
                div()
                    .flex()
                    .items_center()
                    .justify_between()
                    .child(
                        div()
                            .id("preset-label")
                            .text_size(px(12.))
                            .text_color(cx.theme().muted_foreground)
                            .tooltip(|window, cx| {
                                Tooltip::new(
                                    "A preset saves format, quality, max size and AVIF speed. \
                                     The output folder and replace mode stay per run.",
                                )
                                .build(window, cx)
                            })
                            .child("Preset"),
                    )
                    .children(modified.then(|| {
                        div()
                            .debug_selector(|| "recipe-modified".into())
                            .text_size(px(11.))
                            .text_color(cx.theme().yellow)
                            .child("edited")
                    })),
            )
            .child(
                div()
                    .flex()
                    .items_center()
                    .gap_1()
                    .child(
                        div().flex_1().min_w_0().child(
                            Button::new("preset-chooser")
                                .debug_selector(|| "preset-chooser".into())
                                .small()
                                .outline()
                                .w_full()
                                .label(label)
                                .dropdown_caret(true)
                                .disabled(busy)
                                .dropdown_menu(move |mut menu, _, _| {
                                    for (index, row) in rows.iter().enumerate() {
                                        if index == builtin_count {
                                            menu = menu
                                                .separator()
                                                .item(PopupMenuItem::label("My presets"));
                                        }
                                        let audit = chooser.clone();
                                        let recipe = row.clone();
                                        menu = menu.item(
                                            PopupMenuItem::new(format!(
                                                "{} · {}",
                                                row.name,
                                                row.summary()
                                            ))
                                            .checked(selected_id.as_deref() == Some(&row.id))
                                            .on_click(move |_, window, cx| {
                                                if let Some(audit) = audit.upgrade() {
                                                    audit.update(cx, |audit, cx| {
                                                        audit.clear_custom_max_edge(window, cx);
                                                        audit.apply_recipe(
                                                            &recipe, &recipe.id, window, cx,
                                                        );
                                                    });
                                                }
                                            }),
                                        );
                                    }
                                    menu
                                }),
                        ),
                    )
                    .child(
                        Button::new("recipe-actions")
                            .debug_selector(|| "recipe-actions".into())
                            .small()
                            .outline()
                            .icon(IconName::Ellipsis)
                            .tooltip("Save, rename, delete, import or export presets")
                            .disabled(busy)
                            .dropdown_menu(move |menu, _, _| {
                                let update = actions.clone();
                                let save_as = actions.clone();
                                let rename = actions.clone();
                                let delete = actions.clone();
                                let import = actions.clone();
                                let export = actions.clone();
                                let with_dir = |audit: &gpui_kit::WeakEntity<Audit>,
                                                cx: &mut App,
                                                act: fn(&mut Audit, &Path, &mut Context<Audit>)| {
                                    if let Some(audit) = audit.upgrade() {
                                        audit.update(cx, |audit, cx| {
                                            if let Some(dir) = audit.recipe_dir_or_notify(cx) {
                                                act(audit, &dir, cx);
                                            }
                                        });
                                    }
                                };
                                menu.item(
                                    PopupMenuItem::new(update_label.clone())
                                        .disabled(!can_update)
                                        .on_click(move |_, _, cx| {
                                            with_dir(&update, cx, Audit::update_recipe)
                                        }),
                                )
                                .item(PopupMenuItem::new("Save as new preset…").on_click(
                                    move |_, window, cx| {
                                        if let Some(audit) = save_as.upgrade() {
                                            audit.update(cx, |audit, cx| {
                                                audit.open_recipe_prompt(
                                                    RecipePrompt::SaveAs,
                                                    window,
                                                    cx,
                                                )
                                            });
                                        }
                                    },
                                ))
                                .item(
                                    PopupMenuItem::new("Rename…")
                                        .disabled(!has_personal)
                                        .on_click(move |_, window, cx| {
                                            if let Some(audit) = rename.upgrade() {
                                                audit.update(cx, |audit, cx| {
                                                    audit.open_recipe_prompt(
                                                        RecipePrompt::Rename,
                                                        window,
                                                        cx,
                                                    )
                                                });
                                            }
                                        }),
                                )
                                .item(
                                    PopupMenuItem::new("Delete")
                                        .disabled(!has_personal)
                                        .on_click(move |_, _, cx| {
                                            with_dir(&delete, cx, Audit::delete_recipe)
                                        }),
                                )
                                .separator()
                                .item(PopupMenuItem::new("Import…").on_click(move |_, _, cx| {
                                    if let Some(audit) = import.upgrade() {
                                        audit.update(cx, |audit, cx| audit.import_recipe_file(cx));
                                    }
                                }))
                                .item(
                                    PopupMenuItem::new("Export…")
                                        .disabled(!has_personal)
                                        .on_click(move |_, _, cx| {
                                            if let Some(audit) = export.upgrade() {
                                                audit.update(cx, |audit, cx| {
                                                    audit.export_recipe_file(cx)
                                                });
                                            }
                                        }),
                                )
                            }),
                    ),
            )
            .children(
                self.recipe_prompt
                    .map(|prompt| self.recipe_prompt_row(prompt, cx)),
            )
    }

    /// The name box, inline under the row, with the one verb it serves and a
    /// way out. Save as starts blank; Rename starts from the current name.
    fn recipe_prompt_row(&self, prompt: RecipePrompt, cx: &Context<Self>) -> impl IntoElement {
        let busy = self.converting;
        div()
            .flex()
            .items_center()
            .gap_1()
            .child(
                div()
                    .flex_1()
                    .min_w_0()
                    .debug_selector(|| "recipe-name-input".into())
                    .child(Input::new(&self.recipe_name_input).small().disabled(busy)),
            )
            .child(
                Button::new("recipe-save")
                    .debug_selector(|| "recipe-save".into())
                    .small()
                    .outline()
                    .label(match prompt {
                        RecipePrompt::SaveAs => "Save",
                        RecipePrompt::Rename => "Rename",
                    })
                    .disabled(busy)
                    .on_click(cx.listener(|audit, _, window, cx| {
                        audit.confirm_recipe_prompt(window, cx);
                    })),
            )
            .child(
                Button::new("recipe-cancel")
                    .small()
                    .ghost()
                    .icon(IconName::Close)
                    .tooltip("Cancel")
                    .on_click(cx.listener(|audit, _, _, cx| {
                        audit.recipe_prompt = None;
                        cx.notify();
                    })),
            )
    }

    /// The bound target's display name from loaded recipes. A bound id with
    /// no recipe stays visible as missing: preparation refuses it by name.
    fn job_target_label(&self) -> String {
        match crate::job::resolve_target(&self.work_job, &self.recipes) {
            Ok(None) => "No target".to_string(),
            Ok(Some(recipe)) => recipe.name.clone(),
            Err(message) => message,
        }
    }

    /// Product sets over the current folder: which files belong to which
    /// product views, and whether each view is ready, stale, missing, or
    /// still unmapped. Rendered from cached states only; the filesystem work
    /// happens in actions and refreshes, never here.
    fn sets_section(&self, cx: &Context<Self>) -> impl IntoElement + use<> {
        let busy = self.converting;
        let mut products = Vec::with_capacity(self.work_job.products.len());
        for index in 0..self.work_job.products.len() {
            products.push(self.sets_product(index, cx));
        }
        let mut stale_rows = Vec::with_capacity(self.work_stale.len());
        for index in 0..self.work_stale.len() {
            stale_rows.push(self.sets_stale_row(index, cx));
        }
        let open = self.sets_open;
        let job = cx.entity().downgrade();
        // Folded by default: sets are a second job over the folder, and five
        // verbs plus three boxes sat under every conversion whether or not
        // anyone had a product to name.
        let header =
            div()
                .flex()
                .items_center()
                .justify_between()
                .child(
                    Button::new("sets-toggle")
                        .debug_selector(|| "sets-toggle".into())
                        .small()
                        .ghost()
                        .icon(if open {
                            IconName::ChevronDown
                        } else {
                            IconName::ChevronRight
                        })
                        .label("Product sets")
                        .on_click(cx.listener(|audit, _, _, cx| {
                            audit.sets_open = !audit.sets_open;
                            cx.notify();
                        })),
                )
                .child(
                    // The job's name is the job menu: the five verbs used to be
                    // two rows of ghost text under the heading.
                    div().debug_selector(|| "sets-job-name".into()).child(
                        Button::new("sets-job")
                            .small()
                            .ghost()
                            .label(self.work_job.name.clone())
                            .dropdown_caret(true)
                            .disabled(busy)
                            .dropdown_menu(move |menu, _, _| {
                                let new_job = job.clone();
                                let delete = job.clone();
                                let csv = job.clone();
                                let import = job.clone();
                                let export = job.clone();
                                let act = |audit: &gpui_kit::WeakEntity<Audit>,
                                       cx: &mut App,
                                       act: fn(&mut Audit, &mut Context<Audit>)| {
                                if let Some(audit) = audit.upgrade() {
                                    audit.update(cx, act);
                                }
                            };
                                menu.item(
                                    PopupMenuItem::new("New job").on_click(move |_, _, cx| {
                                        act(&new_job, cx, Audit::new_job)
                                    }),
                                )
                                .item(PopupMenuItem::new("Delete job").on_click(move |_, _, cx| {
                                    if let Some(audit) = delete.upgrade() {
                                        audit.update(cx, |audit, cx| {
                                            if let Some(dir) = audit.job_dir_or_notify(cx) {
                                                audit.delete_job(&dir, cx);
                                            }
                                        });
                                    }
                                }))
                                .separator()
                                .item(PopupMenuItem::new("Import CSV…").on_click(
                                    move |_, _, cx| act(&csv, cx, Audit::import_csv_file),
                                ))
                                .item(PopupMenuItem::new("Import job…").on_click(
                                    move |_, _, cx| act(&import, cx, Audit::import_job_file),
                                ))
                                .item(
                                    PopupMenuItem::new("Export job…").on_click(move |_, _, cx| {
                                        act(&export, cx, Audit::export_job_file)
                                    }),
                                )
                            }),
                    ),
                );
        div()
            .debug_selector(|| "sets-section".into())
            .flex()
            .flex_col()
            .flex_shrink_0()
            .gap_1()
            .child(header)
            .children(open.then(|| {
                div()
                    .flex()
                    .items_center()
                    .gap_1()
                    .child(
                        div()
                            .flex_1()
                            .min_w_0()
                            .whitespace_nowrap()
                            .overflow_hidden()
                            .text_ellipsis()
                            .text_size(px(11.))
                            .text_color(cx.theme().muted_foreground)
                            .debug_selector(|| "sets-target".into())
                            .child(format!("Target: {}", self.job_target_label())),
                    )
                    .child(
                        Button::new("sets-target-current")
                            .small()
                            .ghost()
                            .label("Use current")
                            .disabled(busy)
                            .on_click(cx.listener(|audit, _, _, cx| {
                                let Some(dir) = audit.job_dir_or_notify(cx) else {
                                    return;
                                };
                                audit.bind_current_recipe_as_target(&dir, cx);
                            })),
                    )
                    .child(
                        Button::new("sets-target-clear")
                            .small()
                            .ghost()
                            .label("Clear")
                            .disabled(busy || self.work_job.target_recipe.is_none())
                            .on_click(cx.listener(|audit, _, _, cx| {
                                let Some(dir) = audit.job_dir_or_notify(cx) else {
                                    return;
                                };
                                audit.clear_job_target(&dir, cx);
                            })),
                    )
            }))
            .children(open.then(|| {
                // Stacked: side by side, the two boxes cut their own
                // placeholders to "Product n" and "SKU hint (".
                div()
                    .flex()
                    .flex_col()
                    .gap_1()
                    .child(
                        div()
                            .debug_selector(|| "sets-product-name".into())
                            .child(Input::new(&self.product_name_input).small().disabled(busy)),
                    )
                    .child(
                        div()
                            .debug_selector(|| "sets-product-sku".into())
                            .child(Input::new(&self.product_sku_input).small().disabled(busy)),
                    )
                    .child(
                        Button::new("sets-add-product")
                            .small()
                            .outline()
                            .w_full()
                            .label("Add product")
                            .disabled(busy)
                            .on_click(cx.listener(|audit, _, _, cx| {
                                let Some(dir) = audit.job_dir_or_notify(cx) else {
                                    return;
                                };
                                audit.add_product(&dir, cx);
                            })),
                    )
            }))
            .children(open.then_some(products).into_iter().flatten())
            .children((open && !self.work_stale.is_empty()).then(|| {
                div()
                    .flex()
                    .flex_col()
                    .gap_1()
                    .child(
                        div()
                            .flex()
                            .items_center()
                            .justify_between()
                            .child(
                                div()
                                    .text_size(px(11.))
                                    .font_weight(FontWeight::SEMIBOLD)
                                    .text_color(cx.theme().yellow)
                                    .child("Outdated deliverables"),
                            )
                            .child(
                                Button::new("sets-select-stale")
                                    .small()
                                    .ghost()
                                    .label("Select stale")
                                    .disabled(busy)
                                    .on_click(cx.listener(|audit, _, _, cx| {
                                        audit.select_stale_sources(cx);
                                    })),
                            ),
                    )
                    .children(stale_rows)
            }))
            .into_any_element()
    }

    /// One product with its roles, mapped files and row operations. Missing
    /// files offer relink where they stand; every file offers removal.
    fn sets_product(&self, index: usize, cx: &Context<Self>) -> impl IntoElement + use<> {
        let Some(product) = self.work_job.products.get(index) else {
            return div().into_any_element();
        };
        let product_id = product.id.clone();
        let mut roles = Vec::with_capacity(product.roles.len());
        for role in &product.roles {
            roles.push(self.sets_role(&product_id, &role.id, cx));
        }
        div()
            .flex()
            .flex_col()
            .gap_1()
            .rounded_md()
            .border_1()
            .border_color(cx.theme().border)
            .p_2()
            .child(
                div()
                    .flex()
                    .items_center()
                    .justify_between()
                    .child(
                        div()
                            .text_size(px(12.))
                            .font_weight(FontWeight::SEMIBOLD)
                            .text_color(cx.theme().foreground)
                            .child(format!("{} · {}", product.name, product.sku_hint)),
                    )
                    .child(
                        Button::new(("sets-delete-product", index))
                            .small()
                            .ghost()
                            .label("Delete")
                            .disabled(self.converting)
                            .on_click(cx.listener(move |audit, _, _, cx| {
                                let Some(dir) = audit.job_dir_or_notify(cx) else {
                                    return;
                                };
                                audit.delete_product(&dir, &product_id, cx);
                            })),
                    ),
            )
            .children(roles)
            .child(
                div()
                    .flex()
                    .items_center()
                    .gap_1()
                    .child(
                        div()
                            .flex_1()
                            .min_w_0()
                            .debug_selector(|| "sets-role-name".into())
                            .child(
                                Input::new(&self.role_name_input)
                                    .small()
                                    .disabled(self.converting),
                            ),
                    )
                    .child(
                        Button::new(("sets-add-role", index))
                            .small()
                            .ghost()
                            .label("Add role")
                            .disabled(self.converting)
                            .on_click(cx.listener({
                                let product_id = product.id.clone();
                                move |audit, _, window, cx| {
                                    let Some(dir) = audit.job_dir_or_notify(cx) else {
                                        return;
                                    };
                                    audit.add_role(&dir, &product_id, window, cx);
                                }
                            })),
                    ),
            )
            .into_any_element()
    }

    /// One role with its status, its mapped files, and the map control for
    /// the current file selection.
    fn sets_role(
        &self,
        product_id: &str,
        role_id: &str,
        cx: &Context<Self>,
    ) -> impl IntoElement + use<> {
        let status = self
            .work_states
            .iter()
            .find(|state| state.product_id == product_id && state.role_id == role_id);
        let (label, required) = self
            .work_job
            .products
            .iter()
            .find(|product| product.id == product_id)
            .and_then(|product| product.roles.iter().find(|role| role.id == role_id))
            .map(|role| (role.label.clone(), role.required))
            .unwrap_or_default();
        let detail = match status.map(|state| state.status) {
            None | Some(crate::job::RoleStatus::Unmapped) => "unmapped".to_string(),
            Some(crate::job::RoleStatus::Ready) => {
                let count = status.map(|state| state.sources.len()).unwrap_or(0);
                format!("ready · {count}")
            }
            Some(crate::job::RoleStatus::Stale) => "stale".to_string(),
            Some(crate::job::RoleStatus::Missing) => "missing".to_string(),
        };
        let mut files = Vec::new();
        if let Some(state) = status {
            for source in &state.sources {
                files.push(self.sets_file(source, cx));
            }
        }
        let product_id = product_id.to_string();
        let role_id = role_id.to_string();
        div()
            .flex()
            .flex_col()
            .gap_1()
            .child(
                div()
                    .flex()
                    .items_center()
                    .justify_between()
                    .child(
                        div()
                            .flex()
                            .items_center()
                            .gap_1()
                            .child(
                                div()
                                    .text_size(px(12.))
                                    .text_color(cx.theme().foreground)
                                    .child(format!(
                                        "{label}{}",
                                        if required {
                                            " · required".to_string()
                                        } else {
                                            String::new()
                                        }
                                    )),
                            )
                            .child(
                                div()
                                    .text_size(px(11.))
                                    .text_color(cx.theme().muted_foreground)
                                    .child(detail),
                            ),
                    )
                    .child(
                        div()
                            .flex()
                            .gap_1()
                            .child(
                                Button::new(format!("sets-map-{product_id}-{role_id}"))
                                    .small()
                                    .ghost()
                                    .label("Map selected")
                                    .disabled(self.converting)
                                    .on_click(cx.listener({
                                        let product_id = product_id.clone();
                                        let role_id = role_id.clone();
                                        move |audit, _, _, cx| {
                                            let Some(dir) = audit.job_dir_or_notify(cx) else {
                                                return;
                                            };
                                            audit.map_selected(&dir, &product_id, &role_id, cx);
                                        }
                                    })),
                            )
                            .child(
                                Button::new(format!("sets-delete-role-{product_id}-{role_id}"))
                                    .small()
                                    .ghost()
                                    .label("Delete")
                                    .disabled(self.converting)
                                    .on_click(cx.listener({
                                        move |audit, _, _, cx| {
                                            let Some(dir) = audit.job_dir_or_notify(cx) else {
                                                return;
                                            };
                                            audit.delete_role(&dir, &product_id, &role_id, cx);
                                        }
                                    })),
                            ),
                    ),
            )
            .children(files)
            .into_any_element()
    }

    /// One mapped file: its name, its freshness, and the operations its state
    /// allows. Relink only shows on missing files, where it is the way back.
    fn sets_file(
        &self,
        source: &crate::job::MappedSource,
        cx: &Context<Self>,
    ) -> impl IntoElement + use<> {
        let name = source
            .path
            .file_name()
            .map(|name| name.to_string_lossy().into_owned())
            .unwrap_or_else(|| source.path.display().to_string());
        let state = match source.status {
            crate::job::SourceStatus::Fresh => "fresh",
            crate::job::SourceStatus::Stale => "stale",
            crate::job::SourceStatus::Missing => "missing",
        };
        let mapping_id = source.mapping_id.clone();
        let relink_id = mapping_id.clone();
        div()
            .flex()
            .items_center()
            .justify_between()
            .child(
                div()
                    .flex()
                    .items_center()
                    .gap_1()
                    .child(
                        div()
                            .text_size(px(12.))
                            .text_color(cx.theme().foreground)
                            .child(name),
                    )
                    .child(
                        div()
                            .text_size(px(11.))
                            .text_color(cx.theme().muted_foreground)
                            .child(state),
                    ),
            )
            .child(
                div()
                    .flex()
                    .gap_1()
                    .children(
                        (source.status == crate::job::SourceStatus::Missing).then(|| {
                            Button::new(format!("sets-relink-{relink_id}"))
                                .small()
                                .ghost()
                                .label("Relink")
                                .disabled(self.converting)
                                .on_click(cx.listener(move |audit, _, _, cx| {
                                    audit.relink_mapping(&relink_id, cx);
                                }))
                        }),
                    )
                    .child(
                        Button::new(format!("sets-unmap-{mapping_id}"))
                            .small()
                            .ghost()
                            .label("Remove")
                            .disabled(self.converting)
                            .on_click(cx.listener(move |audit, _, _, cx| {
                                let Some(dir) = audit.job_dir_or_notify(cx) else {
                                    return;
                                };
                                audit.unmap(&dir, &mapping_id, cx);
                            })),
                    ),
            )
            .into_any_element()
    }

    /// One stale deliverable: which output stopped matching and why.
    fn sets_stale_row(&self, index: usize, cx: &Context<Self>) -> impl IntoElement + use<> {
        let Some(stale) = self.work_stale.get(index) else {
            return div().into_any_element();
        };
        let name = stale
            .output
            .file_name()
            .map(|name| name.to_string_lossy().into_owned())
            .unwrap_or_else(|| stale.output.display().to_string());
        let reason = match stale.reason {
            crate::job::StaleReason::SourceChanged => "source changed",
            crate::job::StaleReason::OutputGone => "output changed",
        };
        div()
            .flex()
            .items_center()
            .gap_1()
            .child(
                div()
                    .text_size(px(12.))
                    .text_color(cx.theme().foreground)
                    .child(name),
            )
            .child(
                div()
                    .text_size(px(11.))
                    .text_color(cx.theme().yellow)
                    .child(reason),
            )
            .into_any_element()
    }

    /// The quality knob: label and value on one line, the slider full-width
    /// under them, Lossless under that. While lossless is on, the slider is
    /// replaced by the word — a live slider would answer a stray drag and
    /// silently leave lossless.
    fn panel_quality(&self, cx: &mut Context<Self>) -> impl IntoElement {
        let lossless = self.quality == Quality::LOSSLESS;
        div()
            .flex()
            .flex_col()
            .gap_1()
            .child(
                div()
                    .flex()
                    .items_center()
                    .justify_between()
                    .child(
                        div()
                            .text_size(px(12.))
                            .text_color(cx.theme().muted_foreground)
                            .child("Quality"),
                    )
                    .child(
                        div()
                            .font_family(cx.theme().mono_font_family.clone())
                            .text_size(px(12.))
                            .text_color(if lossless {
                                cx.theme().muted_foreground
                            } else {
                                cx.theme().foreground
                            })
                            .child(match self.quality.0 {
                                Some(value) => format!("{}", value.round() as u32),
                                None => "lossless".to_string(),
                            }),
                    ),
            )
            .child(
                div()
                    .debug_selector(|| "quality-control".to_string())
                    .when(lossless, |rail| rail.h(px(0.)))
                    .when(!lossless && self.converting, |rail| {
                        rail.child(
                            Progress::new("quality-locked")
                                .value(self.quality.0.unwrap_or(100.))
                                .color(cx.theme().primary)
                                .h(px(6.)),
                        )
                    })
                    .when(!lossless && !self.converting, |rail| {
                        rail.child(Slider::new(&self.quality_slider).horizontal())
                    }),
            )
            // AVIF is lossy-only here, and a switch that lies about that is worse
            // than no switch.
            .children(self.format.supports_lossless().then(|| {
                div()
                    .flex()
                    .items_center()
                    .justify_between()
                    .h_6()
                    .text_size(px(13.))
                    .child("Lossless")
                    .child(
                        Switch::new("lossless")
                            .small()
                            .checked(lossless)
                            .accessibility_label("Lossless")
                            .disabled(self.converting)
                            .on_click(cx.listener(|audit, _, _, cx| {
                                if audit.converting {
                                    return;
                                }
                                // A second click on a lit toggle has to turn it off, or
                                // lossless is a one-way door.
                                audit.quality = if audit.quality == Quality::LOSSLESS {
                                    Quality::lossy(audit.slider_quality)
                                } else {
                                    Quality::LOSSLESS
                                };
                                audit.clear_results();
                                audit.schedule_estimate(cx);
                                cx.notify();
                            })),
                    )
            }))
            // WebP saves transparency lossless no matter what the slider says.
            // The caption owns that fact next to the knob, so the number above
            // never implies control it does not have over see-through pixels.
            .children((self.format == Format::WebP && !lossless).then(|| {
                div()
                    .debug_selector(|| "quality-transparency-note".into())
                    .text_size(px(11.))
                    .text_color(cx.theme().muted_foreground)
                    .child("Transparency stays lossless")
            }))
    }

    /// The payoff and the commit, immediately after the knobs that determine it:
    /// what the folder costs now, what it would cost converted, and the button.
    fn output_summary(&self, cx: &mut Context<Self>) -> impl IntoElement {
        let target_count = self.target_count();
        // Source bytes only appear before a conversion. While results stream
        // in, avoid walking thousands of rows on every progress redraw.
        let source = if !self.converting && self.results.is_empty() {
            self.target_bytes()
        } else {
            0
        };

        // Four states, one shape: a headline, its tone, a sentence of detail,
        // the share remaining, and the percent tag.
        let (state, headline, tone, detail, bar, tag) = if self.converting {
            let done = self.results.len() + self.failures.len();
            let total = self.active_target_count.unwrap_or(target_count);
            (
                Some(("CONVERTING".to_string(), cx.theme().foreground)),
                format!("{done} of {total}"),
                cx.theme().foreground,
                format!(
                    "Converting to {} {}…",
                    self.format.display(),
                    self.quality.label()
                ),
                Some((1. - done as f32 / total.max(1) as f32, cx.theme().primary)),
                None,
            )
        } else if !self.results.is_empty() {
            let (before, after) = self.converted_totals();
            let growth = after > before;
            let delta = before.abs_diff(after);
            let percent = delta as f32 / before.max(1) as f32 * 100.;
            (
                Some((
                    conversion_result_state(self.stopped_run, self.results.len()),
                    if self.stopped_run.is_some() {
                        cx.theme().muted_foreground
                    } else {
                        cx.theme().green
                    },
                )),
                format!(
                    "{} {}",
                    format_bytes(delta),
                    if growth { "larger" } else { "saved" }
                ),
                if growth {
                    cx.theme().yellow
                } else {
                    cx.theme().green
                },
                format!(
                    "{} files · {} → {}",
                    self.results.len(),
                    format_bytes(before),
                    format_bytes(after)
                ),
                Some((
                    after as f32 / before.max(1) as f32,
                    if growth {
                        cx.theme().yellow
                    } else {
                        cx.theme().green
                    },
                )),
                Some((growth, percent)),
            )
        } else if let Some((projected, sampled, refused)) = self.estimate {
            let growth = projected > source;
            let delta = source.abs_diff(projected);
            let percent = delta as f32 / source.max(1) as f32 * 100.;
            (
                Some(("ESTIMATE".to_string(), cx.theme().muted_foreground)),
                format!("≈{} output", format_bytes(projected)),
                if growth {
                    cx.theme().yellow
                } else {
                    cx.theme().green
                },
                // The preset by name at the commit moment, so what will run
                // is said where the number is, not only up in the card.
                format!(
                    "from {}{}{} · {}",
                    format_bytes(source),
                    sampling_note(sampled, target_count),
                    if refused > 0 {
                        format!(" · {refused} refused")
                    } else {
                        String::new()
                    },
                    self.preset_label(),
                ),
                if sampled == 0 {
                    None
                } else {
                    Some((
                        projected as f32 / source.max(1) as f32,
                        if growth {
                            cx.theme().yellow
                        } else {
                            cx.theme().green
                        },
                    ))
                },
                // Everything refused means no ratio to brag about: the detail
                // line already names the refusals, and a −100% tag would claim
                // savings a run will never deliver.
                if sampled == 0 {
                    None
                } else {
                    Some((growth, percent))
                },
            )
        } else {
            (
                None,
                "Sizing it up…".to_string(),
                cx.theme().muted_foreground,
                format!("{} on disk", format_bytes(source)),
                None,
                None,
            )
        };

        let (fraction, colour) = bar
            .map(|(remaining, colour)| (1. - remaining, colour))
            .unwrap_or((0., gpui_kit::transparent_black()));

        div()
            .flex()
            .flex_col()
            .gap_1()
            .when(target_count > 0, |summary| {
                summary
                    .children(state.map(|(label, colour)| {
                        div()
                            .debug_selector(|| "output-state".into())
                            .text_size(px(10.))
                            .font_weight(FontWeight::SEMIBOLD)
                            .text_color(colour)
                            .child(label)
                    }))
                    .child(meter("saving", fraction, colour, 3.))
                    .child(
                        div()
                            .flex()
                            .items_center()
                            .gap_2()
                            .child(
                                div()
                                    .font_family("SF Pro Display")
                                    .text_size(px(16.))
                                    .font_weight(FontWeight::SEMIBOLD)
                                    .text_color(tone)
                                    .whitespace_nowrap()
                                    .child(headline),
                            )
                            .children(tag.map(|(growth, percent)| {
                                let tag = if growth {
                                    Tag::warning()
                                } else {
                                    Tag::success()
                                };
                                tag.small().child(if growth {
                                    format!("+{percent:.0}%")
                                } else {
                                    format!("−{percent:.0}%")
                                })
                            }))
                            // A run disables every other control in the window,
                            // by design, so the way out of it has to sit beside
                            // the count that says how long it has left.
                            .children(self.converting.then(|| {
                                let stopping = self.convert_stopping();
                                div().debug_selector(|| "convert-stop".into()).child(
                                    Button::new("convert-stop")
                                        .small()
                                        .outline()
                                        .label(if stopping { "Stopping…" } else { "Stop" })
                                        .disabled(stopping)
                                        .on_click(cx.listener(|audit, _, _, cx| {
                                            audit.cancel_conversion(cx)
                                        })),
                                )
                            })),
                    )
                    .child(
                        div()
                            .font_family(cx.theme().mono_font_family.clone())
                            .text_size(px(11.))
                            .text_color(cx.theme().muted_foreground)
                            .child(detail),
                    )
            })
            .when(!self.results.is_empty() && !self.converting, |block| {
                block
                    .children((self.published_results.is_empty()).then(|| {
                        // Element ids never reach `debug_bounds`: the wrapper
                        // is the selector contract the tests assert on.
                        let waiting = self.publish_waiting();
                        div().debug_selector(|| "conversion-publish".into()).child(
                            Button::new("conversion-publish")
                                .outline()
                                .small()
                                .w_full()
                                .icon(IconName::ArrowUp)
                                .label(if self.sirv_pairing.is_some() {
                                    "Publish to Sirv"
                                } else {
                                    "Connect & publish"
                                })
                                .tooltip(waiting.clone().unwrap_or_else(|| {
                                    "Upload the converted files to optimized/ on Sirv".into()
                                }))
                                .disabled(
                                    self.scan_blocks_delivery()
                                        || self.sirv_busy()
                                        || waiting.is_some(),
                                )
                                .on_click(cx.listener(|audit, _, _, cx| audit.publish_results(cx))),
                        )
                    }))
                    .children((!self.published_results.is_empty()).then(|| {
                        div()
                            .debug_selector(|| "conversion-copy-embed".into())
                            .child(
                                Button::new("conversion-copy-embed")
                                    .outline()
                                    .small()
                                    .w_full()
                                    .icon(IconName::Copy)
                                    .label("Copy embed")
                                    .tooltip("Copy responsive Sirv image markup")
                                    .on_click(
                                        cx.listener(|audit, _, _, cx| audit.copy_result_embeds(cx)),
                                    ),
                            )
                    }))
                    .child(
                        Button::new("reveal")
                            .outline()
                            .small()
                            .w_full()
                            .icon(IconName::FolderOpen)
                            .label("Show output")
                            .on_click(cx.listener(|audit, _, _, cx| audit.reveal_output(cx))),
                    )
            })
            .when(
                (self.results.is_empty()
                    && (!self.completed_outputs.is_empty() || self.existing_output > 0))
                    && !self.converting,
                |block| {
                    block.child(
                        Button::new("reveal")
                            .outline()
                            .small()
                            .w_full()
                            .icon(IconName::FolderOpen)
                            .label("Show output")
                            .on_click(cx.listener(|audit, _, _, cx| audit.reveal_output(cx))),
                    )
                },
            )
            // Offered whenever the folder's run record still holds an original,
            // including on a later launch: the undo is a fact on disk, not a
            // memory of this session.
            .when(self.restorable > 0 && !self.converting, |block| {
                block.child(
                    div().debug_selector(|| "restore-originals".into()).child(
                        Button::new("restore")
                            .outline()
                            .small()
                            .w_full()
                            .label("Restore originals")
                            .tooltip(format!(
                                "Move {} original{} back out of {}/ and remove what replaced them",
                                self.restorable,
                                if self.restorable == 1 { "" } else { "s" },
                                crate::scan::BACKUP_DIR
                            ))
                            .on_click(cx.listener(|audit, _, _, cx| audit.restore_originals(cx))),
                    ),
                )
            })
            .child(
                Button::new("convert")
                    .primary()
                    .w_full()
                    .when(self.converting, |button| button.outline())
                    .when(
                        target_count == 0 || self.keep_format_overwrites_sources(),
                        |button| button.ghost(),
                    )
                    .label(self.conversion_action_label())
                    .disabled(
                        self.converting
                            || self.scanning.is_some()
                            || target_count == 0
                            || self.keep_format_overwrites_sources(),
                    )
                    .on_click(cx.listener(|audit, _, _, cx| audit.start_conversion(cx))),
            )
    }
}
