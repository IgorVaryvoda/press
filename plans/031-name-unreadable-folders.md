# Plan 031: Report the folders the scan could not read

> **Executor instructions**: Follow this plan step by step. Run every
> verification command and confirm the expected result before moving on. If a
> STOP condition occurs, stop and report; do not improvise. When done, update
> the status row for this plan in `plans/README.md`.

## Status

- **Priority**: P1
- **Effort**: M
- **Risk**: LOW — additive data on an existing struct, one new notice line
- **Depends on**: none
- **Category**: bug / truthful reporting
- **Planned at**: `origin/main` `8e5628f` (post 029/030 merge), 2026-08-22

## Why this matters

The app's contract is truthful reporting: failures keep filenames, stale
results are dropped, files that grew are reported as grown. The scan breaks
that contract for whole subtrees. `scan::scan` walks with
`.filter_map(Result::ok)`, so a permission-denied directory — a mounted drive
that went away, another user's home, a broken symlink loop — silently
disappears. The audit says "5,732 images" over a folder that holds more, and
the user has no way to know part of their library was never looked at.
`unreadable` exists precisely so decode failures are *named*; walk failures
deserve the same treatment.

This also revives the retired batch-1 finding 002's root cause: the scanner's
failure contract was never implementable while walk errors were discarded.

## Current state

`src/scan.rs:139-141` — errors discarded at the walk site:

```rust
for file in WalkDir::new(root)
    .follow_links(false)
    .into_iter()
    .filter_map(Result::ok)
    .filter(|entry| entry.file_type().is_file())
{
```

Everything below the `filter_map` only ever sees successful entries. A
directory whose `readdir` fails yields an `Err` entry in this iterator and is
dropped here; a file inside a readable directory whose metadata cannot be
stat'd fails the same way.

`Scan` (`src/scan.rs:30-49`) already models the three kinds of "not counted"
the app knows about: `skipped_raw` (counted by design), `unreadable` (named),
`existing_output` (counted, free). This plan adds the fourth kind: places the
walk itself could not enter.

Consumers of `Scan` that construct it literally and must gain the new field:

- `src/main.rs:221` (single-file branch in `main`)
- `src/audit/mod.rs:1013` (single-file branch in `request_path`)
- `src/audit/tests.rs:382` (test fixture)

Rendering surface: `Audit::notices` (`src/audit/view.rs:1535`) already renders
`would not decode: …` from `self.unreadable`; this plan adds a sibling line,
and `install_dataset` (`src/audit/mod.rs:928`) copies the field next to its
`unreadable` assignment. Headless output in `main`
(`src/main.rs:232-238`) prints counts; walk failures go to stderr so stdout
stays machine-parsable.

## Commands you will need

| Purpose | Command | Expected |
|---------|---------|----------|
| Fast check | `cargo check --locked` | exit 0 |
| Tests | `cargo test --locked` | all pass, screenshot still ignored |
| Clippy | `cargo clippy --all-targets -- -D warnings` | exit 0 |
| Format | `cargo fmt --check` | exit 0 |

## Scope

**In scope**:
- `src/scan.rs` (`Scan`, `scan()`, tests)
- `src/main.rs` (literal `Scan` construction; headless stderr lines; tests if
  any live there after the split)
- `src/audit/mod.rs` (`Audit` field, `install_dataset`)
- `src/audit/view.rs` (`notices`)

**Out of scope**:
- Retrying failed entries or any interactive handling.
- Recursive permission diagnosis (naming the deepest failing path is enough;
  `WalkDir` reports the path it reached when the error occurred).
- Changing walk order, raw skipping, or `OUTPUT_DIR` logic.

## Git workflow

Commit as one change: `fix: name the folders the scan could not read`.
Do not push unless asked.

## Steps

### Step 1: Failing test first

In `src/scan.rs` tests module, write the behaviour test. Skip gracefully when
the environment can still read a `chmod 000` directory (root):

```rust
#[test]
fn a_folder_it_cannot_enter_is_named_not_swallowed() {
    let root = temp_dir("walk-error");
    let locked = root.join("locked");
    std::fs::create_dir_all(&locked).unwrap();
    // Any decodable image works; reuse whatever fixture the neighbouring
    // unreadable test writes.
    std::fs::write(root.join("ok.png"), minimal_png()).unwrap();
    std::fs::set_permissions(&locked, std::fs::Permissions::from_mode(0o000))
        .unwrap();

    let scanned = scan(&root);

    // Restore before asserting so cleanup cannot fail on the locked dir.
    std::fs::set_permissions(&locked, std::fs::Permissions::from_mode(0o755))
        .unwrap();

    if scanned.entries.len() == 2 {
        // Root read the locked folder anyway; nothing to assert here.
        return;
    }
    assert_eq!(scanned.entries.len(), 1);
    assert_eq!(scanned.walk_errors, vec![locked]);
}
```

The root-detection-by-behaviour above avoids adding `libc` for one call. Use
the same minimal-image fixture helper the existing `unreadable` tests use
(`temp_dir` already exists in this module).

Run: `cargo test --locked a_folder_it_cannot_enter`
Expected: FAIL — `no field walk_errors on Scan`.

### Step 2: Carry the field

Add to `Scan` beside `unreadable`:

```rust
/// Directories the walk could not enter, named like `unreadable`: "permission
/// denied" somewhere in the tree means every number above is short and the
/// user deserves to know where.
pub walk_errors: Vec<PathBuf>,
```

Replace the walk preamble:

```rust
let mut walk_errors = Vec::new();
for entry in WalkDir::new(root).follow_links(false) {
    let Ok(entry) = entry else {
        // WalkDir attaches the path it reached when an entry fails; keep
        // whatever name it offers.
        if let Some(path) = entry.as_ref().unwrap_err().path() {
            walk_errors.push(path.to_path_buf());
        }
        continue;
    };
    if !entry.file_type().is_file() {
        continue;
    }
    // … existing OUTPUT_DIR / raw / candidate body unchanged …
}
```

Keep the rest of the function byte-identical. Return `walk_errors` in `Scan`.
Fix every literal `Scan { … }` construction listed under Current state with
`walk_errors: Vec::new()`.

Run: `cargo test --locked a_folder_it_cannot_enter`
Expected: PASS.

### Step 3: Surface it in the window

In `src/audit/mod.rs`: add `pub walk_errors: Vec<PathBuf>` to `Audit` beside
`unreadable`, set it in `install_dataset` next to
`self.unreadable = scanned.unreadable;`.

In `src/audit/view.rs` `notices()`, add a sibling line modelled on the
`unreadable` one:

```rust
if !self.walk_errors.is_empty() {
    parts.push(format!(
        "could not enter: {}",
        named(self.walk_errors.iter().filter_map(|path| {
            path.file_name().map(|name| name.to_string_lossy().into_owned())
        }))
    ));
}
```

Use the same `named()` truncation the unreadable line uses; do not invent a
second vocabulary for the same idea.

### Step 4: Surface it headless

In `main`'s pre-window summary, after the existing `println!` of counts:

```rust
for path in &scanned.walk_errors {
    eprintln!("imageguide: could not enter {}", path.display());
}
```

Stderr, not stdout: the convert pipeline's stdout stays machine-parsable.

### Step 5: Gates

```bash
cargo test --locked
cargo clippy --all-targets -- -D warnings
cargo fmt --check
```

Expected: all green, test count up by at least one, screenshot still ignored.

## Done criteria

- [ ] A folder with a `chmod 000` subfolder scans the readable siblings AND
      names the locked folder in the window's notice line and on stderr
      headless.
- [ ] No existing notice text changed; `would not decode` still means decode
      failures only.
- [ ] Suite, clippy, fmt green; new test passes as non-root and skips clean
      as root.
- [ ] No changes outside `src/scan.rs`, `src/main.rs`, `src/audit/mod.rs`,
      `src/audit/view.rs`.

## STOP conditions

- `WalkDir`'s error type stops exposing `.path()` after a dependency bump —
  report instead of degrading to unnamed counting.
- The GPUI test harness cannot reach `notices()` without launching a real
  window on this host — verify the notice by launching the release binary on
  a prepared fixture folder and capturing with grim, per the repo's live
  visual proof contract.
