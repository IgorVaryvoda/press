//! Local crash reports that stay on the device until the user shares one.

use std::backtrace::Backtrace;
use std::fmt::Write as _;
use std::io::{self, Write as _};
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicUsize, Ordering};
use std::time::{SystemTime, UNIX_EPOCH};

use gpui::{App, InteractiveElement as _, ParentElement as _, Window};
use gpui_component::WindowExt;
use gpui_component::button::{Button, ButtonVariants};
use gpui_component::dialog::{DialogDescription, DialogFooter, DialogHeader, DialogTitle};

const REPORT_LIMIT: usize = 5;
const REPORT_EMAIL: &str = "igor@varyvoda.com";
static REPORT_SEQUENCE: AtomicUsize = AtomicUsize::new(0);

pub const PROMPT_TITLE: &str = "Press encountered a problem";
pub const PROMPT_DESCRIPTION: &str = "Press saved a diagnostic report on this device. Submit bug report opens an email draft and the crash report folder so you can attach it. Nothing is sent automatically.";
pub const PROMPT_NOT_NOW: &str = "Not now";
pub const PROMPT_SUBMIT: &str = "Submit bug report";

#[derive(Debug)]
struct MissingConfigDirectory;

impl std::fmt::Display for MissingConfigDirectory {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        formatter.write_str("no config directory for crash reports")
    }
}

impl std::error::Error for MissingConfigDirectory {}

pub fn install() {
    let default = std::panic::take_hook();
    std::panic::set_hook(Box::new(move |info| {
        match directory() {
            Some(directory) => match write_report(&directory, &render(info)) {
                Ok(path) => eprintln!("press: crash report written to {}", path.display()),
                Err(error) => eprintln!("press: could not write a crash report: {error}"),
            },
            None => eprintln!("press: no config directory for crash reports"),
        }
        default(info);
    }));
}

/// Freeze one eligible report before installing the panic hook. A later panic can
/// write another report, but it must not change the question this launch asks.
pub fn pending_snapshot() -> Option<PathBuf> {
    match directory() {
        Some(directory) => match pending_snapshot_in(&directory) {
            Ok(report) => report,
            Err(error) => {
                eprintln!("press: could not check crash reports: {error}");
                None
            }
        },
        None => None,
    }
}

fn pending_snapshot_in(directory: &Path) -> io::Result<Option<PathBuf>> {
    let entries = match std::fs::read_dir(directory) {
        Ok(entries) => entries,
        Err(error) if error.kind() == io::ErrorKind::NotFound => return Ok(None),
        Err(error) => return Err(error),
    };
    let mut reports = Vec::new();
    for entry in entries {
        let entry = entry?;
        let path = entry.path();
        if !is_report(&path)? {
            continue;
        }
        let marker = prompted_path(&path);
        if is_regular_file(&marker)? {
            continue;
        }
        reports.push(path);
    }
    reports.sort_unstable();
    Ok(reports.pop())
}

fn is_report(path: &Path) -> io::Result<bool> {
    let Some(name) = path.file_name().and_then(|name| name.to_str()) else {
        return Ok(false);
    };
    Ok(is_report_name(name) && is_nonempty_regular_file(path)?)
}

fn is_report_name(name: &str) -> bool {
    let Some(stem) = name.strip_suffix(".log") else {
        return false;
    };
    let Some(rest) = stem.strip_prefix("crash-") else {
        return false;
    };
    let mut fields = rest.split('-');
    let (Some(nanos), Some(pid), Some(sequence), None) =
        (fields.next(), fields.next(), fields.next(), fields.next())
    else {
        return false;
    };
    nanos.len() == 20
        && nanos.bytes().all(|byte| byte.is_ascii_digit())
        && !pid.is_empty()
        && pid.bytes().all(|byte| byte.is_ascii_digit())
        && sequence.len() >= 4
        && sequence.bytes().all(|byte| byte.is_ascii_digit())
}

fn is_nonempty_regular_file(path: &Path) -> io::Result<bool> {
    match std::fs::symlink_metadata(path) {
        Ok(metadata) => Ok(metadata.file_type().is_file() && metadata.len() > 0),
        Err(error) if error.kind() == io::ErrorKind::NotFound => Ok(false),
        Err(error) => Err(error),
    }
}

fn is_regular_file(path: &Path) -> io::Result<bool> {
    match std::fs::symlink_metadata(path) {
        Ok(metadata) => Ok(metadata.file_type().is_file()),
        Err(error) if error.kind() == io::ErrorKind::NotFound => Ok(false),
        Err(error) => Err(error),
    }
}

fn prompted_path(report: &Path) -> PathBuf {
    let mut marker = report.as_os_str().to_os_string();
    marker.push(".prompted");
    PathBuf::from(marker)
}

pub fn acknowledge(report: &Path) -> io::Result<()> {
    match std::fs::symlink_metadata(report) {
        Ok(metadata) if metadata.file_type().is_file() => {}
        Ok(_) => {
            return Err(io::Error::new(
                io::ErrorKind::InvalidInput,
                "invalid crash report",
            ));
        }
        Err(error) if error.kind() == io::ErrorKind::NotFound => return Ok(()),
        Err(error) => return Err(error),
    }

    let marker = prompted_path(report);
    match std::fs::OpenOptions::new()
        .write(true)
        .create_new(true)
        .open(&marker)
    {
        Ok(_) => Ok(()),
        Err(error) if error.kind() == io::ErrorKind::AlreadyExists && is_regular_file(&marker)? => {
            Ok(())
        }
        Err(error) => Err(error),
    }
}

pub fn defer_prompt(window: &Window, cx: &mut App, report: Option<PathBuf>) {
    defer_prompt_with(window, cx, report, show_prompt);
}

fn defer_prompt_with(
    window: &Window,
    cx: &mut App,
    report: Option<PathBuf>,
    show: impl FnOnce(&mut Window, &mut App, PathBuf, fn() -> io::Result<()>) + 'static,
) {
    let Some(report) = report else {
        return;
    };
    window.defer(cx, move |window, cx| show(window, cx, report, email_report));
}

fn show_prompt(
    window: &mut Window,
    cx: &mut App,
    report: PathBuf,
    handoff: fn() -> io::Result<()>,
) {
    show_prompt_with(window, cx, report, handoff);
}

fn show_prompt_with(
    window: &mut Window,
    cx: &mut App,
    report: PathBuf,
    handoff: impl Fn() -> io::Result<()> + 'static,
) {
    let handoff = std::rc::Rc::new(handoff);
    window.open_alert_dialog(cx, move |dialog, _, _| {
        let report = report.clone();
        let handoff = handoff.clone();
        dialog
            .close_button(false)
            .keyboard(false)
            .content(|content, _, _| {
                content.child(
                    DialogHeader::new()
                        .child(
                            DialogTitle::new().child(
                                gpui::div()
                                    .id("crash-prompt-title")
                                    .debug_selector(|| "crash-prompt-title".into())
                                    .child(PROMPT_TITLE),
                            ),
                        )
                        .child(
                            DialogDescription::new().child(
                                gpui::div()
                                    .id("crash-prompt-description")
                                    .debug_selector(|| "crash-prompt-description".into())
                                    .child(PROMPT_DESCRIPTION),
                            ),
                        ),
                )
            })
            .footer(
                DialogFooter::new()
                    .child(
                        gpui::div()
                            .debug_selector(|| "crash-prompt-not-now".into())
                            .child(
                                Button::new("crash-prompt-not-now")
                                    .label(PROMPT_NOT_NOW)
                                    .on_click(|_, window, cx| window.close_dialog(cx)),
                            ),
                    )
                    .child(
                        gpui::div()
                            .debug_selector(|| "crash-prompt-submit".into())
                            .child(
                            Button::new("crash-prompt-submit")
                                .primary()
                                .label(PROMPT_SUBMIT)
                                .on_click(move |_, window, cx| match handoff() {
                                    Ok(()) => {
                                        if let Err(error) = acknowledge(&report) {
                                            eprintln!(
                                                "press: could not acknowledge crash report: {error}"
                                            );
                                        }
                                        window.close_dialog(cx);
                                    }
                                    Err(error) => {
                                        eprintln!(
                                            "press: could not open crash report handoff: {error}"
                                        );
                                    }
                                }),
                        ),
                    ),
            )
    });
}

pub fn try_reveal_reports() -> io::Result<()> {
    let directory = prepare_reports_directory()?;
    open_reports_directory(directory, |target| crate::open_with_desktop(target))
}

pub fn email_report() -> io::Result<()> {
    email_report_with(try_reveal_reports, |target| {
        crate::open_with_desktop(target)
    })
}

fn email_report_with(
    reveal_reports: impl FnOnce() -> io::Result<()>,
    open_email: impl FnOnce(&str) -> io::Result<()>,
) -> io::Result<()> {
    let reveal = reveal_reports();
    let email = open_email(&email_target());
    match reveal {
        Err(error) => Err(error),
        Ok(()) => email,
    }
}

fn email_target() -> String {
    format!(
        "mailto:{REPORT_EMAIL}?subject=Press%20{}%20crash%20report&body=Please%20attach%20the%20newest%20.log%20file%20from%20the%20Press%20crash%20reports%20folder.",
        env!("CARGO_PKG_VERSION")
    )
}

fn directory() -> Option<PathBuf> {
    crate::settings::path()?
        .parent()
        .map(|parent| parent.join("crashes"))
}

fn prepare_reports_directory() -> io::Result<PathBuf> {
    prepare_directory(directory(), |directory| std::fs::create_dir_all(directory))
}

fn prepare_directory(
    directory: Option<PathBuf>,
    create_directory: impl FnOnce(&Path) -> io::Result<()>,
) -> io::Result<PathBuf> {
    let directory =
        directory.ok_or_else(|| io::Error::new(io::ErrorKind::NotFound, MissingConfigDirectory))?;
    create_directory(&directory)?;
    Ok(directory)
}

fn open_reports_directory(
    directory: PathBuf,
    open_directory: impl FnOnce(&std::ffi::OsStr) -> io::Result<()>,
) -> io::Result<()> {
    open_directory(directory.as_os_str())?;
    Ok(())
}

fn render(info: &std::panic::PanicHookInfo<'_>) -> String {
    let time = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap_or_default()
        .as_millis();
    let thread = std::thread::current();
    let message = info
        .payload()
        .downcast_ref::<&str>()
        .copied()
        .or_else(|| info.payload().downcast_ref::<String>().map(String::as_str))
        .unwrap_or("non-string panic");
    let mut report = format!(
        "Press {} crash report\nsend_to: {REPORT_EMAIL}\nunix_time_ms: {time}\nplatform: {} {}\nprocess: {}\nthread: {}\npanic: {message}\n",
        env!("CARGO_PKG_VERSION"),
        std::env::consts::OS,
        std::env::consts::ARCH,
        std::process::id(),
        thread.name().unwrap_or("unnamed"),
    );
    if let Some(location) = info.location() {
        let _ = writeln!(
            report,
            "location: {}:{}:{}",
            location.file(),
            location.line(),
            location.column()
        );
    }
    let _ = write!(report, "\nbacktrace:\n{}", Backtrace::force_capture());
    report
}

fn write_report(directory: &Path, report: &str) -> io::Result<PathBuf> {
    std::fs::create_dir_all(directory)?;
    let time = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap_or_default()
        .as_nanos();
    let sequence = REPORT_SEQUENCE.fetch_add(1, Ordering::Relaxed);
    let path = directory.join(format!(
        "crash-{time:020}-{}-{sequence:04}.log",
        std::process::id()
    ));
    let mut file = std::fs::OpenOptions::new()
        .write(true)
        .create_new(true)
        .open(&path)?;
    file.write_all(report.as_bytes())?;
    file.sync_all()?;
    prune_reports(directory);
    Ok(path)
}

fn prune_reports(directory: &Path) {
    let Ok(entries) = std::fs::read_dir(directory) else {
        return;
    };
    let mut reports: Vec<_> = entries
        .filter_map(Result::ok)
        .map(|entry| entry.path())
        .filter(|path| {
            path.file_name()
                .and_then(|name| name.to_str())
                .is_some_and(|name| name.starts_with("crash-") && name.ends_with(".log"))
        })
        .collect();
    reports.sort_unstable();
    let remove = reports.len().saturating_sub(REPORT_LIMIT);
    for path in reports.into_iter().take(remove) {
        remove_report_and_marker(&path);
    }
}

fn remove_report_and_marker(report: &Path) {
    remove_report_and_marker_with(
        report,
        |path| std::fs::remove_file(path),
        |path| std::fs::remove_file(path),
    );
}

fn remove_report_and_marker_with(
    report: &Path,
    remove_report: impl FnOnce(&Path) -> io::Result<()>,
    remove_marker: impl FnOnce(&Path) -> io::Result<()>,
) {
    match remove_report(report) {
        Ok(()) => {
            let _ = remove_marker(&prompted_path(report));
        }
        Err(error) if error.kind() == io::ErrorKind::NotFound => {
            let _ = remove_marker(&prompted_path(report));
        }
        Err(_) => {}
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use gpui::{
        AppContext, Context, IntoElement, Render, Styled, TestAppContext, VisualTestContext,
    };
    use gpui_component::Root;
    use std::{cell::Cell, rc::Rc, sync::Arc};

    struct PromptHarness;

    impl Render for PromptHarness {
        fn render(&mut self, window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
            gpui::div()
                .size_full()
                .children(Root::render_dialog_layer(window, cx))
        }
    }

    fn prompt_window(cx: &mut TestAppContext) -> &mut VisualTestContext {
        cx.update(crate::init_theme);
        let (_, cx) = cx.add_window_view(|window, cx| {
            let harness = cx.new(|_| PromptHarness);
            Root::new(harness, window, cx)
        });
        cx
    }

    fn draw(cx: &mut VisualTestContext) {
        cx.simulate_resize(gpui::size(gpui::px(900.), gpui::px(640.)));
        cx.executor()
            .advance_clock(std::time::Duration::from_millis(500));
        cx.update(|window, cx| window.draw(cx).clear(cx));
        cx.update(|window, cx| window.draw(cx).clear(cx));
    }

    fn test_directory(name: &str) -> PathBuf {
        let directory = std::env::temp_dir().join(format!(
            "press-{name}-{}-{}",
            std::process::id(),
            REPORT_SEQUENCE.fetch_add(1, Ordering::Relaxed)
        ));
        std::fs::create_dir_all(&directory).unwrap();
        directory
    }

    fn report(directory: &Path, nanos: u64) -> PathBuf {
        let path = directory.join(format!("crash-{nanos:020}-42-0000.log"));
        std::fs::write(&path, "panic").unwrap();
        path
    }

    #[derive(Debug)]
    struct InjectedPreparationError;

    impl std::fmt::Display for InjectedPreparationError {
        fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
            formatter.write_str("injected preparation failure")
        }
    }

    impl std::error::Error for InjectedPreparationError {}

    #[test]
    fn directory_preparation_preserves_missing_config_and_error_identity() {
        let missing = prepare_directory(None, |_| unreachable!()).unwrap_err();
        assert_eq!(missing.kind(), io::ErrorKind::NotFound);
        assert_eq!(missing.to_string(), "no config directory for crash reports");
        assert!(
            missing
                .get_ref()
                .is_some_and(|source| source.is::<MissingConfigDirectory>())
        );

        let expected = PathBuf::from("reports");
        let prepared = prepare_directory(Some(expected.clone()), |_| Ok(())).unwrap();
        assert_eq!(prepared, expected);

        let payload = Arc::new(InjectedPreparationError);
        let injected = io::Error::new(io::ErrorKind::NotFound, payload.clone());
        let error =
            prepare_directory(Some(PathBuf::from("reports")), |_| Err(injected)).unwrap_err();
        assert_eq!(error.kind(), io::ErrorKind::NotFound);
        assert_eq!(error.to_string(), "injected preparation failure");
        assert!(
            !error
                .get_ref()
                .is_some_and(|source| source.is::<MissingConfigDirectory>())
        );
        let returned_payload = error
            .get_ref()
            .and_then(|source| source.downcast_ref::<Arc<InjectedPreparationError>>())
            .expect("injected payload should be preserved");
        assert!(Arc::ptr_eq(&payload, returned_payload));
    }

    #[derive(Debug)]
    struct InjectedOpenerError;

    impl std::fmt::Display for InjectedOpenerError {
        fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
            formatter.write_str("injected opener failure")
        }
    }

    impl std::error::Error for InjectedOpenerError {}

    #[test]
    fn folder_opener_preserves_target_and_error_identity() {
        let expected = PathBuf::from("reports");
        let mut opened = None;
        open_reports_directory(expected.clone(), |target| {
            opened = Some(PathBuf::from(target));
            Ok(())
        })
        .unwrap();
        assert_eq!(opened, Some(expected));

        let payload = Arc::new(InjectedOpenerError);
        let injected = io::Error::new(io::ErrorKind::PermissionDenied, payload.clone());
        let error =
            open_reports_directory(PathBuf::from("reports"), |_| Err(injected)).unwrap_err();
        assert_eq!(error.kind(), io::ErrorKind::PermissionDenied);
        assert_eq!(error.to_string(), "injected opener failure");
        let returned_payload = error
            .get_ref()
            .and_then(|source| source.downcast_ref::<Arc<InjectedOpenerError>>())
            .expect("injected payload should be preserved");
        assert!(Arc::ptr_eq(&payload, returned_payload));
    }

    #[test]
    fn email_handoff_attempts_mail_after_every_reveal_outcome() {
        let target = format!(
            "mailto:{REPORT_EMAIL}?subject=Press%20{}%20crash%20report&body=Please%20attach%20the%20newest%20.log%20file%20from%20the%20Press%20crash%20reports%20folder.",
            env!("CARGO_PKG_VERSION")
        );
        let reveal_errors = [
            io::ErrorKind::NotFound,
            io::ErrorKind::PermissionDenied,
            io::ErrorKind::Other,
        ];

        for kind in reveal_errors {
            let opened = Arc::new(std::sync::atomic::AtomicUsize::new(0));
            let opened_by_email = opened.clone();
            let expected_target = target.clone();
            let error = email_report_with(
                || Err(io::Error::from(kind)),
                move |actual| {
                    assert_eq!(actual, expected_target);
                    opened_by_email.fetch_add(1, Ordering::Relaxed);
                    Ok(())
                },
            )
            .unwrap_err();
            assert_eq!(error.kind(), kind);
            assert_eq!(opened.load(Ordering::Relaxed), 1);
        }

        let opened = Arc::new(std::sync::atomic::AtomicUsize::new(0));
        let opened_by_email = opened.clone();
        let expected_target = target.clone();
        let mail_error = email_report_with(
            || Ok(()),
            move |actual| {
                assert_eq!(actual, expected_target);
                opened_by_email.fetch_add(1, Ordering::Relaxed);
                Err(io::Error::from(io::ErrorKind::ConnectionRefused))
            },
        )
        .unwrap_err();
        assert_eq!(mail_error.kind(), io::ErrorKind::ConnectionRefused);
        assert_eq!(opened.load(Ordering::Relaxed), 1);

        let opened = Arc::new(std::sync::atomic::AtomicUsize::new(0));
        let opened_by_email = opened.clone();
        let expected_target = target.clone();
        let first_error = email_report_with(
            || Err(io::Error::from(io::ErrorKind::PermissionDenied)),
            move |actual| {
                assert_eq!(actual, expected_target);
                opened_by_email.fetch_add(1, Ordering::Relaxed);
                Err(io::Error::from(io::ErrorKind::ConnectionRefused))
            },
        )
        .unwrap_err();
        assert_eq!(first_error.kind(), io::ErrorKind::PermissionDenied);
        assert_eq!(opened.load(Ordering::Relaxed), 1);

        let opened = Arc::new(std::sync::atomic::AtomicUsize::new(0));
        let opened_by_email = opened.clone();
        let expected_target = target.clone();
        email_report_with(
            || Ok(()),
            move |actual| {
                assert_eq!(actual, expected_target);
                opened_by_email.fetch_add(1, Ordering::Relaxed);
                Ok(())
            },
        )
        .unwrap();
        assert_eq!(opened.load(Ordering::Relaxed), 1);
    }

    #[test]
    fn crash_reports_are_local_and_bounded() {
        let directory = std::env::temp_dir().join(format!(
            "press-crash-test-{}-{}",
            std::process::id(),
            REPORT_SEQUENCE.fetch_add(1, Ordering::Relaxed)
        ));
        std::fs::create_dir_all(&directory).unwrap();
        std::fs::write(directory.join("keep.txt"), "not a crash report").unwrap();
        for index in 0..=REPORT_LIMIT {
            std::fs::write(
                directory.join(format!("crash-{index:020}-0-0000.log")),
                index.to_string(),
            )
            .unwrap();
        }
        let written = write_report(&directory, "panic and backtrace").unwrap();

        let reports = std::fs::read_dir(&directory)
            .unwrap()
            .filter_map(Result::ok)
            .filter(|entry| entry.file_name().to_string_lossy().starts_with("crash-"))
            .count();
        assert_eq!(reports, REPORT_LIMIT);
        assert_eq!(
            std::fs::read_to_string(written).unwrap(),
            "panic and backtrace"
        );
        assert!(directory.join("keep.txt").exists());
        std::fs::remove_dir_all(directory).unwrap();
    }

    #[test]
    fn pending_crash_snapshot_selects_one_existing_report() {
        let directory = test_directory("pending-snapshot");
        let oldest = report(&directory, 1);
        let newest = report(&directory, 3);
        let marked = report(&directory, 4);
        std::fs::write(prompted_path(&marked), "").unwrap();
        std::fs::write(directory.join("crash-00000000000000000002-42-0000.log"), "").unwrap();
        assert_eq!(pending_snapshot_in(&directory).unwrap(), Some(newest));
        assert!(oldest.exists());
        std::fs::remove_dir_all(directory).unwrap();
    }

    #[test]
    fn invalid_markers_do_not_hide_pending_reports() {
        let directory = test_directory("invalid-marker");
        let newest = report(&directory, 9);
        std::fs::create_dir(prompted_path(&newest)).unwrap();
        assert_eq!(pending_snapshot_in(&directory).unwrap(), Some(newest));
        std::fs::remove_dir_all(directory).unwrap();
    }

    #[test]
    fn acknowledgement_is_idempotent_and_never_changes_report_bytes() {
        let directory = test_directory("acknowledgement");
        let report = report(&directory, 1);
        let before = std::fs::read(&report).unwrap();
        acknowledge(&report).unwrap();
        acknowledge(&report).unwrap();
        assert_eq!(std::fs::read(&report).unwrap(), before);
        assert!(is_regular_file(&prompted_path(&report)).unwrap());
        std::fs::remove_dir_all(directory).unwrap();
    }

    #[test]
    fn acknowledgement_handles_missing_and_replaced_reports() {
        let directory = test_directory("acknowledgement-missing");
        let missing = directory.join("crash-00000000000000000001-42-0000.log");
        acknowledge(&missing).unwrap();
        let directory_report = report(&directory, 2);
        std::fs::remove_file(&directory_report).unwrap();
        std::fs::create_dir(&directory_report).unwrap();
        assert_eq!(
            acknowledge(&directory_report).unwrap_err().kind(),
            io::ErrorKind::InvalidInput
        );
        #[cfg(unix)]
        {
            let target = directory.join("target.log");
            std::fs::write(&target, "panic").unwrap();
            let symlink_report = directory.join("crash-00000000000000000003-42-0000.log");
            std::os::unix::fs::symlink(&target, &symlink_report).unwrap();
            assert_eq!(
                acknowledge(&symlink_report).unwrap_err().kind(),
                io::ErrorKind::InvalidInput
            );

            let report_with_symlink_marker = report(&directory, 4);
            std::os::unix::fs::symlink(&target, prompted_path(&report_with_symlink_marker))
                .unwrap();
            assert!(acknowledge(&report_with_symlink_marker).is_err());
        }
        std::fs::remove_dir_all(directory).unwrap();
    }

    #[test]
    fn retention_keeps_marker_when_report_removal_fails() {
        let directory = test_directory("retention-marker");
        let report = report(&directory, 1);
        let marker = prompted_path(&report);
        std::fs::write(&marker, "").unwrap();
        remove_report_and_marker_with(
            &report,
            |_| Err(io::Error::from(io::ErrorKind::PermissionDenied)),
            |_| panic!("marker must stay when report removal fails"),
        );
        assert!(marker.exists());
        remove_report_and_marker_with(&report, |_| Ok(()), |path| std::fs::remove_file(path));
        assert!(!marker.exists());
        std::fs::remove_dir_all(directory).unwrap();
    }

    #[gpui::test]
    fn crash_prompt_is_modal_and_uses_exact_visible_copy(cx: &mut TestAppContext) {
        let directory = test_directory("modal-copy");
        let report = report(&directory, 1);
        let calls = Rc::new(Cell::new(0));
        let handoff_calls = calls.clone();
        let cx = prompt_window(cx);
        cx.update(|window, cx| {
            show_prompt_with(window, cx, report.clone(), move || {
                handoff_calls.set(handoff_calls.get() + 1);
                Ok(())
            });
        });
        draw(cx);

        assert_eq!(PROMPT_TITLE, "Press encountered a problem");
        assert_eq!(
            PROMPT_DESCRIPTION,
            "Press saved a diagnostic report on this device. Submit bug report opens an email draft and the crash report folder so you can attach it. Nothing is sent automatically."
        );
        assert_eq!(PROMPT_NOT_NOW, "Not now");
        assert_eq!(PROMPT_SUBMIT, "Submit bug report");
        for selector in [
            "crash-prompt-title",
            "crash-prompt-description",
            "crash-prompt-not-now",
            "crash-prompt-submit",
        ] {
            assert!(cx.debug_bounds(selector).is_some(), "missing {selector}");
        }

        cx.simulate_keystrokes("escape");
        draw(cx);
        cx.update(|window, cx| assert!(window.has_active_dialog(cx)));
        assert_eq!(calls.get(), 0);
        assert!(!prompted_path(&report).exists());

        let not_now = cx.debug_bounds("crash-prompt-not-now").unwrap();
        cx.simulate_click(not_now.center(), gpui::Modifiers::none());
        draw(cx);
        cx.update(|window, cx| assert!(!window.has_active_dialog(cx)));
        assert_eq!(calls.get(), 0);
        assert!(!prompted_path(&report).exists());
        std::fs::remove_dir_all(directory).unwrap();
    }

    #[gpui::test]
    fn public_defer_prompt_reaches_a_painted_prompt(cx: &mut TestAppContext) {
        let directory = test_directory("public-defer-prompt");
        let report = report(&directory, 1);
        let cx = prompt_window(cx);
        cx.update(|window, cx| defer_prompt(window, cx, Some(report)));
        cx.run_until_parked();
        draw(cx);

        assert!(cx.debug_bounds("crash-prompt-title").is_some());
        std::fs::remove_dir_all(directory).unwrap();
    }

    #[gpui::test]
    fn backdrop_click_leaves_crash_prompt_pending(cx: &mut TestAppContext) {
        let directory = test_directory("backdrop-click");
        let report = report(&directory, 1);
        let calls = Rc::new(Cell::new(0));
        let handoff_calls = calls.clone();
        let cx = prompt_window(cx);
        cx.update(|window, cx| {
            show_prompt_with(window, cx, report.clone(), move || {
                handoff_calls.set(handoff_calls.get() + 1);
                Ok(())
            });
        });
        draw(cx);
        cx.simulate_click(
            gpui::point(gpui::px(10.), gpui::px(40.)),
            gpui::Modifiers::none(),
        );
        draw(cx);

        assert_eq!(calls.get(), 0);
        assert!(!prompted_path(&report).exists());
        assert_eq!(pending_snapshot_in(&directory).unwrap(), Some(report));
        cx.update(|window, cx| assert!(window.has_active_dialog(cx)));
        std::fs::remove_dir_all(directory).unwrap();
    }

    #[gpui::test]
    fn window_removal_leaves_crash_report_pending(cx: &mut TestAppContext) {
        let directory = test_directory("window-removal");
        let report = report(&directory, 1);
        let calls = Rc::new(Cell::new(0));
        let handoff_calls = calls.clone();
        let cx = prompt_window(cx);
        cx.update(|window, cx| {
            show_prompt_with(window, cx, report.clone(), move || {
                handoff_calls.set(handoff_calls.get() + 1);
                Ok(())
            });
        });
        draw(cx);
        cx.update(|window, _| window.remove_window());
        cx.run_until_parked();

        assert_eq!(calls.get(), 0);
        assert!(!prompted_path(&report).exists());
        assert_eq!(pending_snapshot_in(&directory).unwrap(), Some(report));
        std::fs::remove_dir_all(directory).unwrap();
    }

    #[gpui::test]
    fn not_now_leaves_the_report_pending(cx: &mut TestAppContext) {
        let directory = test_directory("not-now");
        let report = report(&directory, 1);
        let calls = Rc::new(Cell::new(0));
        let handoff_calls = calls.clone();
        let cx = prompt_window(cx);
        cx.update(|window, cx| {
            show_prompt_with(window, cx, report.clone(), move || {
                handoff_calls.set(handoff_calls.get() + 1);
                Ok(())
            });
        });
        draw(cx);
        let not_now = cx.debug_bounds("crash-prompt-not-now").unwrap();
        cx.simulate_click(not_now.center(), gpui::Modifiers::none());
        draw(cx);

        assert_eq!(calls.get(), 0);
        assert!(!prompted_path(&report).exists());
        assert_eq!(pending_snapshot_in(&directory).unwrap(), Some(report));
        cx.update(|window, cx| assert!(!window.has_active_dialog(cx)));
        std::fs::remove_dir_all(directory).unwrap();
    }

    #[gpui::test]
    fn failed_submit_keeps_the_dialog_open_and_report_pending(cx: &mut TestAppContext) {
        let directory = test_directory("failed-submit");
        let report = report(&directory, 1);
        let calls = Rc::new(Cell::new(0));
        let handoff_calls = calls.clone();
        let cx = prompt_window(cx);
        cx.update(|window, cx| {
            show_prompt_with(window, cx, report.clone(), move || {
                handoff_calls.set(handoff_calls.get() + 1);
                Err(io::Error::from(io::ErrorKind::ConnectionRefused))
            });
        });
        draw(cx);
        let submit = cx.debug_bounds("crash-prompt-submit").unwrap();
        cx.simulate_click(submit.center(), gpui::Modifiers::none());
        draw(cx);

        assert_eq!(calls.get(), 1);
        assert!(!prompted_path(&report).exists());
        cx.update(|window, cx| assert!(window.has_active_dialog(cx)));
        std::fs::remove_dir_all(directory).unwrap();
    }

    #[gpui::test]
    fn successful_submit_acknowledges_after_generic_handoff(cx: &mut TestAppContext) {
        let directory = test_directory("successful-submit");
        let report = report(&directory, 1);
        let calls = Rc::new(Cell::new(0));
        let handoff_calls = calls.clone();
        let cx = prompt_window(cx);
        cx.update(|window, cx| {
            show_prompt_with(window, cx, report.clone(), move || {
                handoff_calls.set(handoff_calls.get() + 1);
                Ok(())
            });
        });
        draw(cx);
        let submit = cx.debug_bounds("crash-prompt-submit").unwrap();
        cx.simulate_click(submit.center(), gpui::Modifiers::none());
        draw(cx);

        assert_eq!(calls.get(), 1);
        assert!(prompted_path(&report).exists());
        cx.update(|window, cx| assert!(!window.has_active_dialog(cx)));
        std::fs::remove_dir_all(directory).unwrap();
    }

    #[gpui::test]
    fn production_prompt_submit_uses_generic_handoff(cx: &mut TestAppContext) {
        let directory = test_directory("production-handoff");
        let report = report(&directory, 1);
        let received = Rc::new(Cell::new(None));
        let received_handoff = received.clone();
        let cx = prompt_window(cx);
        cx.update(|window, cx| {
            defer_prompt_with(
                window,
                cx,
                Some(report.clone()),
                move |_, _, actual, handoff| {
                    assert_eq!(actual, report);
                    received_handoff.set(Some(handoff));
                },
            );
        });
        cx.run_until_parked();

        assert_eq!(
            received.get().map(|handoff| handoff as *const () as usize),
            Some(email_report as *const () as usize)
        );
        std::fs::remove_dir_all(directory).unwrap();
    }
}
