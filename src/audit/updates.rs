//! User-controlled updates. Only the check runs at launch.

use super::*;
use crate::update;
use cargo_packager_updater::Update;

#[derive(Default)]
pub(super) enum State {
    #[default]
    Idle,
    Checking,
    Available(Box<Update>),
    Downloading,
    Ready(Box<Update>, Vec<u8>),
    Applying,
    Installed,
}

#[derive(Default)]
pub(super) struct Updater {
    pub state: State,
    pub(super) dismissed: bool,
    pub(super) message: String,
}

impl Audit {
    pub(crate) fn check_for_updates(&mut self, explicit: bool, cx: &mut Context<Self>) {
        self.updater.dismissed = false;
        if !matches!(self.updater.state, State::Idle) {
            cx.notify();
            return;
        }
        self.updater.state = State::Checking;
        self.updater.message = "Checking for updates…".into();
        // Startup checks stay quiet until there is news or an error.
        self.updater.dismissed = !explicit;
        cx.notify();
        cx.spawn(async move |this, cx| {
            let result = cx.background_executor().spawn(async { update::check() }).await;
            let _ = this.update(cx, |audit, cx| {
                audit.updater.state = State::Idle;
                audit.updater.dismissed = false;
                match result {
                    Ok(update::Check::Available(update)) => {
                        audit.updater.message = format!("Press {} is available", update.version);
                        audit.updater.state = State::Available(update);
                    }
                    Ok(update::Check::Current) => {
                        audit.updater.message = format!("Press {} is up to date", env!("CARGO_PKG_VERSION"));
                        audit.updater.dismissed = !explicit;
                    }
                    Ok(update::Check::Unsupported) => {
                        audit.updater.message = "Update this installation with its package manager or download a new installer.".into();
                        audit.updater.dismissed = !explicit;
                    }
                    Err(error) => audit.updater.message = format!("Couldn’t check for updates: {error}"),
                }
                cx.notify();
            });
        }).detach();
    }

    pub(super) fn download_update(&mut self, cx: &mut Context<Self>) {
        if !matches!(self.updater.state, State::Available(_)) {
            return;
        }
        let State::Available(update) =
            std::mem::replace(&mut self.updater.state, State::Downloading)
        else {
            return;
        };
        self.updater.message = format!("Downloading Press {}…", update.version);
        cx.notify();
        cx.spawn(async move |this, cx| {
            let (update, result) = cx
                .background_executor()
                .spawn(async move {
                    let result = update.download();
                    (update, result)
                })
                .await;
            let _ = this.update(cx, |audit, cx| {
                match result {
                    Ok(bytes) => {
                        audit.updater.message =
                            format!("Press {} is ready to apply", update.version);
                        audit.updater.state = State::Ready(update, bytes);
                    }
                    Err(error) => {
                        audit.updater.message =
                            format!("Couldn’t download Press {}: {error}", update.version);
                        audit.updater.state = State::Available(update);
                    }
                }
                cx.notify();
            });
        })
        .detach();
    }

    pub(super) fn apply_update(&mut self, cx: &mut Context<Self>) {
        if !matches!(self.updater.state, State::Ready(..) | State::Installed) {
            return;
        }
        if !self.update_can_restart() {
            self.updater.message =
                "Update ready. Finish the current work before applying and restarting.".into();
            cx.notify();
            return;
        }
        if let settings::WriteOutcome::Failed { error, .. } = self.flush_settings() {
            self.updater.message = format!("Couldn’t save settings before restarting: {error}");
            cx.notify();
            return;
        }
        if matches!(self.updater.state, State::Installed) {
            self.restart_after_update(cx);
            return;
        }
        let State::Ready(update, bytes) =
            std::mem::replace(&mut self.updater.state, State::Applying)
        else {
            return;
        };
        self.updater.message = format!("Applying Press {}…", update.version);
        cx.notify();
        cx.spawn(async move |this, cx| {
            let (update, result) = cx
                .background_executor()
                .spawn(async move {
                    let result = update.install(bytes);
                    (update, result)
                })
                .await;
            let _ = this.update(cx, |audit, cx| {
                match result {
                    Ok(()) => {
                        audit.updater.state = State::Installed;
                        audit.restart_after_update(cx);
                    }
                    Err(error) => {
                        audit.updater.message = format!(
                            "Couldn’t apply Press {}: {error}. Download it again to retry.",
                            update.version
                        );
                        audit.updater.state = State::Available(update);
                    }
                }
                cx.notify();
            });
        })
        .detach();
    }

    fn restart_after_update(&mut self, cx: &mut Context<Self>) {
        match update::relaunch() {
            Ok(()) => cx.quit(),
            Err(error) => {
                self.updater.message = format!("Update installed, but couldn’t restart: {error}");
                cx.notify();
            }
        }
    }

    pub(crate) fn update_banner(&mut self, cx: &mut Context<Self>) -> Option<gpui_kit::AnyElement> {
        if self.updater.dismissed || self.updater.message.is_empty() {
            return None;
        }
        let action = match self.updater.state {
            State::Available(_) => Some("Download update"),
            State::Ready(..) => Some("Apply and restart"),
            State::Installed => Some("Restart Press"),
            State::Idle => Some("Check again"),
            _ => None,
        };
        Some(
            div()
                .debug_selector(|| "update-banner".into())
                .flex()
                .items_center()
                .gap_3()
                .px_3()
                .py_2()
                .flex_shrink_0()
                .bg(cx.theme().secondary)
                .text_color(cx.theme().foreground)
                .border_b_1()
                .border_color(cx.theme().border)
                .child(
                    div()
                        .flex_1()
                        .min_w_0()
                        .text_sm()
                        .child(self.updater.message.clone()),
                )
                .children(action.map(|label| {
                    div().debug_selector(|| "update-action".into()).child(
                        Button::new("update-action")
                            .small()
                            .outline()
                            .label(label)
                            .on_click(cx.listener(|audit, _, _, cx| match audit.updater.state {
                                State::Available(_) => audit.download_update(cx),
                                State::Ready(..) | State::Installed => audit.apply_update(cx),
                                State::Idle => audit.check_for_updates(true, cx),
                                _ => {}
                            })),
                    )
                }))
                .when(!self.update_is_applying(), |bar| {
                    bar.child(
                        div().debug_selector(|| "dismiss-update".into()).child(
                            Button::new("dismiss-update")
                                .small()
                                .ghost()
                                .icon(IconName::Close)
                                .tooltip("Dismiss update notice")
                                .on_click(cx.listener(|audit, _, _, cx| {
                                    audit.updater.dismissed = true;
                                    cx.notify();
                                })),
                        ),
                    )
                })
                .into_any_element(),
        )
    }
}
