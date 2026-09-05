# Workbench implementation slices

Status: proposed, 2026-09-05. This is the cross-document execution order, not a
claim that work is scheduled or complete. Start from [roadmap](../ROADMAP.md).
Each implementation PR must name one accountable person for the role below and
record its actual code baseline and proof. Role labels are not assigned staffing.

## Scope and dependencies

| Slice | Outcome | Accountable role | Dependency |
| --- | --- | --- | --- |
| W0 | Preserve landed fixes; make recipe execution trustworthy | Processing engineer | Current baseline audit |
| W1 | Persistent personal recipes | Desktop engineer | W0's supported transform contract |
| W2 | Optional product-set jobs and target mapping | Desktop engineer | W1 identity/persistence conventions |
| W3 | One end-to-end supplier pilot | Integration owner with Studio counterpart | W0-W2 plus confirmed native auth/intake contract |
| W4 | One contextual quoted Studio service | Integration owner with billing counterpart | W0; W3's reusable identity boundary; durable service/ledger contract |
| W5 | Maintained templates and multi-target local deliverables | Processing/product owner | W1-W2; pilot-specific target requirements |
| W6 | Explicit own-workspace connection and shared review | Studio integration owner | W3 identity; validated user demand and existing canonical services |
| W7 | Repeat-work conveniences and sponsored corrections | Relevant desktop or service owner | Measured repeat demand; W4 for sponsorship; separate bounded pilots |

Auth/API discovery for W3 can run beside W1-W2. W5 can supply one pilot-needed
recipe earlier, but a marketplace catalog is not a prerequisite for W3. W4 is
optional to normal supplier submission; do not wait for AI checkout to deliver W3.
W6 and W7 are gated extensions, not a commitment to start parallel projects now.

Keep detailed contracts in their owning documents: [workbench](product-workbench.md),
[recipes](delivery-recipes.md), [supplier intake](supplier-portal-integration.md),
[connected services](studio-connected-services.md), and
[packaging/pilot](adoption-and-packaging.md). Do not copy a second contradictory
state machine or policy table into implementation briefs.

## W0: trustworthy processing before new promises

Re-read the current engine and [engineering follow-up](engineering-follow-up.md).
Shared lossless-depth checks and Studio ICC preservation have landed; preserve them.
Do not reimplement those fixes from the original review. Introduce only the remaining
common requested/effective recipe and per-file result contract needed by W1.

Move remaining blocking destination proof off the UI thread with explicit planning
ownership, retain previous results on refusal, and protect source/output identity
across changes. Preserve collision/recovery behavior and named unsupported results.
Use realistic tagged/deep/animated images and slow/interrupted destination fixtures.

Exit evidence: local tests/lint/format, real-window planning/cancel checks, unchanged
originals, accurate failures and cross-path results. A supported simple recipe may
ship without every future transform; unsupported operations must remain unavailable.
No UI framework rewrite or second worker/manifest system.

## W1: save and reuse a personal recipe

Implement the versioned typed recipe in the existing conversion boundary, normalize
built-ins through it, and add atomic storage plus Save current, Duplicate, Rename,
Delete, Import and Export. A changed control produces a modified copy, not silent
mutation of a saved preset. GUI and CLI resolve explicit recipes the same way;
headless operation does not inherit the last visible GUI selection.

Exit demonstration: save, restart, load, adjust, export/import and execute a recipe
offline without an account. Bad/future schema, unsafe paths and unsupported required
steps fail before writes. Deleting a recipe never deletes deliverables. Do not build
a remote preset marketplace, executable DSL or general workflow canvas.

## W2: optional product sets, not a desktop PIM

Add a stable local job and product-set mapping on top of Files. Support manual and
CSV/filename-assisted mapping, roles, missing/unmapped/ambiguous states, and one
selected target. Connected bindings reserve explicit scoped canonical IDs; local
names and SKUs never grant access or create server products. Keep quick conversion
available without a product-set form.

Exit demonstration: recover a job after restart, handle equal SKUs for different
recipients, relink a missing source and invalidate affected outputs after an edit.
Keyboard/selection/filter actions keep operating on the shown scope. Portable job
export strips credentials, private connection data and absolute paths. No global
catalog, shared editing or two-way sync reconciliation engine in this slice.

## W3: supplier-first vertical slice

First produce a reviewed integration contract with the Studio counterpart: secure
native sign-in, supplier/recipient scope, assignment/requirements reads, canonical
mapping, ordinary submission entitlements, upload finalization, receipts, correction
and resubmission. Resolve whether existing routes/services supply each behavior;
record confirmed gaps before building UI against guessed endpoints. Do not weaken
server guards or require a merchant key to meet the deadline.

Then implement one retailer flow: authenticate, load assignments and policy revision,
map local files, prepare, inspect exact outputs, submit, reconcile and display feedback.
Use bounded polling initially. Browser fallback stays visible. A revoked assignment
or changed policy stops/revalidates the affected submission, not the entire local job.
No personal paid Studio subscription is required solely for authorized submission;
this needs a server entitlement decision, not a client-side bypass.

Exit evidence: the existing supplier acceptance matrix, particularly wrong-recipient
refusal, partial batch/restart, response lost after acceptance and rejection/resubmit.
A non-primary operator can guide setup from the documented flow. Do not include
sponsorship, extensive channel connectors or a new desktop approval queue.

## W4: contextual hosted value with recoverable spend

Choose one concrete operation from pilot demand, such as a hosted background-removal
alternative or a lifestyle candidate. Reuse the existing Studio integration where
appropriate but confirm a server quote/accept/job/receipt contract before presenting
new guarantees. Bind source identity, parameters, actor, payer, model and maximum
spend to an immutable attempt; use the canonical server ledger.

Add contextual discovery, explicit local/cloud and cost disclosure, one payer per
job, cancellation semantics, status recovery and exact result retrieval. The first
version can use the user's own authorized workspace; sponsored allowances are later.
No automatic fallback charge to another workspace. Keep/Discard does not imply a
new charge, refund, approval or submission.

Exit evidence: fake-service tests for duplicate clicks, expired quotes, logout,
unknown provider outcome, lost response/download, cancel and partial failure;
then a separately authorized cost-capped live pilot. Do not replace existing direct
AI behavior without an explicit migration, or advertise new billing guarantees before
the server can meet them.

## W5: maintained templates and multiple deliverables

Select two or three targets from actual pilot work. Source and version requirements
by channel/region/category/role as applicable; assign a maintenance owner and show
coverage/effective dates. Use the same recipe engine for exact canvas, naming and
bounded byte budgets as those transforms are implemented. Unknown visual checks
stay Not checked/Needs review, never Marketplace approved.

Support separate outputs per source/master and target, with independent paths,
reports and retry state. A master-required retailer submission cannot receive the
last selected low-resolution marketplace derivative by accident.

Exit evidence: source/recipe/policy revision changes, impossible constraints, case
and naming collisions, transparent/tagged images and one-target failure while others
succeed. No direct marketplace-account publishing, remote recipe feed or speculative
catalog of hundreds of templates. A single-target subset may ship first.

## W6: the supplier becomes a workspace customer by choice

Connect/create the user's own Studio workspace and explicitly save selected assets
or share deliverables for review using existing server-owned workflows. Preserve
local work and identify recipient audience, asset ownership and workspace. Do not
silently promote retailer material into the user's personal business account.

Exit demonstration: invited supplier and independently owned workspace coexist without
cross-access, billing confusion or accidental public links. Existing comments/status
come from Studio, not a second review database. A browser roundtrip restores task
context; an entirely desktop service session is equally successful. Full cloud
administration and a desktop PIM editor remain out of scope.

## W7: add only the repeat work the pilot exposes

Treat these as separately approved increments, not one large automation project.
For editor handoff, choose an installed application, prefer a working copy and
reinspect stable saved revisions. For watched export folders, bound scope, ignore
partial files, debounce changes and offer local preparation with Pause. No script
execution from imported recipes or silent remote spending/submission.

For sponsorship, first implement server grants scoped by supplier/task/operation,
expiry and caps, using existing reservations/receipts. Test concurrent last-credit
requests, revoked grants and response-loss recovery. Separate sponsor and personal
payer contexts in UI and metrics. Sponsorship is not needed to keep suppliers using
ordinary portal submission. Expansion requires the packaging pilot's usefulness,
correctness and cost evidence.

## Implementation touchpoints to inspect

These are existing file areas at the baseline, not instructions to expand the
central Audit object indefinitely. New modules are justified by ownership, not by
an abstract layering target.

| Area | Existing entry points | Keep / extend |
| --- | --- | --- |
| Processing and identity | `src/convert.rs`, `src/scan.rs`, `src/compare.rs`, `src/manifest.rs`, `src/output.rs` | One safe preparation/output boundary; source/master and recipe revision |
| Local job UI | `src/audit/mod.rs`, `state.rs`, `panel.rs`, `media.rs`, `convert_job.rs`, `tests.rs` | Preserve index/generation ownership; separate product-job state when needed |
| Persistence and CLI | `src/settings.rs`, `src/main.rs` | Explicit recipe persistence and headless determinism |
| Hosted actions | `src/studio.rs`, `src/audit/studio_actions.rs` | Reuse transport where compatible; add confirmed scoped service lifecycle |
| Existing non-portal actions | `src/sirv.rs`, `src/local_ai.rs` | Keep distinct from supplier approval and remote billing authority |

Server work is a coordinated dependency, not a change authorized or implemented by
this docs PR. Keep private operational details out of this public repository. Record
wire schemas, entitlement decisions and server tests in the appropriate owning repo;
link only artifacts that their owner permits publishing.

## Review and release evidence

Every implementation slice needs a small source diff, tests for its actual invariants,
a real task demonstration where UI changes, clear unsupported cases, and a rollback
that preserves local work and accepted remote receipts. Use the pinned toolchain:

```sh
cargo test --locked
cargo clippy --locked --all-targets -- -D warnings
cargo fmt --check
```

UI proof must include actual target operating systems, keyboard focus, narrow windows,
scale factors, pending/failed state and restart. Source comments and test definitions
are not proof of native behavior. Connected slices additionally require scoped-server
contract tests and failure injection before a limited live pilot. No production
credentials or billable calls belong in ordinary tests.

A documentation-only PR validates links, scope, source references and internal
consistency; it does not rerun or certify Rust/native/server gates. Do not reopen
already-landed code fixes because an old plan says Pending. Recheck the baseline
before each implementation, and record what actually ran in that PR.
