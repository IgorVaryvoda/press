//! The macOS menu bar. Without one, AppKit has no key equivalent for Quit,
//! Close, Hide or Minimize, and the app cannot be quit from the keyboard.

use std::io;

use gpui::{App, Entity, KeyBinding, Menu, MenuItem, actions};

use crate::audit::Audit;

actions!(
    press,
    [
        Quit,
        HideSelf,
        HideOthers,
        ShowAll,
        CloseWindow,
        Minimize,
        Zoom,
        OpenFolder,
        OpenImage,
        AboutPress,
        ShowCrashReports,
        EmailCrashReport
    ]
);

fn about_copy() -> String {
    format!(
        "Press {}\n\nPress is a local ecommerce-image preflight tool. Ordinary audits and conversions stay on this computer.",
        env!("CARGO_PKG_VERSION")
    )
}

fn present_about_press(cx: &mut App) {
    cx.background_executor()
        .spawn(async move {
            rfd::AsyncMessageDialog::new()
                .set_title("About Press")
                .set_description(about_copy())
                .set_buttons(rfd::MessageButtons::Ok)
                .show()
                .await;
        })
        .detach();
}

fn register_about_press(presenter: impl Fn(&mut App) + 'static, cx: &mut App) {
    cx.on_action(move |_: &AboutPress, cx| presenter(cx));
}

fn register_show_crash_reports(
    handler: impl Fn() -> io::Result<()> + 'static,
    reporter: impl Fn(&'static str, io::Result<()>, &mut App) + 'static,
    cx: &mut App,
) {
    cx.on_action(move |_: &ShowCrashReports, cx| {
        reporter("Show Crash Reports", handler(), cx);
    });
}

/// The reporter sees every outcome, not only the failures: a later success has to
/// clear the toast the last failure left on screen.
fn report_crash_action(
    audit: Entity<Audit>,
    key: &'static str,
    title: &'static str,
) -> impl Fn(&'static str, io::Result<()>, &mut App) + 'static {
    move |_, outcome, cx| {
        audit.update(cx, |audit, cx| match outcome {
            Ok(()) => audit.clear_error(key, cx),
            Err(error) => audit.notify_error(key, title, error.to_string(), cx),
        });
    }
}

fn register_email_crash_report(
    handler: impl Fn() -> io::Result<()> + 'static,
    reporter: impl Fn(&'static str, io::Result<()>, &mut App) + 'static,
    cx: &mut App,
) {
    cx.on_action(move |_: &EmailCrashReport, cx| {
        reporter("Email Crash Report", handler(), cx);
    });
}

fn help_menu() -> Menu {
    Menu::new("Help").items(vec![
        MenuItem::action("About Press", AboutPress),
        MenuItem::separator(),
        MenuItem::action("Show Crash Reports", ShowCrashReports),
        MenuItem::action("Email Crash Report…", EmailCrashReport),
    ])
}

pub fn init(audit: Entity<Audit>, cx: &mut App) {
    cx.on_action(|_: &Quit, cx| cx.quit());
    cx.on_action(|_: &HideSelf, cx| cx.hide());
    cx.on_action(|_: &HideOthers, cx| cx.hide_other_apps());
    cx.on_action(|_: &ShowAll, cx| cx.unhide_other_apps());
    cx.on_action(|_: &CloseWindow, cx| {
        if let Some(window) = cx.active_window() {
            window
                .update(cx, |_, window, _| window.remove_window())
                .ok();
        }
    });
    cx.on_action(|_: &Minimize, cx| {
        if let Some(window) = cx.active_window() {
            window
                .update(cx, |_, window, _| window.minimize_window())
                .ok();
        }
    });
    cx.on_action(|_: &Zoom, cx| {
        if let Some(window) = cx.active_window() {
            window.update(cx, |_, window, _| window.zoom_window()).ok();
        }
    });
    let for_folder = audit.clone();
    cx.on_action(move |_: &OpenFolder, cx| {
        for_folder.update(cx, |audit, cx| audit.pick(true, cx));
    });
    let for_image = audit.clone();
    cx.on_action(move |_: &OpenImage, cx| {
        for_image.update(cx, |audit, cx| audit.pick(false, cx));
    });
    register_about_press(present_about_press, cx);
    register_show_crash_reports(
        crate::crash::try_reveal_reports,
        report_crash_action(
            audit.clone(),
            "crash-reports",
            "Couldn’t show crash reports",
        ),
        cx,
    );
    register_email_crash_report(
        crate::crash::email_report,
        report_crash_action(audit, "crash-email", "Couldn’t prepare crash report email"),
        cx,
    );

    // The menu shows each item's key equivalent from the keymap, so the
    // bindings and the menus describe one truth.
    cx.bind_keys([
        KeyBinding::new("cmd-q", Quit, None),
        KeyBinding::new("cmd-h", HideSelf, None),
        KeyBinding::new("alt-cmd-h", HideOthers, None),
        KeyBinding::new("cmd-w", CloseWindow, None),
        KeyBinding::new("cmd-m", Minimize, None),
        KeyBinding::new("cmd-o", OpenFolder, None),
        KeyBinding::new("shift-cmd-o", OpenImage, None),
    ]);

    cx.set_menus(vec![
        Menu::new("Press").items(vec![
            MenuItem::action("Hide Press", HideSelf),
            MenuItem::action("Hide Others", HideOthers),
            MenuItem::action("Show All", ShowAll),
            MenuItem::separator(),
            MenuItem::action("Quit Press", Quit),
        ]),
        Menu::new("File").items(vec![
            MenuItem::action("Open Folder…", OpenFolder),
            MenuItem::action("Open Image…", OpenImage),
            MenuItem::separator(),
            MenuItem::action("Close Window", CloseWindow),
        ]),
        Menu::new("Window").items(vec![
            MenuItem::action("Minimize", Minimize),
            MenuItem::action("Zoom", Zoom),
        ]),
        help_menu(),
    ]);
}

#[cfg(test)]
mod tests {
    use std::{
        cell::{Cell, RefCell},
        rc::Rc,
        sync::Arc,
    };

    use gpui::{MenuItem, TestAppContext};

    use super::*;

    #[gpui::test]
    fn about_action_invokes_its_presenter(cx: &mut TestAppContext) {
        let calls = Rc::new(Cell::new(0));
        let presented = calls.clone();
        cx.update(|cx| {
            register_about_press(move |_| presented.set(presented.get() + 1), cx);
            cx.dispatch_action(&AboutPress);
        });

        assert_eq!(calls.get(), 1);
    }

    #[derive(Debug)]
    struct InjectedShowCrashReportsError;

    impl std::fmt::Display for InjectedShowCrashReportsError {
        fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
            formatter.write_str("injected Show Crash Reports failure")
        }
    }

    impl std::error::Error for InjectedShowCrashReportsError {}

    #[gpui::test]
    fn show_crash_reports_surfaces_its_name_and_original_error(cx: &mut TestAppContext) {
        let calls = Rc::new(Cell::new(0));
        let fail = Rc::new(Cell::new(true));
        let payload = Arc::new(InjectedShowCrashReportsError);
        let reported = Rc::new(RefCell::new(Vec::new()));
        let handler_calls = calls.clone();
        let handler_fail = fail.clone();
        let handler_payload = payload.clone();
        let reporter_calls = reported.clone();
        let reporter_payload = payload.clone();

        cx.update(|cx| {
            register_show_crash_reports(
                move || {
                    handler_calls.set(handler_calls.get() + 1);
                    if handler_fail.replace(false) {
                        Err(io::Error::new(
                            io::ErrorKind::PermissionDenied,
                            handler_payload.clone(),
                        ))
                    } else {
                        Ok(())
                    }
                },
                move |action, outcome, _| {
                    assert_eq!(action, "Show Crash Reports");
                    let Err(error) = outcome else {
                        return;
                    };
                    assert_eq!(error.kind(), io::ErrorKind::PermissionDenied);
                    assert_eq!(error.to_string(), "injected Show Crash Reports failure");
                    let reported_payload = error
                        .get_ref()
                        .and_then(|source| {
                            source.downcast_ref::<Arc<InjectedShowCrashReportsError>>()
                        })
                        .expect("reporter should receive the original error payload");
                    assert!(Arc::ptr_eq(&reporter_payload, reported_payload));
                    reporter_calls.borrow_mut().push(());
                },
                cx,
            );
            cx.dispatch_action(&ShowCrashReports);
        });

        assert_eq!(calls.get(), 1);
        assert_eq!(reported.borrow().len(), 1);

        cx.update(|cx| cx.dispatch_action(&ShowCrashReports));

        assert_eq!(calls.get(), 2);
        assert_eq!(reported.borrow().len(), 1);
    }

    #[derive(Debug)]
    struct InjectedEmailCrashReportError;

    impl std::fmt::Display for InjectedEmailCrashReportError {
        fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
            formatter.write_str("injected Email Crash Report failure")
        }
    }

    impl std::error::Error for InjectedEmailCrashReportError {}

    #[gpui::test]
    fn email_crash_report_surfaces_its_named_failure(cx: &mut TestAppContext) {
        let calls = Rc::new(Cell::new(0));
        let payload = Arc::new(InjectedEmailCrashReportError);
        let reported = Rc::new(RefCell::new(Vec::new()));
        let handler_calls = calls.clone();
        let handler_payload = payload.clone();
        let reporter_calls = reported.clone();
        let reporter_payload = payload.clone();

        cx.update(|cx| {
            register_email_crash_report(
                move || {
                    handler_calls.set(handler_calls.get() + 1);
                    Err(io::Error::new(
                        io::ErrorKind::PermissionDenied,
                        handler_payload.clone(),
                    ))
                },
                move |action, outcome, _| {
                    assert_eq!(action, "Email Crash Report");
                    let Err(error) = outcome else {
                        return;
                    };
                    assert_eq!(error.kind(), io::ErrorKind::PermissionDenied);
                    assert_eq!(error.to_string(), "injected Email Crash Report failure");
                    let reported_payload = error
                        .get_ref()
                        .and_then(|source| {
                            source.downcast_ref::<Arc<InjectedEmailCrashReportError>>()
                        })
                        .expect("reporter should receive the original error payload");
                    assert!(Arc::ptr_eq(&reporter_payload, reported_payload));
                    reporter_calls.borrow_mut().push(());
                },
                cx,
            );
            cx.dispatch_action(&EmailCrashReport);
        });

        assert_eq!(calls.get(), 1);
        assert_eq!(reported.borrow().len(), 1);
    }

    #[test]
    fn about_copy_names_press_and_the_installed_version() {
        let copy = about_copy();

        assert!(copy.contains("Press"));
        assert!(copy.contains(env!("CARGO_PKG_VERSION")));
        assert!(copy.contains("ecommerce-image preflight"));
        assert!(copy.contains("audits and conversions stay on this computer"));
    }

    #[test]
    fn help_menu_starts_with_the_about_action() {
        let menu = help_menu();
        assert_eq!(menu.name, "Help");
        assert_eq!(menu.items.len(), 4);

        match &menu.items[0] {
            MenuItem::Action { name, action, .. } => {
                assert_eq!(name, "About Press");
                assert!(action.partial_eq(&AboutPress));
            }
            _ => panic!("the first Help item is About Press"),
        }
        assert!(matches!(menu.items[1], MenuItem::Separator));
        match &menu.items[2] {
            MenuItem::Action { name, action, .. } => {
                assert_eq!(name, "Show Crash Reports");
                assert!(action.partial_eq(&ShowCrashReports));
            }
            _ => panic!("the third Help item is Show Crash Reports"),
        }
        match &menu.items[3] {
            MenuItem::Action { name, action, .. } => {
                assert_eq!(name, "Email Crash Report…");
                assert!(action.partial_eq(&EmailCrashReport));
            }
            _ => panic!("the fourth Help item is Email Crash Report"),
        }
    }
}
