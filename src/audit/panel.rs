//! The action bar and its rails.
//!
//! The bar floats over the list and holds verbs only: Convert, the two local
//! models, and hosted AI operations. Choosing one opens its rail on the
//! right, carrying that operation's settings and the button that commits it.
//! No operation borrows another's controls, and the bar never has to explain
//! itself — which is what the old right-hand inspector column was doing for
//! conversion alone.

use super::*;

/// Width of an open rail. The table and the gallery lay themselves out against
/// the viewport minus this, so a rail never silently squeezes their column
/// math. A closed rail takes nothing.
pub(super) const RAIL_WIDTH: f32 = 300.;

/// Room the list leaves under itself for the floating bar. Without it the bar
/// covers the last row and no amount of scrolling reveals it.
pub(super) const BAR_CLEARANCE: f32 = 64.;

/// Below this much room, the three secondary verbs drop to icons. The bar has
/// to fit the list it floats over, and at the minimum window with a rail open
/// there are 460px to fit into.
const BAR_LABELS_WIDTH: f32 = 720.;
/// Below this, the readout goes too: the verbs are what the bar is for.
const BAR_READOUT_WIDTH: f32 = 560.;

/// The named outputs most runs want. Listed once; the rows and the settings
/// they apply cannot disagree.
const PRESETS: [(&str, &str, Format, Quality, MaxEdge); 3] = [
    (
        "Recommended",
        "WebP · quality 80 · original size",
        Format::WebP,
        Quality(Some(80.)),
        MaxEdge::FULL,
    ),
    (
        "Small files",
        "AVIF · quality 60 · max 2400px",
        Format::Avif,
        Quality(Some(60.)),
        MaxEdge(Some(2400)),
    ),
    (
        "Pixel-perfect",
        "WebP · lossless · original size",
        Format::WebP,
        Quality::LOSSLESS,
        MaxEdge::FULL,
    ),
];

pub(super) fn active_preset(format: Format, quality: Quality, edge: MaxEdge) -> Option<usize> {
    PRESETS
        .iter()
        .position(|(_, _, preset_format, preset_quality, preset_edge)| {
            format == *preset_format && quality == *preset_quality && edge == *preset_edge
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
        let (projected, _) = self.estimate?;
        let source = self.selected_target_bytes;
        if source == 0 {
            return None;
        }
        let growth = projected > source;
        let percent = source.abs_diff(projected) as f32 / source as f32 * 100.;
        Some(format!(
            "· ≈{} output, {:.0}% {}",
            format_bytes(projected),
            percent,
            if growth { "larger" } else { "saved" },
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

    /// One local model as a verb. Both need exactly one image, and both say why
    /// when they cannot run rather than sitting there grey and mute.
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
        let blocked = match single {
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
    }

    /// The open rail. Header, the operation's own settings, and its commit at
    /// the foot — the same shape whichever operation it belongs to.
    pub(super) fn rail_view(&self, cx: &mut Context<Self>) -> Option<gpui_kit::AnyElement> {
        if self.rail == Rail::None {
            return None;
        }
        let title = self.rail.title();
        let body = match self.rail {
            Rail::Convert => self.convert_rail(cx).into_any_element(),
            Rail::RemoveBackground | Rail::Upscale => self.local_ai_rail(self.rail, cx),
            Rail::Studio => self.studio_rail(cx),
            Rail::None => return None,
        };
        Some(
            div()
                .debug_selector(|| "rail".into())
                .w(px(RAIL_WIDTH))
                .flex_shrink_0()
                .flex()
                .flex_col()
                .border_l_1()
                .border_color(cx.theme().border)
                .bg(cx.theme().secondary)
                .child(
                    div()
                        .flex()
                        .items_center()
                        .justify_between()
                        .flex_shrink_0()
                        .h(px(42.))
                        .pl_3()
                        .pr_1()
                        .border_b_1()
                        .border_color(cx.theme().border)
                        .child(
                            div()
                                .font_family("SF Pro Display")
                                .text_size(px(14.))
                                .font_weight(FontWeight::SEMIBOLD)
                                .text_color(cx.theme().foreground)
                                .child(title),
                        )
                        .child(
                            // Closing the rail mid-run would take Stop away with
                            // it, and the tab that reopens the rail is disabled
                            // while converting: there would be no way back.
                            div().debug_selector(|| "close-rail".into()).child(
                                Button::new("close-rail")
                                    .small()
                                    .ghost()
                                    .icon(IconName::Close)
                                    .tooltip("Close")
                                    .disabled(self.converting)
                                    .on_click(cx.listener(|audit, _, _, cx| {
                                        audit.rail = Rail::None;
                                        cx.notify();
                                    })),
                            ),
                        ),
                )
                .child(body)
                .into_any_element(),
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
                    .flex()
                    .flex_col()
                    .flex_1()
                    .min_h_0()
                    .overflow_y_scroll()
                    .gap_3()
                    .px_3()
                    .py_3()
                    // Where the output lands, said once and always. Every other
                    // number in this rail is about size; this one is about the
                    // question people actually ask before pressing Convert.
                    .child(self.destination_row(cx))
                    .child(
                        div()
                            .flex()
                            .flex_col()
                            .gap_1()
                            .child(self.preset_row(0, cx))
                            .child(self.preset_row(1, cx))
                            .child(self.preset_row(2, cx)),
                    )
                    .child(div().h(px(1.)).bg(cx.theme().border))
                    .child(
                        div()
                            .flex()
                            .items_center()
                            .justify_between()
                            .child(
                                div()
                                    .text_size(px(10.5))
                                    .font_weight(FontWeight::SEMIBOLD)
                                    .text_color(cx.theme().muted_foreground)
                                    .child("FINE-TUNE"),
                            )
                            // No preset is lit because the settings are yours.
                            // Without this the three unlit rows read as a bug.
                            .when(
                                active_preset(self.format, self.quality, self.max_edge).is_none(),
                                |heading| {
                                    heading.child(
                                        div()
                                            .debug_selector(|| "custom-settings-active".into())
                                            .text_size(px(10.5))
                                            .font_weight(FontWeight::SEMIBOLD)
                                            .text_color(cx.theme().blue)
                                            .child("CUSTOM"),
                                    )
                                },
                            ),
                    )
                    .child(
                        div()
                            .flex()
                            .flex_col()
                            .gap_2()
                            .child(self.panel_setting(
                                "Format",
                                self.format_group(cx).small().compact(),
                                cx,
                            ))
                            .child(self.panel_quality(cx))
                            .child(self.panel_setting("Max size", self.resize_control(cx), cx)),
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
        let custom = self.output != Output::Optimized;
        let replacing = self.output == Output::Replace;
        div()
            .debug_selector(|| "output-destination".into())
            .flex()
            .flex_col()
            .gap_0p5()
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
                        Button::new("output-replace")
                            .xsmall()
                            .ghost()
                            .label("Replace")
                            .tooltip("Convert in place, keeping every original in press-originals/")
                            .disabled(self.converting)
                            .on_click(cx.listener(|audit, _, _, cx| audit.use_replace_output(cx)))
                    }))
                    .child(
                        Button::new("output-choose")
                            .xsmall()
                            .ghost()
                            .label("Change")
                            .tooltip("Choose the folder converted files are written to")
                            .disabled(self.converting)
                            .on_click(cx.listener(|audit, _, _, cx| audit.pick_output(cx))),
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

    /// One preset as a full-width selectable row: the name, and under it the
    /// exact settings it applies — no memory required. Lit only while the live
    /// settings are exactly what it names, so a hand-tuned output lights none.
    fn preset_row(&self, index: usize, cx: &mut Context<Self>) -> impl IntoElement {
        let (name, summary, _, _, _) = PRESETS[index];
        let selected = active_preset(self.format, self.quality, self.max_edge) == Some(index);
        div()
            .id(("preset", index))
            .flex()
            .flex_col()
            .px_2()
            .py_1()
            .rounded_md()
            .cursor_pointer()
            .when(selected, |row| {
                row.bg(cx.theme().list_active)
                    .border_1()
                    .border_color(cx.theme().list_active_border)
            })
            .when(!selected, |row| {
                row.border_1()
                    .border_color(gpui_kit::transparent_black())
                    .hover(|row| row.bg(cx.theme().list_hover))
            })
            .child(
                div()
                    .text_size(px(13.))
                    .text_color(cx.theme().foreground)
                    .child(name),
            )
            .child(
                div()
                    .text_size(px(11.))
                    .text_color(cx.theme().muted_foreground)
                    .child(summary),
            )
            .on_click(cx.listener(move |audit, _, window, cx| {
                if audit.converting {
                    return;
                }
                let (_, _, format, quality, edge) = PRESETS[index];
                audit.format = format;
                audit.quality = quality;
                audit.max_edge = edge;
                audit.clear_custom_max_edge(window, cx);
                if let Some(value) = quality.0 {
                    // Keep the slider where the preset put things, or the knob
                    // below would contradict the number in the estimate.
                    audit.slider_quality = value;
                    audit
                        .quality_slider
                        .update(cx, |slider, cx| slider.set_value(value, window, cx));
                }
                audit.clear_results();
                audit.schedule_estimate(cx);
                cx.notify();
            }))
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
                Switch::new("lossless")
                    .checked(lossless)
                    .label("Lossless")
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
                    }))
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
        } else if let Some((projected, sampled)) = self.estimate {
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
                format!(
                    "from {}{}",
                    format_bytes(source),
                    sampling_note(sampled, target_count),
                ),
                Some((
                    projected as f32 / source.max(1) as f32,
                    if growth {
                        cx.theme().yellow
                    } else {
                        cx.theme().green
                    },
                )),
                Some((growth, percent)),
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
            .when(
                (!self.results.is_empty()
                    || !self.completed_outputs.is_empty()
                    || self.existing_output > 0)
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
