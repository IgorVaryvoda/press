# Plan 040: A credential change must retire in-flight walks too

> **Executor instructions**: Follow this plan step by step. Run every
> verification command and confirm the expected result before moving on. If
> anything in the "STOP conditions" section occurs, stop and report — do not
> improvise. The reviewer maintains `plans/README.md`.
>
> **Drift check (run first, BEFORE your own commits)**: your worktree is
> `/home/igor/Projects/imageguide-desktop-codex-036-038`, branch
> `improve/036-038-integration`. If HEAD is not `45f1394` at the moment you
> START, stop. Your own Step 3 commit moving HEAD past it is expected, not
> drift. This plan file lives in the primary tree, not your worktree — do
> not copy or commit it there; `git status` cleanliness refers to your
> implementation changes only.

## Status

- **Priority**: P1
- **Effort**: S
- **Risk**: LOW
- **Depends on**: plans/038-sirv-transfer-control.md (executed, `45f1394`)
- **Category**: bug
- **Planned at**: integration branch `45f1394`, 2026-08-23 (successor to
  038's round-2 residue, split per the review cap)

## Why this matters

`adopt_new_credentials` (added by plan 038) retires the client, listing,
presence, counts, and any active transfer, then re-walks — but it does not
bump `sirv_pairing_generation`. A walk started under the OLD credentials
carries the same dataset and pairing generations, so `walk_landing_applies`
lets it land after the replacement walk begins, reinstalling the old
account's listing under the new client. The next push then plans the old
account's files against the new one. (This race predates 038 — before it, a
credential save invalidated nothing at all — but 038 built the machinery
that closes it in one line.)

## Current state

- `src/audit/sirv_actions.rs` — `adopt_new_credentials` (added at
  `45f1394`, ~line 330):

```rust
    pub(super) fn adopt_new_credentials(
        &mut self,
        credentials: sirv::Credentials,
        cx: &mut Context<Self>,
    ) {
        if let Some(pairing) = self.sirv_pairing.as_mut() {
            pairing.client = Arc::new(parking_lot::Mutex::new(sirv::Client::new(credentials)));
            pairing.files = Listing::Walking;
        } else {
            return;
        }
        self.cancel_sirv_transfer();
        self.sirv_local_presence.clear();
        self.sirv_counts = None;
        self.walk_sirv_pairing(cx);
    }
```

- The landing gate (plan 036): `walk_landing_applies(dataset_then,
  dataset_now, pairing_then, pairing_now)` — pure, tested; `pair_sirv` and
  `unpair_sirv` both bump `self.sirv_pairing_generation` for exactly this
  purpose.
- The existing test `new_credentials_retire_the_old_listing`
  (`src/audit/tests.rs`, ~:448) asserts client/listing/presence/counts/job
  state but not generation movement.

## Commands you will need

| Purpose | Command | Expected on success |
|---|---|---|
| Tests | `cargo test --locked` | all pass (111 passed, 1 ignored at branch baseline) |
| Lint | `cargo clippy --all-targets -- -D warnings` | exit 0 |
| Format | `cargo fmt --check` | exit 0 |

## Scope

**In scope**: `src/audit/sirv_actions.rs`, `src/audit/tests.rs`.
**Out of scope**: everything else — one line plus test assertions.

## Steps

### Step 1: Bump the pairing generation

In `adopt_new_credentials`, immediately after the `if let/else return`
block (before `cancel_sirv_transfer`), add:

```rust
        // A walk started under the old credentials must land nowhere: it
        // carries the old account's listing. Same invalidation pair_sirv
        // and unpair_sirv already use.
        self.sirv_pairing_generation = self.sirv_pairing_generation.wrapping_add(1);
```

**Verify**: `cargo build --release --locked` → exit 0.

### Step 2: Pin it

In `new_credentials_retire_the_old_listing`, capture
`let generation_before = audit.sirv_pairing_generation;` before the call
and assert `audit.sirv_pairing_generation != generation_before` after it,
with a one-line comment naming the stale-walk rejection this feeds
(`walk_landing_applies` is already tested for the mismatch directions).

**Verify**: `cargo test --locked` → all pass;
`cargo clippy --all-targets -- -D warnings` → exit 0;
`cargo fmt --check` → exit 0.

### Step 3: Correct the overclaim

The doc comment on `adopt_new_credentials` says "any transfer in flight" —
extend the list to include in-flight walks now that it is true.

**Verify**: `cargo fmt --check` → exit 0. Commit (branch
`improve/036-038-integration`); on the known `.git/worktrees` lock, leave
uncommitted and report STOPPED naming that cause.

## Done criteria

- [ ] `cargo test --locked` exits 0; the generation assertion exists
- [ ] `cargo clippy --all-targets -- -D warnings` exits 0
- [ ] `cargo fmt --check` exits 0
- [ ] `grep -n "sirv_pairing_generation" src/audit/sirv_actions.rs` → present in `pair_sirv`, `unpair_sirv`, `walk_sirv_pairing`, AND `adopt_new_credentials`
- [ ] `git status` (in your worktree) clean or reported per the lock rule

## STOP conditions

- HEAD is not `45f1394` when you start.
- The one-line change breaks any existing test — that would mean a landing
  path depends on stale walks, which needs the reviewer, not a workaround.

## Maintenance notes

- With this, every pairing-identity change (pair, unpair, folder change via
  dataset generation, credentials) invalidates in-flight walks the same way.
  Keep that property if a fifth path ever appears.
