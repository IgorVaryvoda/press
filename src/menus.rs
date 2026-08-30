//! The macOS menu bar. Without one, AppKit has no key equivalent for Quit,
//! Close, Hide or Minimize, and the app cannot be quit from the keyboard.

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
    cx.on_action(move |_: &OpenImage, cx| {
        audit.update(cx, |audit, cx| audit.pick(false, cx));
    });
    register_about_press(present_about_press, cx);
    cx.on_action(|_: &ShowCrashReports, _| {
        if let Err(error) = crate::crash::try_reveal_reports() {
            eprintln!("press: could not show crash reports: {error}");
        }
    });
    cx.on_action(|_: &EmailCrashReport, _| crate::crash::email_report());

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
    use std::{cell::Cell, rc::Rc};

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
