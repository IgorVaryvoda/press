# Plan 034: Give the macOS app a menu bar, Cmd+Q, and a sane exit

> **Executor instructions**: Follow this plan step by step. Run every
> verification command and confirm the expected result before moving to the
> next step. If anything in the "STOP conditions" section occurs, stop and
> report — do not improvise. When done, update the status row for this plan
> in `plans/README.md` — unless a reviewer dispatched you and told you they
> maintain the index.
>
> **Drift check (run first)**: `git diff --stat 97cb1a5..HEAD -- src/main.rs src/audit/mod.rs src/audit/state.rs`
> If any in-scope file changed since this plan was written, compare the
> "Current state" excerpts against the live code before proceeding; on a
> mismatch, treat it as a STOP condition.

## Status

- **Priority**: P1
- **Effort**: M
- **Risk**: LOW
- **Depends on**: none
- **Category**: bug (platform integration)
- **Planned at**: commit `97cb1a5`, 2026-08-23

## Why this matters

On macOS, GPUI gives an application no menu bar and no keyboard shortcuts
unless the app registers them. This app registers none, so Cmd+Q, Cmd+W,
Cmd+H, Cmd+M and Cmd+O all do nothing. Worse: GPUI's default quit mode on
macOS is "keep running when the last window closes", and the app registers no
`on_window_closed` or `on_reopen` handler. Closing the window therefore
leaves a headless process with a Dock icon, no window, no menu bar, and no
way back in — Force Quit is the only exit. This is the single biggest
"not a native macOS app" complaint, and the fix is a contained block of
registration calls in `run_window`.

## Current state

- `src/main.rs:350-387` — `run_window` is the whole app lifecycle:

```rust
fn run_window(launch: Launch) {
    #[cfg(feature = "updater")]
    update::install_if_available();

    application()
        .with_assets(gpui_component_assets::Assets)
        .run(move |cx: &mut App| {
            init_theme(cx);
            // ... thumbnail trim, settings load ...
            cx.open_window(
                WindowOptions {
                    window_bounds: Some(WindowBounds::Windowed(bounds)),
                    window_min_size: Some(size(px(WINDOW_MIN_WIDTH), px(WINDOW_MIN_HEIGHT))),
                    app_id: Some("imageguide".to_string()),
                    ..Default::default()
                },
                |window, cx| {
                    let audit = audit::build_audit(launch, window, cx);
                    cx.new(|cx| Root::new(audit, window, cx).bg(cx.theme().background))
                },
            )
            .unwrap();
            cx.activate(true);
        });
}
```

No `set_menus`, no `bind_keys`, no `set_quit_mode` anywhere in `src/`
(`grep -rn "set_menus\|bind_keys\|set_quit_mode" src/` returns nothing).

- `src/audit/mod.rs:589` (approximately) — `fn pick(&mut self, folders: bool, cx: &mut Context<Self>)`
  is the existing "ask the desktop for a folder or a file" entry point. It is
  private (`fn`, no visibility modifier). The toolbar already calls it; the
  File menu will too.

- The pinned GPUI checkout is `~/.cargo/git/checkouts/zed-a70e2ad075855582/8bbbeb3`
  (rev in `Cargo.lock`). Every API this plan uses was verified present there:
  - `App::set_menus` (`crates/gpui/src/app.rs:2400`), `App::bind_keys` (`:2215`),
    `App::on_action` (`:2234`), `App::set_quit_mode` (`:1628`), `QuitMode` enum (`:324`),
    `App::quit` (`:1014`), `App::hide` (`:1289`), `App::hide_other_apps` (`:1294`),
    `App::unhide_other_apps` (`:1299`), `App::on_app_quit` (`:2325`).
  - `Menu` has THREE public fields (`name`, `items`, `disabled`) and a
    builder: `Menu::new(name).items(vec![...])`
    (`crates/gpui/src/platform/app_menu.rs:4-30`). Use the builder, never a
    struct literal — a literal that omits `disabled` does not compile, and
    this module's macOS-only compile risk must stay zero.
  - `MenuItem::action`, `MenuItem::separator` (`crates/gpui/src/platform/app_menu.rs`).
  - `Window::remove_window` (`crates/gpui/src/window.rs:2025`),
    `Window::minimize_window` (`:5719`), `Window::zoom_window` (`:2530`).
    All of these, and every other API named here, are cross-platform `gpui`
    items — nothing in the new module needs a macOS-only API, which is what
    lets the Linux build type-check all of it (see Step 3).
  - `gpui::actions!` macro (`crates/gpui/src/action.rs`).
  - `QuitMode::Default` resolves to `Explicit` on macOS and `LastWindowClosed`
    elsewhere (`crates/gpui/src/app.rs:1885-1893`) — that is why only macOS
    keeps a zombie process.
  - Deliberately NOT used: `App::set_app_identity`. The platform default is
    an empty no-op (`crates/gpui/src/platform.rs:253-261`) and the macOS
    platform does not override it — only Windows does (AppUserModelID). On
    macOS, identity comes solely from the packaged bundle; calling it would
    change Windows behavior while doing nothing for the problem.

- Settings persistence is debounced and can be lost on a fast exit:
  `Audit::remember_settings` (`src/audit/state.rs:8-31`) sets
  `settings_save_pending`, waits `SETTINGS_SAVE_DELAY` (500 ms,
  `src/audit/mod.rs:91`), then writes via the module-level
  `fn write_settings(settings: &settings::Settings)` (`src/audit/mod.rs:444`).
  A quit inside that window silently drops the last resize/folder change.
  This already bites the Linux close path today; the new Cmd+Q/Cmd+W paths
  would widen it, so this plan adds a quit-time flush (Step 5).

- Repo conventions: comments explain *why* as short paragraphs (see the
  comments in `run_window` itself). Rust 2024, `cargo fmt`, clippy warnings
  are errors. Commit style: `fix:`/`feat:` conventional, one concern per commit.

## Commands you will need

| Purpose | Command | Expected on success |
|---|---|---|
| Build | `cargo build --release --locked` | exit 0 |
| Tests | `cargo test --locked` | all pass (87 passed + 1 ignored at baseline) |
| Lint | `cargo clippy --all-targets -- -D warnings` | exit 0 |
| Format | `cargo fmt --check` | exit 0 |

Note: this host is Linux. The new `menus.rs` deliberately uses only
cross-platform gpui APIs and is NOT cfg-gated, so the Linux build fully
type-checks it (Step 3); only its *invocation* is macOS-conditional via a
runtime `cfg!` (Step 4). Menu *behavior* still needs a real Mac; CI's
`macos-latest` job is the macOS compile gate.

## Scope

**In scope** (the only files you should modify):
- `src/main.rs` — quit mode, menu module hookup, quit-flush registration
- `src/menus.rs` (create) — menu bar, actions, key bindings (macOS-run, all-platform-compiled)
- `src/audit/mod.rs` — `pick` visibility, `register_quit_flush`
- `src/audit/state.rs` — `flush_settings`
- `src/audit/tests.rs` — flush test
- `plans/README.md` — status row

**Out of scope** (do NOT touch):
- `src/audit/view.rs`, `src/audit/toolbar.rs` — the in-window UI already has
  its own pick buttons; do not rewire them.
- `cx.activate(true)` at `src/main.rs:385` — leave it as is. Changing focus
  behavior was considered and deliberately deferred (finding MACOS-13).
- Any Edit menu / clipboard menu items. Text inputs already get Cmd+C/V/X/A
  from `gpui_component::init`; menu duplication of those is deferred.
- Linux/Windows key bindings (e.g. Ctrl+O) — deferred, noted in maintenance.

## Git workflow

- Branch: `improve/034-macos-menu-and-quit`
- Conventional commits, e.g. `feat: give macOS a menu bar and quit shortcuts`
- Do NOT push or open a PR unless the operator instructed it.

## Steps

### Step 1: Make `pick` callable from outside the audit module

In `src/audit/mod.rs`, change `fn pick(` to `pub(crate) fn pick(`.

**Verify**: `cargo build --release --locked` → exit 0.

### Step 2: Quit when the last window closes

In `run_window`'s `.run(...)` closure in `src/main.rs`, immediately after
`init_theme(cx);`, add:

```rust
// GPUI's macOS default keeps the process alive after the last window
// closes, which suits a document-based app with a menu bar to come back
// through. This is a single-window tool: the red light means quit.
cx.set_quit_mode(gpui::QuitMode::LastWindowClosed);
```

Do NOT call `set_app_identity` — on this pin it is a no-op on macOS and
would change Windows taskbar identity instead (see "Current state").

**Verify**: `cargo build --release --locked` → exit 0.

### Step 3: Create `src/menus.rs`

Add `mod menus;` to the module list at the top of `src/main.rs` — **without**
a `#[cfg]` gate. Every API the module uses is cross-platform `gpui`, so the
Linux build type-checks the whole module; that is this plan's only local
compile gate for code whose *behavior* is macOS-only. (The call site in
Step 4 uses `cfg!(...)`, a compiled runtime constant, precisely so the module
never becomes dead code on Linux — `#[cfg]` there would strip the call and
clippy's `-D warnings` would flag the module.)

Create `src/menus.rs` with this content shape (adjust only if a GPUI
signature demands it — and record any such adjustment in your report):

```rust
//! The macOS menu bar. Without one, AppKit has no key equivalent for Quit,
//! Close, Hide or Minimize, and the app cannot be quit from the keyboard.

use gpui::{App, Entity, KeyBinding, Menu, MenuItem, actions};

use crate::audit::Audit;

actions!(
    imageguide,
    [Quit, HideSelf, HideOthers, ShowAll, CloseWindow, Minimize, Zoom, OpenFolder, OpenImage]
);

pub fn init(audit: Entity<Audit>, cx: &mut App) {
    cx.on_action(|_: &Quit, cx| cx.quit());
    cx.on_action(|_: &HideSelf, cx| cx.hide());
    cx.on_action(|_: &HideOthers, cx| cx.hide_other_apps());
    cx.on_action(|_: &ShowAll, cx| cx.unhide_other_apps());
    cx.on_action(|_: &CloseWindow, cx| {
        if let Some(window) = cx.active_window() {
            window.update(cx, |_, window, _| window.remove_window()).ok();
        }
    });
    cx.on_action(|_: &Minimize, cx| {
        if let Some(window) = cx.active_window() {
            window.update(cx, |_, window, _| window.minimize_window()).ok();
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
        Menu::new("ImageGuide").items(vec![
            MenuItem::action("Hide ImageGuide", HideSelf),
            MenuItem::action("Hide Others", HideOthers),
            MenuItem::action("Show All", ShowAll),
            MenuItem::separator(),
            MenuItem::action("Quit ImageGuide", Quit),
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
    ]);
}
```

Use the `Menu::new(...).items(...)` builder exactly as shown — `Menu` has a
third public field (`disabled`) that a struct literal must not forget, and
the builder cannot. Check `MenuItem::action`'s signature in
`~/.cargo/git/checkouts/zed-a70e2ad075855582/8bbbeb3/crates/gpui/src/platform/app_menu.rs`
before writing (it takes the action by value). If the shapes differ
materially, STOP.

**Verify**: `cargo build --release --locked` → exit 0. Because the module is
NOT cfg-gated, this compile **fully type-checks `src/menus.rs`** on Linux —
treat any error here as a real error in the module, never as "macOS-only
code, expected to fail".

### Step 4: Wire the menus into `run_window`

`build_audit` returns the `Entity<Audit>` inside the `open_window` closure;
capture it so `menus::init` can be called after the window exists. In
`run_window`, replace the `cx.open_window(...)` call with:

```rust
let mut audit_slot = None;
cx.open_window(
    WindowOptions { /* unchanged */ },
    |window, cx| {
        let audit = audit::build_audit(launch, window, cx);
        audit_slot = Some(audit.clone());
        cx.new(|cx| Root::new(audit, window, cx).bg(cx.theme().background))
    },
)
.unwrap();
if let Some(audit) = audit_slot {
    // cfg! keeps the call compiled (and the module alive) on every
    // platform while running it only where a menu bar exists.
    if cfg!(target_os = "macos") {
        menus::init(audit.clone(), cx);
    }
    audit::register_quit_flush(audit, cx); // Step 5
}
cx.activate(true);
```

The `WindowOptions` block and everything before it stay byte-identical.

**Verify**: `cargo build --release --locked` → exit 0.

### Step 5: Flush pending settings on quit

Settings writes are debounced by 500 ms (see "Current state"); Cmd+Q,
Cmd+W-then-quit, and the existing Linux window-close all race that timer and
can drop the last resize or folder change. Fix it once at the root with
GPUI's quit hook, which every quit path runs through.

- In `src/audit/state.rs`, next to `remember_settings`, add:

```rust
    /// Write the settings now, cancelling the debounce. Every quit path
    /// runs through this; without it, quitting inside the 500 ms window
    /// silently forgets the last resize or folder change.
    pub(crate) fn flush_settings(&mut self) {
        self.settings_save_pending = false;
        write_settings(&self.settings);
    }
```

  (`write_settings` is the module-level fn at `src/audit/mod.rs:444`; it is
  already visible from `state.rs` via `use super::*`.)

- In `src/audit/mod.rs`, add a small registration helper (place it near
  `build_audit`, matching its visibility style):

```rust
/// Flush the debounced settings write when the app quits, whatever path
/// the quit took — menu, Cmd+W on the last window, or the close button.
pub(crate) fn register_quit_flush(audit: gpui::Entity<Audit>, cx: &mut gpui::App) {
    cx.on_app_quit(move |cx| {
        audit.update(cx, |audit, _| audit.flush_settings()).ok();
        async {}
    })
    .detach();
}
```

  Check `on_app_quit`'s exact signature at
  `~/.cargo/git/checkouts/zed-a70e2ad075855582/8bbbeb3/crates/gpui/src/app.rs:2325`
  before writing: it takes a callback returning a `Future` and returns a
  `Subscription` (hence `.detach()`); the callback parameter type and whether
  `Entity::update` there returns `Result` may differ slightly — adapt the
  body, keep the behavior: synchronous `flush_settings` before the future
  resolves. If `on_app_quit` does not run the callback before process exit in
  a way that lets the write complete (the write is synchronous file I/O, so
  it completes inside the callback), STOP and report.

**Verify**: `cargo test --locked` → all pass, plus the new test from the
Test plan below.

### Step 6: Full gates

**Verify**:
- `cargo test --locked` → all pass, same count as baseline (87 passed + 1 ignored before this plan).
- `cargo clippy --all-targets -- -D warnings` → exit 0.
- `cargo fmt --check` → exit 0.

## Test plan

- One new test in `src/audit/tests.rs`:
  `flushing_settings_clears_the_pending_debounce` — build an audit with the
  existing harness, call `remember_settings` with a changed size (this sets
  `settings_save_pending`), then call `flush_settings()` and assert
  `settings_save_pending` is false. Do NOT assert on the settings file:
  `write_settings` is deliberately a no-op under `#[cfg(test)]`
  (`src/audit/mod.rs:442-448` — "a render must not touch the user's real
  config file"), and `settings::path()` has no test override. The disk-write
  line inside `flush_settings` is one call to the same `write_settings` the
  debounced path already uses; the reviewer verifies it by reading, and the
  real-exit behavior belongs to the macOS/manual acceptance list below.
- Menu/shortcut behavior is macOS-native windowing that neither the Linux
  host nor the ignored screenshot harness can exercise; the Linux build now
  type-checks all of `src/menus.rs` (Step 3), and CI's `macos-latest` job is
  the macOS compile gate. Manual acceptance (maintainer, on a Mac): menu bar
  shows ImageGuide/File/Window; Cmd+Q quits; Cmd+W closes the window AND the
  process exits; Cmd+O opens the folder picker; Cmd+H hides; Cmd+M
  minimizes; a resize followed immediately by Cmd+Q restores at the new size
  on next launch.

## Done criteria

- [ ] `cargo test --locked` exits 0, including the new flush test
- [ ] `cargo clippy --all-targets -- -D warnings` exits 0 (proves `menus.rs` compiles on Linux — it is not cfg-gated)
- [ ] `cargo fmt --check` exits 0
- [ ] `grep -n "set_quit_mode" src/main.rs` → one match
- [ ] `grep -rn "set_app_identity" src/` → no matches
- [ ] `grep -n "set_menus" src/menus.rs` → one match
- [ ] `grep -n "cfg!(target_os" src/main.rs` → the menus call site (runtime `cfg!`, not `#[cfg]`)
- [ ] `grep -n "on_app_quit" src/audit/mod.rs` → one match
- [ ] `git status` (in your worktree) shows changes only in the in-scope files
- [ ] Your report contains the "Manual acceptance (maintainer, on a Mac)"
      checklist from the Test plan, marked **PENDING MAC VERIFICATION** —
      this plan's primary outcome is menu dispatch on macOS, which no gate
      on this host can prove. The reviewer carries that state into the
      index; the plan is not silently "done" without it.
- [ ] `plans/README.md` status row updated

## STOP conditions

Stop and report back (do not improvise) if:

- Any GPUI API named in "Current state" is absent or has a materially
  different signature on the pinned checkout.
- `cx.on_action` handlers cannot capture the `Entity<Audit>` (borrow or
  ownership error you cannot resolve with a clone) — do not restructure
  `build_audit` to work around it.
- The Linux build breaks in a way that requires editing any out-of-scope file.
- CI's macOS job (if you can run it) fails to compile `src/menus.rs` twice
  after a reasonable fix attempt.

## Maintenance notes

- `QuitMode::LastWindowClosed` is a product choice: close = quit. If the app
  ever becomes multi-window or gains a background task worth surviving the
  window (a long Sirv sync), revisit with `on_reopen` instead.
- Follow-up deliberately deferred: Ctrl+O / Ctrl+Q bindings for Linux and
  Windows; an Edit menu with `MenuItem::os_action` items; a Settings menu
  item (Cmd+,) once `open_settings` is reachable without the window toolbar;
  `set_dock_menu` with recent folders.
- Reviewer scrutiny: confirm the `audit_slot` capture does not move `launch`
  twice, and that nothing new runs before `init_theme` (gpui-component types
  must not be constructed before `gpui_component::init`).
- Known ceiling, on purpose: the File menu's Open items stay enabled during
  a conversion and silently no-op (`Audit::pick` guards on `converting`).
  The toolbar buttons expose that state as disabled; wiring
  `on_validate_app_menu_command` to grey the menu items is the follow-up if
  it ever grates.
