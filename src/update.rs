use cargo_packager_updater::{Config, check_update};
use std::sync::atomic::{AtomicU8, Ordering};

/// The repository was renamed from `imageguide-desktop` to `press`. Every copy
/// installed before that polls the old path, and GitHub redirects it here for
/// as long as nothing else claims the old name — so the old name must never be
/// used for another repository under this account.
const ENDPOINT: &str = "https://github.com/IgorVaryvoda/press/releases/latest/download/latest.json";

/// What the last update attempt did, for the window to show.
/// 0 = nothing attempted yet, 1 = installed while work was active,
/// 2 = install or check failed (message in `UPDATE_MESSAGE`).
static UPDATE_STATE: AtomicU8 = AtomicU8::new(0);
static UPDATE_MESSAGE: parking_lot::Mutex<Option<String>> = parking_lot::Mutex::new(None);

pub fn update_state() -> u8 {
    UPDATE_STATE.load(Ordering::Relaxed)
}

/// One line, only meaningful when `update_state()` is nonzero.
pub fn update_message() -> Option<String> {
    UPDATE_MESSAGE.lock().clone()
}

/// The one-line notice the window shows about the last update attempt, if any.
/// `None` when the updater is idle or the feature is compiled out.
#[cfg(feature = "updater")]
pub fn notice() -> Option<String> {
    match update_state() {
        1 => Some("installed an update — restart to use it".to_string()),
        2 => update_message().map(|message| format!("could not update: {message}")),
        _ => None,
    }
}

/// True when the executable path has the shape of an installed macOS app
/// bundle. The bar is `.app/Contents/MacOS/`, not just `Contents/MacOS/`:
/// cargo-packager-updater resolves the "installed app" from this path, and
/// outside a real bundle that resolution is the executable's parent
/// directory, which an update then `remove_dir_all`s — for a
/// `target/release/press` that deletes the build tree.
///
/// Known accepted-but-imperfect cases, on purpose: an app run straight off
/// a read-only DMG and a Gatekeeper-translocated app both still match this
/// shape (translocated paths keep the `.app/Contents/MacOS/` form, and
/// Apple provides no supported translocation detector). For those, the
/// update fails or escalates exactly as it does today — that behavior is
/// decision memo B's territory, not this guard's.
///
/// Component-wise, not substring: the updater resolves the bundle by
/// walking up two parents from the executable, so
/// `Foo.app/Contents/MacOS/helpers/exe` would resolve to `Foo.app/Contents`
/// and delete that. Only the immediate layout counts.
fn updater_shell_path_is_safe(path: &std::path::Path) -> bool {
    !path
        .to_string_lossy()
        .chars()
        .any(|character| matches!(character, '\'' | '"' | '\\' | '\n' | '\r'))
}

fn mac_bundle_path(exe: &std::path::Path) -> bool {
    let parent_is = |path: Option<&std::path::Path>, name: &str| {
        path.and_then(|p| p.file_name()).is_some_and(|n| n == name)
    };
    let parent = exe.parent();
    let contents = parent.and_then(|p| p.parent());
    let bundle = contents.and_then(|p| p.parent());
    parent_is(parent, "MacOS")
        && parent_is(contents, "Contents")
        && bundle
            .and_then(|p| p.file_name())
            .and_then(|n| n.to_str())
            .is_some_and(|n| n.ends_with(".app"))
        // updater 0.2.3 inserts this path into both an AppleScript string and a
        // single-quoted privileged shell command when /Applications needs admin
        // access. Refuse characters that can end either string.
        && updater_shell_path_is_safe(exe)
}

/// True when this process is actually an AppImage payload run: the runtime
/// exports APPIMAGE (the image the updater will replace) and APPDIR (where
/// the payload is mounted or extracted), and the running executable must
/// live under that APPDIR. An inherited APPIMAGE from another program's
/// environment fails this containment, and both the FUSE-mount and the
/// --appimage-extract-and-run execution modes pass it.
fn appimage_run(
    exe: &std::path::Path,
    appimage_set: bool,
    appdir: Option<&std::path::Path>,
) -> bool {
    appimage_set && appdir.is_some_and(|dir| exe.starts_with(dir))
}

fn updatable_install(exe: &std::path::Path) -> bool {
    if cfg!(target_os = "macos") {
        mac_bundle_path(exe) && updater_shell_path_is_safe(&std::env::temp_dir())
    } else if cfg!(target_os = "linux") {
        let appdir = std::env::var_os("APPDIR").map(std::path::PathBuf::from);
        appimage_run(
            exe,
            std::env::var_os("APPIMAGE").is_some(),
            appdir.as_deref(),
        )
    } else {
        // Windows installs via NSIS, which the updater handles through the
        // installer rather than by moving directories.
        true
    }
}

fn public_key() -> &'static str {
    // include_str! preserves the key file's final newline, but the updater's
    // strict base64 decoder does not accept whitespace.
    include_str!("../assets/updater.pub").trim()
}

pub fn install_if_available() -> bool {
    let Ok(exe) = std::env::current_exe() else {
        return false;
    };
    if !updatable_install(&exe) {
        return false;
    }

    let config = Config {
        endpoints: vec![ENDPOINT.parse().expect("the update URL is valid")],
        pubkey: public_key().into(),
        ..Default::default()
    };
    let version = env!("CARGO_PKG_VERSION")
        .parse()
        .expect("the package version is semver");

    match check_update(version, config) {
        Ok(Some(update)) => match update.download_and_install() {
            Ok(()) => {
                *UPDATE_MESSAGE.lock() = None;
                UPDATE_STATE.store(1, Ordering::Relaxed);
                eprintln!("press: installed update; restarting");
                true
            }
            Err(error) => {
                *UPDATE_MESSAGE.lock() = Some(error.to_string());
                UPDATE_STATE.store(2, Ordering::Relaxed);
                eprintln!("press: could not install update: {error}");
                false
            }
        },
        Ok(None) => false,
        Err(error) => {
            *UPDATE_MESSAGE.lock() = Some(error.to_string());
            UPDATE_STATE.store(2, Ordering::Relaxed);
            eprintln!("press: could not check for updates: {error}");
            false
        }
    }
}

#[cfg(target_os = "linux")]
fn appimage_path(appimage: Option<std::ffi::OsString>) -> std::io::Result<std::ffi::OsString> {
    appimage.ok_or_else(|| {
        std::io::Error::new(
            std::io::ErrorKind::NotFound,
            "the AppImage path is unavailable",
        )
    })
}

/// Start the newly installed package with the same launch arguments. Windows'
/// NSIS updater already exits and relaunches Press before installation returns.
pub fn relaunch() -> std::io::Result<()> {
    #[cfg(target_os = "linux")]
    let executable = appimage_path(std::env::var_os("APPIMAGE"))?;
    #[cfg(target_os = "macos")]
    let executable = std::env::current_exe()?;
    #[cfg(target_os = "windows")]
    unreachable!("the NSIS updater relaunches before returning");

    #[cfg(not(target_os = "windows"))]
    std::process::Command::new(executable)
        .args(std::env::args_os().skip(1))
        .env_remove("APPIMAGE")
        .env_remove("APPDIR")
        .spawn()
        .map(|_| ())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn release_endpoint_is_https() {
        assert!(ENDPOINT.starts_with("https://"));
    }

    #[test]
    fn embedded_public_key_is_ready_for_the_strict_base64_decoder() {
        assert!(!public_key().bytes().any(|byte| byte.is_ascii_whitespace()));
    }

    #[test]
    fn a_bundled_mac_path_is_updatable() {
        assert!(mac_bundle_path(std::path::Path::new(
            "/Applications/Press.app/Contents/MacOS/press"
        )));
    }

    #[test]
    fn a_target_release_binary_is_not_a_mac_bundle() {
        assert!(!mac_bundle_path(std::path::Path::new(
            "/home/user/repo/target/release/press"
        )));
    }

    #[test]
    fn a_bare_contents_macos_layout_without_an_app_is_refused() {
        assert!(!mac_bundle_path(std::path::Path::new(
            "/tmp/build/Contents/MacOS/press"
        )));
    }

    #[test]
    fn a_helper_nested_below_macos_is_refused() {
        assert!(!mac_bundle_path(std::path::Path::new(
            "/tmp/Foo.app/Contents/MacOS/helpers/press"
        )));
    }

    #[test]
    fn a_mac_bundle_path_that_can_escape_the_updaters_shell_quote_is_refused() {
        assert!(!mac_bundle_path(std::path::Path::new(
            "/Applications/Press'; touch pwned; '.app/Contents/MacOS/press"
        )));
        assert!(!mac_bundle_path(std::path::Path::new(
            "/Applications/Press\".app/Contents/MacOS/press"
        )));
        assert!(!updater_shell_path_is_safe(std::path::Path::new(
            "/tmp/Press' && touch pwned && '"
        )));
    }

    #[test]
    fn an_appimage_payload_under_its_appdir_is_updatable() {
        assert!(appimage_run(
            std::path::Path::new("/tmp/.mount_PresskQjHd/usr/bin/press"),
            true,
            Some(std::path::Path::new("/tmp/.mount_PresskQjHd")),
        ));
    }

    #[test]
    fn an_inherited_appimage_variable_does_not_match_a_loose_binary() {
        assert!(!appimage_run(
            std::path::Path::new("/home/user/repo/target/release/press"),
            true,
            Some(std::path::Path::new("/tmp/.mount_Other")),
        ));
        assert!(!appimage_run(
            std::path::Path::new("/home/user/repo/target/release/press"),
            true,
            None,
        ));
    }

    #[test]
    fn an_extracted_appimage_run_is_still_updatable() {
        assert!(appimage_run(
            std::path::Path::new("/tmp/appimage_extracted_1234/usr/bin/press"),
            true,
            Some(std::path::Path::new("/tmp/appimage_extracted_1234")),
        ));
    }

    #[cfg(target_os = "linux")]
    #[test]
    fn a_linux_relaunch_uses_the_updated_outer_appimage() {
        let path = std::ffi::OsString::from("/home/user/Applications/Press.AppImage");
        assert_eq!(
            appimage_path(Some(path.clone())).expect("the AppImage path is present"),
            path
        );
        assert!(appimage_path(None).is_err());
    }
}

#[cfg(test)]
mod notice_tests {
    use super::*;
    use std::sync::atomic::Ordering;

    // One test, not several: the slot is a process global, so parallel tests
    // would overwrite each other's state.
    #[test]
    fn the_notice_names_each_state() {
        UPDATE_STATE.store(0, Ordering::Relaxed);
        assert_eq!(notice(), None, "an idle updater is silent");

        UPDATE_STATE.store(1, Ordering::Relaxed);
        assert_eq!(
            notice(),
            Some("installed an update — restart to use it".to_string())
        );

        UPDATE_STATE.store(2, Ordering::Relaxed);
        *UPDATE_MESSAGE.lock() = Some("disk full".to_string());
        assert_eq!(notice(), Some("could not update: disk full".to_string()));

        UPDATE_STATE.store(0, Ordering::Relaxed);
        *UPDATE_MESSAGE.lock() = None;
    }
}
