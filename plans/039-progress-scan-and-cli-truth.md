# Plan 039: Un-reverse the progress bar, scan folders named `optimized`, and make the CLI honest

> **Executor instructions**: Follow this plan step by step. Run every
> verification command and confirm the expected result before moving to the
> next step. If anything in the "STOP conditions" section occurs, stop and
> report — do not improvise. When done, update the status row for this plan
> in `plans/README.md` — unless a reviewer dispatched you and told you they
> maintain the index.
>
> **Drift check (run first)**: `git diff --stat 97cb1a5..HEAD -- src/audit/statusbar.rs src/scan.rs src/main.rs`
> If any in-scope file changed since this plan was written, compare the
> "Current state" excerpts against the live code before proceeding; on a
> mismatch, treat it as a STOP condition. (Plan 034 also edits `src/main.rs`
> — its edits are confined to `run_window`/modules; this plan's are confined
> to `parse_args`/`convert_headless`/`main`.)

## Status

- **Priority**: P1
- **Effort**: S
- **Risk**: LOW
- **Depends on**: none
- **Category**: bug
- **Planned at**: commit `97cb1a5`, 2026-08-23

## Why this matters

Three small, certain, user-visible wrongs. (1) The conversion progress bar
renders backwards: full at 0 files done, empty at the last file — the app's
longest-running operation reports its progress inverted. (2) Opening a
folder named `optimized` (the app's own output folder — the obvious folder
to open to inspect results) scans as empty, because the output-skip rule
matches path components of the *absolute* path, root included. (3) The
scriptable CLI lies: `--convert` exits 0 even when every file failed, an
unparseable `--quality`/`--max-edge` value silently keeps the default, and
any typo'd flag is adopted as the folder path.

## Current state

- `src/audit/statusbar.rs:22-35` — the converting branch fills the `bar`
  slot with a *progress* fraction:

```rust
        let (headline, tone, detail, bar, tag) = if self.converting {
            let done = self.results.len() + self.failures.len();
            let total = self.active_target_count.unwrap_or(target_count);
            (
                format!("{done} of {total}"),
                cx.theme().foreground,
                format!("Converting to {} {}…", ...),
                Some((done as f32 / total.max(1) as f32, cx.theme().primary)),
                None,
            )
```

  while the consumer at `src/audit/statusbar.rs:115-117` expects the share
  **remaining** (the comment at `:20-21` says so: "a headline, the share it
  leaves behind, and a sentence of detail"):

```rust
        let (fraction, colour) = bar
            .map(|(remaining, colour)| (1. - remaining, colour))
            .unwrap_or((0., gpui::transparent_black()));
```

  The other two branches pass `after/before` and `projected/source` — both
  genuine "remaining share" values. Only the converting branch disagrees
  with the convention, so the meter runs backwards exactly there.

- `src/scan.rs:143-166` — the walk and the skip rule:

```rust
    for entry in WalkDir::new(root).follow_links(false) {
        ...
        if file
            .path()
            .components()
            .any(|part| part.as_os_str() == OUTPUT_DIR)
        {
            if file.path().starts_with(&output_root) {
                existing_output += 1;
            }
            continue;
        }
```

  `file.path()` is root-prefixed, so a root of `~/Photos/optimized` (or any
  root with an ancestor named `optimized`) makes every file match and the
  scan returns empty — not even counted as `existing_output`, because
  `output_root` is `root.join("optimized")`.

- Pinned scan tests: `the_output_folder_is_not_audited_as_input`
  (`src/scan.rs:376`) and
  `only_this_roots_output_folder_counts_as_what_a_run_would_replace`
  (`src/scan.rs:397`). Both must keep passing unchanged.

- `src/main.rs:57-95` — `parse_args` reads `std::env::args()` directly;
  value flags swallow parse failures:

```rust
            "--max-edge" => {
                if let Some(value) = rest.next().and_then(|value| value.parse().ok()) {
                    max_edge = MaxEdge(Some(value));
                }
            }
            ...
            _ => root = Some(PathBuf::from(argument)),
```

- `src/main.rs:245-248` — `--convert` cannot fail:

```rust
    if args.convert {
        convert_headless(&root, &entries, args.format, args.quality, args.max_edge);
        return;
    }
```

  `convert_headless` (`:98-174`) counts `failed` into a local tuple, prints
  it, and returns `()`.

- `src/main.rs` has no `#[cfg(test)]` module at all. Test conventions:
  snake_case behaviour sentences; pure-function tests live in the same file
  as the function (see `src/scan.rs`'s tests, `src/settings.rs`'s tests).

## Commands you will need

| Purpose | Command | Expected on success |
|---|---|---|
| Tests | `cargo test --locked` | all pass (87 passed + 1 ignored at baseline) |
| Scan tests | `cargo test --locked scan` | pass |
| Lint | `cargo clippy --all-targets -- -D warnings` | exit 0 |
| Format | `cargo fmt --check` | exit 0 |
| CLI check | `cargo run --release -- /nonexistent --convert; echo $?` | prints an error, then `2` |

(These are bash/POSIX commands — `$?`. Only if you are in an interactive
fish shell does it become `$status`.)

## Scope

**In scope** (the only files you should modify):
- `src/audit/statusbar.rs` — one branch of the bar tuple
- `src/scan.rs` — skip rule + one new test
- `src/main.rs` — `parse_args`, `convert_headless` exit status, tests module
- `plans/README.md` — status row

**Out of scope** (do NOT touch):
- The meter widget or the other two bar states.
- `scan::probe`, RAW handling, `existing_output` semantics for nested roots.
- Any UI conversion-loop change (recorded separately as CODE-10).
- `run_window` and the window/menu code in `src/main.rs` (plan 034's area).

## Git workflow

- Branch: `improve/039-progress-scan-cli`
- Conventional commits; one concern per commit — this plan is naturally three
  commits (`fix: draw conversion progress forward`, `fix: scan a root named
  optimized`, `fix: honest exit codes and flag parsing for --convert`).
- Do NOT push or open a PR unless the operator instructed it.

## Steps

### Step 1: The progress bar runs forward

In `src/audit/statusbar.rs`, converting branch, change the bar term to the
remaining share so it matches the consumer's `1. -` convention:

```rust
                Some((
                    1. - done as f32 / total.max(1) as f32,
                    cx.theme().primary,
                )),
```

Add one line to the comment at `:20-21` naming the contract explicitly, e.g.
"the share it leaves behind — every branch passes *remaining*, and the
consumer draws `1 - remaining`".

**Verify**: `cargo test --locked` → all pass. This is a rendered-state
change: under the repo's live visual proof contract, launch the release app
on a folder large enough to watch (hundreds of files), start a conversion,
and capture the bar early and late (`grim` under the host compositor) — the
bar must grow. If you cannot run the app, state that and leave the capture
to the reviewer.

### Step 2: The skip rule goes relative

In `src/scan.rs`, change the component check to the path *relative to root*:

```rust
        let relative = file.path().strip_prefix(root).unwrap_or(file.path());
        if relative
            .components()
            .any(|part| part.as_os_str() == OUTPUT_DIR)
        {
            if file.path().starts_with(&output_root) {
                existing_output += 1;
            }
            continue;
        }
```

Add a test alongside the two pinned ones, following their tempdir pattern
exactly (copy the setup shape from `the_output_folder_is_not_audited_as_input`
at `src/scan.rs:376`):

- `a_root_named_optimized_is_still_audited` — create a scratch dir whose
  final component is literally `optimized`, put one decodable image in it
  (the existing tests show how they materialise one), scan it, assert
  `entries.len() == 1`.

**Verify**: `cargo test --locked scan` → all pass, including the new test and
the two pinned ones unchanged.

### Step 3: `parse_args` becomes strict and testable

In `src/main.rs`, restructure without changing accepted good inputs:

```rust
fn parse_args() -> Args {
    match parse_args_from(std::env::args().skip(1)) {
        Ok(args) => args,
        Err(message) => {
            eprintln!("imageguide: {message}");
            std::process::exit(2);
        }
    }
}

fn parse_args_from(rest: impl Iterator<Item = String>) -> Result<Args, String> {
    ...same loop, with three changes...
}
```

The three changes inside the loop:

1. `--max-edge` / `--quality`: a missing or unparseable value is
   `return Err(format!("--max-edge needs a number, got {value:?}"))` (same
   shape for `--quality`, and a missing value — iterator exhausted — is the
   same error with "nothing" as the value) instead of silently keeping the
   default.
2. Unknown options are collected, not adopted as paths and not fatal here:
   add `unknown: Vec<String>` to `Args`; in the catch-all arm,
   `if argument.starts_with('-') { unknown.push(argument); continue; }`.
   Strictness is decided by the *mode*, in `main`:
   - `--convert` (scriptable): any unknown option →
     `eprintln!("imageguide: unknown option {first}"); std::process::exit(2);`
     — a typo'd `--avfi` must not convert a whole folder with the wrong
     settings and exit 0.
   - windowed launch: `eprintln!("imageguide: ignoring unknown option {argument}")`
     per entry, then proceed — macOS LaunchServices and login items can pass
     process arguments no user typed, and exiting on them means the app icon
     bounces once and dies.
3. Support the conventional `--` end-of-options delimiter, so a path that
   itself starts with a dash stays expressible (today `imageguide -photos`
   works because the catch-all takes anything; change 2 would otherwise
   orphan such paths): on `"--"`, treat every remaining argument as a plain
   path (the last one wins, matching the existing catch-all behavior).
4. Everything else still becomes `root` (unchanged).

**Verify**: `cargo build --release --locked` → exit 0.

### Step 4: `--convert` reports failure

A headless run has three failure sources, and all three must reach the exit
code — conversion failures alone are not enough, because a file whose
`probe` failed never reaches `entries` at all: it lands in
`Scan::unreadable` (`src/scan.rs:191`), and unreadable *folders* land in
`walk_errors`. A script must not read "everything converted" from a run
that could not even open half the input.

Make `convert_headless` return the failure count: change its signature to
`-> usize`, return `failed` (from the totals tuple at `src/main.rs:152`).
At the call site, fold in the scan's own failures (both are in scope there:
`scanned.unreadable`, `scanned.walk_errors` — note the single-file path
builds an empty `Scan`, so this is safe for both):

```rust
    if args.convert {
        let failed = convert_headless(&root, &entries, args.format, args.quality, args.max_edge);
        let unread = scanned_unreadable_count + walk_error_count; // use the locals in scope
        if unread > 0 {
            eprintln!("imageguide: {unread} files or folders could not be read");
        }
        std::process::exit(if failed + unread == 0 { 0 } else { 1 });
    }
```

While in `convert_headless`, make its closing `println!("written to {}", out_dir.display())`
(`src/main.rs:173`) conditional on at least one successful conversion
(`entries.len() - failed > 0` — the same numbers the summary line already
uses). An all-failed or empty run currently claims "written to
…/optimized" when nothing was created — the exact dishonesty this plan
exists to remove.

(Adapt the two counts to the actual locals — `scanned` may have been
partially moved into `entries` by this point; capture the counts before the
move, next to the existing `walk_errors` printing loop at `src/main.rs:241-243`.)

**Verify** (bash):
- `cargo run --release -- /nonexistent --convert; echo $?` → error + `2`.
- Scratch dir with one valid image: `cargo run --release -- <dir> --convert; echo $?` → `0`.
- Same dir plus a text file named `fake.jpg`:
  `cargo run --release -- <dir> --convert; echo $?` → `1` (the fake lands
  in `unreadable`, and unreadable input is a failed run).
- `cargo run --release -- <dir> --convert --avfi; echo $?` → "unknown
  option" + `2`, and NO files written.

### Step 5: The first tests in `main.rs`

Add a `#[cfg(test)] mod tests` at the bottom of `src/main.rs` (it will be
the file's first — keep it small and pure):

- `flags_parse_into_their_fields` — a table over
  `["--convert", "--avif", "--quality", "40", "x"]`-style slices asserting
  format/quality/root land right (build the iterator with
  `vec.into_iter().map(String::from)`).
- `a_bad_quality_value_is_an_error_not_a_default` —
  `["--quality", "abc"]` → `Err` containing `--quality`.
- `a_bad_max_edge_value_is_an_error_not_a_default` —
  `["--max-edge", "abc"]` → `Err` containing `--max-edge`.
- `a_missing_value_is_an_error` — `["--quality"]` (end of argv) → `Err`;
  same assertion for `["--max-edge"]`.
- `an_unknown_option_is_collected_not_a_path` — `["--nope", "/tmp"]` →
  `Ok`, `root == Some("/tmp")`, `unknown == ["--nope"]`.
- `a_double_dash_ends_option_parsing` — `["--", "-photos"]` → `Ok`,
  `root == Some("-photos")`, `unknown` empty.
- One more Step 4 check, in the fixture list there: an all-failed run's
  stdout does NOT contain `written to` (fold into the existing `fake.jpg`
  fixture: run it in a folder with ONLY the fake, expect exit 1 and no
  `written to` line).
- `restored_window_size` already has behaviour worth pinning while you are
  here? No — out of scope; do not add unrelated tests.

Note: `Quality`/`MaxEdge` comparisons — check what they derive
(`src/convert.rs`); assert on `.label()` strings if `PartialEq` is missing,
rather than adding derives to out-of-scope files. (`Quality::lossy(40.)`
labels are stable — see `quality.label()` usage at `src/main.rs:161`.)

**Verify**: `cargo test --locked` → all pass, including ≥3 new tests.

### Step 6: Full gates

**Verify**: `cargo test --locked`, `cargo clippy --all-targets -- -D warnings`,
`cargo fmt --check` → all green.

## Test plan

Listed in Steps 2 and 5: one scan test (root named `optimized`), six
`parse_args_from` tests. The progress-bar fix is covered by the visual proof
capture (Step 1) — there is no pure seam worth extracting for a one-term
arithmetic fix, and inventing one would be noise.

## Done criteria

- [ ] `cargo test --locked` exits 0 with ≥7 new tests
- [ ] `cargo clippy --all-targets -- -D warnings` exits 0
- [ ] `cargo fmt --check` exits 0
- [ ] `grep -n "1. - done" src/audit/statusbar.rs` → one match
- [ ] `grep -n "strip_prefix(root)" src/scan.rs` → one match in `scan`
- [ ] `grep -n "parse_args_from" src/main.rs` → definition + wrapper + tests
- [ ] The four bash CLI checks in Step 4 produce exactly the listed exit
      codes (`2`, `0`, `1`, `2`)
- [ ] `git status` (in your worktree) shows changes only in the in-scope files
- [ ] `plans/README.md` status row updated

## STOP conditions

Stop and report back (do not improvise) if:

- Either pinned scan test fails after Step 2 — the relative check changed
  more than the root-naming case, which means an ancestor-vs-descendant
  subtlety this plan got wrong.
- `convert_headless`'s return-type change ripples beyond its one call site.
- Anything in Step 3 changes the parse of a currently-valid invocation from
  the README (`imageguide ~/folder --convert --avif`,
  `--quality 40`, `--max-edge 1600`, `--lossless`, `--grid`, `--webp`).

## Maintenance notes

- The bar tuple now has a stated contract ("remaining share"); if a fourth
  state is ever added, it must pass remaining, not progress — the comment
  from Step 1 is the guard.
- The CLI now exits 1 on partial failure; any future CI/script usage of
  `--convert` can rely on it. Exit 2 remains "bad invocation".
- Deferred, recorded separately: moving the startup scan off the launch path
  (CODE-14), collapsing the two conversion loops and adding cancellation
  (CODE-10), output-name stability across sort orders (CODE-08).
