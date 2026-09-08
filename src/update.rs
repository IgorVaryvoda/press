use cargo_packager_updater::{Config, check_update};

/// The repository was renamed from `imageguide-desktop` to `press`. Every copy
/// installed before that polls the old path, and GitHub redirects it here for
/// as long as nothing else claims the old name — so the old name must never be
/// used for another repository under this account.
const ENDPOINT: &str = "https://github.com/IgorVaryvoda/press/releases/latest/download/latest.json";

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

#[derive(Debug)]
pub enum Check {
    Available(Box<cargo_packager_updater::Update>),
    Current,
    Unsupported,
}

/// Only fetch release metadata. Download and installation require separate actions.
pub fn check() -> Result<Check, String> {
    let exe = std::env::current_exe().map_err(|error| error.to_string())?;
    if !updatable_install(&exe) {
        return Ok(Check::Unsupported);
    }
    let config = Config {
        endpoints: vec![ENDPOINT.parse().expect("the update URL is valid")],
        pubkey: public_key().into(),
        ..Default::default()
    };
    let version = env!("CARGO_PKG_VERSION")
        .parse()
        .expect("the package version is semver");
    check_update(version, config)
        .map(|update| update.map_or(Check::Current, |update| Check::Available(Box::new(update))))
        .map_err(|error| error.to_string())
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
pub(crate) mod tests {
    use super::*;

    pub const PAYLOAD: &[u8] = b"#!/bin/sh\nexit 0\n";

    pub fn fixture_update(
        url: &str,
        target: std::path::PathBuf,
    ) -> Box<cargo_packager_updater::Update> {
        Box::new(cargo_packager_updater::Update {
            config: Config { pubkey: "dW50cnVzdGVkIGNvbW1lbnQ6IG1pbmlzaWduIHB1YmxpYyBrZXk6IDQ1OTgwQTJBMDE5NDg2MTYKUldRV2hwUUJLZ3FZUmRiZjBvbG5Saldoa2lhaFVzRkNCNlE2NG9YK3UxQnhVNDdoQjlvTWllK0gK".into(), ..Default::default() },
            body: None,
            current_version: env!("CARGO_PKG_VERSION").into(),
            version: "9.9.9".into(),
            date: None,
            target: "linux-x86_64".into(),
            extract_path: target,
            download_url: url.parse().unwrap(),
            signature: "dW50cnVzdGVkIGNvbW1lbnQ6IHNpZ25hdHVyZSBmcm9tIGNhcmdvLXBhY2thZ2VyIHNlY3JldCBrZXkKUlVRV2hwUUJLZ3FZUlUwdTlOY3k4TURublgyZmUvcTVHTFRwRVI2ZjJTRVVmRU1tWGpnUmJsZm1Rd2dlYTRWaVV1Ry80ZXBvRFB6MDE1Sm1HVzdDRDlBMmlhbko0Y1pSUHdVPQp0cnVzdGVkIGNvbW1lbnQ6IHRpbWVzdGFtcDoxNzg4ODQ3MDQ5CWZpbGU6dXBkYXRlClMzNjYrMi9sZ3M3QmJhSTdibitvS2NncDF3ZndPY05iRnVxSDVHRGJKaEt5Qy92TStXRDJmcUNYR1V5cExPd093OXNGY2xCYTVmQm4vMjFmSWhsRkRBPT0K".into(),
            timeout: Some(std::time::Duration::from_secs(5)),
            headers: Default::default(),
            format: cargo_packager_updater::UpdateFormat::AppImage,
        })
    }

    #[test]
    fn release_endpoint_is_https() {
        assert!(ENDPOINT.starts_with("https://"));
    }

    #[test]
    fn embedded_public_key_is_ready_for_the_strict_base64_decoder() {
        assert!(!public_key().bytes().any(|byte| byte.is_ascii_whitespace()));
    }

    /// Just enough base64 for one small key file. The crate has no base64
    /// dependency and one test is not a reason to add one.
    fn decode_base64(text: &str) -> Vec<u8> {
        const ALPHABET: &[u8] = b"ABCDEFGHIJKLMNOPQRSTUVWXYZabcdefghijklmnopqrstuvwxyz0123456789+/";
        let mut bits = 0u32;
        let mut held = 0;
        let mut out = Vec::new();
        for byte in text
            .bytes()
            .filter(|byte| !byte.is_ascii_whitespace() && *byte != b'=')
        {
            let value = ALPHABET
                .iter()
                .position(|candidate| *candidate == byte)
                .expect("the key file is standard base64") as u32;
            bits = (bits << 6) | value;
            held += 6;
            if held >= 8 {
                held -= 8;
                out.push((bits >> held) as u8);
            }
        }
        out
    }

    /// `scripts/install.sh` verifies the AppImage against a copy of this key
    /// written out as a shell literal, because a shell script cannot
    /// `include_str!` anything. Nothing but this test keeps the two in step, and
    /// a stale copy makes the installer reject every genuine release.
    #[test]
    fn the_install_script_carries_the_embedded_public_key() {
        let manifest = std::path::Path::new(env!("CARGO_MANIFEST_DIR"));
        let encoded = std::fs::read_to_string(manifest.join("assets/updater.pub")).unwrap();
        let script = std::fs::read_to_string(manifest.join("scripts/install.sh")).unwrap();
        assert!(!encoded.trim().is_empty(), "assets/updater.pub is empty");
        assert!(!script.trim().is_empty(), "scripts/install.sh is empty");

        let decoded = String::from_utf8(decode_base64(&encoded)).unwrap();
        let key = decoded
            .lines()
            .nth(1)
            .expect("a minisign key file is a comment then the key");
        assert!(!key.is_empty(), "assets/updater.pub has no key on line 2");
        // `lines()` rather than a substring: a Windows checkout may carry CRLF.
        assert!(
            script.lines().any(|line| line == format!("pubkey={key}")),
            "scripts/install.sh does not carry the key {key}"
        );
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
