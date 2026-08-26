//! The Sirv settings panel and status line.

use super::*;

impl Audit {
    /// One labelled row of the settings form.
    pub(super) fn settings_row(
        label: &'static str,
        input: gpui::Entity<InputState>,
        secret: bool,
        cx: &Context<Self>,
    ) -> gpui::Div {
        div()
            .flex()
            .items_center()
            .gap_2()
            .child(
                div()
                    .w(px(110.))
                    .flex_shrink_0()
                    .text_size(px(12.))
                    .text_color(cx.theme().muted_foreground)
                    .child(label),
            )
            .child(Input::new(&input).small().when(secret, |field| {
                field.content_type(InputContentType::Password).mask_toggle()
            }))
    }

    /// A section heading plus its status line, if one has anything to say.
    pub(super) fn settings_status(status: Option<(bool, String)>, cx: &Context<Self>) -> gpui::Div {
        match status {
            None => div(),
            Some((ok, message)) => div()
                .text_size(px(11.))
                .text_color(if ok { cx.theme().green } else { cx.theme().red })
                .child(message),
        }
    }

    /// The settings panel: the CDN keys.
    pub(super) fn settings_panel_view(&self, cx: &mut Context<Self>) -> gpui::AnyElement {
        let Some(panel) = self.settings_panel.as_ref() else {
            return div().into_any_element();
        };
        let can_save = credentials_complete(
            &panel.client_id.read(cx).value(),
            &panel.client_secret.read(cx).value(),
        );

        div()
            .w(px(480.))
            .flex()
            .flex_col()
            .gap_3()
            .p_5()
            .rounded_lg()
            .bg(cx.theme().secondary)
            .border_1()
            .border_color(cx.theme().border)
            .child(
                div()
                    .flex()
                    .flex_col()
                    .gap_1()
                    .child(
                        div()
                            .font_family("SF Pro Display")
                            .text_size(px(15.))
                            .font_weight(FontWeight::SEMIBOLD)
                            .text_color(cx.theme().foreground)
                            .child("Sirv account"),
                    )
                    .child(
                        div()
                            .text_size(px(12.))
                            .text_color(cx.theme().foreground)
                            .child(
                                "Used only for Sirv sync. Both fields are required. Credentials \
                                 stay on this computer.",
                            ),
                    ),
            )
            .child(Self::settings_row(
                "Client ID",
                panel.client_id.clone(),
                false,
                cx,
            ))
            .child(Self::settings_row(
                "Client secret",
                panel.client_secret.clone(),
                true,
                cx,
            ))
            .child(
                div()
                    .flex()
                    .items_center()
                    .justify_between()
                    .child(Self::settings_status(panel.cdn_status.clone(), cx))
                    .child(
                        div()
                            .flex()
                            .gap_2()
                            .child(
                                Button::new("settings-close")
                                    .ghost()
                                    .small()
                                    .label("Close")
                                    .on_click(cx.listener(|audit, _, window, cx| {
                                        audit.close_settings(window, cx);
                                    })),
                            )
                            .child(
                                Button::new("settings-save")
                                    .primary()
                                    .small()
                                    .label("Save credentials")
                                    .disabled(!can_save)
                                    .on_click(
                                        cx.listener(|audit, _, _, cx| audit.save_sirv_settings(cx)),
                                    ),
                            ),
                    ),
            )
            .into_any_element()
    }
}
