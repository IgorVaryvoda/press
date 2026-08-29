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
        ShowCrashReports,
        EmailCrashReport
    ]
);

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
    cx.on_action(|_: &ShowCrashReports, _| crate::crash::reveal_reports());
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
        Menu::new("Help").items(vec![
            MenuItem::action("Show Crash Reports", ShowCrashReports),
            MenuItem::action("Email Crash Report…", EmailCrashReport),
        ]),
    ]);
}
