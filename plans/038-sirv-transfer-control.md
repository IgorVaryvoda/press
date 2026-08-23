# Plan 038: Let the user stop, confirm, and understand Sirv transfers

> **Executor instructions**: Follow this plan step by step. Run every
> verification command and confirm the expected result before moving to the
> next step. If anything in the "STOP conditions" section occurs, stop and
> report — do not improvise. When done, update the status row for this plan
> in `plans/README.md` — unless a reviewer dispatched you and told you they
> maintain the index.
>
> **Drift check (run first)**: `git diff --stat 97cb1a5..HEAD -- src/sirv.rs src/audit/sirv_actions.rs src/audit/view.rs src/audit/statusbar.rs src/audit/mod.rs`
> Plans 036/037 touch some of these files first; the excerpts below are from
> `97cb1a5` — re-verify each against live code before editing it. On a
> mismatch beyond those plans' stated edits, STOP.

## Status

- **Priority**: P1
- **Effort**: M
- **Risk**: LOW
- **Depends on**: none (merge-order after 036/037 to avoid conflicts in the same files)
- **Category**: bug / ux
- **Planned at**: commit `97cb1a5`, 2026-08-23

## Why this matters

A running Sirv transfer cannot be stopped: the only control wired to
`cancel_sirv_transfer` is "Unpair", and it is disabled while a transfer runs.
The two destructive buttons ("Overwrite N on Sirv", "Take N from Sirv") fire
on a single click with no confirmation — a mis-click can start replacing
2,000 local originals with nothing to do but quit the app. When a job
finishes, it never clears: the status bar reads "Sirv push: 40 of 40"
forever, and errors surface as raw truncated JSON bodies
(`readdir: {"errors":[...]}`) instead of a sentence. Editing credentials in
the settings panel does not touch the live client, so fixing a typo'd secret
appears to do nothing. Together these are the core of "barely usable".

## Current state

All excerpts at `97cb1a5`.

- `src/audit/view.rs` (paired action row, ~:190-243) — the buttons:
  `"sirv-pull"`, `"sirv-push"` (`.disabled(busy || count == 0)`),
  `"sirv-push-changed"` label `format!("Overwrite {changed} on Sirv")`,
  `"sirv-pull-changed"` label `format!("Take {changed} from Sirv")` (both
  `.disabled(busy)`, single click → `start_push_changed`/`start_pull_changed`),
  and `"sirv-unpair"` `.disabled(busy)` → `unpair_sirv`. No stop control
  exists anywhere; `"sirv-close"` only hides the panel.

- `src/audit/sirv_actions.rs:218-226` — cancellation machinery exists and is
  tested (`src/audit/tests.rs:345-370`):

```rust
    pub(super) fn cancel_sirv_transfer(&mut self) {
        self.sirv_generation = self.sirv_generation.wrapping_add(1);
        if let Some(job) = self.sirv_job.as_mut()
            && !job.finished
        {
            job.finished = true;
            job.failures.push("stopped".into());
        }
    }
```

- `src/audit/sirv_actions.rs:197-203` — `unpair_sirv` clears pairing,
  counts, browser, and cancels — but never sets `self.sirv_job = None`.
  Nothing anywhere sets `sirv_job` back to `None`; `SirvJob` is only ever
  replaced by the next transfer (`:322`, `:429`).

- `src/audit/statusbar.rs:263-285` — the notices row renders any
  `sirv_job`, finished or not, forever:

```rust
        if let Some(job) = &self.sirv_job {
            let verb = match job.kind { ... };
            let failures = if job.failures.is_empty() { String::new() } else {
                format!(", {} failed: {}", job.failures.len(),
                    job.failures.iter().take(3).cloned().collect::<Vec<_>>().join(", "))
            };
            parts.push(format!("{verb}: {} of {}{failures}", job.done, job.total));
        }
```

- `src/audit/view.rs` (finished-job line inside the Sirv panel, ~:165-176) —
  joins **all** failures with no cap:

```rust
                                let failures = if job.failures.is_empty() {
                                    String::new()
                                } else {
                                    format!(", {} failed: {}", job.failures.len(),
                                        job.failures.join(", "))
                                };
```

- `src/sirv.rs:492-516` — `sirv_error` builds the message shown to users:
  the raw response body, trimmed, capped at 200 chars, prefixed with a stage
  word ("readdir: {...}"). `Error` is:

```rust
pub struct Error {
    pub status: u16,      // check actual visibility; used as error.status elsewhere
    pub message: String,
}
```

  and reaches the UI via `error.to_string()` (`Display`). Find the `Display`
  impl (or `#[derive]`) near the struct — grep `impl std::fmt::Display for Error`
  in `src/sirv.rs`.

- `src/audit/sirv_actions.rs:251-274` — `save_sirv_settings` writes the file
  and reports; it never rebuilds `sirv_pairing.client`, whose `Credentials`
  and cached token were captured at pair time. The browser reuses that same
  client (`:9-14`), so a corrected secret takes effect only after unpair +
  re-pair (or restart).

- `Audit`'s Sirv fields live in `src/audit/mod.rs` (`sirv_pairing`,
  `sirv_browser`, `sirv_counts`, `sirv_job`, `sirv_generation`, around
  `:220-230` and `:365-370`).

- UI conventions: buttons are `gpui_component::button::Button` with
  `.outline()`/`.ghost()`/`.primary()`, `.small()`, `cx.listener(...)`
  handlers — copy the neighbouring buttons' style. Test names are snake_case
  sentences; `#[gpui::test]` harness patterns live in `src/audit/tests.rs`
  (see `:345-370` for a Sirv-state test that never touches the network).

## Commands you will need

| Purpose | Command | Expected on success |
|---|---|---|
| Tests | `cargo test --locked` | all pass |
| Lint | `cargo clippy --all-targets -- -D warnings` | exit 0 |
| Format | `cargo fmt --check` | exit 0 |

Live visual proof (from `plans/README.md`'s standing contract): changes to
rendered states need before/after captures from the real app. Launch
`target/release/imageguide <folder>` under the host compositor, capture with
`grim`. States to capture: the paired row with a running job (Stop visible),
the confirm state of an overwrite button, and a finished job with failures.
If no Sirv account is available to the executor, say so in the report and
leave captures to the reviewer — do not fake them.

## Scope

**In scope** (the only files you should modify):
- `src/audit/view.rs` — Stop button, confirm flow, failure cap
- `src/audit/sirv_actions.rs` — job lifecycle, client refresh on save
- `src/audit/mod.rs` — the confirm-state field and `SirvJob::stopping`
- `src/audit/statusbar.rs` — drop finished clean jobs from notices
- `src/sirv.rs` — human-readable `Error` display + tests
- `src/audit/tests.rs` — lifecycle tests
- `plans/README.md` — status row

**Out of scope** (do NOT touch):
- The transfer loops' structure, retries, parallelism (SIRV-04/07/08).
- Pairing persistence / moving actions out of the modal (SIRV-12 — recorded,
  larger product change).
- A "Test connection" button (recorded follow-up of SIRV-10).

## Git workflow

- Branch: `improve/038-sirv-transfer-control`
- Conventional commits, e.g. `feat: a stop button and confirm step for Sirv transfers`
- Do NOT push or open a PR unless the operator instructed it.

## Steps

### Step 1: Cancellation that stays busy until acknowledged

The current `cancel_sirv_transfer` marks the job finished immediately, but
the loop only checks its generation *between* files — so the moment Stop (or
unpair) fires, `sirv_busy()` reads false while the in-flight file is still
downloading/writing, and every transfer starter re-enables. A second,
possibly opposite, destructive transfer could then run concurrently with the
old one's last file. Make cancellation two-phase:

- In `src/audit/mod.rs`, add `pub(super) stopping: bool` to `SirvJob`.
  Update EVERY struct literal in the same edit or the crate does not
  compile: the two production constructors in `sirv_actions.rs` (`:322`,
  `:429`) AND the test constructor in `src/audit/tests.rs:352` (the
  cancellation test builds a `SirvJob` literal directly). `grep -rn "SirvJob {" src/`
  before moving on and account for every hit.
- Rewrite `cancel_sirv_transfer` (`src/audit/sirv_actions.rs:218-226`):

```rust
    /// Ask the running transfer to stop. The loop checks before each file,
    /// so the file in flight finishes and nothing after it starts. The job
    /// stays busy (`finished == false`) until the loop acknowledges — the
    /// moment Stop re-enabled the other buttons used to be the moment a
    /// second transfer could race the first one's last file.
    pub(super) fn cancel_sirv_transfer(&mut self) {
        self.sirv_generation = self.sirv_generation.wrapping_add(1);
        if let Some(job) = self.sirv_job.as_mut()
            && !job.finished
        {
            job.stopping = true;
        }
    }
```

- In BOTH loops (`run_pull` and `run_push`), the supersession check
  currently bails silently:

```rust
                if Self::sirv_superseded(&this, cx, generation) {
                    return;
                }
```

  Replace with an acknowledging bail that finishes *this* loop's job:

```rust
                if Self::sirv_superseded(&this, cx, generation) {
                    this.update(cx, |audit, cx| {
                        if let Some(job) = audit.sirv_job.as_mut()
                            && job.generation == generation
                            && !job.finished
                        {
                            job.finished = true;
                            job.failures.push("stopped".into());
                            // A partial transfer changed the world: pulled
                            // files sit on disk unscanned, pushed files sit
                            // remote unlisted. Reconcile exactly as the
                            // normal completion path does, or every count
                            // and row after a Stop describes the state
                            // before it.
                            audit.request_path(audit.root.clone(), cx); // pull loop
                            // audit.walk_sirv_pairing(cx);             // push loop
                            cx.notify();
                        }
                    })
                    .ok();
                    return;
                }
```

  In the **pull** loop the reconciliation line is
  `audit.request_path(audit.root.clone(), cx)` (the same call its normal
  terminal update makes at `src/audit/sirv_actions.rs:392`); in the
  **push** loop it is `audit.walk_sirv_pairing(cx)` (same as `:519`).

  Also in the **push** loop (found in 036's execution review): the folder
  provisioning that precedes the per-file loop — the background task that
  `mkdir`s every `push_folders` entry — runs with NO supersession check, so
  a cancel during provisioning keeps creating folders on the retired
  pairing. Add the same acknowledging supersession check immediately BEFORE
  spawning the mkdir task. A cancel arriving *while* the task runs still
  completes that batch — the task holds the client lock and cannot read
  audit state; say so in a one-line comment (bounded staleness: one
  provisioning batch), and check supersession once more right after the
  task returns, before entering the per-file loop. Note
  the reconciliation sits INSIDE the `job.generation == generation` guard:
  when the supersession came from unpair (job cleared) or from a newer
  transfer (job replaced), the guard fails and no reconciliation runs —
  unpair has nothing to reconcile and a newer job owns the state. A folder
  change keeps the job, so its ack reconciles against the new root, which
  is idempotent with the scan that change already scheduled
  (`scan_generation` supersedes the older one).

  ("stopped" is pushed here, at acknowledgement, and nowhere else — the
  progress callback overwrites `job.failures` with the loop's own list on
  every file, so anything pushed at cancel time would be erased by a
  still-in-flight file's completion.)

- In the paired action row in `src/audit/view.rs`, inside the
  `.when_some(paired, ...)` block: when `busy`, render
  `Button::new("sirv-stop").outline().small()` first in the row, labelled
  `"Stop"`, or `"Stopping…"` and `.disabled(true)` when the job's
  `stopping` flag is set. Handler: `audit.cancel_sirv_transfer(); cx.notify();`.
  Do not change the other buttons' disabled logic — they stay disabled the
  whole time because `sirv_busy()` now stays true until acknowledgement.

Known accepted edge: `unpair_sirv` clears the job outright (Step 3), so
after an unpair the busy flag drops while the old loop's final file may
still be in flight; the generation guard keeps that loop from touching any
later job, and the pairing (with its buttons) is gone. One comment in
`unpair_sirv` records this.

**Verify**: `cargo test --locked` → all pass; `cargo clippy --all-targets -- -D warnings` → exit 0.

### Step 2: Two-step confirm on the destructive buttons

- In `src/audit/mod.rs`, add to `Audit`:
  `pub(super) sirv_confirm: Option<SirvJobKind>` (initialise `None` with the
  other Sirv fields). `SirvJobKind` already derives what it derives — check
  it has `PartialEq, Clone, Copy`; add those derives if missing.
- In `src/audit/view.rs`: the `"sirv-push-changed"` button becomes two-step:
  - if `self.sirv_confirm == Some(SirvJobKind::PushChanged)`: label
    `format!("Really overwrite {changed} on Sirv?")`, style `.primary()`,
    handler: clear `sirv_confirm`, call `start_push_changed`.
  - else: label as today, handler: `audit.sirv_confirm = Some(SirvJobKind::PushChanged); cx.notify();`
  - same for `"sirv-pull-changed"` with `SirvJobKind::PullChanged` and
    `format!("Really replace {changed} local files?")`.
- Clear `sirv_confirm` (set `None`) in: `unpair_sirv`, the `"sirv-close"`
  handler, `start_push`/`start_pull` (a click elsewhere withdraws the arm),
  and wherever the panel opens (`open_sirv_browser`).

**Verify**: `cargo test --locked` → all pass.

### Step 3: Finished jobs stop nagging

- `unpair_sirv` (`src/audit/sirv_actions.rs:197-203`): add
  `self.sirv_job = None;` after the `cancel_sirv_transfer()` call (cancel
  first so the generation bump retires the loop; then clear — the loop's
  acknowledgement update guards on `job.generation`, finds no job, and
  lands nowhere).
- **Update the pinned test that this changes.** `src/audit/tests.rs:345-370`
  (`unpairing_stops_a_running_transfer`) currently asserts the job survives
  unpair carrying a "stopped" failure. The behavior it pins is exactly the
  chronic-notice bug this step removes. Rewrite its assertions: after
  `unpair_sirv`, `sirv_job.is_none()` and `sirv_pairing.is_none()`; keep its
  first half (the generation bump retiring the loop) intact. Rename it if
  the name no longer states the behavior
  (`unpairing_discards_the_job_and_stops_the_loop`).
- `src/audit/statusbar.rs` notices: skip the job line when
  `job.finished && job.failures.is_empty()` — a clean finished job is the
  panel's news, not a standing warning. Failed jobs stay until unpair or the
  next transfer replaces them.
- `src/audit/view.rs` finished-job line: cap like the statusbar —
  `take(3)` then `format!(" and {} more", rest)` when more remain. Match the
  statusbar's exact shape so the two read alike.

**Verify**: `cargo test --locked` → all pass.

### Step 4: Errors in sentences

In `src/sirv.rs`, find `Error`'s `Display` impl and make it lead with a
sentence chosen by status, keeping the body as detail:

```rust
impl std::fmt::Display for Error {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self.status {
            0 => write!(f, "{}", self.message), // transport/body errors already read as sentences
            401 | 403 => write!(f, "Sirv rejected the credentials ({}): {}", self.status, self.message),
            404 => write!(f, "not found on Sirv ({}): {}", self.status, self.message),
            429 => write!(f, "Sirv is rate limiting this account ({}): {}", self.status, self.message),
            // Unmapped HTTP statuses keep BOTH the code and the detail — a
            // bare JSON body with the status dropped is the exact failure
            // mode this step removes.
            status => write!(f, "Sirv error {status}: {}", self.message),
        }
    }
}
```

The existing `Display` impl is at `src/sirv.rs:79` (approximately) and
already retains the status — start from it, keep every current caller
compiling, and make sure no branch loses `self.status` for HTTP errors.
Add tests in the sirv tests module:

- `a_401_reads_as_a_credentials_problem` — `Error { status: 401, message: "token: {...}".into() }.to_string()`
  starts with `"Sirv rejected the credentials"`.
- `a_transport_error_keeps_its_message` — status 0 passes through unchanged.
- `an_unmapped_status_keeps_its_code_and_detail` — status 500 →
  string contains `"500"` AND the original message.

**Verify**: `cargo test --locked sirv` → all pass including the three new tests.

### Step 5: Saved credentials reach the live client

In `save_sirv_settings` (`src/audit/sirv_actions.rs:251-274`), on the
`Ok(())` branch, rebuild the pairing's client so the next request uses the
new secret and a fresh token:

```rust
                Ok(()) => {
                    if let Some(pairing) = self.sirv_pairing.as_mut() {
                        pairing.client = Arc::new(parking_lot::Mutex::new(sirv::Client::new(
                            sirv::Credentials { client_id: client_id.clone(), client_secret: client_secret.clone() },
                        )));
                        // A listing that failed under the old secret would
                        // otherwise sit as Failed — with the transfer row
                        // disabled — until the user guesses that unpair +
                        // re-pair is the recovery. Corrected credentials
                        // retry it immediately.
                        if matches!(pairing.files, Listing::Failed(_)) {
                            pairing.files = Listing::Walking;
                            retry_walk = true;
                        }
                    }
                    (true, "Saved.".into())
                }
```

then, after the `panel.cdn_status = ...` assignment (the borrow of
`self.settings_panel` must end first): `if retry_walk { self.walk_sirv_pairing(cx); }`
with `let mut retry_walk = false;` declared before the match. (The
`client_id`/`client_secret` locals are consumed by `save_credentials` just
above — clone before the call, matching the borrow structure you find.)
An in-flight transfer keeps its own `Arc` clone of the old client and is
unaffected; say that in a one-line comment.

**Verify**: `cargo test --locked` → all pass.

### Step 6: Lifecycle tests

In `src/audit/tests.rs`, modeled on the cancellation test at `:345-370`
(which drives Sirv state without network):

- `unpairing_clears_the_finished_job` — install a finished `SirvJob` on the
  audit, call `unpair_sirv`, assert `sirv_job.is_none()`.
- `an_armed_overwrite_is_withdrawn_by_unpair` — set
  `sirv_confirm = Some(SirvJobKind::PushChanged)`, call `unpair_sirv`,
  assert it is `None`.
- `saving_credentials_retries_a_failed_listing` — build an audit with a
  pairing whose `files` is `Listing::Failed("old secret".into())` and a
  settings panel holding non-empty credentials; call `save_sirv_settings`;
  assert **synchronously, without advancing the executor** (the retry it
  schedules would attempt the network):
  `matches!(pairing.files, Listing::Walking)` and the pairing's client was
  replaced (e.g. capture the old `Arc` pointer first and assert
  `!Arc::ptr_eq(..)`). Note `save_credentials` writes a real file — set the
  `IMAGEGUIDE_CONFIG_DIR` env override (see `src/sirv.rs:529`) to a scratch
  dir in this test, as the existing credential tests do.

**Verify**: `cargo test --locked` → all pass including the new tests.

### Step 7: Full gates

**Verify**: `cargo test --locked`, `cargo clippy --all-targets -- -D warnings`,
`cargo fmt --check` → all green.

## Test plan

Steps 4 and 6 list the automated tests (two Display tests, two lifecycle
tests). The Stop button and confirm flow are render-state changes under the
repo's live visual proof contract — capture the three states listed in
"Commands you will need", or explicitly hand that to the reviewer.

## Done criteria

- [ ] `cargo test --locked` exits 0, with ≥6 new tests and the rewritten
      unpair test asserting `sirv_job.is_none()`
- [ ] `cargo clippy --all-targets -- -D warnings` exits 0
- [ ] `cargo fmt --check` exits 0
- [ ] `grep -n "sirv-stop" src/audit/view.rs` → one match
- [ ] `grep -n "stopping" src/audit/sirv_actions.rs` → cancel sets it; both loops acknowledge
- [ ] `grep -c "job.finished = true" src/audit/sirv_actions.rs` → matches only in the loops' acknowledge/terminal updates, none in `cancel_sirv_transfer`
- [ ] `grep -n "sirv_job = None" src/audit/sirv_actions.rs` → one match (in `unpair_sirv`)
- [ ] `grep -c "job.failures.join" src/audit/view.rs` → 0 (the uncapped join is gone)
- [ ] `git status` (in your worktree) shows changes only in the in-scope files
- [ ] `plans/README.md` status row updated

## STOP conditions

Stop and report back (do not improvise) if:

- `SirvJobKind` cannot take `PartialEq`/`Copy` without breaking a derive
  contract elsewhere.
- `Error`'s `Display` is load-bearing for parsing anywhere (grep for
  `.to_string()` on sirv errors being matched on — there should be none).
- The confirm state fights the render borrow (`self.sirv_confirm` read while
  `self` is borrowed in the builder chain) beyond an honest restructure of
  the row closure — report the exact borrow error.
- The cancellation test harness at `src/audit/tests.rs:345` does not in fact
  construct Sirv state without network — then write only the tests it can
  support and report the gap.

## Maintenance notes

- The confirm state is deliberately per-kind, not a modal dialog. If a real
  dialog system lands later (gpui-component has one via `Root`), migrate
  both confirms there.
- SIRV-12 (persist pairing across launches, show sync state outside the
  modal, guard folder-change re-targeting) is the recorded next step for
  this panel — it changes `settings.rs` schema and was kept out of this
  plan on purpose.
- Reviewer scrutiny: cancel-then-clear ordering in `unpair_sirv` (a cleared
  job must still have been marked stopped for the in-flight loop's
  generation check), and that no new button starts a transfer while `busy`.
