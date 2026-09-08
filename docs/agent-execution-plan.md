# Agent-friendly local execution

Proposed extension of the existing CLI, 2026-09-08. Owner role: processing/CLI
engineer. The [execution ledger](workbench-execution-plan.md) owns priority.
No saved-plan command, MCP service or new report schema is implemented by this PR.

## Start from the real interface

Current source includes `press audit`, `press convert`, `--json`, `--dry-run`,
`--preset-file`, output manifests, restore, and a bundled `press skill` document.
Use `src/main.rs`, `src/recipe.rs`, `src/manifest.rs` and
`.agents/skills/press-cli/SKILL.md` as the starting points, not a second image engine.

There is already documentation drift: the bundled skill describes destination
refusal as exit 2 with no JSON, while current conversion reporting includes schema
2 run-level errors. It also describes skip behavior as purely timestamp-based,
whereas `queue_run` distinguishes recognized managed outputs from legacy outputs.
Establish exact behavior with executable fixtures before editing that contract.
Do not copy the old skill's description into a new agent integration.

The product promise is **bounded, inspectable local work**, not "an LLM can do
anything with your images." Intent may come from an agent; permission comes from
the user or an explicitly configured automation scope, never a report or recipe.
Keep ordinary CLI work local, with no implicit upload or paid-operation fallback.

## A1: contract and distribution hygiene

Create fixtures for audit success/partial failure, conversion success/partial
failure, invalid invocation, refused destination, dry run, changed settings,
legacy outputs, replace and restore. Assert exit status, stdout JSON shape, stderr
and filesystem effects together. A partial result can have successful writes;
report it accurately. Verify that `press skill` matches its source document.

Document supported versions and compatibility, examples, explicit option override
precedence and machine-readable named failures. Preserve source-present behavior
or ship a documented versioned change; do not silently normalize away an existing
consumer's exit-code meaning. For planning, distinguish checks completed from
pixel-dependent checks deferred to execution.

Publish one worked agent example and one project/build example using existing
commands. Test setup on a clean supported installation. The current single binary
links the desktop stack; a CLI-only package is an evidence-led packaging option,
not a prerequisite or an already available headless distribution. Keep MCP and
framework-specific wrappers deferred until a real client cannot use the CLI.

## A2: a saved plan is a snapshot, not authorization

Proposed lifecycle: **inspect -> resolve -> preview a plan -> authorize execution
-> execute/reconcile -> inspect receipt**. Name the commands only in the
implementation PR after the data contract is reviewed.

| Plan field | Required meaning |
| --- | --- |
| Plan identity/schema | Stable ID, supported schema and digest of normalized execution inputs |
| Sources | Explicit selection, content identities at execution boundaries, dimensions/depth and supported preparation decisions |
| Recipe snapshot | Full effective settings, resolved defaults, transform/engine revision and refusals; not just a saved preset's display name |
| Targets | Independent target IDs, locally authorized output roots, source-to-output mapping and collision decisions |
| Policy/checks | Optional requirement snapshot/revision, checks completed, deferred or unsupported, and applicable time context |
| Write scope | Whether originals stay untouched; replace/restore requires its separate explicit scope |

Keep portable task data separate from machine-local execution bindings. A plan
copied to another machine must be rebound and revalidated, not treated as permission
to access its original absolute paths. No shell snippets or automatic downloads.

Before execution revalidate sources, output ownership, destination confinement,
policy applicability and engine compatibility. A changed input or different
collision outcome invalidates the affected plan; explain it and require a new
review rather than silently executing a different job. Snapshot/hash handling must
cover the actual bytes processed, not only a stat performed before the read.

A matching plan digest proves consistency with the plan, not user consent,
trustworthy authorship or permission to replace originals. Batch automation can
have an explicit local scope, but importing a task cannot create that scope.

## A3: execution and receipt semantics

Use stable run/item/target IDs and the existing safe writer/manifest recovery
boundary. Track pending, written, failed and cancelled/unstarted work distinctly;
verify completed outputs before reuse. Retrying retrieval or inspecting an existing
result is not another encode. If a process dies between output install and receipt
persistence, reconcile disk identity and the write record rather than guessing.
Do not promise universal exactly-once filesystem execution.

The receipt records actual output hashes, sizes, dimensions, effective recipe and
engine identity, policy/check outcomes and named failures. Local receipts remain
inspectable data, not signed marketplace approvals. Redacted shareable receipts
and machine-local recovery records are different views; never strip information
needed for restore from its existing internal manifest.

Regenerating an outdated result and retrying a failed item are different actions.
Protect unrecorded or externally edited outputs. A command-line policy failure
must not be conflated with decoder failure or invalid syntax. Finalize the
versioned status/exit-code contract with fixtures, not prose alone.

## Acceptance and release

| Scenario | Required outcome |
| --- | --- |
| Agent asked only to audit | No source/output writes and no image upload |
| Unsupported, corrupt or oversized task | Named refusal before effects; no guessed defaults |
| Source changes after planning, including preserved timestamps | Revalidation refuses stale execution or builds a newly reviewed plan |
| Same effective recipe via GUI/CLI | Same supported transforms and output verification, independent of GUI preferences |
| Output modified by another program | Preserve it; explain ownership conflict |
| Cancel, partial failure or process restart | Successful outputs identifiable; unstarted items stay unstarted; only required work is retried |
| Plan contains instructions disguised as data | No arbitrary command, path access, upload or spend |
| Upgrade changes encoder semantics | Record new engine identity; do not silently reuse an incompatible verified result |

A1 can ship independently. A2/A3 depend on the relevant identity and persistence
work in [engineering follow-up](engineering-follow-up.md); they are not gates for
ordinary conversion or a supplier pilot. Use local fixtures, not live Studio keys.
Run the pinned Rust gates plus black-box CLI tests and record actual results.
Measure correctly completed authorized tasks and setup effort, not skill installs.
Rollback disables only the new protocol while retaining documented existing CLI
behavior, previous outputs and recoverable manifests.
