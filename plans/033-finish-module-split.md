> **STATUS: DONE** — executed 2026-08-22 on `improve/batch-4`, commits
> `8a0ef96`…`bb5f1ac`. Final sizes: largest file `sirv.rs` 881; audit modules:
> mod 865, view 772, sirv_actions 548, table 500, state 388, header ~230,
> statusbar ~330, compare_view ~360, sirv_view ~350, media 143,
> convert_job 120, gallery 15. 85 tests pass, clippy/fmt clean. Visual
> surface check pending (screen locked at execution time).

# Plan 033: Finish the module split — Sirv out of the audit, view by feature

> **Executor instructions**: Follow this plan step by step. This is a
> structure-only refactor: move existing items, fix module paths and
> visibility, do not rewrite behavior while moving. Run every verification
> command and confirm the expected result before moving on. If a STOP
> condition occurs, stop and report; do not improvise. When done, update this
> plan's row in `plans/README.md`.

## Status

- **Priority**: P1
- **Effort**: M
- **Risk**: MED — pure moves, but `impl Audit` blocks span two files today and
  the compiler will hold us to exact visibility
- **Depends on**: none (029/030 landed at `df8a46e`; batch 4 fixes are in)
- **Category**: structure
- **Planned at**: branch `improve/batch-4` @ `e50489f`, 2026-08-22

## Why this matters

Plan 030 split the old monolith but left the halves oversized:
`src/audit/view.rs` is 2,147 lines and `src/audit/mod.rs` is 2,023. Both mix
unrelated concerns: `mod.rs` holds all Audit state plus every background job
(conversion, estimates, thumbnails, and four hundred lines of Sirv transfer),
and `view.rs` holds every rendered surface — toolbar, gallery, table cells,
comparison view, settings panel, and Sirv browser — in one file. Any change to
one feature scrolls past five others, and merge conflicts concentrate on
exactly these files.

The seams already exist as method groups; nothing new is invented. After this
plan no file exceeds ~900 lines and each has one reason to change.

## Current state

`src/audit/mod.rs` (2,023) contains:

| Lines | Group |
|---|---|
| ~69–355 | gallery layout math, density colour, sort/compare, free helpers |
| ~355–433 | SirvPairing/SirvJob/SirvBrowser/SettingsPanel/Comparison structs |
| ~441–930 | selection/cursor/sort/filter + estimate scheduling |
| ~931–1096 | dataset install, request_path, pick/reveal |
| ~1097–1373 | Sirv browser open/browse/descend/pair/unpair/settings save |
| ~1374–1638 | run_pull/run_push/start_conversion-adjacent job loops |
| ~1639–1791 | compare open, thumbs request/trim, sync labels |

`src/audit/view.rs` (2,147) contains, inside `impl Audit` + `Render`:

| Lines | Group |
|---|---|
| ~52–207 | gallery tiles |
| ~208–232, 948–1022, 1190–1321 | toolbar controls (resize/format groups, header, controls strip) |
| ~233–582 | comparison view (zoom, pan, chips, footer) |
| ~583–702 | settings panel view |
| ~703–947 | Sirv browser view |
| ~1005–1088, 1322–1656 | summary bar, notices, finding buttons |

`src/main.rs` (387) is fine now. `src/sirv.rs` (884) is a client + pure sync
logic in one file; acceptable, not touched here except where types move with
their callers.

## Target layout

```
src/audit/
├── mod.rs          (~350) Audit struct fields, build_audit, install_dataset,
│                   request_path, Launch plumbing, shared helpers
├── state.rs        (~600) selection/cursor/sort/filter, estimate scheduling,
│                   targets/totals math            [impl Audit]
├── sirv_actions.rs (~700) browser open/browse/pair/unpair, settings save,
│                   run_pull/run_push, walk, refresh counts   [impl Audit]
├── media.rs        (~250) open_compare, request_thumb, trim_thumbs,
│                   sync_label                        [impl Audit]
├── convert_job.rs  (~200) start_conversion loop      [impl Audit]
├── gallery.rs      (~300) GalleryLayout, gallery_layout(), tile() rendering
├── toolbar.rs      (~450) header(), controls(), control_group,
│                   resize/format groups, toolbar_button
├── statusbar.rs    (~400) summary(), notices(), finding_button
├── compare_view.rs (~450) compare_view() + chips/footer
├── sirv_view.rs    (~350) sirv_browser_view(), settings_row/status/panel
├── table.rs        (500, unchanged)
└── tests.rs        (860, unchanged)
```

`view.rs` disappears; its `Render for Audit` impl moves to `mod.rs` bottom.
Every moved block is cut-and-paste plus `use super::*;` adjustments — no
signature changes, no renames beyond what visibility requires (`pub(crate)`
or `pub(super)` on items crossing module lines).

## Commands you will need

| Purpose | Command | Expected |
|---------|---------|----------|
| Fast check after EVERY move | `cargo check --locked` | exit 0 |
| Tests | `cargo test --locked` | 85 pass, 1 ignored |
| Clippy | `cargo clippy --all-targets -- -D warnings` | exit 0 |
| Format | `cargo fmt && cargo fmt --check` | exit 0 |

## Scope

**In scope**: `src/audit/**`, `src/main.rs` only if a `use` breaks.
**Out of scope**: any behavior change, any rename of public API, `src/sirv.rs`
internal reorganization, new abstractions, trait extraction.

## Git workflow

One commit per completed target file (12 commits max), subjects like
`refactor: move gallery layout into audit/gallery`. Do not push unless asked.

## Steps

### Step 1: Split mod.rs's impl blocks

Move methods from `mod.rs` into the four new action modules, keeping `impl
Audit` blocks intact (Rust allows multiple impl blocks across modules in the
same crate). Order matters for reviewability:

1. `convert_job.rs` ← `start_conversion` (+ its helpers it alone uses).
2. `state.rs` ← selection/cursor/sort/filter/estimate/targets group.
3. `sirv_actions.rs` ← everything Sirv: structs (`SirvPairing`, `SirvJob`,
   `SirvBrowser`) stay wherever they compile cleanest, but the *methods*
   (browse/descend/pair/unpair/run_pull/run_push/walk/refresh/busy/save) go
   together.
4. `media.rs` ← open_compare, request_thumb, trim_thumbs, sync_label.
5. What remains in `mod.rs`: struct definition, build_audit,
   install_dataset, request_path, pick, reveal_output, Render impl, free
   functions that serve several modules.

After EACH numbered move: `cargo check --locked` must pass before starting
the next. The napkin warns about PUT-tail mis-anchoring; same rule applies to
structural edits — never batch two moves without an intervening check.

### Step 2: Split view.rs

Move render methods into the six view modules. `tile()` goes with
`gallery_layout()` into `gallery.rs`. `Render for Audit` stays in `mod.rs`.
Delete `view.rs` when empty. Same rule: check between every move.

### Step 3: Visibility pass

Run `cargo check --locked`. For each E0603 (private), widen minimally:
`pub(super)` preferred, `pub(crate)` only if tests need it. No `pub`.

### Step 4: Gates

```bash
cargo test --locked          # 85 passed; count must NOT drop
cargo clippy --all-targets -- -D warnings
cargo fmt --check
wc -l src/*.rs src/audit/*.rs   # no file > 900
```

Live proof: launch the release binary on a real folder, confirm list view,
gallery view, comparison (double-click a row), settings panel, and Sirv
browser all still render. Capture with grim per house contract.

## Done criteria

- [ ] No source file over 900 lines; largest is `tests.rs`
- [ ] `view.rs` gone; every new module has one reason to change
- [ ] Test count unchanged (85 + 1 ignored); clippy/fmt green
- [ ] All five UI surfaces verified live in the real app
- [ ] No behavior change: git diff shows only moves + use-line adjustments

## STOP conditions

- A method refuses to move because it shares private state with a neighbor in
  a way that would need `RefCell`/restructuring — report instead of inventing.
- Test count changes for any reason.
- GPUI's derive/macro machinery rejects split impl blocks (it does not, but
  verify early with the first move).
