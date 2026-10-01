# Napkin (Claude sessions)

A richer napkin exists at `.Codex/napkin.md`. Read it first; it holds the
host gotchas, benchmarks, and user preferences. This file adds what Claude
sessions learn.

## Corrections
| Date | Source | What went wrong | What to do instead |
|------|--------|-----------------|--------------------|
| 2026-08-31 | self | `ls` is aliased (eza) and rejects `--icons` when given odd args in non-interactive fish | Use `/bin/ls` in Bash tool calls |
| 2026-09-19 | self | `ydotool mousemove --absolute` saturates at the layout corner on this host (pointer acceleration), so every scripted click missed | Place the pointer with `hyprctl dispatch movecursor <global-x> <global-y>`, then `ydotool click 0xC0`. Read the window origin from `hyprctl clients -j` after every resize: Hyprland re-centres a floating window, so a stale grim region captures the desktop |

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
| 2026-09-18 | self | `cargo test <bare_name> -- --exact` ran 0 tests and still exited 0, so two new tests looked green without running | With `--exact` the filter must be the full path (`audit::tests::<name>`); list first with `cargo test -- --list \| grep <name>` |
- 2026-09-18: A new Convert-rail section placed above `handoff_section` failed
  `the_review_card_stays_inside_a_narrow_window` by half a pixel (card 123+331.5
  vs region 87+367). New rail sections go last, after the handoff card, not
  between it and the settings.
- 2026-09-18: Landed A2/A3 saved plans (`codex/workbench-identity`) onto main
  v0.6.8 and added the window surface (`src/audit/plan_actions.rs`, Saved plan
  section in the Convert rail). Three Opus reviews found ten defects, all fixed;
  the worst was `execute_item` writing the plan's AVIF speed into the
  process-wide dial, which is harmless in a CLI process and poisons the window's
  later conversions and its settings file. Gates: 766 passed, clippy, fmt, diff
  --check. Pushed, not merged to main.
- 2026-09-18: Gate runs started while edits are still landing compile a mixed
  tree; their green means nothing. Run the last gate pass after the final edit,
  and treat every earlier pass as a smoke test.
- 2026-09-18: PR CI timing on this repo: ubuntu ~7m, windows ~23m, macOS
  ~1h12m. A macOS job sitting in "Test" for over an hour is normal here, not a
  hang. `ci.yml` has no `concurrency` group, so pushing a fix does not cancel
  the in-flight run: both report, and the slow macOS answer is not lost.
- 2026-09-18: Windows CI failed clippy (not tests) on `-D warnings` dead code:
  `tests/cli_contract.rs` helpers used only by `#[cfg(unix)]` FIFO tests. Gate
  the helper with `#[cfg(unix)]` too. Local Linux clippy cannot catch this;
  sweep for helpers whose callers are all unix-gated before pushing.
| 2026-09-18 | self | Ran `cargo clippy --all-targets --features updater` without `-- -D warnings` and called the D1 merge green; it carried 3 warnings (one dead parameter, two arg-count lints) that CI turns into errors | Always run CI's exact command, `-- -D warnings` included. A warning count of 0 is the gate, not the exit code |
| 2026-09-18 | CI | A new job.rs test used `PathBuf::from("/tmp")` as an absolute job root; Windows refused it ("job root /tmp is not absolute") and only CI saw it | Use `std::env::temp_dir()` for any absolute-path fixture. The napkin's Windows rule covers separators; this is the same rule for roots |
- 2026-09-18: The v0.6.9 macOS release job died at minute 74, after all 718
  tests passed, on `codesign ... --timestamp` for the twelfth bundled dylib:
  "A timestamp was expected but was not found" — Apple's timestamp service, not
  our code. The packaging step now retries three times (signing is idempotent,
  it replaces the signature it finds). Re-run the failed job when it happens
  again; the assets already built are not lost.
| 2026-09-19 | self | `pkill -f 'target/debug/press'` killed the Bash tool's own wrapper shell, because the wrapper's command line contains that pattern (exit 144, no output) | Kill the app by exact name (`pkill -x press`) or by PID; never `pkill -f` on a string your own command contains |
- 2026-09-19: Checked checkboxes looked blank because `init_theme` set the
  colour set but never `theme.tokens.primary`. Checkbox, radio, switch and tab
  fill from the TOKEN set and draw their glyph in `primary_foreground`: stock
  white fill plus the app's white tick = an empty white square. Set
  `tokens.primary{,_hover,_active,_foreground}` beside the colour set; a test
  pins the two halves together.
- 2026-09-19: Folders no longer open fully ticked. Removing the two
  `select_all_visible()` calls broke 58 tests, all of which assumed a pre-ticked
  folder; the fix is for the test harnesses to tick through `toggle_select_all`
  (the control a person clicks), not to restore the behaviour. `grim` captures
  this Hyprland session fine — the napkin's "no pixels" note is about the
  nested gamescope path, not the live desktop.
- 2026-09-19: UI audit run. `./scripts/ux-eval capture <scenario> --fixture <dir>
  --allow-external-fixture --skip-build --binary target/debug/press` works with the
  debug binary; no release build needed. Two caveats: `--fixture` overrides a
  scenario's own launch (so `empty-start` must run without it), and the scripted
  clicks in `conversion-result`, `filter-no-results` and `local-ai-upscale` are
  stale — they hit nothing in the current layout, and the capture then shows the
  plain audit list rather than the state the scenario names. Drive interactive
  states on the live Hyprland desktop instead.
- 2026-09-19: Audit fixture that exercises the real list:
  `/home/igor/.cache/press-ux-audit/shop` (13 images, Pillow noise + Lanczos
  upsamples, one transparent PNG, one PNG named `.jpg`, one long filename, an
  `archive/` subfolder). Light `docs` images cannot show chips, truncation or the
  stacked result column.
- 2026-09-19: UI audit fixes landed on `fix/ui-audit-20260919`. Two root causes
  worth remembering: `install_tree_page` expanded every ancestor of the opened
  folder without listing it, so each kept a permanent "Loading…" child and hid
  its siblings; and `Output::Replace.root(audited) == audited`, so treating the
  output tree as "hidden" deleted the open folder from the browser. Any new
  ancestor listing must re-`reveal_open_folder`, because rows arriving above the
  open folder move it out of view.
- 2026-09-19: gpui-component table cells are `py_1` inside a 36px row, so a
  stacked two-line cell has 28px. Two 11px lines at the default line height
  measure ~31 and the table's `overflow_hidden` clips the second one. Give
  stacked cell lines an explicit `line_height`. `DataTable::stripe(true)` also
  paints striped filler rows below the last real row; Press draws its own zebra
  in `render_tr` instead.
| 2026-09-19 | self | A fresh session does not inherit Hyprland's environment, so `hyprctl` failed with "HYPRLAND_INSTANCE_SIGNATURE not set" and the app launched with no window | Export `HYPRLAND_INSTANCE_SIGNATURE=$(/bin/ls /run/user/1000/hypr/)` and `WAYLAND_DISPLAY=wayland-1` in every Bash call that launches or drives the app |
- 2026-09-19: Second UI audit (round two) on `fix/ui-audit-20260919`. The 22
  round-one fixes all held. The worst new finding is pre-existing: `browse`
  receives `self.output.root(&path)` (mod.rs ~1593), which for `Output::Replace`
  is the audited folder itself, and `scan::input_root` (scan.rs:958) refuses a
  root inside the output. So while replace mode is on, every rescan fails —
  Restore originals, re-opening the folder from the tree, and the subfolder
  toggle — and the stale results stay on screen. `browser_output_root` is a
  different value and does not feed the scan.
- 2026-09-19: `AuditTable::set_viewport_width` (table.rs ~259) keeps the previous
  column order and appends only new columns, so each layout change permutes the
  header. After a run the Options gutter lands mid-table and Result drifts away
  from File size. Second audit fixture: `/home/igor/.cache/press-audit2/shop`
  (adds `tiny-noise.jpg`, which grows on re-encode, and an `optimized/
  product-01.webp/` directory that forces exactly one conversion failure).
| 2026-09-19 | user | Drove the app on the live Hyprland desktop for a verification pass, stealing the pointer and focus while the user was working | Capture in headless gamescope instead. `/tmp/press-shot.py` (same machinery as `scripts/ux-eval`: `gamescope --backend headless`, `xdotool --window <xid>` on the nested X display, one `gst-launch-1.0 pipewiresrc` frame) takes `--size`, `--fixture`, `--arg` and repeatable `--do 'click X Y' / 'key ...' / 'type ...' / 'wait N'`, so a verification pass needs no entry in the tracked `ux/scenarios.json` and never touches the user's session |
- 2026-09-19: Round-two fixes. Two lessons beyond the findings themselves:
  a child with `.w_full()` inside the rail's content-sized `flex_col` made the
  whole panel lay out wider than its 320px and every control ran off the edge —
  put the width constraint on the scrolling container (`rail-settings`), not on
  the prose. And the replace-mode row rename must key on
  `conversion_destination`, not on `self.output`: flipping the switch after an
  `optimized/` run renamed every row to an output that had never taken an
  original's place. Both only showed up in a real capture; the suite was green.
- 2026-09-19: Not bugs, checked and dropped: "Save changes" on a built-in preset
  IS disabled (it reads dim only when zoomed); the toast stack's collapsed peek
  at a bottom anchor is the library's intended stacking, not an overlap.
- 2026-09-26: Sirv/Studio integration items 1-5 (credits, publish originals,
  CDN delivery check, alt text, batch-API evaluation). Facts worth keeping:
  Press talks to Studio only through `/api/zapier/*` (source in
  `~/Projects/ai-image-tools/src/start/routes/api/zapier/`); direct routes
  return `credits_used`, `/me` returns `credits`. The Sirv CDN answers HEAD
  with `content-length`/`content-type` and re-encodes every request (a JPEG
  with no params came back AVIF), so `?s=N&scale.option=noup` + a browser
  Accept header measures what a visitor gets. `plans/` is gitignored: plan
  README edits stay local; tracked status goes in `docs/`. The Studio key in
  `~/.config/imageguide/studio` on this host is a dev.sirv.studio key
  (production says 401). Debug builds take `PRESS_STUDIO_API=https://dev.sirv.studio`;
  the private ux-eval copy copies that key into its sandbox with
  `PRESS_EVAL_STUDIO=1`. Direct Python calls to dev need `User-Agent: node`.
  Dev `/me` `credits` (91817) and a run's `credits_remaining` (94985.9)
  disagree: two wallets; ask Studio which one pays before trusting either.
- 2026-09-26: Headless capture without `/tmp/press-shot.py`: copy
  `scripts/ux-eval` to /tmp, hardcode `ROOT` and point `SCENARIOS_PATH` at a
  temp JSON, then `capture <name> --fixture <copy> --allow-external-fixture
  --skip-build --binary target/debug/press --output-root /tmp/...`. Waits cap
  at 30 s per step; chain two for a debug-build conversion. At 1100×720 the
  Convert commit button is at (938, 662) after `ctrl+a`.
| 2026-09-26 | self | Ran `<binary> --list` over every `target/debug/deps/press-*` to find the test binary; one was the app, which took `--list` as a folder and opened a window on the user's live desktop | Get the test binary from `cargo test --bin press --no-run` (its `Executable` line). Never run an unknown `press-*` binary outside headless gamescope |
| 2026-09-26 | self | `updates_wait_for_download_and_apply_and_keep_dismissed_state` failed ~1 in 3 runs; looked like my change | Something on this host sends `HEAD /` (Host: localhost:PORT) to new listening ports of processes whose cwd is this repo. Same binary from `/tmp` fails 0/12, from the repo 2/12; gamescope does not isolate it. Run that test from `/tmp`, and read the request before blaming a diff |
- 2026-09-26: Live Sirv proof in headless gamescope: the private ux-eval
  copy copies `~/.config/imageguide/sirv` into its temp config when
  `PRESS_EVAL_SIRV=1`. Sirv folder `/Image Editor Backup 202310111041` holds
  one image (`marta.jpg`, 172483 B); a local folder holding its API-downloaded
  bytes pairs as "same size", so delivery checks and Publish-originals run
  with no upload. Clicks at 1100×720: Sirv header (107,20), that folder
  center(-60,+28), Pair center(-85,+72).
- 2026-09-19: Released v0.7.0 from main 5ac41d0 (two rounds of UI audit fixes:
  d7ac68d, ebe21cc, eda8f78). Followed the napkin's own gate this time — pushed
  main, waited for all three CI OSes green (run 35441369100, 1h4m; macOS is the
  long pole at ~60 min), and only then bumped, tagged and pushed. Release run
  35444451833 was green on all five package jobs in 1h40m; the macOS Intel job
  alone took ~100 min and the timestamp retry added in v0.6.9 held. All 16
  assets published, `latest.json` serves v0.7.0.
- 2026-09-30: Header is now GNOME Files style: menu, sidebar toggle, Back and
  Forward (`history-back`/`history-forward`, Alt+←/→, mouse side buttons), a
  `path-bar` pill with `crumb-N` items, then Sirv, filter, view, rail toggle.
  Sirv is no longer left-anchored: the two Sirv scenarios click (-410, 20),
  which hits it at 760, 1100 and 1440. History moves in `install_dataset`,
  keyed by `history_step`, so a folder that fails to open costs no history.
  `/tmp/press-shot.py out.png WxH <fixture> '<json actions>'` wraps
  `ux-eval`'s `capture_frame` for one-off headless shots.
- 2026-09-30: Sirv split view (`sirv-split`): `sirv_rows` is built in
  `refresh_sirv_counts`, merged by key, because `sirv_local_presence` can lag
  a transfer and put one file on both the local and remote-only lists. Paired,
  the header Sirv button exists only while unpaired (it pairs; (-410, 20)
  hits it at 760/1100/1440). Paired, the view switch reads List | Grid | Sirv
  and `set_sirv_split` is the one way in or out; at 1100 the Sirv segment is
  (1030, 22) and Grid (990, 22). "Change Sirv folder…" is in the bar's `⋯`. Live proof
  fixture: `~/.cache/press-sync-demo` + Sirv `/Image Editor Backup 202310111041`;
  `PRESS_SHOT_SIRV=1 /tmp/press-shot.py` copies the real Sirv keys into the
  sandbox. Pulling `marta.jpg` there turns its row green.
| 2026-09-30 | user | Shipped a read-only split view and a four-row Sirv bar; user: "cluttered as fuck", "no action can be performed" | A comparison view must act on what it shows (per-row arrow + ticked-row footer). Keep a status bar to one line; secondary verbs go behind `⋯`. Check the bar at the narrowest real layout (list mode, Convert panel open) before calling it done |
- 2026-09-30: Split view ticks are `sirv_selected` (keys). They survive the
  rescan and re-walk after a transfer (pruned in `refresh_sirv_counts`); only
  pair, unpair and new credentials clear them. xdotool clicks 0.25 s apart can
  merge; put a 1 s wait between scripted row ticks.
- 2026-09-30: Sirv audit round. Pairings persist in `<config>/imageguide/
  sirv-pairs` (`remote<TAB>local`), restored on launch and on folder open;
  window tests never touch it (`remember_pairing_unless_test`,
  `restore_sirv_pairing` is a no-op under `cfg!(test)`). Sync classification
  uses `sirv::paired_key` (no key for subfolder files, the listing is one
  level); `relative_key` stays only for publishing results. A pull adds files
  in place (`adopt_pulled_entry`), never a rescan: a rescan cancels Studio,
  local AI and plan runs. Plain Push stats each file (`Client::exists`, Sirv
  answers 404 for absent) before uploading. `/tmp/press-shot.py` takes
  `"button": 3` for right-clicks and `PRESS_SHOT_PAIR` to seed a pairing.
| 2026-09-30 | self | Renamed the sync words to "in sync / different" to match the bar; a test (`sirv_size_evidence_words_name_only_sizes`) guards a deliberate rule that size equality is evidence, never "synced" | Size-based states say "same size" / "different size" everywhere, bar count included. Grep tests for the words before renaming user-facing vocabulary |
- 2026-09-30: Sirv to "10/10" round. Deep pairing: `sirv::walk_remote` lists
  subfolders when the dataset has them (`SirvPairing.deep` follows
  `dataset_subfolders`; `sync_sirv_depth` re-walks after the subfolder rescan).
  `sirv::compared_folder` is the one rule for which folders both sides compare
  (dot, press-originals, packages anywhere; optimized/ at the top) and
  `paired_key(root, path, deep)` applies it locally. Presence is a size map, so
  non-image files both sides hold get rows; counts derive from rows. CDN
  previews: `?w=48&h=48&scale.option=fit&format=jpg` (Sirv returns ~0.6 KB
  JPEG). Live test used Sirv `/press-test` (created, verified, deleted):
  upload, stat-before-upload refusal, Replace on Sirv/here, nested upload,
  download all passed. `/tmp/sirvapi.py {mkdir,ls,upload,rm}` drives the API.
| 2026-10-01 | user | The split view sat on top of List/Grid and only the Sirv button reached it: clicking Grid "did nothing" and the arrows moved a hidden cursor, so "hotkeys don't work" | A full-content view must be a peer in the view switch, and every other view choice must leave it. Reproduce with the user's own settings + pairing (`PRESS_SHOT_SETTINGS`, `PRESS_SHOT_PAIR`) before guessing |
- 2026-10-01: Two host flakes under the full parallel suite, both pass alone:
  `updates_wait_for_download_and_apply_and_keep_dismissed_state` (stray
  connection to its port; run the binary from /tmp) and
  `thumbs::tests::an_os_thumbnail_feeds_a_file_another_app_already_drew`
  ("the store hits"). Rerun before blaming a diff.
| 2026-10-01 | user | Launched `target/debug/press` for the user to try; on a loaded host (load ~40) each List/Grid/Sirv switch pinned the UI thread ~1 s per burst and read as "frozen". Release build measured 0% for the same clicks | Give the user `target/release/press` (`cargo build --release`, ~9 min cold). Debug builds are for headless captures only. To get a stack from a hung window, start it under `gdb -batch -ex run -ex "thread apply all bt"` (ptrace_scope=1 blocks attaching later; no perf on this host) |
