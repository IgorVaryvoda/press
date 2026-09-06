# Napkin (Claude sessions)

A richer napkin exists at `.Codex/napkin.md`. Read it first; it holds the
host gotchas, benchmarks, and user preferences. This file adds what Claude
sessions learn.

## Corrections
| Date | Source | What went wrong | What to do instead |
|------|--------|-----------------|--------------------|
| 2026-08-31 | self | `ls` is aliased (eza) and rejects `--icons` when given odd args in non-interactive fish | Use `/bin/ls` in Bash tool calls |

## Notes
- 2026-08-31: AGENTS.md drifted: it says `main.rs` holds the whole Audit UI
  at ~3090 lines, but the audit UI now lives in `src/audit/*` and `main.rs`
  is ~1300 lines. Verify AGENTS.md line references against source.
- 2026-09-01: Audit for "top 5 improvements". Verified in code: custom output
  folder outside the audited root fails every file (`write_output` strips
  the audited `root`, not `out_dir`, src/convert.rs:508); `pick_output` has
  no validation so output == source folder overwrites originals; `output::
  Context` exists but conversion never calls it; no ICC handling anywhere;
  `looks_like_an_image` omits heic/heif so HEIC folders scan to nothing;
  Studio API host is `dev.sirv.studio` (src/studio.rs:15). Explorer
  subagent claims all held up when spot-checked.
- 2026-09-01: Parallel fix batch runs in sibling worktrees
  `imageguide-desktop-fix-{output-boundary,colour-metadata,heic-visible,convert-stop,trust-chain}`
  on branches `fix/*` from main b96953d, sharing
  `CARGO_TARGET_DIR=/home/igor/Projects/imageguide-desktop/target` so gpui
  is compiled once. Baseline on main: 328 passed, 1 ignored. Production
  Studio API host is `https://www.sirv.studio` (probed: same 401 JSON as dev).
| 2026-09-01 | self | Five worktrees shared one `CARGO_TARGET_DIR`; cargo hashes a path package relative to its workspace root, so every worktree wrote the same `deps/press-<hash>` test binary and agents saw phantom failures (`avif_keeps_transparency`) and missing tests | Give each parallel worktree its own target dir (copy `debug/{build,deps,.fingerprint}` minus `press-*` and minus any dir holding a `CMakeCache.txt`), or run gates serially in one review worktree with a dedicated target |
- 2026-09-01: Batch integrated on branch `fix/audit-batch-2026-09` (worktree
  `imageguide-desktop-integration`), not merged to main. Merge lessons: the
  `Launch` struct has no Default, so any branch adding a field breaks sibling
  test fixtures; git conflict hunks here are diff3-style (`|||||||`), keep
  ours+theirs and drop the base section; a shared function's closing brace can
  sit in the common suffix after the conflict.
| 2026-09-01 | CI | Windows failed 3 tests after the audit batch: `plan_outputs` keyed paths by string so `\`-joined plans never matched `/`-spelled sources, and a CRLF checkout broke a `contains("\npubkey=…\n")` test | Key paths by component, and read scripts by `lines()` in tests; expect Windows to be the platform that finds path and line-ending slips |
- 2026-09-02: Second batch, two waves. Wave 1 worktrees
  `imageguide-desktop-feat-{select-all,replace-mode,subfolders,formats}` on
  `feat/*` from main 174b1fc, each with a private
  `CARGO_TARGET_DIR=/home/igor/Projects/press-target-{a,b,c,d}` (copies of
  main's debug deps minus `press-*` and cmake dirs; script in the session
  scratchpad `mktarget.sh`). Wave 2 (CLI flags + CI, perf, failure badges)
  branches from the wave-1 integration. Baseline: 356 tests, CI green on 3 OSes.
- 2026-09-02: Released v0.4.0 from main 245b8d8 (six of seven batch items:
  select-all, replace mode + manifest + restore, subfolders, JPEG/Same/custom
  edge, CLI --output/--skip-existing/--dry-run + broken-pipe fix + CI trim,
  perf scaled decode/AVIF speed/thumb cache/compare reuse). Release recipe:
  bump Cargo.toml + Cargo.lock press entry, `chore: release vX`, annotated
  tag `vX`, push main and tag; release.yml verifies tag == Cargo version.
  Failure badges (feat/failure-badges) still in progress, lands after.
| 2026-09-02 | CI | v0.4.0 tag failed CI: `a_superseded_estimate_stops_before_it_decodes_the_rest` raced two equal-due timers; passed locally, "0 of 8 samples" on 4-core runners. Then I pushed a clippy failure because `{ a; b; c; } && push` only checks the last command | In gpui tests `advance_clock` runs every runnable task, so a burst cannot be interrupted from outside: use a `#[cfg(test)]` thread-local hook inside the loop (`ESTIMATE_HOOK`, like `scan::TEST_HOOKS`). Chain gates with `a && b && c && push`, never a brace block |
| 2026-09-02 | self | Background `gh run watch` calls were killed at 10 min: the Bash tool timeout caps at 600000 ms even when asked for more | Poll long workflows in bounded ~9-minute loops and re-arm; never rely on one long watcher |
| 2026-09-06 | self | `git stash show -p stash@{1} -- src` fails with "Too many revisions"; `stash show` takes no pathspec | Use `git diff 'stash@{N}^1' 'stash@{N}' -- src` for a scoped stash diff |
- 2026-09-06: `plans/README.md` rows marked `DONE — full gates and Linux visual
  proof` (1574–1578) were committed only on `codex/ui-fixes-20260905`; the
  primary working tree mirrored them uncommitted and `main` had none. A DONE
  row means a worktree commit exists, not that `main` carries it. Check
  `git log origin/main..<branch>` before trusting DONE.
- 2026-09-06: Landed product-set jobs + UI fixes 1574–1578 as `b1d1a9a` on
  main (not pushed in that session). Removed all sibling worktrees, cleared
  stashes (`dcb84c4`, `08868d4`, `a3ed98f`), safe-deleted 13 landed branches.
  Auto mode blocks `git branch -D` and combined destructive commands: run
  `git branch -d` for the provable set and hand force deletes to the user.
  Still present, need `-D`: `advisor/1563` (in main as c77809c),
  `improve/016-checkbox-pointer`, `improve/preflight-batch-10-integration`
  (API removed by 89505b8), `advisor/batch2` (Sirv work superseded),
  `codex/ui-{fixes,comparison}-20260905` (content in b1d1a9a).
- 2026-09-06: gpui-component 0.6 table headers sort only from the trailing
  sort icon; clicking the label selects the column. Press now owns header
  clicks in `AuditTable::render_th` (`sort-head-N` selectors) and draws the
  Name arrow beside its label, so Name's `TableCol` must stay non-`sortable()`
  or the library adds a second arrow at the far edge.
- 2026-09-06: `./scripts/ux-eval capture audit-core --fixture <dir>
  --allow-external-fixture --skip-build` proves the list against a generated
  fixture in ~10 s per scenario. `docs` has three light rows, so it cannot
  show finding chips, long names, `optimized/` counts or the bar clearance;
  Pillow noise PNGs (`Image.frombytes` on `os.urandom`) make `heavy` rows.
- 2026-09-06: The VibeQ MCP server is project-scoped to ai-image-tools in
  `~/.claude.json` (`https://work.sirv.studio/mcp`), so its tools are absent
  here. Direct HTTP from Python needs `User-Agent: node`; the default UA gets
  Cloudflare error 1010. Press tasks there carry tag `press` and no domain.
- 2026-09-06: The bundled gpui-kit icon set has no list or grid glyph
  (`layout-dashboard`, `menu`, `gallery-vertical-end` only), so the header
  view switch is a text `ButtonGroup` (List | Grid) via `toolbar::segment`.
  Press's own SVGs live in `assets/icons/studio/` if a glyph is ever needed.
- 2026-09-06: `ux/scenarios.json` clicks are absolute window coordinates. The
  two Sirv credential scenarios clicked (250,23), where nothing lived at any
  size. With Open and Sirv anchored at the header's left, (155,20) hits Sirv
  at 760, 1100 and 1440; keep primary header controls left-anchored so fixed
  scenario clicks stay valid across sizes.
- 2026-09-06: Released v0.5.0 from main: list and header UI batch (`b7d52a0`)
  on top of product-set jobs and versioned recipes. Same recipe as v0.4.0;
  CI not watched in that session, check the release run before announcing.
- 2026-09-06: Operations rail = always-present 56px tool strip (`rail-strip`,
  `strip-<slug>`) plus an open panel (`rail`) sized by `rail_size` and a
  6px grab edge (`rail-resize`); the workspace root follows the drag in
  `drag_rail`, and a drag past `RAIL_MIN - RAIL_SNAP` collapses it. Width
  persists as `rail_width=` in settings. `rail_width()` includes the strip,
  so table/gallery/bar layout math already accounts for it.
- 2026-09-06: Preset UI: the chooser menu lists rows only (tests navigate it
  with `up enter`); verbs live behind the `recipe-actions` dots. Save changes
  bumps `revision` via `update_recipe`; Save as / Rename use the inline prompt
  (`recipe_prompt`, `open_recipe_prompt`). `PopupMenuItem::label` is a fine
  section header; `Switch::small()` exists; clippy wants `update(cx, act)`
  for a `fn` pointer, not a closure around it.
- 2026-09-06: `ux/scenarios.json` window-relative clicks accept negative x/y
  measured from the right/bottom edge; `panel-collapsed` (-28,70) hits the
  strip's first tool and `preset-actions` (-90,206) hits the dots at 1100×720.
- 2026-09-06: Released v0.6.0 from main: tool strip + resizable panel +
  preset rework on top of the v0.5.0 header batch. CI not watched in that
  session either; check both release runs before announcing.
