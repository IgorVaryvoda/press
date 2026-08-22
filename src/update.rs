use cargo_packager_updater::{Config, check_update};
use std::sync::atomic::{AtomicU8, Ordering};

const ENDPOINT: &str =
    "https://github.com/IgorVaryvoda/imageguide-desktop/releases/latest/download/latest.json";

/// What the last update attempt did, for the window to show.
/// 0 = nothing attempted yet, 1 = installed, restart pending,
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

pub fn install_if_available() {
    std::thread::spawn(|| {
        let config = Config {
            endpoints: vec![ENDPOINT.parse().expect("the update URL is valid")],
            pubkey: include_str!("../assets/updater.pub").into(),
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
                    eprintln!("imageguide: installed update; restart to use it");
                }
                Err(error) => {
                    *UPDATE_MESSAGE.lock() = Some(error.to_string());
                    UPDATE_STATE.store(2, Ordering::Relaxed);
                    eprintln!("imageguide: could not install update: {error}");
                }
            },
            Ok(None) => {}
            Err(error) => {
                *UPDATE_MESSAGE.lock() = Some(error.to_string());
                UPDATE_STATE.store(2, Ordering::Relaxed);
                eprintln!("imageguide: could not check for updates: {error}");
            }
        }
    });
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn release_endpoint_is_https() {
        assert!(ENDPOINT.starts_with("https://"));
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
