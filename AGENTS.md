# Repository Guidelines

## Project Overview

Press audits a local folder of images and re-encodes it (`convert::Format` is `WebP, Avif, JpegXl, Jpeg, Png, Same`) without uploading anything. It is the desktop companion to imageguide.dev and its Chrome extension: those audit pages and stop there; this one rewrites the files. Three views share one window: the audit list (or gallery), a 1:1 before/after comparison, and an empty state. A headless `convert` verb does the same work with no window.

## Architecture & Data Flow

Single crate, one binary (`press`). UI is [GPUI](https://www.gpui.rs) with [gpui-component](https://github.com/longbridge/gpui-kit) widgets.

```
scan::scan(root) ──► Launch ──► build_audit ──► Entity<Audit> (all UI state)
                                    │
     ┌──────────────────────────────┼──────────────────────────────────────┐
     ▼                              ▼                                      ▼
thumbs::load (per visible row)  compare::build (per view)  convert::write_output / write_recorded
  Arc<RenderImage>                Pair (in-memory)          optimized/, --output, or in place (--replace)
```

- `src/scan.rs` — header-only folder walk. Never decodes to learn dimensions. `Entry` carries `path/format/width/height/bytes`; `extension_lies()` flags files whose magic bytes disagree with the extension; output goes to `OUTPUT_DIR = "optimized"`, which the walk skips, as it skips `BACKUP_DIR = "press-originals"` (replace mode's originals) and the run manifest.
- `src/convert.rs` — re-encode. WebP via libwebp (real transparency forces lossless), AVIF via system libavif/libaom with libyuv conversion where packaged (no lossless AVIF). AVIF speed defaults to 6; `--avif-speed <0..10>` and the `avif_speed=` settings key override it, and `avif::set_speed` is the one place the range is enforced. A run, a comparison request and an estimate each capture the speed once and pass it to `encode_with_speed`; a recorded write takes it from its own `Stamp`, so the bytes and the manifest line describing them cannot come from two reads of a setting that moved in between. `prepare`/`encode_prepared` are the one decode-resize-budget path every encoder answers through — the writer, the comparison, the estimate and the dry run — and they hand back `scan::DecodedSource`, so pixels, profile, depth, the container the bytes were identified as and the identity of those bytes travel together. `MaxEdge` downscales with Lanczos3, never up. `output_path` mirrors the source tree.
- `src/compare.rs` — original-vs-converted pair built in memory, decode-encode-decode so both sides are real pixels. The source is prepared through `convert::prepare`, the same decode/resize/profile path the writer uses; nothing on screen is ever a source. A `Pair` carries the `SourceIdentity` of the bytes it consumed (and of the installed output, for a written comparison), and `Pair::confirm` re-reads them on the background executor before the result lands. `Key` (path+format+quality+max_edge+avif_speed+stat revision) is a cheap miss, never the freshness authority. Completed pairs are not cached or built ahead — arrowing through AVIF comparisons re-encodes each step — while the preview cache and preview lookahead stay.
- `src/thumbs.rs` — decode + 96px thumbnail + BGRA swap (`to_bgra`, shared with compare). `None` means draw a gap, not an error.
- `src/manifest.rs` — `.press-manifest.jsonl` in the output root, one appended line per written output: which source it came from, what it measured, and where a replaced original was moved. Written before the original moves, so a killed run is still recoverable. `plan_outputs` reads it so a later run never walks over an earlier one's output, and `restore` walks it backwards to undo a replace run.
- `src/settings.rs` — hand-rolled `key=value` file at `<config>/imageguide/settings`; tolerant parse. The `imageguide` folder name predates the rename to Press and stays, so saved credentials survive.
- `src/main.rs` — everything else: `main`, `parse_args`/`parse_args_from`, `convert_headless`, `run_window`, `Launch`, `init_theme`. The `Audit` UI itself lives in `src/audit/` (25 files): `struct Audit` and `build_audit` in `src/audit/mod.rs`, `impl Render for Audit` in `src/audit/view.rs`, `struct AuditTable` and `impl TableDelegate` in `src/audit/table.rs`.

Key patterns an editor must respect:

- **Indices, not moves.** `Audit::entries` is never reordered. `visible: Vec<usize>` holds filtered+sorted indices; thumbs, ticks and results are keyed by entry index so they survive re-sorting.
- **Heavy work off the main thread.** Scans, thumbnails, encodes, compare builds and estimates all run on `cx.background_executor().spawn(...)` inside `cx.spawn(async move |this, cx| { ...; this.update(cx, ...) }).detach()`.
- **Generation counters invalidate stale async work.** `dataset_generation`, `scan_generation`, `estimate_generation` on `Audit`, plus `handoff_generation`, `job_request_generation`, `sirv_generation`, `sirv_pairing_generation`, `sirv_browser_generation`. Check the generation you captured before applying a result; a slider drag supersedes an in-flight estimate.
- **`convert::workers(format)` bounds conversion** — WebP/JPEG/PNG get `cores.clamp(2, 8)`, AVIF 2, a kept-format (`Same`) folder 4, JPEG XL 1 (its own encoder already uses all the machine's cores). Each in-flight file holds a fully decoded image up to `convert::MAX_DECODE_BYTES` (1 GiB), so this is a memory bound, not just a parallelism knob.
- **Thumbnails are viewport-driven.** `render_td`/gallery bands call `request_thumb`; `requested: HashSet` dedupes; never decode eagerly.
- **Theme comes from `cx.theme()`**, set once in `src/main.rs::init_theme`. Colour set and `theme.tokens.button_primary*` must agree or buttons render black-on-blue. Fonts: SF Pro Text / SF Pro Display, mono Fira Code.
- **Checkbox keyboard ownership**: unmodified Space/Enter must stop at a wrapper `on_key_down` (`is_checkbox_activation_key`), or the component toggles and the root cursor handler toggles again.
- **`TableState` caches column groups**: after a viewport/result-signature change, update the delegate and call `TableState::refresh` from `cx.defer`, never during `Audit::render`.

## Key Directories

| Path | Purpose |
|---|---|
| `src/` | All source. `main.rs` (entry point, ~24 top-level `mod` declarations) plus the `src/audit/` UI module (25 files). |
| `docs/` | README screenshots (`audit.webp`, `comparison.webp`, `gallery.webp`, `results.webp`) — compressed by the tool itself — plus the product/execution ledger: `workbench-execution-plan.md` (status ledger), `engineering-follow-up.md`, `agent-execution-plan.md`, `delivery-recipes.md`, and other strategy docs. `ROADMAP.md` at the repo root links them. |
| `plans/` | Numbered executor briefs (`NNN-slug.md`) from improve-style audits, indexed by `plans/README.md`. Statuses: DONE / REJECTED / etc. Read the relevant plan before touching its area. `plans/` is listed in `.gitignore`, so it does not ship in commits or PR diffs; the product-level ledger that *is* committed is `docs/workbench-execution-plan.md`. |
| `.github/workflows/` | CI (`ci.yml`): build/test/clippy/fmt on `ubuntu-latest`, `macos-latest`, `windows-2022`. `release.yml` packages macOS (arm64 + intel), Windows and Linux. |

## Development Commands

```bash
cargo build --release        # needs dav1d and libavif/libaom; Linux also packages libyuv
cargo test --locked --features updater          # CI's test command; screenshot test stays ignored
cargo clippy --all-targets --features updater -- -D warnings
cargo fmt --check
cargo run --release -- ~/path/to/folder                          # audit window
cargo run --release -- convert ~/path/to/folder --format avif    # headless convert, no window
cargo run --release -- convert ~/path/to/folder --replace        # convert in place; originals to press-originals/
cargo run --release -- restore ~/path/to/folder                  # put those originals back
cargo test --bin press -- --ignored --nocapture screenshot   # known-broken on this Linux host (no HeadlessRenderer); prove UI with the real app instead
```

The older flag syntax (`press ~/folder --convert --avif`) still parses; `press --help` is the complete, current command reference (`convert`, `audit`, `check`, `restore`, `plan`, `execute`, `reconcile`, `handoff`, `supplier`, `studio`, `skill`, `update`, ...).

On macOS the UI tests (`audit::tests::`, in `src/audit/tests.rs`) are slow; iterate with a filter such as `cargo test --locked --bin press manifest::`.

CI runs the same gates with `--locked --features updater` (see `.github/workflows/ci.yml`).

## Code Conventions & Common Patterns

- Rust 2024, `cargo fmt` canonical; clippy warnings are errors in CI.
- Comments explain *why*, often as a short paragraph over the item they justify. Match that voice; do not add narration comments.
- Test names are snake_case sentences stating behaviour: `ties_fall_back_to_the_filename`, `a_file_decodes_by_its_contents_not_its_name`.
- No config/argument crates: `parse_args` and `settings.rs` are hand-rolled and tolerant of junk input.
- Errors are named, not counted: failures keep filenames (`Audit::failures: Vec<String>`); "N failed" alone is not a report.
- Truthful UI: a file that grew is reported as grown; stale results are dropped, not shown next to a new folder.
- Commit subjects: `fix: ...` / conventional, one concern per commit.

## Important Files

- `src/main.rs` — entry point, window lifecycle, theme (`init_theme`), CLI parsing (`parse_args`/`parse_args_from`), headless verb dispatch (`convert_headless`, `restore`, ...). Black-box CLI tests live in `tests/cli_contract.rs`, `tests/studio_cli.rs` and `tests/headless_output.rs`, not here.
- `src/audit/` — the entire `Audit` UI (25 files). Files an editor most often needs: `mod.rs` (`Audit` struct, `build_audit`, dataset install, restore), `view.rs` (`impl Render for Audit`), `table.rs` (`AuditTable`, `impl TableDelegate`), `gallery.rs`, `panel.rs` (right rail, convert/restore buttons), `convert_job.rs`, `sirv_actions.rs`, `media.rs` (thumbnail scheduling), `state.rs` (selection/sort/filter), `tests.rs` (UI tests).
- `src/output.rs`, `src/job.rs`, `src/recipe.rs`, `src/saved_plan.rs`, `src/requirements.rs`, `src/handoff.rs`, `src/supplier.rs`, `src/studio.rs`/`src/studio_ledger.rs`, `src/sirv.rs`, `src/local_ai.rs`, `src/update.rs`, `src/crash.rs` — the modules the rest of this file doesn't cover: destination proof, background job records, saved recipes/plans, requirements checks, handoff packaging, supplier-portal integration, Studio state and its ledger, Sirv upload, local-AI captioning, self-update (`updater` feature), and crash reporting, respectively.
- `src/scan.rs` — `Scan`/`Entry`, header-only walk, `OUTPUT_DIR`.
- `src/convert.rs` — `Format`/`Quality`/`MaxEdge`, `plan_outputs`, `convert_to`, `write_output`/`write_recorded`, `workers`.
- `src/compare.rs` — `Key`/`Pair`, `build`.
- `Cargo.toml` — version policy comment; read before adding dependencies.
- `Cargo.lock` — the actual version pin for gpui-pre and gpui-component.
- `rust-toolchain.toml` — pinned 1.97.1; use it, not a system default.
- `.Codex/napkin.md` and `.claude/napkin.md` — session lessons: host-specific gotchas and patterns that work (Hyprland/ydotool/grim for UI proof). One is written by Codex sessions, the other by Claude sessions; they have diverged and consolidating them is a maintainer decision, not done here.

## Runtime/Tooling Preferences

- Rust toolchain is pinned; use the pinned toolchain, not a system default.
- The whole UI stack is one dependency: `gpui-kit` re-exports the matching gpui, platform, component and asset crates. Tests add `gpui-pre-platform/test-support` for the headless renderer, which the facade does not forward. `Cargo.lock` pins the versions; CI builds `--locked`.
- Linux build uses rfd `xdg-portal` (no GTK); other targets use rfd defaults. The facade enables gpui platform features `wayland,x11,font-kit,runtime_shaders`. AVIF encoding links system libavif >= 1.0 with its libaom backend and libyuv where packaged.
- `gpui-kit-assets` must be registered as the asset source or every `IconName` renders blank.
- CI builds and runs the test suite on all three targets (`ubuntu-latest`, `macos-latest`, `windows-2022`); Windows installs `libavif[aom,dav1d]:x64-windows-static` via vcpkg. Real-window UI proof (screenshots, click-through) is still host-specific — see the napkins.

## Testing & QA

- Framework: built-in `cargo test`; UI tests use `#[gpui_kit::test]` with `TestAppContext`/`VisualTestContext` (`cx.update(init_theme)`, `cx.add_window_view(...)`, `simulate_click`, `debug_bounds(selector)`).
- Coverage: table/gallery layout thresholds, sort stability (ties fall back to filename), checkbox pointer/keyboard ownership, conversion round-trips, AVIF alpha preservation, scan rules, settings round-trip.
- Helpers: `entry(name,w,h,bytes,format)`, `pointer_checkbox_audit(grid, cx)`, `photo(w,h)` (deterministic noise — flat colours compress to nothing and make assertions false).
- The ignored screenshot test fails on this Linux host (`render_to_image not available`) — that is host-specific, not a platform verdict; for visual proof, launch the real release binary and capture it (Hyprland/ydotool/grim on Linux; macOS maintainers prove UI with the real app too) instead of fixing the harness incidentally.
- Baseline gate before landing (matches CI): `cargo test --locked --features updater`, `cargo clippy --all-targets --features updater -- -D warnings`, `cargo fmt --check` all green.
