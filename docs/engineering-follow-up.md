# Processing trust: current evidence and next gaps

Updated 2026-09-08 against `77ab9191a8986604fa54f2f41a41863c33584302`
(v0.6.6). These are source observations, not reproduced native failures or proof
that a test suite passed. The [execution ledger](workbench-execution-plan.md)
owns sequencing. This replaces the September 5 backlog where code has moved on.

## Preserve source-present work

| Area | Evidence at this baseline | Do not rebuild |
| --- | --- | --- |
| Destination preflight | `src/audit/convert_job.rs::start_conversion` calls output-context proof on the background executor, with run/dataset ownership | The former request to move that proof off the click handler |
| Presets | `src/recipe.rs`, `src/audit/recipe_actions.rs`, CLI `--preset-file` | A parallel preset schema/library or GUI-only resolver |
| Product-set jobs | `src/job.rs`, `src/audit/job_actions.rs` | A second persistence/mapping model because W2 used to say Proposed |
| Output identity | Recipe fingerprint and AVIF-speed fields are present in `src/manifest.rs` | Another unrelated manifest or settings-aware skip mechanism |
| Existing processing guards | Earlier shared lossless-depth and prepared-upload ICC changes remain part of the baseline | Reintroduction of old lossy/depth/profile fallbacks |

Source presence is narrower than complete correctness. Re-run the relevant
acceptance journeys and record the exact commit, OS, fixtures and commands in an
implementation PR before changing a source-present item to runtime-verified.

## F1: finish recipe, source and result identity

Extend `recipe::fingerprint_settings`, manifest records and job references instead
of creating separate identity rules for GUI, CLI and future policy checks. Current
fingerprinting uses settings labels and optional speed, not a complete processing
revision contract. Test precise numeric settings, resolved defaults, codec options
and implementation changes. A display string is not a canonical numeric encoding.

`job::SourceRef` stores path, size and second-resolution modification time. That
is useful for cheap UI freshness, not proof that the exact bytes are unchanged.
Revalidate/hash at processing and submission boundaries, tied to bytes actually
consumed. Do not stat or hash every visible row on every render. Preserve the
identity of historical outputs and protect externally edited/unrecorded files.

Current `Job.target_recipe` and `ServerBinding` are starting structures, not proof
that execution consumes every target reference or that local role mappings are
server-authorized. Verify the complete bind -> resolve -> convert -> result path,
including a deleted/edited preset. Snapshot the effective recipe for each run.

Exit: same source/request has consistent preparation/refusal in preview, estimate,
export and upload preparation; changed source/recipe/engine cannot produce a false
verified reuse. Keep named excluded samples rather than projecting all-file success.

## F2: make portable jobs safe before widening sharing

The inspected `Job::to_portable` writes `base.display()` into `base_hint` and copies
reserved `ServerBinding` fields. `from_portable` rejects absolute/rooted paths but
the inspected join does not reject parent components itself. These are specific
source-level review targets, not a claimed exploited vulnerability.

Define a shared untrusted-import boundary before promoting browser/agent/shared-job
imports. Remove machine-specific path hints; strip connected bindings or require
explicit reauthorization/rebinding. Names/SKUs may also be private: preview the
metadata being exported. Do not claim that a job without tokens contains no
private information.

Reject traversal, drive-relative/UNC/device paths and path escapes under both
platform syntaxes, then enforce actual root confinement including symlinks and
junctions before source reads and output writes. Bound bytes before reading a
whole file, plus counts/lengths at parsing. Imports do not execute, upload, spend
or inherit authority. Existing local jobs need a non-destructive migration path.

Exit: adversarial fixtures cannot read outside a chosen root or import permissions;
a shared export contains only approved metadata; missing sources become relink
states without guessing. Keep tests on supported Windows and Unix paths.

## F3: persistence and asynchronous ownership

Both recipe and job `write_atomic` helpers remove an existing destination before
rename on Windows. Investigate crash/failure behavior rather than describing that
sequence as uninterrupted atomic replacement. Preserve the last good file during
failed writes and test recovery; avoid a new storage framework just for this fix.

`job_actions::refresh_job_states` captures dataset generation. Test rapid mapping,
recipe and job changes within the same dataset, and fence results by relevant job
revision/attempt where needed. Test multiple saved jobs sharing a root: the current
loader chooses the first match, which is not an explicit user decision. Do not
lose or silently switch another job's mappings while adding multiple targets.

Exit: failed save/restart preserves recoverable prior state, and an older async
result cannot relabel newer job work. Exercise target changes, cancel, relink,
source replacement and late results in the real UI.

## F4: contract hygiene and bounded execution

The README and bundled Agent Skill still describe some older skip/error behavior.
Follow [A1](agent-execution-plan.md) to lock exact stdout/stderr/exit semantics to
fixtures; preserve intentional compatibility. Correct docs in the same code/test
PR rather than maintaining a second prose-only contract.

Retain format-aware workers and thumbnail/estimate cache limits. Multi-target
execution must share a total decode-memory budget rather than multiply every
existing queue's concurrency. Include transient buffers and decoder behavior in
measurements; header estimates are admission hints, not guarantees. Keep the
single crate and existing safe output writer/restore semantics.

Source profile preservation is not proof of color-managed rendering or an sRGB
pixel conversion. Use realistic tagged/deep/animated/transparent fixtures and
independent output checks. Do not use synthetic uniform pixels alone to certify
compression quality, product color or cutout edges.

## Validation obligations

For implementation changes, run the pinned-toolchain gates and relevant feature
variants; use the current build definitions rather than old AGENTS line numbers:

```sh
cargo test --locked
cargo clippy --locked --all-targets -- -D warnings
cargo fmt --check
```

Add black-box CLI fixtures, supported-OS filesystem tests and real-window proof
for changed journeys. Record what actually ran. Test slow/refused destinations,
cancel/restart, replace/restore and unsupported depth without source loss. Use
fake/loopback services for remote failure injection; live paid tests need separate
authorized inputs/accounts and a hard budget.

This documentation PR runs no Rust/native/server tests and certifies no platform
runtime. Do not equate test definitions, CI configuration or a passing local codec
test with end-to-end supplier authorization or billing correctness.
