# Plan 037: Make Sirv pull honest — never silently overwrite, never silently truncate

> **Executor instructions**: Follow this plan step by step. Run every
> verification command and confirm the expected result before moving to the
> next step. If anything in the "STOP conditions" section occurs, stop and
> report — do not improvise. When done, update the status row for this plan
> in `plans/README.md` — unless a reviewer dispatched you and told you they
> maintain the index.
>
> **Drift check (run first)**: `git diff --stat 97cb1a5..HEAD -- src/sirv.rs src/audit/sirv_actions.rs src/audit/mod.rs`
> Plan 036 intentionally touches `src/sirv.rs` and `src/audit/sirv_actions.rs`
> first; the excerpts below are quoted at `97cb1a5` — re-verify each against
> live code. If a quoted region changed beyond plan 036's stated edits, STOP.

## Status

- **Priority**: P1
- **Effort**: M
- **Risk**: MED (changes what a pull writes; the failure mode being fixed is data loss)
- **Depends on**: plans/036-sirv-walk-keys-and-guards.md
- **Category**: bug / security (path confinement)
- **Planned at**: commit `97cb1a5`, 2026-08-23 (revised after scrutiny round 1)

## Why this matters

`start_pull` documents itself as additive: "Existing files are never
overwritten — pull is additive by design, so it can never destroy local
work." That is false. "Missing locally" is computed against the *image scan*
(`self.entries`), which drops RAW files, non-images, undecodable images and
everything under `optimized/`. A remote `notes.txt`, `IMG.CR2` or
`optimized/hero.webp` therefore counts as missing, and the pull clobbers the
local copy with `std::fs::write`, silently. The same mis-count keeps the
"Pull N missing" button permanently non-zero. Additional hazards in the same
path: downloads larger than 512 MB are silently truncated by
`.take(MAX_TRANSFER)` and written as successes; the disk writes run on the
UI thread; and a remote key whose *ancestor* is a local symlink
(`root/sub -> /outside`) writes outside the paired folder even though
`safe_key` passes it (that check is purely lexical).

## Current state

- `src/audit/sirv_actions.rs:294-315` — the plan's local side comes from the
  image scan only:

```rust
    pub(super) fn run_pull(&mut self, differing: bool, cx: &mut Context<Self>) {
        ...
        let remote: Vec<sirv::Node> = files.values().cloned().collect();
        let local_sizes: HashMap<String, u64> = self
            .entries
            .iter()
            .filter_map(|entry| {
                sirv::relative_key(&self.root, &entry.path).map(|key| (key, entry.bytes))
            })
            .collect();
        let plan = sirv::pull_plan(&remote, &dir, &local_sizes, differing);
        if plan.is_empty() {
            return;
        }
        let total = plan.len();
        ...creates SirvJob, bumps sirv_generation, then cx.spawn the loop...
```

  `scan::scan` (`src/scan.rs:154-171`) keeps only files `probe()` decoded,
  skips `is_raw`, and skips the whole `optimized/` subtree. So `local_sizes`
  is a subset of the files on disk. `run_pull` is invoked synchronously from
  button handlers on the UI context (`src/audit/view.rs:197-199`), so any
  filesystem sweep placed before its `cx.spawn` runs on the UI thread.

- `src/audit/sirv_actions.rs:337-367` — the transfer loop: only the download
  runs on the background executor; the directory creation and write run in
  the `cx.spawn` future (foreground):

```rust
                let outcome = cx
                    .background_executor()
                    .spawn({
                        let client = client.clone();
                        let remote_path = format!("{dir}/{key}");
                        async move { client.lock().download(&remote_path) }
                    })
                    .await;
                let failure = match outcome {
                    Ok(bytes) => {
                        let target = root.join(key);
                        match target.parent().map(std::fs::create_dir_all) {
                            Some(Err(error)) => Some(format!("{key}: could not create folder: {error}")),
                            None | Some(Ok(())) => std::fs::write(&target, bytes)
                                .err()
                                .map(|error| format!("{key}: {error}")),
                        }
                    }
                    Err(error) => Some(format!("{key}: {error}")),
                };
```

- `src/sirv.rs:424-446` — `download` caps the body and treats a truncated
  read as success:

```rust
            let mut bytes = Vec::new();
            response
                .into_reader()
                .take(MAX_TRANSFER)
                .read_to_end(&mut bytes)
                .map_err(...)?;
            Ok(bytes)
```

  `MAX_TRANSFER` is `512 * 1024 * 1024` (`src/sirv.rs:34`).

- `src/sirv.rs:168-177` — `safe_key` is purely lexical: it rejects absolute
  keys and `..` components but knows nothing about symlinks on disk.

- `src/audit/sirv_actions.rs:151-195` — `walk_sirv_pairing` runs
  `client.walk(&dir)` on the background executor and lands the listing via
  `this.update`, storing `Listing::Ready(HashMap<String /*key*/, Node>)`.
  (Plan 036 adds a pairing-generation guard here; this plan extends the same
  background task.)

- `src/audit/sirv_actions.rs:529-554` — `refresh_sirv_counts` computes
  `to_pull` as remote keys not present among the *scanned image* keys, so it
  over-counts the same way the pull plan does. It is called from UI-thread
  contexts (dataset install, walk landing), so it must not stat the disk.

- `src/sirv.rs:180-198` — `pull_plan` is pure and tested; it takes
  `local_sizes: &HashMap<String, u64>` and treats "not in the map" as
  "missing locally". This plan does not change `pull_plan`; it changes what
  the map contains and where it is built.

- The repo's atomic-write convention: `src/convert.rs:426-436` writes to a
  `.part` sibling and renames; `src/thumbs.rs:137-142` does the same. Those
  write into the app-owned `optimized/` tree; pull targets arbitrary user
  files, so this plan hardens the shape further (exclusive creation, atomic
  no-replace) rather than copying it as-is.

- `walkdir` is already a dependency (`Cargo.toml`), but this plan does not
  need it — presence is computed per remote key, not by walking.

## Commands you will need

| Purpose | Command | Expected on success |
|---|---|---|
| Tests | `cargo test --locked` | all pass |
| One module | `cargo test --locked sirv` | sirv tests pass |
| Lint | `cargo clippy --all-targets -- -D warnings` | exit 0 |
| Format | `cargo fmt --check` | exit 0 |

## Scope

**In scope** (the only files you should modify):
- `src/sirv.rs` — `write_pulled`, `read_capped`, `local_sizes_for` helpers + tests
- `src/audit/sirv_actions.rs` — background plan computation, write path, presence set, counts
- `src/audit/mod.rs` — `sirv_local_presence` field + the dataset-transition rewalk
- `src/audit/tests.rs` — the two regression tests in Step 5
- `plans/README.md` — status row

**Out of scope** (do NOT touch):
- `pull_plan`'s signature or semantics — it is pinned by tests; feed it a
  truthful map instead.
- `scan.rs` — do not grow the scan to carry non-image files; the disk is the
  source of truth here.
- The push loop, retry logic, parallelism (SIRV-04/07/08 — recorded findings).
- The `differing` classification (size-only comparison is a recorded product
  decision, memo E in `plans/batch4-decisions.md`).

## Git workflow

- Branch: `improve/037-sirv-pull-safety`
- Conventional commits, e.g. `fix: pull diffs against the disk, not the image scan`
- Do NOT push or open a PR unless the operator instructed it.

## Steps

### Step 1: Pure helpers in `src/sirv.rs`

Add three helpers near `pull_plan`, with the module's comment voice:

```rust
/// The on-disk size of every remote key's local twin. The image scan is the
/// wrong source for this: it drops RAW files, non-images and `optimized/`
/// output, and a pull that trusts it will overwrite exactly those. The disk
/// is the only honest witness for "exists locally". `symlink_metadata`, not
/// `metadata`: a symlink counts as "something is here".
pub fn local_sizes_for<'a>(
    root: &Path,
    keys: impl IntoIterator<Item = &'a str>,
) -> HashMap<String, u64> {
    keys.into_iter()
        .filter(|key| safe_key(key))
        .filter_map(|key| {
            let meta = root.join(key).symlink_metadata().ok()?;
            Some((key.to_string(), meta.len()))
        })
        .collect()
}

/// Read at most `cap` bytes and refuse a body that reaches past it. The old
/// `.take(cap)` alone returned a silently truncated buffer as success, and a
/// truncated image written to disk is corruption with a success message.
pub fn read_capped(reader: impl std::io::Read, cap: u64) -> Result<Vec<u8>, String> {
    let mut bytes = Vec::new();
    reader
        .take(cap + 1)
        .read_to_end(&mut bytes)
        .map_err(|error| error.to_string())?;
    if bytes.len() as u64 > cap {
        return Err(format!("larger than the {cap}-byte transfer cap"));
    }
    Ok(bytes)
}

/// Write pulled bytes for `key` under `root`.
///
/// Three properties, each load-bearing:
/// - **Confinement.** `safe_key` is lexical; a local symlink ancestor
///   (`root/sub -> /outside`) still redirects the write. Every existing
///   ancestor between `root` and the target is checked with
///   `symlink_metadata` and refused if it is a symlink.
/// - **No silent replace.** Without `overwrite`, the final installation is
///   `hard_link(part, target)`, which fails atomically if the target
///   exists — there is no check-then-rename window for another writer.
/// - **No partial files.** Bytes land in an exclusively created `.part`
///   sibling first (`create_new` refuses to follow a symlink or truncate a
///   leftover), then move into place.
pub fn write_pulled(root: &Path, key: &str, bytes: &[u8], overwrite: bool) -> Result<(), String> {
    if !safe_key(key) {
        return Err("unsafe remote name".into());
    }
    let target = root.join(key);

    // Refuse symlinked ancestors before creating anything through them.
    let mut ancestor = root.to_path_buf();
    let parts: Vec<&str> = key.split('/').collect();
    for part in &parts[..parts.len().saturating_sub(1)] {
        ancestor = ancestor.join(part);
        match ancestor.symlink_metadata() {
            Ok(meta) if meta.file_type().is_symlink() => {
                return Err(format!("{} is a symlink; refusing to write through it", ancestor.display()));
            }
            _ => {}
        }
    }
    if let Some(parent) = target.parent() {
        std::fs::create_dir_all(parent).map_err(|error| format!("could not create folder: {error}"))?;
    }

    let mut part_name = target.as_os_str().to_owned();
    part_name.push(".part");
    let part = PathBuf::from(part_name);
    {
        use std::io::Write;
        let mut file = std::fs::OpenOptions::new()
            .write(true)
            .create_new(true)
            .open(&part)
            .map_err(|error| format!("could not create {}: {error}", part.display()))?;
        file.write_all(bytes).map_err(|error| {
            let _ = std::fs::remove_file(&part);
            error.to_string()
        })?;
    }

    let installed = if overwrite {
        std::fs::rename(&part, &target).map_err(|error| error.to_string())
    } else {
        // hard_link is the atomic "create only if absent" install; a target
        // that appeared since planning fails here instead of being replaced.
        std::fs::hard_link(&part, &target)
            .map_err(|error| {
                if error.kind() == std::io::ErrorKind::AlreadyExists {
                    "exists locally; use overwrite to replace it".to_string()
                } else {
                    error.to_string()
                }
            })
            .and_then(|()| std::fs::remove_file(&part).map_err(|error| error.to_string()))
    };
    if installed.is_err() {
        let _ = std::fs::remove_file(&part);
    }
    installed
}
```

Notes for you: `hard_link` on a filesystem without hardlinks (FAT/exFAT)
fails — the error surfaces per file with the OS message, which is acceptable
and honest; add one sentence saying so in the doc comment. A leftover
`a.jpg.part` from a crash makes `create_new` fail with a clear message
naming the file — also deliberate: silently truncating a file that was
already there is exactly the bug class this plan removes.

**Verify**: `cargo build --release --locked` → exit 0.

### Step 2: `download` uses the capped reader

In `Client::download` (`src/sirv.rs:424-446`), replace the manual
`take`/`read_to_end` with:

```rust
            let bytes = read_capped(response.into_reader(), MAX_TRANSFER)
                .map_err(|message| Error { status: 0, message: format!("download body: {message}") })?;
            Ok(bytes)
```

**Verify**: `cargo build --release --locked` → exit 0.

### Step 3: Plan and write on the background executor

Restructure `run_pull` (`src/audit/sirv_actions.rs:294-398`) so no
filesystem sweep or write touches the UI thread:

1. Keep the early guards (`sirv_pairing`, `sirv_busy`, `Listing::Ready`).
2. Clone what the background needs: `files` (the key→Node map), `dir`,
   `client`, `root`, `differing` — and, for the differing mode only, the
   image-scoped size map built from `self.entries` exactly as the current
   code builds `local_sizes` (that construction reads memory, not disk, so
   it stays on the UI thread where `self.entries` lives).
3. Bump `sirv_generation` and capture it, but do NOT build the plan or the
   job yet.
4. `cx.spawn`: first background task computes the plan. **The two modes use
   different local maps, and that difference is the destructive-scope
   contract.** The additive mode ("Pull N missing") must see every file on
   disk, or it overwrites what the scan ignored. The differing mode ("Take
   N from Sirv") must see ONLY the scanned images, because its button
   count (`changed` in `refresh_sirv_counts`) is image-scoped — feeding it
   the full-disk map would make "Take 1 from Sirv" silently overwrite a
   size-mismatched `notes.txt`, RAW, or `optimized/` file the count never
   mentioned:

```rust
            let plan = cx.background_executor().spawn({
                let root = root.clone();
                let files = files.clone();
                let dir = dir.clone();
                async move {
                    let remote: Vec<sirv::Node> = files.values().cloned().collect();
                    let local_sizes = if differing {
                        entry_sizes // the image-scoped map captured in step 2
                    } else {
                        sirv::local_sizes_for(&root, files.keys().map(String::as_str))
                    };
                    sirv::pull_plan(&remote, &dir, &local_sizes, differing)
                }
            }).await;
```

5. Land via `this.update`: if the generation moved or `sirv_busy()` is now
   true, return without creating a job; if `plan.is_empty()`, return; else
   create the `SirvJob` exactly as today (same fields, same `kind` mapping)
   and `cx.notify()`.
6. The per-file loop stays structurally as today, with one change: the
   background task per file does download **and** write:

```rust
                let outcome = cx
                    .background_executor()
                    .spawn({
                        let client = client.clone();
                        let remote_path = format!("{dir}/{key}");
                        let root = root.clone();
                        let key = key.clone();
                        async move {
                            let bytes = client.lock().download(&remote_path)
                                .map_err(|error| error.to_string())?;
                            sirv::write_pulled(&root, &key, &bytes, differing)
                        }
                    })
                    .await;
                let failure = outcome.err().map(|error| format!("{key}: {error}"));
```

7. On a per-file success, also record presence (Step 4):
   inside the existing `this.update` progress block, add
   `audit.sirv_local_presence.insert(key.clone());`.

Delete the now-unused synchronous `local_sizes` construction and the old
foreground write block.

**Verify**: `cargo test --locked` → all pass;
`grep -n "std::fs::write" src/audit/sirv_actions.rs` → no matches.

### Step 4: Honest counts without UI-thread stats

`refresh_sirv_counts` must not stat the disk (it runs on the UI thread).
Give it a precomputed presence set instead:

- In `src/audit/mod.rs`, add to `Audit`:
  `pub(super) sirv_local_presence: HashSet<String>` (initialise empty with
  the other Sirv fields).
- In `walk_sirv_pairing`'s background task (the one plan 036 already
  reshapes), after the walk succeeds, compute presence **there, off the UI
  thread**, and return it alongside the nodes. The landing closure stores
  both: keys the walk found whose `root.join(key).symlink_metadata()` is
  `Ok` go into `audit.sirv_local_presence`; the listing lands as today. You
  will need `root` cloned into that task (`self.root.clone()` captured next
  to `dir`) and the unpaired keys computed inside the background task —
  reuse `sirv::unpair_remote(&walked_dir, &node.filename)` there, which
  plan 036 already makes correct for nested files. Presence uses
  `sirv::local_sizes_for(&root, keys)` and keeps only the key set
  (`into_keys().collect()`), so there is one stat implementation, not two.
- `refresh_sirv_counts`: replace the `local_keys`-based `to_pull` with

```rust
                let to_pull = files
                    .keys()
                    .filter(|key| !self.sirv_local_presence.contains(*key))
                    .count();
```

  (Keep `to_push`/`changed` exactly as they are — image-scoped by design.)
- `unpair_sirv` clears the set. Pull successes insert into it (Step 3.7),
  so the "Pull N missing" number falls as files arrive and reaches 0.
- **The set is a property of (pairing, local root) — rebuild it when the
  root changes.** The dataset transition keeps a live pairing and refreshes
  counts against the *new* root (`src/audit/mod.rs:501-504` — "the pairing
  survives, the numbers do not"), so a presence set from folder A would
  make folder B's counts lie in both directions. In that transition path,
  when `self.sirv_pairing` is `Some`: clear `sirv_local_presence`, set
  `pairing.files = Listing::Walking`, and call `self.walk_sirv_pairing(cx)`
  instead of only `refresh_sirv_counts()` — the walk recomputes both the
  listing and the presence off-thread against the new root, and
  `refresh_sirv_counts` correctly reports nothing while `Walking`.
- Add one comment on the field: presence is a snapshot from the last walk,
  patched by completed pulls; a file created by hand between walks is
  counted stale until the next walk — the same staleness contract the
  listing itself already has.

**Verify**: `cargo test --locked` → all pass;
`cargo clippy --all-targets -- -D warnings` → exit 0.

### Step 5: Tests

In `src/sirv.rs`'s tests module (scratch dirs: copy the pattern the
credentials-store tests in this file already use):

- `a_plain_pull_refuses_an_existing_file` — write a file, call
  `write_pulled(root, "a.jpg", b"new", false)` → `Err` containing
  "exists locally", file content unchanged.
- `an_overwrite_pull_replaces_atomically` — `write_pulled(root, "a.jpg", b"new", true)`
  → `Ok`, content replaced, no `.part` sibling left.
- `a_missing_parent_is_created` — key two levels deep in a fresh dir → `Ok`.
- `a_leftover_part_file_is_never_truncated` — create `a.jpg.part` with
  content, call `write_pulled(root, "a.jpg", b"new", false)` → `Err`, the
  `.part` content unchanged.
- `a_symlinked_ancestor_is_refused` (`#[cfg(unix)]`) — `root/sub` is a
  symlink to a sibling dir outside root; `write_pulled(root, "sub/a.jpg", ..., false)`
  → `Err` naming the symlink; nothing written outside root.
- `a_symlink_at_the_target_counts_as_existing` (`#[cfg(unix)]`) — symlink at
  `root/a.jpg` → plain pull `Err`.
- `local_sizes_sees_every_file_kind` — a dir holding `a.jpg`, `notes.txt`,
  and `optimized/out.webp`; `local_sizes_for` over those keys returns all
  three with correct sizes.
- `a_body_at_the_cap_passes_and_one_past_it_fails` — `read_capped` over a
  `std::io::Cursor`: exactly `cap` bytes → `Ok` with full content; `cap + 1`
  bytes → `Err` containing "transfer cap".
- `a_differing_pull_never_selects_files_the_audit_does_not_show` — in the
  same sirv tests module: a remote listing containing `notes.txt` (size 5)
  and `a.jpg` (size 5); an image-scoped map containing only `a.jpg` at size
  9; `pull_plan(remote, dir, &map, true)` → exactly `["a.jpg"]`. This pins
  the destructive-scope contract from Step 3: the "Take N" plan can only
  touch keys the count counted.

And in `src/audit/tests.rs` (gpui harness, modeled on the Sirv state tests
around `:345`):

- `opening_another_folder_rewalks_the_pairing` — build an audit with a
  pairing whose `files` is `Listing::Ready(..)` and a non-empty
  `sirv_local_presence`; drive the folder-change path (the same entry the
  existing "opening another folder" tests use); assert **synchronously,
  without advancing the executor** (the rewalk it schedules would attempt
  the network): `sirv_local_presence.is_empty()` and
  `matches!(pairing.files, Listing::Walking)`.

**Verify**: `cargo test --locked` → all pass including the ten new tests.

### Step 6: Fix the stale doc comment and full gates

The comment on `start_pull` ("Existing files are never overwritten…",
`src/audit/sirv_actions.rs:282-284`) becomes true with this plan; extend it
with one sentence: the guarantee is enforced at install time by an atomic
no-replace link, not inferred from the scan.

**Verify**: `cargo test --locked`, `cargo clippy --all-targets -- -D warnings`,
`cargo fmt --check` → all green.

## Test plan

Covered in Step 5 — nine new tests in `src/sirv.rs` plus one in
`src/audit/tests.rs`. Honest coverage statement: the full transfer loop
(plan → job → per-file download/write → terminal update) still has no test
and cannot have one until the SIRV-18 transport extraction lands; what IS
pinned here is every pure decision the loop delegates to — the plan's
destructive scope, the write guards, the cap, the presence lifecycle at a
root change. The window-freeze fix is observable only live; note it for the
reviewer.

## Done criteria

- [ ] `cargo test --locked` exits 0, with ≥10 new tests (9 sirv + 1 audit harness)
- [ ] `cargo clippy --all-targets -- -D warnings` exits 0
- [ ] `cargo fmt --check` exits 0
- [ ] `grep -n "std::fs::write" src/audit/sirv_actions.rs` → no matches
- [ ] `grep -n "local_sizes_for\|write_pulled\|read_capped" src/sirv.rs src/audit/sirv_actions.rs` → helpers defined once, used at the described sites
- [ ] `grep -n "sirv_local_presence" src/audit/mod.rs src/audit/sirv_actions.rs` → field + walk landing + pull patch + counts + unpair clear
- [ ] `git status` (in your worktree) shows changes only in the in-scope files
- [ ] `plans/README.md` status row updated

## STOP conditions

Stop and report back (do not improvise) if:

- Plan 036 has not been executed (check its status row) — key correctness
  must land first or this plan stats and confines the wrong paths for
  nested files.
- `Listing::Ready`'s map keys turn out not to be unpaired relative keys
  (verify against `walk_sirv_pairing`'s construction before Step 1).
- `hard_link` semantics on this platform do not fail with `AlreadyExists`
  when the target exists (verify with a 5-line scratch test before relying
  on it).
- The background plan computation cannot re-check `sirv_busy` at landing
  without a race you can name — report it rather than dropping the check.
- Any existing `pull_plan` test fails — you changed semantics you were told
  to leave alone.

## Maintenance notes

- The pull-side "additive" guarantee now lives in exactly one place:
  `write_pulled`'s atomic no-replace install. Any future "sync both ways"
  feature must go through it, or say loudly why not.
- `sirv_local_presence` is a walk-time snapshot patched by pull successes.
  If SIRV-12 (persist pairing, re-walk on folder change) lands, rebuild the
  set wherever the walk re-runs — it already travels with the walk task.
- Known ceilings, on purpose: `hard_link` fails on filesystems without hard
  links (per-file error, honest); DMG-style read-only roots fail at
  `create_new` with the OS message. And one accepted race, documented here
  so nobody re-finds it: the ancestor symlink check is check-then-use — a
  local process replacing a directory with a symlink *between* the check
  and the write can still redirect it. Closing that needs handle-relative
  `openat`/`O_NOFOLLOW` I/O (a `cap-std`-shaped dependency). This app's
  untrusted party is the remote server, whose keys ARE confined; a
  same-user local attacker racing your own pull is outside its threat
  model, and the dependency policy weighs against buying that defense.
- Deferred, recorded separately: retry/backoff on 429 and transport errors
  (SIRV-04), parallel transfers (SIRV-07), streaming uploads and incremental
  listing refresh (SIRV-08), remote-only rows in the table (SIRV-13).
