//! Local crash reports that stay on the device until the user shares one.

use std::backtrace::Backtrace;
use std::fmt::Write as _;
use std::io::{self, Write as _};
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicUsize, Ordering};
use std::time::{SystemTime, UNIX_EPOCH};

const REPORT_LIMIT: usize = 5;
const REPORT_EMAIL: &str = "igor@varyvoda.com";
static REPORT_SEQUENCE: AtomicUsize = AtomicUsize::new(0);

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

pub fn reveal_reports() -> io::Result<()> {
    let directory = directory().ok_or_else(|| {
        io::Error::new(
            io::ErrorKind::NotFound,
            "no config directory is available for crash reports",
        )
    })?;
    std::fs::create_dir_all(&directory)?;
    crate::reveal_path(&directory)
}

pub fn email_report() -> io::Result<()> {
    email_report_with(reveal_reports, |url| crate::open_with_desktop(url))
}

fn email_report_with(
    reveal: impl FnOnce() -> io::Result<()>,
    open: impl FnOnce(&str) -> io::Result<()>,
) -> io::Result<()> {
    let revealed = reveal();
    let mailed = open(&format!(
        "mailto:{REPORT_EMAIL}?subject=Press%20{}%20crash%20report&body=Please%20attach%20the%20newest%20.log%20file%20from%20the%20Press%20crash%20reports%20folder.",
        env!("CARGO_PKG_VERSION")
    ));
    revealed.and(mailed)
}

fn directory() -> Option<PathBuf> {
    crate::settings::path()?
        .parent()
        .map(|parent| parent.join("crashes"))
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
        let _ = std::fs::remove_file(path);
    }
}

#[cfg(test)]
mod tests {
    use super::*;

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
    fn email_is_attempted_even_when_revealing_reports_fails() {
        let attempted = std::cell::Cell::new(false);
        let error = email_report_with(
            || {
                Err(io::Error::new(
                    io::ErrorKind::PermissionDenied,
                    "folder denied",
                ))
            },
            |url| {
                attempted.set(true);
                assert!(url.starts_with("mailto:igor@varyvoda.com?"));
                Ok(())
            },
        )
        .unwrap_err();

        assert!(attempted.get());
        assert_eq!(error.kind(), io::ErrorKind::PermissionDenied);
        assert_eq!(error.to_string(), "folder denied");
    }
}
