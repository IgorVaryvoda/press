# Plan 032: Show the updater's outcome in the window

> **Executor instructions**: Follow this plan step by step. Run every
> verification command and confirm the expected result before moving on. If a
> STOP condition occurs, stop and report; do not improvise. When done, update
> the status row for this plan in `plans/README.md`.

## Status

- **Priority**: P2
- **Effort**: S
- **Risk**: LOW — additive UI state behind the `updater` feature flag
- **Depends on**: none
- **Category**: UX
- **Planned at**: `origin/main` `8e5628f` (post 029/030 merge), 2026-08-22

## Why this matters

The updater is invisible to the person using the window.
`update::install_if_available` runs on a raw thread and reports only through
`eprintln!`. A packaged build that installs an update tells nobody: the user
keeps working, quits eventually, and the next launch is different with no
explanation. The app's own standard is truthful reporting — a conversion that
grew files says so, a failed listing names itself. "Your update is installed,
restart to use it" deserves the same treatment. This plan does not change
*when* updates install (auto-install stays; it is a signed updater and the
current design decision — see `plans/batch4-decisions.md`); it makes the
outcome visible.

## Current state

`src/update.rs` (36 lines total):

```rust
match check_update(version, config) {
    Ok(Some(update)) => match update.download_and_install() {
        Ok(()) => eprintln!("imageguide: installed update; restart to use it"),
        Err(error) => eprintln!("imageguide: could not install update: {error}"),
    },
    Ok(None) => {}
    Err(error) => eprintln!("imageguide: could not check for updates: {error}"),
}
```

Called from `run_window` (`src/main.rs:343-346`), guarded by
`#[cfg(feature = "updater")]`. The whole module compiles away without the
feature; everything this plan adds must stay behind the same `cfg`.

The window renders continuously while scans, thumbnails and estimates run, so
a plain process-global slot read from `Audit::notices` is enough — no
channels, no new dependencies, matching the house style of hand-rolled state.

## Commands you will need

| Purpose | Command | Expected |
|---------|---------|----------|
| Fast check (with feature) | `cargo check --locked --features updater` | exit 0 |
| Fast check (without) | `cargo check --locked` | exit 0 |
| Tests | `cargo test --locked` | all pass |
| Clippy | `cargo clippy --all-targets --features updater -- -D warnings` | exit 0 |
| Format | `cargo fmt --check` | exit 0 |

## Scope

**In scope**:
- `src/update.rs` (report outcome through a shared slot instead of only
  stderr)
- `src/main.rs` or `src/audit/view.rs` (one notice line; put the mapping
  helper where it compiles clean in both feature configurations)

**Out of scope**:
- Prompt-before-install, deferral, scheduling, or an update settings page
  (see `plans/batch4-decisions.md` for the consent question).
- The headless `--convert` path (no window exists; stderr already covers it).

## Git workflow

One commit: `feat: tell the window when an update was installed`.
Do not push unless asked.

## Steps

### Step 1: Give the module a readable outcome

In `src/update.rs`, add a process-global slot and report into it alongside
the existing stderr lines:

```rust
use std::sync::atomic::{AtomicU8, Ordering};

/// What the last update attempt did, for the window to show.
/// 0 = nothing attempted yet, 1 = installed, restart pending,
/// 2 = install or check failed (message in UPDATE_MESSAGE).
static UPDATE_STATE: AtomicU8 = AtomicU8::new(0);
static UPDATE_MESSAGE: parking_lot::Mutex<Option<String>> =
    parking_lot::Mutex::new(None);

pub fn update_state() -> u8 {
    UPDATE_STATE.load(Ordering::Relaxed)
}

/// One line, only meaningful when `update_state()` is nonzero.
pub fn update_message() -> Option<String> {
    UPDATE_MESSAGE.lock().clone()
}
```

Set state `1` on the success arm, `2` with the error string on both failure
arms. Keep every existing `eprintln!` — headless and CI still read them.
`parking_lot::Mutex::new` in a static is const-callable; this matches the
house rule against `lock().unwrap()`.

### Step 2: Render it once, quietly

Add one mapping helper and append it in `Audit::notices`
(`src/audit/view.rs:1535`) as one more `parts.push(...)` so it inherits the
existing single-line treatment:

```rust
fn update_notice() -> Option<String> {
    #[cfg(feature = "updater")]
    match crate::update::update_state() {
        1 => Some("installed an update — restart to use it".to_string()),
        2 => crate::update::update_message()
            .map(|message| format!("could not update: {message}")),
        _ => None,
    }
    #[cfg(not(feature = "updater"))]
    None
}
```

Wording follows the house voice: what happened, then what to do. Do not add a
modal, badge, or button; a restart affordance can be its own plan if the
maintainer wants one.

Note on polling: the updater thread finishes within seconds of launch while
the window keeps rendering during scans/thumbnails. If testing shows the
notice can appear after the last render has settled, add a one-shot timer in
`run_window` that wakes the window once via `cx.notify()` after ~5 s —
measure first, add only if needed.

### Step 3: Feature-flag discipline

Run both checks from the command table. The non-feature build must compile
cleanly: the new call sites are `#[cfg]`-branched, and `crate::update` does
not exist without the feature.

### Step 4: Gates

```bash
cargo test --locked
cargo clippy --all-targets --features updater -- -D warnings
cargo fmt --check
```

Live proof (this host): build `--features updater`, launch the release
binary, and confirm the normal path renders unchanged (state 0). Forcing a
real update download in a test run is out of scope; unit-test
`update_notice`'s mapping by exercising states through the atomic directly:

```rust
#[test]
fn installed_update_reads_as_a_restart_line() {
    // set state 1, assert the mapped string, reset
}
```

## Done criteria

- [ ] With the feature off: binary identical in behaviour, clippy clean.
- [ ] With the feature on and no update pending: no new UI anywhere.
- [ ] State 1 renders "installed an update — restart to use it" in the
      notices row; state 2 renders the failure reason.
- [ ] Stderr output unchanged.
- [ ] Suite, clippy (both feature configurations), fmt green.

## STOP conditions

- `cargo-packager-updater`'s API changes shape under a lock-file refresh such
  that outcomes cannot be classified into installed/failed — report.
- The notice cannot be shown without a modal given gpui-component's current
  primitives — report rather than inventing a floating overlay.
