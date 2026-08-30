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

pub fn reveal_reports() {
    if let Err(error) = try_reveal_reports() {
        if is_missing_config_directory(&error) {
            eprintln!("press: no config directory for crash reports");
        } else {
            eprintln!("press: could not reveal crash reports: {error}");
        }
    }
}

pub fn try_reveal_reports() -> io::Result<()> {
    let directory = prepare_reports_directory()?;
    open_reports_directory(directory, |target| crate::open_with_desktop(target))
}

pub fn email_report() {
    reveal_reports();
    crate::open_url(&format!(
        "mailto:{REPORT_EMAIL}?subject=Press%20{}%20crash%20report&body=Please%20attach%20the%20newest%20.log%20file%20from%20the%20Press%20crash%20reports%20folder.",
        env!("CARGO_PKG_VERSION")
    ));
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

fn is_missing_config_directory(error: &io::Error) -> bool {
    error
        .get_ref()
        .is_some_and(|source| source.is::<MissingConfigDirectory>())
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
    use std::sync::Arc;

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
        assert!(is_missing_config_directory(&missing));

        let expected = PathBuf::from("reports");
        let prepared = prepare_directory(Some(expected.clone()), |_| Ok(())).unwrap();
        assert_eq!(prepared, expected);

        let payload = Arc::new(InjectedPreparationError);
        let injected = io::Error::new(io::ErrorKind::NotFound, payload.clone());
        let error =
            prepare_directory(Some(PathBuf::from("reports")), |_| Err(injected)).unwrap_err();
        assert_eq!(error.kind(), io::ErrorKind::NotFound);
        assert_eq!(error.to_string(), "injected preparation failure");
        assert!(!is_missing_config_directory(&error));
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
}
