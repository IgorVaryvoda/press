# Plan 036: Keep nested Sirv files on their real keys, and bound the listing loops

> **Executor instructions**: Follow this plan step by step. Run every
> verification command and confirm the expected result before moving to the
> next step. If anything in the "STOP conditions" section occurs, stop and
> report — do not improvise. When done, update the status row for this plan
> in `plans/README.md` — unless a reviewer dispatched you and told you they
> maintain the index.
>
> **Drift check (run first)**: `git diff --stat 97cb1a5..HEAD -- src/sirv.rs src/audit/sirv_actions.rs src/audit/mod.rs src/audit/view.rs src/audit/tests.rs`
> If any in-scope file changed since this plan was written, compare the
> "Current state" excerpts against the live code before proceeding; on a
> mismatch, treat it as a STOP condition.

## Status

- **Priority**: P1
- **Effort**: M
- **Risk**: LOW
- **Depends on**: none
- **Category**: bug
- **Planned at**: commit `97cb1a5`, 2026-08-23

## Why this matters

The recursive Sirv walk joins the parent path onto **folder** entries but not
onto **file** entries. The code's own doc comment states the API returns names
relative to the listed folder, so every nested file comes back as its bare
basename: `sub/c.jpg` is keyed as `c.jpg`, collides with a root-level
`c.jpg`, makes every nested pull a 404, and makes every nested local file
re-upload on every push. On top of that: pairing the account root fails with
an internal-invariant error one click from the first screen; the readdir
pagination loop has no page cap or repeated-token guard; and an in-flight
pairing walk can land against a *different* pairing than the one it listed.

## Current state

- `src/sirv.rs:379-421` — `Client::walk`. The doc comment above it says:
  "readdir's `filename` fields are relative to the listed folder (the API
  docs' own example lists `/REST%20API%20Examples` and gets back
  `"aurora.jpg"`, not the absolute path)." The body joins folders but not files:

```rust
        for node in nodes {
            if node.is_folder() {
                let name = node.filename.trim_end_matches('/');
                // Absolute entries pass through; relative ones join the parent.
                let child = if name.starts_with('/') {
                    name.to_string()
                } else {
                    format!("{current}/{name}")
                };
                stack.push(child);
            } else {
                all.push(node);        // <-- filename left as the bare name
            }
        }
```

- `src/audit/sirv_actions.rs:177-181` — the walk's consumer keys each node
  against the *pairing root*: `sirv::unpair_remote(&dir, &node.filename)`.
  `unpair_remote` (`src/sirv.rs:149-159`) returns relative names unchanged,
  so a file listed under `/photos/sub` as `c.jpg` becomes key `c.jpg`
  instead of `sub/c.jpg`. The keys land in a `HashMap` (`Listing::Ready`),
  so colliding basenames silently overwrite each other.

- `src/audit/sirv_actions.rs:128-147` — `pair_sirv` trims trailing slashes:
  `browser.path.trim_end_matches('/')`. The browser starts at `"/"`
  (`:46`), so pairing at the root produces `dir == ""`, and
  `walk("")` (`src/sirv.rs:380-387`) normalises to `""` and returns
  `Err("pairing folder is empty")`. The "Pair this folder" button
  (`src/audit/view.rs`, id `"sirv-pair"`) is enabled whenever the listing is
  `Some(Ok(_))`, including at the root.

- `src/sirv.rs:337-368` — `readdir`'s continuation loop has no iteration
  cap, no check that the token changed, and no cap on `nodes.len()`; the
  `WALK_LIMIT` check (`= 20_000`, `src/sirv.rs:37`) lives in `walk` and
  fires only between directories. `walk` also keeps no visited set, and a
  folder entry with an empty filename yields `child == format!("{current}/")`,
  which re-lists the same folder forever without ever growing `all`.

- `src/audit/sirv_actions.rs:151-195` — `walk_sirv_pairing` guards only on
  `dataset_generation`, and the landing closure re-reads `pairing.dir` from
  *current* state:

```rust
        let generation = self.dataset_generation;
        cx.spawn(async move |this, cx| {
            let walked = ...walk(&dir)...;
            this.update(cx, |audit, cx| {
                if audit.dataset_generation != generation { return; }
                let Some(pairing) = audit.sirv_pairing.as_mut() else { return; };
                match walked {
                    Ok(nodes) => {
                        let dir = pairing.dir.clone();   // <-- current dir, not the walked one
                        pairing.files = Listing::Ready(...unpair_remote(&dir, ...)...);
```

  Re-pairing from `/a` to `/b` while `/a`'s walk is in flight lets `/a`'s
  nodes land, unpaired against `/b`, as `/b`'s listing.

- Existing test surface: `src/sirv.rs:626-943` tests the pure helpers
  (`unpair_remote`, `safe_key`, `pull_plan`, `push_folders`, credentials
  parsing). `src/audit/tests.rs` covers `sirv_push_plan` (~:172-243) and
  transfer cancellation (~:345-370) with `#[gpui::test]`. `Client`'s HTTP
  paths (`readdir`, `walk`) have no tests and no injectable base URL —
  that extraction is a separately recorded finding (SIRV-18), out of scope
  here. Keep new logic in pure functions so it lands under test anyway.

- Conventions: test names are snake_case behaviour sentences; comments are
  short *why* paragraphs; clippy warnings are errors.

## Commands you will need

| Purpose | Command | Expected on success |
|---|---|---|
| Tests | `cargo test --locked` | all pass (87 passed + 1 ignored at baseline) |
| One module | `cargo test --locked sirv` | sirv tests pass |
| Lint | `cargo clippy --all-targets -- -D warnings` | exit 0 |
| Format | `cargo fmt --check` | exit 0 |

## Scope

**In scope** (the only files you should modify):
- `src/sirv.rs` — walk/readdir fixes, new pure helper + tests
- `src/audit/sirv_actions.rs` — pairing generation, root-pair guard
- `src/audit/mod.rs` — one new field on `Audit` (pairing generation counter)
- `src/audit/view.rs` — disable "Pair this folder" at the root
- `src/audit/tests.rs` — walk-landing gate tests (required; see Test plan)
- `plans/README.md` — status row

**Out of scope** (do NOT touch):
- `pull_plan`, `push_folders`, `safe_key`, `classify` — their semantics are
  pinned by tests and by plan 037, which builds on this plan.
- The `Client` transport/base-URL extraction (SIRV-18) — recorded, not this plan.
- Any transfer-loop change (plan 037/038 territory).

## Git workflow

- Branch: `improve/036-sirv-walk-keys`
- Conventional commits, e.g. `fix: keep nested Sirv files on their full keys`
- Do NOT push or open a PR unless the operator instructed it.

## Steps

### Step 1: One join rule for files and folders

In `src/sirv.rs`, extract the join logic into a pure function near
`unpair_remote`:

```rust
/// Join a readdir entry name onto the folder that was listed. Some account
/// shapes return absolute names, others names relative to the listed folder;
/// recursion and diff keys both need the absolute form.
pub fn join_listing_name(current: &str, name: &str) -> String {
    let name = name.trim_end_matches('/');
    if name.starts_with('/') {
        name.to_string()
    } else {
        format!("{}/{name}", current.trim_end_matches('/'))
    }
}
```

In `walk`, use it for both branches:

```rust
        for mut node in nodes {
            if node.is_folder() {
                stack.push(join_listing_name(&current, &node.filename));
            } else {
                node.filename = join_listing_name(&current, &node.filename);
                all.push(node);
            }
        }
```

(The skip rules for degenerate names come in Step 3 — do Step 3 in the same
edit if you prefer, but keep the tests separable.)

**Verify**: `cargo test --locked sirv` → existing tests still pass (walk has
none; `unpair_remote` tests are unaffected because file nodes now arrive
absolute, which `unpair_remote` already handles — its absolute-name branch is
tested).

### Step 2: Tests for the join rule

In `src/sirv.rs`'s tests module, add:

- `a_relative_file_name_joins_the_folder_being_listed` —
  `join_listing_name("/photos/sub", "c.jpg")` → `"/photos/sub/c.jpg"`, and
  `unpair_remote("/photos", &that)` → `Some("sub/c.jpg".into())`.
- `an_absolute_file_name_passes_through` —
  `join_listing_name("/photos/sub", "/photos/sub/c.jpg")` → unchanged.
- `a_trailing_slash_on_the_folder_does_not_double` —
  `join_listing_name("/photos/", "sub")` → `"/photos/sub"`.

**Verify**: `cargo test --locked sirv` → all pass including the three new tests.

### Step 3: Bound readdir and the walk

In `readdir` (`src/sirv.rs:337-368`):

- Add a pure helper next to the other pure helpers and use it in the loop —
  a consecutive-repeat check is NOT enough, because a token cycle
  `A → B → A` of empty pages never repeats consecutively and never grows
  `nodes`, so only a full seen-set (or a hard page cap) bounds the loop:

```rust
/// True when this continuation token has not been seen before in this
/// listing. A repeated token — adjacent or in a cycle — means the server
/// is looping, and following it would list forever.
pub fn continuation_advances(seen: &mut HashSet<String>, token: &str) -> bool {
    seen.insert(token.to_string())
}
```

  In the loop: initialise `let mut seen = HashSet::new();` before it; when a
  `Some(token)` arrives, `if !continuation_advances(&mut seen, &token) { return Err(Error { status: 0, message: format!("{dirname}: readdir repeated a continuation token") }); }`.
- After `nodes.extend(contents)`, if `nodes.len() > WALK_LIMIT`, return the
  same "more than {WALK_LIMIT} files" error `walk` uses (move that message
  into a small helper or duplicate the format string — match `walk`'s wording
  at `src/sirv.rs:416`).
- A hard page cap, because a hostile-or-broken server can mint infinitely
  many *unique* tokens over empty pages, which neither the seen-set nor the
  node cap ever trips: `const READDIR_PAGE_LIMIT: usize = 512;` (512 pages
  × ~100 entries ≈ 51k, already past `WALK_LIMIT`, so no legitimate listing
  hits it). Count pages in the loop; past the cap, return
  `Error { status: 0, message: format!("{dirname}: listing did not finish after {READDIR_PAGE_LIMIT} pages") }`.

In `walk`:

- Keep a `HashSet<String>` of directories already listed; skip a `child`
  that was already visited instead of pushing it — and bound the set,
  because a server minting one novel subfolder per response evades both the
  visited-set and the file-only `WALK_LIMIT`: when `visited.len()` exceeds
  `WALK_LIMIT`, return an error reusing the same "sync it in parts" wording
  (a tree with more *folders* than the file cap is equally unsyncable).
- Skip **any** node — file or folder — whose trimmed name
  (`node.filename.trim_end_matches('/')`) is empty, `"."`, or `".."`,
  before joining. This matters for files, not just folders: `Node.filename`
  is `#[serde(default)]` (`src/sirv.rs:41-43`), so a malformed entry
  deserializes with an empty name, joins to `"{current}/"`, unpaires to an
  empty key, and then inflates the "Pull N missing" count with a key
  `pull_plan` will silently drop via `safe_key` — a button whose number and
  action disagree.

**Verify**: `cargo build --release --locked` → exit 0. The loop itself is
HTTP-bound and untestable until SIRV-18 lands, but the token rule is not:
add two tests on `continuation_advances` —
`a_fresh_token_advances_the_listing` and
`a_token_cycle_is_refused_even_when_not_adjacent` (insert `"a"`, `"b"`,
then `"a"` again → `false`).

### Step 4: Refuse pairing at the account root — in the action, then the button

First the invariant, then the affordance. In `pair_sirv`
(`src/audit/sirv_actions.rs:128-147`), after the trim, guard the empty
result so no future call site can re-create the broken pairing:

```rust
        // The account root pairs to "", which `walk` rejects; a pairing
        // whose header reads "Unpair " with no name is worse than no
        // pairing. The browser's button says why; this guard holds even
        // if a future caller forgets to.
        if dir.is_empty() {
            return;
        }
```

In `src/audit/view.rs`, the browser footer builds the `"sirv-pair"` button:

```rust
                    Button::new("sirv-pair")
                        .primary()
                        .small()
                        .label("Pair this folder")
                        .disabled(!matches!(browser.nodes, Some(Ok(_))))
```

Change the disabled condition to also hold at the root, and say why in the
label when it does:

```rust
                    let at_root = browser.path.trim_end_matches('/').is_empty();
                    // ...
                        .label(if at_root { "Open a folder to pair it" } else { "Pair this folder" })
                        .disabled(at_root || !matches!(browser.nodes, Some(Ok(_))))
```

(Adapt to the surrounding builder-chain style; the `at_root` binding may need
to live before the chain.)

**Verify**: `cargo test --locked` → all pass. `cargo clippy --all-targets -- -D warnings` → exit 0.

### Step 5: Pairing generation

- In `src/audit/mod.rs`, next to the existing `sirv_generation` field on
  `Audit`, add `sirv_pairing_generation: u64` (initialise to 0 where the
  other Sirv fields are initialised — find the struct construction in
  `build_audit`/`Audit::new` and match its style).
- In `src/audit/sirv_actions.rs`:
  - `pair_sirv` and `unpair_sirv`: `self.sirv_pairing_generation = self.sirv_pairing_generation.wrapping_add(1);`
  - The landing gate must be a pure, tested function, not an inline
    comparison a typo can invert without any gate noticing. In
    `src/audit/sirv_actions.rs`:

```rust
/// True when a finished walk still describes the current world: same
/// dataset, same pairing as when it started. A walk that outlives either
/// must land nowhere — installing folder A's listing under folder B's
/// pairing arms a full-folder push at the wrong remote directory.
pub(super) fn walk_landing_applies(
    dataset_then: u64,
    dataset_now: u64,
    pairing_then: u64,
    pairing_now: u64,
) -> bool {
    dataset_then == dataset_now && pairing_then == pairing_now
}
```

  - `walk_sirv_pairing`: capture `let pairing_generation = self.sirv_pairing_generation;`
    alongside `dataset_generation`; in the landing closure, replace the
    existing `audit.dataset_generation != generation` bail with one call:
    `if !walk_landing_applies(generation, audit.dataset_generation, pairing_generation, audit.sirv_pairing_generation) { return; }`. Also key
    `unpair_remote` by the directory that was actually walked, not by
    re-reading `pairing.dir` from current state. Note the outer `dir` is
    MOVED into the background walk's `async move` block
    (`src/audit/sirv_actions.rs:161`), so it cannot be reused directly:
    clone it first —

```rust
        let walked_dir = dir.clone();
        cx.spawn(async move |this, cx| {
            let walked = ...walk(&dir)...;              // consumes `dir`
            this.update(cx, |audit, cx| {
                ...
                // replaces the old `let dir = pairing.dir.clone();`
                ...unpair_remote(&walked_dir, &node.filename)...
```

**Verify**: `cargo test --locked` → all pass (the cancellation tests at
`src/audit/tests.rs:345-370` exercise neighbouring generation logic and must
stay green).

### Step 6: Full gates

**Verify**: `cargo test --locked`, `cargo clippy --all-targets -- -D warnings`,
`cargo fmt --check` → all green.

## Test plan

- Three pure tests on `join_listing_name` (Step 2), in `src/sirv.rs`'s tests
  module, modeled on the existing `unpair_remote` tests there.
- Two pure tests on `continuation_advances` (Step 3).
- Two REQUIRED pure tests on `walk_landing_applies` (Step 5), in
  `src/audit/tests.rs` (it is `pub(super)`, reachable there):
  `a_walk_from_a_previous_pairing_lands_nowhere` — any single generation
  mismatch (dataset or pairing, each direction) → `false`;
  `a_current_walk_lands` — both equal → `true`. No network is involved —
  the gate is pure, which is exactly why Step 5 extracts it. Do NOT add a
  mock transport for the walk itself (that is SIRV-18).
- The readdir/walk HTTP loop bounds are untestable pre-SIRV-18; reasoning
  goes in the commit message.

## Done criteria

- [ ] `cargo test --locked` exits 0, with ≥7 new tests (3 join rule + 2 continuation guard + 2 walk-landing gate)
- [ ] `cargo clippy --all-targets -- -D warnings` exits 0
- [ ] `cargo fmt --check` exits 0
- [ ] `grep -n "join_listing_name" src/sirv.rs` → used in both walk branches
- [ ] `git status` (in your worktree) shows changes only in the in-scope files
- [ ] `git status` shows changes only in the in-scope files
- [ ] `plans/README.md` status row updated

## STOP conditions

Stop and report back (do not improvise) if:

- The walk's consumer (`unpair_remote` at `src/audit/sirv_actions.rs:179`)
  turns out to receive *absolute* filenames from a live account in the
  current code — that would mean the doc comment is wrong and the flattening
  bug does not exist; the change would then be a behavioural no-op that
  still must not break the absolute shape. If any existing test contradicts
  Step 1's expectation, stop.
- `pair_sirv`'s trim has other callers relying on `""` (grep for callers of
  `walk(` and `pairing.dir` before Step 4; there should be none that want
  the empty string).
- Adding the `Audit` field breaks more than the struct literal and its
  initialiser (e.g. a serialization you did not expect).

## Maintenance notes

- After this plan, listing keys for nested files change from `c.jpg` to
  `sub/c.jpg` — any user who pushed nested folders before will see those
  files re-classified (correctly) as out of sync once. That is the fix
  working, not a regression; note it in the commit message.
- Plan 037 (pull safety) assumes keys are correct after this plan; execute
  this one first.
- True account-root pairing (walking `/`) stays unsupported — the path
  algebra for `dir == "/"` (double-slash joins in `run_pull`, `mkdir`,
  `unpair_remote`) is the recorded reason. If root pairing is ever wanted,
  introduce a `join_remote(dir, key)` helper and use it at every
  `format!("{dir}/{key}")` site (`src/audit/sirv_actions.rs:347`, `:458`,
  `:487`).
