# Plan 030: Split `main.rs` along the seams it already has

> **STATUS: DONE** — executed 2026-08-22 on branch
> `improve/029-030-execution` (commits `adde068`, `54d9778`, merged in
> `df8a46e`). Verified at `8e5628f`: `src/audit/{mod,view,table,tests}.rs`
> exist and `src/main.rs` is ~380 lines. The brief below is kept as the
> historical record of what was executed; its line numbers refer to the
> pre-split tree.

> **Executor instructions**: Follow this plan step by step. This is a
> structure-only refactor: move existing items, fix module paths and visibility,
> and do not rewrite behavior while moving it. Run every verification command
> and confirm the expected result before moving on. If anything in "STOP
> conditions" occurs, stop and report; do not improvise. When done, update this
> plan's row in `plans/README.md` unless a reviewer told you they own the index.
>
> **Drift check (run first)**:
> `git diff --stat 1a6540a..HEAD -- src/main.rs src/audit plans/029-resolve-changed-files.md`
>
> This plan deliberately depends on Plan 029, so drift inside `SirvJobKind`,
> `start_pull`, `start_push`, their controls, and their tests is expected after
> 029 lands. Drift that adds an audit module, replaces `Audit`, or changes the
> build/test contract is a STOP condition.

## Status

- **Priority**: P1
- **Effort**: M
- **Risk**: MED — the change is mechanical, but Rust module privacy and GPUI's
  table/view ownership make a one-shot move needlessly fragile
- **Depends on**: `plans/029-resolve-changed-files.md`
- **Category**: tech-debt
- **Planned at**: commit `1a6540a`, 2026-08-22
- **Working-tree snapshot**: `src/main.rs` blob
  `4cefbb449f0a4b0ae4016dc0df86bf4590a77746` (the operator's uncommitted UI
  changes were present and were not modified by the advisor)

## Why this matters

`src/main.rs` is 5,715 lines. The next-largest Rust source file is 783 lines,
and the median of the other source files is 261. It owns CLI parsing, headless
conversion, application startup, audit state, async orchestration, Sirv sync,
four rendered surfaces, the virtual table, and nearly 800 lines of tests.
Twenty-three of the latest thirty commits touched it, so unrelated work keeps
colliding in the same file.

The code already has usable boundaries. This plan exposes those boundaries as
Rust modules without adding traits, service objects, a message bus, new state
types, or dependencies. The target is easier navigation and smaller diffs, not
a new architecture.

## Current state

- `src/main.rs:7-15` declares every existing module; there is no `audit`
  module.
- `src/main.rs:264-387` defines the 60-plus-field `Audit` entity that owns all
  window state.
- `src/main.rs:529-3393` is one `impl Audit` containing state transitions,
  background jobs, Sirv orchestration, thumbnail requests, and rendering
  helpers.
- `src/main.rs:3489-3956` defines `AuditTable`, its column model, and the
  `TableDelegate` implementation.
- `src/main.rs:3958-4452` implements `Render for Audit` and selects the empty,
  scanning, settings, Sirv-browser, comparison, table, and gallery surfaces.
- `src/main.rs:4455-4926` contains the actual binary boundary: `Args`,
  `parse_args`, `convert_headless`, `main`, `init_theme`, `Launch`, and
  `run_window`.
- `src/main.rs:4938-5715` contains the ignored screenshot harness and the audit
  unit/GPUI tests.
- The table caches column groups. Its current required pattern is preserved:
  `Render::render` updates the delegate and calls `TableState::refresh` from
  `cx.defer`, never during the render itself (`src/main.rs:3986-3998`).
- Audit rows remain immutable in `entries`; sorting/filtering only rebuilds
  `visible: Vec<usize>`. Do not change that invariant while moving code.
- Detached work continues to publish through the existing generation counters.
  Do not combine or replace those counters in this refactor.

Current module boundary excerpt (`src/main.rs:7-15`):

```rust
mod avif;
mod compare;
mod convert;
mod scan;
mod settings;
mod sirv;
mod thumbs;
#[cfg(feature = "updater")]
mod update;
```

Current binary/UI boundary (`src/main.rs:4667-4669`):

```rust
/// Build the audit view for a window. Shared by the app and the screenshot harness
/// so that what gets captured is the thing that ships.
fn build_audit(launch: Launch, window: &mut Window, cx: &mut App) -> gpui::Entity<Audit> {
```

## Target layout

```text
src/
├── main.rs          # CLI, headless mode, theme, Launch, app/window startup
└── audit/
    ├── mod.rs       # Audit state, state transitions, async jobs, build_audit
    ├── view.rs      # Audit rendering helpers and Render implementation
    ├── table.rs     # AuditTable, columns, and TableDelegate
    └── tests.rs     # screenshot harness plus audit unit/GPUI tests
```

Target size guardrails are intentionally coarse: `main.rs` should be below 600
lines, and no new `src/audit/*.rs` file should exceed 2,500 lines. Do not split
further merely to chase a smaller number.

## Commands you will need

| Purpose | Command | Expected on success |
|---|---|---|
| Baseline tests | `cargo test --locked` | exit 0; at least 80 passed, 0 failed; screenshot remains ignored |
| Compile each move | `cargo check --locked` | exit 0, no errors |
| Full tests | `cargo test --locked` | exit 0; test count does not decrease |
| Lint | `cargo clippy --all-targets -- -D warnings` | exit 0, no warnings |
| Format | `cargo fmt --check` | exit 0 |
| Release build | `cargo build --release --locked` | exit 0 |
| File sizes | `wc -l src/main.rs src/audit/*.rs` | `main.rs` below 600; every audit file below 2,500 |

## Scope

**In scope** (the only source files to modify or create):

- `src/main.rs`
- `src/audit/mod.rs` (create)
- `src/audit/view.rs` (create)
- `src/audit/table.rs` (create)
- `src/audit/tests.rs` (create)
- `plans/README.md` (status row only when execution completes)

**Out of scope**:

- Any behavior, UI copy, selector, layout, colour, keyboard binding, async
  scheduling, conversion rule, or Sirv rule change.
- Renaming existing types or methods merely because they moved.
- Splitting `Audit` into multiple state objects.
- Traits, facades, controllers, dependency injection, event buses, or a new
  crate/library target.
- New dependencies or Cargo feature changes.
- Plans 001-029, except reading Plan 029 and confirming it is DONE.
- Fixing the known Linux `HeadlessRenderer` screenshot limitation.

## Git workflow

- Start from the branch containing completed Plan 029 and a clean worktree.
- Use branch `refactor/030-split-main` unless the operator gives another name.
- Make one commit after all gates pass: `refactor: split audit UI out of main`.
- Do not push or open a PR unless the operator asks.
- Never discard unrelated local changes. If the worktree is dirty at start,
  stop and ask which changes belong to this refactor.

## Steps

### Step 1: Establish the post-029 baseline

1. Confirm Plan 029's row in `plans/README.md` is `DONE` and verify its changes
   exist in source. A status row alone is not proof.
2. Run `git status --short`. Continue only from a clean worktree.
3. Record `wc -l src/main.rs src/*.rs` and `git hash-object src/main.rs` in the
   execution notes.
4. Run the baseline tests, clippy, and format checks.

**Verify**:

```bash
cargo test --locked
cargo clippy --all-targets -- -D warnings
cargo fmt --check
```

Expected: all exit 0; the test count is no lower than 80 passed, with the
screenshot test still ignored. If Plan 029 adds tests, keep the higher count as
the refactor baseline.

### Step 2: Move the complete audit subsystem behind one module boundary

Create `src/audit/mod.rs` and first move the audit subsystem as one intact
unit. Do not split its internals in the same edit. This makes the compiler
prove the binary/UI boundary before the smaller child-module moves.

Move these existing items from `src/main.rs` into `src/audit/mod.rs` without
changing their bodies:

- Audit/table/gallery constants, estimate/compare/save delays, and thumbnail
  cache size. Keep the window min/default constants and
  `restored_window_size` in `main.rs`, because `run_window` owns them.
- `is_checkbox_activation_key`, gallery geometry, colour/meter/segment helpers.
- `Audit`, `Sort`, `Column`, `Listing`, `SirvPairing`, `SirvJobKind`,
  `SirvJob`, `SirvBrowser`, `SettingsPanel`, and `Comparison`.
- `compare_entries`, `write_settings`, every `impl Audit` block, `Finding`,
  `Stratum`, and the pure projection/target/progress helpers.
- `AuditTable`, `TableColumn`, their implementations, and `impl Render for Audit`.
- `build_audit` and both test modules.

Add `mod audit;` to `main.rs`. Keep `Launch`, `init_theme`,
`restored_window_size`, and the window startup in `main.rs`; descendants can
read private items from the crate root. Expose only the two items root needs:

```rust
pub(crate) struct Audit { /* existing fields */ }

pub(crate) fn build_audit(
    launch: crate::Launch,
    window: &mut Window,
    cx: &mut App,
) -> gpui::Entity<Audit> { /* existing body */ }
```

Change the application call site in `run_window` to
`audit::build_audit(...)`; the screenshot and GPUI test call sites move with
the audit code and stay unqualified. Add `use crate::{Launch, init_theme,
restored_window_size};` to the moved test modules so Step 2 compiles before
they are consolidated in Step 5. Do not make Audit fields or methods
`pub(crate)` to silence privacy errors; the following child modules are
specifically chosen so they can retain private access.

Move imports to the module that uses them and delete now-unused root imports.
Do not create a prelude or shared-import module.

**Verify**: `cargo check --locked` -> exit 0. Then
`cargo test --locked` -> the post-029 baseline count, 0 failed.

### Step 3: Extract the table as a child module

Create `src/audit/table.rs` with the row/column width constants,
`AuditTable`, `TableColumn`, `impl TableColumn`, `impl AuditTable`, and
`impl TableDelegate for AuditTable`. Add this near the top of
`src/audit/mod.rs`:

```rust
mod table;
use table::AuditTable;
```

Use `use super::*;` inside `table.rs`; this is an internal child module, and it
needs direct access to the parent Audit state. Apply the smallest visibility
needed for parent/sibling use:

- `AuditTable`: `pub(super)`
- `AuditTable::new`: `pub(super)`
- `AuditTable::set_viewport_width`: `pub(super)`
- `AuditTable::layout`: `pub(super)` only because the existing test calls it

Keep `TableColumn` private to `table.rs`. Do not add getters to `Audit` or a
table view-model. Preserve the deferred refresh block exactly.

**Verify**: `cargo check --locked` -> exit 0, then
`cargo test --locked table_layout_keeps_decision_columns_at_compact_width` ->
1 passed.

### Step 4: Extract rendering as a child module

Create `src/audit/view.rs` and add `mod view;` to `src/audit/mod.rs`. Move only
code that builds GPUI elements or selects a rendered surface:

- `compare_chip`, `meter`, and `segment`.
- `Audit::tile` and `Audit::control_group`.
- `Audit::compare_view`, `settings_row`, `settings_status`,
  `settings_panel_view`, and `sirv_browser_view`.
- `resize_group`, `format_group`, `visible_bytes`, `toolbar_button`, `header`,
  `controls`, `summary`, `notices`, and `finding_button`.
- The complete `impl Render for Audit`.

Keep state transitions in `mod.rs`, including `open_compare`, dataset/scan
installation, selection, conversion, Sirv jobs, `request_thumb`, `sync_label`,
and `trim_thumbs`. `sync_label` stays in the parent because both `view.rs` and
`table.rs` use it. `density_colour`, gallery geometry, and root chrome also
stay in the parent because they are shared or directly tested.

Define rendering methods in `impl Audit` inside `view.rs`; child modules may
access the parent's private fields and methods. Do not introduce an `AuditView`
type or pass a copy of Audit state into rendering.

**Verify**: `cargo check --locked` -> exit 0, then run:

```bash
cargo test --locked grid_checkbox_pointer_click_stays_inside_checkbox
cargo test --locked table_checkbox_pointer_click_stays_inside_checkbox
cargo test --locked gallery_scroll_resets_only_when_the_production_column_count_changes
```

Expected: each filtered test passes.

### Step 5: Move the audit tests into one child file

Create `src/audit/tests.rs`, add `#[cfg(test)] mod tests;` to
`src/audit/mod.rs`, and move both the ignored screenshot harness and the
existing `tests` module contents into it. Use one test module, with imports
equivalent to:

```rust
use super::*;
use crate::{Launch, init_theme, restored_window_size};
use gpui::{HeadlessAppContext, TestAppContext};
use image::ImageFormat;
```

Retain every test body and the ignored screenshot test. Its fully-qualified
test name may change, but the documented filter `screenshot` must still find
exactly one ignored test. Access the table layout as `table::AuditTable::layout`
or through the existing parent import; do not widen it beyond `pub(super)`.

**Verify**:

```bash
cargo test --locked
cargo test --locked -- --ignored --list screenshot
```

Expected: the full test count matches or exceeds the Step 1 baseline with 0
failed; the second command lists exactly one ignored screenshot test. Do not
attempt to fix or run the renderer in this plan.

### Step 6: Format once and run the full gate

Run `cargo fmt` once after the moves. Review the diff with moved-code detection;
there should be module declarations, imports, narrow visibility changes, and
moved bodies, but no rewritten business or rendering logic.

```bash
cargo fmt
git diff --check
git diff --color-moved=dimmed-zebra -- src/main.rs src/audit
cargo test --locked
cargo clippy --all-targets -- -D warnings
cargo fmt --check
cargo build --release --locked
wc -l src/main.rs src/audit/*.rs
```

Expected: every command exits 0; tests do not decrease; `main.rs` is below 600
lines; each audit file is below 2,500 lines.

## Test plan

No new behavior test is required. This refactor's regression check is the full
existing suite, which currently exercises pure audit math, sorting/filtering,
conversion targets, Sirv cancellation, checkbox ownership, table layout,
gallery geometry/scrolling, dataset replacement, and the production GPUI Root.

The executor must preserve all existing tests and their assertions. A lower
test count is failure, even when the remaining tests pass. The ignored
screenshot test must remain discoverable and ignored; fixing its Linux renderer
is explicitly outside scope.

## Done criteria

- [ ] Plan 029 is implemented and verified before this refactor starts.
- [ ] `src/main.rs` contains CLI/headless/startup code and `mod audit;`, but no
      `Audit`, `AuditTable`, `TableDelegate`, or `Render for Audit` definition.
- [ ] `src/audit/{mod.rs,view.rs,table.rs,tests.rs}` exist with the ownership
      described in Target layout.
- [ ] `rg -n '^(struct Audit|impl Audit|impl Render for Audit|struct AuditTable|impl TableDelegate for AuditTable)' src/main.rs`
      returns no matches.
- [ ] `rg -n '^mod audit;' src/main.rs` returns exactly one match.
- [ ] `cargo test --locked` exits 0 and the test count is not below the Step 1
      baseline.
- [ ] `cargo clippy --all-targets -- -D warnings` exits 0.
- [ ] `cargo fmt --check` exits 0.
- [ ] `cargo build --release --locked` exits 0.
- [ ] `wc -l src/main.rs src/audit/*.rs` shows `main.rs` below 600 lines and
      every audit file below 2,500 lines.
- [ ] `git diff --check` exits 0.
- [ ] No Cargo file or source file outside Scope changed.
- [ ] The executor reviewed `git diff --color-moved=dimmed-zebra` and found no
      functional edits hidden among the moves.
- [ ] Plan 030's row in `plans/README.md` is updated to DONE.

## STOP conditions

Stop and report back instead of improvising if:

- Plan 029 is not actually implemented, its gates fail, or its source behavior
  disagrees with its DONE status.
- The starting worktree has unrelated changes.
- `Audit`, `AuditTable`, or `build_audit` has already moved or been replaced.
- The module split appears to require a new state type, trait, dependency, or
  public API.
- Rust privacy errors cannot be resolved with child-module access plus the
  narrow `pub(crate)`/`pub(super)` items named above.
- Any existing test must be deleted, weakened, ignored, or substantially
  rewritten to make the refactor pass.
- A verification command fails twice after correcting a straightforward move,
  import, or visibility mistake.
- A source file outside Scope appears necessary.

## Maintenance notes

- New audit state and state transitions belong in `audit/mod.rs`; rendered
  controls/surfaces belong in `audit/view.rs`; virtual table behavior belongs in
  `audit/table.rs`; binary startup remains in `main.rs`.
- Do not split `Audit` again just because `mod.rs` remains around 2,000 lines.
  Add another module only when a real feature has a cohesive state boundary and
  the current child-module privacy model no longer holds.
- Reviewers should scrutinize visibility widening and any diff line that is not
  detected as moved code. Those are the two places behavior can leak into this
  otherwise mechanical refactor.
