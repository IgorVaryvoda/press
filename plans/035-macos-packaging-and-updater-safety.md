# Plan 035: Explain macOS permission prompts, and stop the updater deleting loose builds

> **Executor instructions**: Follow this plan step by step. Run every
> verification command and confirm the expected result before moving to the
> next step. If anything in the "STOP conditions" section occurs, stop and
> report — do not improvise. When done, update the status row for this plan
> in `plans/README.md` — unless a reviewer dispatched you and told you they
> maintain the index.
>
> **Drift check (run first)**: `git diff --stat 97cb1a5..HEAD -- Cargo.toml src/update.rs .github/workflows/ci.yml`
> If any in-scope file changed since this plan was written, compare the
> "Current state" excerpts against the live code before proceeding; on a
> mismatch, treat it as a STOP condition.

## Status

- **Priority**: P1
- **Effort**: S
- **Risk**: LOW
- **Depends on**: none
- **Category**: bug / dx
- **Planned at**: commit `97cb1a5`, 2026-08-23

## Why this matters

Three packaging-level gaps make the macOS app feel broken. First, the
generated `Info.plist` has no usage-description strings, so when the app
walks `~/Documents` or `~/Desktop` (its whole purpose) macOS shows a bare
permission prompt with no explanation — users deny it, and the app then just
says "could not enter". Second, `cargo-packager-updater` resolves the
"installed app" from the executable path: outside a `.app` bundle it returns
the executable's **parent directory** and, on update, runs
`fs::remove_dir_all` on it — so a `target/release/imageguide` built with
`--features updater` would delete `target/release/`. Third, nothing in CI
compiles the `updater` feature at all: the module that replaces the user's
installed app is first compiled on a tag push.

## Current state

- `Cargo.toml` — packager metadata ends with:

```toml
[package.metadata.packager.macos]
# The env var override did not take, so the real identity lives here. Ad-hoc
# local packaging can still pass `--signing-identity -` on the command line.
signing-identity = "Developer ID Application: Igor Varyvoda (2Z5ZVRKA23)"
```

No `info-plist-path`, no `minimum-system-version`. There is no `.plist` file
anywhere in the repo (`find . -name '*.plist' -not -path './target/*'` → nothing).
cargo-packager 0.11.8 merges a user plist over its generated one when
`info-plist-path` is set (its `src/package/app/mod.rs:346-356`), and both
config keys exist in its `src/config/mod.rs`.

- `src/update.rs:33-44` — the updater runs on a detached thread from
  `run_window` with no environment guard:

```rust
pub fn install_if_available() {
    std::thread::spawn(|| {
        let config = Config {
            endpoints: vec![ENDPOINT.parse().expect("the update URL is valid")],
            pubkey: include_str!("../assets/updater.pub").into(),
            ..Default::default()
        };
        ...
        match check_update(version, config) {
            Ok(Some(update)) => match update.download_and_install() {
```

  In `cargo-packager-updater-0.2.3/src/lib.rs` (vendored under
  `~/.cargo/registry/src/*/cargo-packager-updater-0.2.3/`),
  `extract_path_from_executable` returns the `.app` bundle only when the
  executable path contains `Contents/MacOS`; otherwise it returns the
  executable's parent directory, and install does
  `fs::remove_dir_all(&self.extract_path)` in the non-privileged branch. On
  macOS, when the rename hits `PermissionDenied`, it escalates through
  `osascript ... with administrator privileges` — an admin-password dialog at
  launch with no context.

- `.github/workflows/ci.yml:47-55` — the gates:

```yaml
      - name: Build
        run: cargo build --release --locked
      - name: Test
        run: cargo test --locked
      - name: Clippy
        run: cargo clippy --all-targets -- -D warnings
```

  `grep -n updater .github/workflows/ci.yml` → no matches. The only build of
  the feature is `before-packaging-command = "cargo build --release --locked
  --features updater"` in `Cargo.toml`, which runs on release runners.

- `plans/batch4-decisions.md`, memo **B** — auto-install-without-asking is a
  recorded maintainer decision: keep it while pre-alpha. This plan does NOT
  change auto-install; it only guards where installing is destructive
  nonsense (loose binaries) and records the new admin-prompt evidence in the
  memo.

- Repo conventions: comments are short *why* paragraphs; tests are snake_case
  behaviour sentences (`release_endpoint_is_https` in `src/update.rs:72` is
  the local example).

## Commands you will need

| Purpose | Command | Expected on success |
|---|---|---|
| Tests (with feature) | `cargo test --locked --features updater` | all pass, incl. new guard tests |
| Lint (with feature) | `cargo clippy --all-targets --features updater -- -D warnings` | exit 0 |
| Format | `cargo fmt --check` | exit 0 |
| Plist sanity | `python3 -c "import plistlib,sys;plistlib.load(open('assets/macos/Info.plist','rb'))"` | exit 0, no output |

## Scope

**In scope** (the only files you should modify):
- `assets/macos/Info.plist` (create)
- `Cargo.toml` — one line in `[package.metadata.packager.macos]`
- `src/update.rs` — installed-location guard + tests
- `.github/workflows/ci.yml` — feature flags on existing gate lines
- `plans/batch4-decisions.md` — append evidence to memo B
- `plans/README.md` — status row

**Out of scope** (do NOT touch):
- Auto-install behavior itself (memo B owns that decision).
- `.github/workflows/release.yml` — signing/notarization works; do not
  refactor it in passing. (The CI/release vcpkg divergence is a separate
  recorded finding, MACOS-10.)
- `assets/updater.pub`, the update endpoint, or the manifest format.

## Git workflow

- Branch: `improve/035-macos-packaging-updater-safety`
- Conventional commits, e.g. `fix: guard the updater against loose binaries`
- Do NOT push or open a PR unless the operator instructed it.

## Steps

### Step 1: Ship an Info.plist supplement

Create `assets/macos/Info.plist`:

```xml
<?xml version="1.0" encoding="UTF-8"?>
<!DOCTYPE plist PUBLIC "-//Apple//DTD PLIST 1.0//EN" "http://www.apple.com/DTDs/PropertyList-1.0.dtd">
<plist version="1.0">
<dict>
    <key>NSDesktopFolderUsageDescription</key>
    <string>ImageGuide audits and converts images in folders you choose, including folders on your Desktop.</string>
    <key>NSDocumentsFolderUsageDescription</key>
    <string>ImageGuide audits and converts images in folders you choose, including folders in your Documents.</string>
    <key>NSDownloadsFolderUsageDescription</key>
    <string>ImageGuide audits and converts images in folders you choose, including folders in your Downloads.</string>
    <key>NSRemovableVolumesUsageDescription</key>
    <string>ImageGuide audits and converts images in folders you choose, including folders on external drives.</string>
    <key>NSNetworkVolumesUsageDescription</key>
    <string>ImageGuide audits and converts images in folders you choose, including folders on network drives.</string>
    <key>NSHumanReadableCopyright</key>
    <string>© Igor Varyvoda</string>
</dict>
</plist>
```

In `Cargo.toml`, extend `[package.metadata.packager.macos]`:

```toml
info-plist-path = "assets/macos/Info.plist"
```

Do NOT add `minimum-system-version`. Advertising a compatibility floor in the
plist without plumbing `MACOSX_DEPLOYMENT_TARGET` through rustc, the C
bridge, and vcpkg would be metadata the binaries do not honor; that pairing
is deferred (see Maintenance notes).

Then confirm the key name against the *vendored* packager this repo pins:
in `~/.cargo/registry/src/*/cargo-packager-0.11.8/src/config/mod.rs`, find
the macOS config struct and verify the serde rename for the Info.plist path
field matches `info-plist-path`, and that
`src/package/app/mod.rs` (~line 346-356) merges the user plist over the
generated one. Cargo treats packager metadata as arbitrary TOML — a typo'd
key is silently ignored, so this read IS the verification that the field
lands. Quote the two locations in your report.

**Verify**: the plist sanity command from the table → exit 0;
`grep -rn "info.plist" ~/.cargo/registry/src/*/cargo-packager-0.11.8/src/config/mod.rs`
→ shows the field and its rename. Then `cargo test --locked` → all pass
(metadata changes must not affect the build).

### Step 2: Guard the updater to installed locations

In `src/update.rs`, add a pure predicate and gate `install_if_available` on it:

```rust
/// True when the executable path has the shape of an installed macOS app
/// bundle. The bar is `.app/Contents/MacOS/`, not just `Contents/MacOS/`:
/// cargo-packager-updater resolves the "installed app" from this path, and
/// outside a real bundle that resolution is the executable's parent
/// directory, which an update then `remove_dir_all`s — for a
/// `target/release/imageguide` that deletes the build tree.
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
}

/// True when this process is actually the mounted AppImage payload. The
/// updater replaces the file APPIMAGE names, so an inherited variable from
/// some other application's environment must not be trusted on its own: a
/// loose binary with a stale APPIMAGE would overwrite that other app. The
/// AppImage runtime mounts its payload under `/tmp/.mount_*`, so require
/// the executable to live in such a mount as well.
fn appimage_run(exe: &std::path::Path, appimage_set: bool) -> bool {
    appimage_set && exe.to_string_lossy().contains("/.mount_")
}

fn updatable_install(exe: &std::path::Path) -> bool {
    if cfg!(target_os = "macos") {
        mac_bundle_path(exe)
    } else if cfg!(target_os = "linux") {
        appimage_run(exe, std::env::var_os("APPIMAGE").is_some())
    } else {
        // Windows installs via NSIS, which the updater handles through the
        // installer rather than by moving directories.
        true
    }
}
```

(Both helpers are pure functions of their inputs so they are testable on any
host; only `updatable_install` reads the environment.)

At the top of `install_if_available`, before spawning the thread:

```rust
let Ok(exe) = std::env::current_exe() else { return };
if !updatable_install(&exe) {
    return;
}
```

**Verify**: `cargo test --locked --features updater` → all pass.

### Step 3: Tests for the guard

In `src/update.rs`'s existing `#[cfg(test)] mod tests`, add (names must state
behaviour, matching the file's style; both helpers are pure, so all of
these run on any host):

- `a_bundled_mac_path_is_updatable` —
  `/Applications/ImageGuide.app/Contents/MacOS/imageguide` → `true`.
- `a_target_release_binary_is_not_a_mac_bundle` —
  `/home/user/repo/target/release/imageguide` → `false`.
- `a_bare_contents_macos_layout_without_an_app_is_refused` —
  `/tmp/build/Contents/MacOS/imageguide` → `false` (the `.app` requirement
  is what rejects bundle-shaped loose dirs).
- `a_helper_nested_below_macos_is_refused` —
  `/tmp/Foo.app/Contents/MacOS/helpers/imageguide` → `false` (the updater
  walks up exactly two parents, so anything deeper would target
  `Foo.app/Contents` for deletion).
- `an_inherited_appimage_variable_alone_is_not_an_appimage_run` —
  `appimage_run(Path::new("/home/user/repo/target/release/imageguide"), true)`
  → `false`; `appimage_run(Path::new("/tmp/.mount_ImageGkQjHd/usr/bin/imageguide"), true)`
  → `true`; same path with `false` → `false`.

**Verify**: `cargo test --locked --features updater` → all pass, including
the five new tests.

### Step 4: Compile the feature in CI

In `.github/workflows/ci.yml`, change the two gate lines:

```yaml
      - name: Test
        run: cargo test --locked --features updater
      - name: Clippy
        run: cargo clippy --all-targets --features updater -- -D warnings
```

Leave the `Build` step as is (release builds without the feature are still a
shipped configuration for `cargo run` users). Scope honesty: this makes CI
compile the updater feature on Linux and macOS. The updater's
`#[cfg(windows)]` installer code still first-compiles on a release run,
because CI has no Windows job — out of scope here; note it in your report.

**Verify**: `cargo test --locked --features updater` and
`cargo clippy --all-targets --features updater -- -D warnings` locally → both
exit 0 (these are exactly what CI will run).

### Step 5: Record the admin-prompt evidence in memo B

Append to the "## B — Auto-install updates without asking" section of
`plans/batch4-decisions.md`:

```markdown
**New evidence (2026-08-23)**: on macOS, when replacing the `.app` hits
`PermissionDenied` (admin-owned /Applications, managed Macs),
cargo-packager-updater 0.2.3 escalates via
`osascript … with administrator privileges` — an unexplained admin-password
dialog during startup (the check runs on a detached thread, racing window
creation, so it can appear before, beside, or after the first frame). That
is a materially worse failure mode than the memo weighed. Raise the
priority of "ask-first" when revisiting; plan 035 only stops non-bundle
installs.
```

**Verify**: `git diff --stat plans/batch4-decisions.md` → one file changed.

### Step 6: Full gates

**Verify**:
- `cargo test --locked` and `cargo test --locked --features updater` → all pass
- `cargo clippy --all-targets --features updater -- -D warnings` → exit 0
- `cargo fmt --check` → exit 0

## Test plan

- Five pure tests on the install-location guards (Step 3), in
  `src/update.rs`'s existing tests module, modeled on
  `release_endpoint_is_https`.
- The plist is validated structurally by `plistlib` (Step 1). Its runtime
  effect (prompt wording) is only checkable on a Mac; note that in your report.

## Done criteria

- [ ] `assets/macos/Info.plist` exists and parses with `plistlib`
- [ ] `grep -n "info-plist-path" Cargo.toml` → one match
- [ ] `grep -n "features updater" .github/workflows/ci.yml` → two matches
- [ ] `cargo test --locked --features updater` exits 0 with the five new tests
- [ ] `cargo clippy --all-targets --features updater -- -D warnings` exits 0
- [ ] `cargo fmt --check` exits 0
- [ ] `git status` (in your worktree) shows changes only in the in-scope files
- [ ] `plans/README.md` status row updated

## STOP conditions

Stop and report back (do not improvise) if:

- cargo-packager 0.11.8's config does not accept `info-plist-path` under
  `[package.metadata.packager.macos]`, or its plist-merge code path is not
  where "Current state" says (check the vendored `src/config/mod.rs` serde
  renames and `src/package/app/mod.rs` merge before editing `Cargo.toml`).
- `cargo test --locked --features updater` fails at baseline, before your
  changes — the feature may already be broken; report, do not fix blind.
- You find yourself wanting to change auto-install behavior — that is memo
  B's decision, not this plan's.

## Maintenance notes

- `minimum-system-version` was deliberately NOT set (scrutiny round 1):
  the plist key is compatibility *metadata*, and nothing currently pins
  `MACOSX_DEPLOYMENT_TARGET` for rustc, the C bridge, or the vcpkg-built
  libavif/libaom — advertising a floor the binaries do not honor is worse
  than advertising none. Setting it is a one-line follow-up once a
  deployment target is established end to end and verified on that OS.
- Release-gate note for the maintainer: after the next tagged macOS build,
  verify the merge actually shipped —
  `plutil -p ImageGuide.app/Contents/Info.plist | grep -c UsageDescription`
  → 4. That is the artifact-level check no Linux gate can run.
- The bundle guard knowingly accepts DMG-run and Gatekeeper-translocated
  apps (no supported detector exists); their update failure/escalation
  behavior is unchanged and belongs to decision memo B.
- Deferred deliberately: `MACOSX_DEPLOYMENT_TARGET` plumbing through CI and
  vcpkg (MACOS-11); file associations + `on_open_urls` (MACOS-06);
  mirroring the release vcpkg setup into CI (MACOS-10).
- If the updater dependency is ever bumped, re-read its
  `extract_path_from_executable` — the guard in Step 2 encodes its 0.2.3
  behavior.
