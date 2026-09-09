# Execution ledger and implementation briefs

Updated 2026-09-08. Baseline: `77ab9191a8986604fa54f2f41a41863c33584302`
(v0.6.6). Read the [roadmap](../ROADMAP.md) first. This is a live decision/status
ledger, not an assigned schedule. Accountable roles below must become named people
in implementation PRs; no staffing, price or deadline is invented here.

## Evidence vocabulary

**Source-present** means inspected code contains the capability, not that its full
journey passed. **Proposed** means not delivered by this plan. **Contract-dependent**
means a counterpart must confirm the relevant API/authorization. **Deferred** means
not required for the next useful job. Record runtime verification separately with
commit, platform, test command, fixtures and observed outcome.

This ledger supersedes blanket September 4/5 statements that all presets/product
jobs are unimplemented. The old workbench, packaging and connected-service docs
remain behavioral designs, not status checklists. Never rebuild a landed subset
because its owning design still uses future tense.

## Current baseline and W0-W7 reconciliation

| Earlier slice | Current evidence | Remaining work |
| --- | --- | --- |
| W0: trustworthy processing | Async destination proof, run ownership, recipe fingerprint fields and earlier depth/ICC guards are source-present | F1-F4: complete execution identity, import/persistence boundaries and relevant runtime proof |
| W1: personal recipes | `src/recipe.rs`, recipe UI actions and CLI `--preset-file` are source-present | Verify migration/round-trip, precision/default semantics and GUI/CLI execution parity; no new preset library |
| W2: product-set jobs | `src/job.rs` and `src/audit/job_actions.rs` contain persistence, mappings, target reference, import/export and relink actions | Verify full journeys; fix portable path handling, name/SKU metadata-privacy preview, job identity and stale-result ownership |
| W3: supplier pilot | Integration design exists; the `ServerBinding`/role-mapping fields in `src/job.rs` are reserved structures, not native supplier authorization | Confirm Studio contracts, then S1-S3 |
| W4: contextual hosted service | Existing direct Studio actions are not proof of quote/idempotency/charge recovery | Preserve current behavior; C1 only after confirmed service/ledger contract |
| W5: templates/multiple outputs | Recipe model exists; current job has a single `target_recipe` | Pull D1 forward; D2 supplies a real target's checks; D3 catalog remains demand-led |
| W6: own workspace/review | Proposed connected-work design | C2 when shared work is needed and canonical services support it |
| W7: conveniences/sponsorship | Proposed expansion | Deferred; each gets an independent demand/security/economics decision |

Read [engineering follow-up](engineering-follow-up.md) for exact observations.
No runtime test was executed for the original documentation refresh. The
September 8 implementation follow-up below records later source and verification
separately; the remaining obligations in this table still apply.

## September 8 implementation follow-up

Muse Spark's implementation through `e447f41` adds the following bounded slices.
This follow-up starts at that commit and completes the interrupted hosted-service
rehearsal, with Luna xhigh executing code and Codex reviewing it. These are local
tools and fixtures, not confirmed Studio contracts or completed pilot journeys.

| Slice | Source evidence | Remaining boundary |
| --- | --- | --- |
| F1 | `d3ed98a`: numeric recipe fingerprints include a processing revision; manifests record source hashes and compare them for reuse | The manifest hashes the source at record time, not from the decoder's input buffer. Full processing-boundary identity and GUI/CLI parity remain open |
| F2 | `8d2462a`, `7062892`: portable paths reject traversal and enforce the chosen root; exported bindings and machine path hints are stripped | Metadata privacy preview, bounded reads throughout the older import/library paths, and native Windows filesystem proof remain open |
| F3 | `93456ac`: recipe/job writes use shared replacement and refresh results check job revision | Same-root GUI loading is deterministic but still selects a job without an explicit choice; real-window late-result/restart proof remains open |
| F4/A1 | `56e07dc`: black-box CLI fixtures cover JSON, named skips, failures, dry run and restore; bundled skill updated | Clean-install distribution evidence and the proposed A2/A3 saved-plan protocol remain open |
| H1-H3 subset | `df76559`, `9f2031a`, `64adc5c`: validate a handoff file, map resources under a chosen root, and inspect a supplied deployment tree | No ImageGuide counterpart contract, browser deployment, or live re-audit is proved by a local tree check |
| D1 subset | `0b8888c`: repeated `--target recipe=namespace` converts serially through the existing engine and reports each target | GUI target editing/execution, maintained D2 requirements and D3 catalogs are not delivered; memory measurements remain open |
| S1-S3 rehearsal | `e447f41`: assignment fixtures, attempt files and explicit `--fake` submission/status/correction commands | Native authorization, canonical intake and an authorized retailer pilot remain blocked on S1 |
| C1 rehearsal | Interrupted `src/studio_ledger.rs` and CLI completed in this follow-up | Fixture-only quote/accept/reconcile/cancel/retrieve behavior; no upload, hosted inference, real credit reservation or billing authority |

The follow-up review also corrects supplier attempt identity/recovery and ensures
each AVIF target executes with its saved speed. Local rehearsal commands must
preserve corrupt recovery files, retain ambiguous outcomes for reconciliation,
and refuse stale input or an existing retrieval destination.

Verified on Linux x86_64 with Rust 1.97.1, against the working tree based on
`e447f41` (no new commit or remote landing):

- `cargo test --locked --quiet`: 619 passed, 2 ignored.
- `cargo test --locked --features updater --quiet`: 635 passed, 3 ignored.
- `cargo clippy --locked --all-targets --all-features -- -D warnings`: passed.
- `cargo fmt --check` and `git diff --check`: passed.
- The built binary's `press skill` output matches its source document byte for byte.

The process fixtures demonstrate corrupt supplier-history preservation, legacy
attempt refusal, explicit assignment context, cancellation refusal after transfer,
and receipt recovery across separate processes. Hosted fixtures demonstrate
quote/accept/status/retrieve, stale-input refusal, lookup-only reconciliation,
same-attempt explicit retry, expiry, and refusal to overwrite a retrieved file.
An independent three-target AVIF run recorded speed 10, explicit default 6 and an
unset default correctly; the repeated run converted zero files and skipped all
three. The corresponding regression is in `tests/cli_contract.rs`.

Legacy supplier attempts without assignment context remain inspectable but cannot
be reused for effects. Fake supplier scripts specify each status step explicitly;
they do not simulate autonomous server progress. No release, native UI
demonstration, macOS/Windows run or live supplier/hosted pilot was performed.
The next connected step remains a confirmed Studio contract and authorized pilot;
the incomplete local foundation/GUI obligations in the table are still open.

## Work selection and dependencies

Start with a concrete job, not all rows at once. The recommended local path is
relevant F1/F2/F3 checks, then H1-H3 or D1 according to available participants.
A1 contract hygiene can proceed independently, and its fixture PR closes the
contract half of F4; F4's execution-budget half travels with the multi-target work
that needs it. I1 can establish whether phone inputs are a real blocker before
selecting a decoder.

A committed supplier pilot outranks speculative acquisition work. S1 discovery
can proceed immediately; S2/S3 consume only the local foundations and requirements
they need. **Do not put H, A2/A3, D1/D3, HEIC or AI checkout on S's critical path**
unless that pilot's actual inputs/workflow require them. D2 may be implemented for
S before D1. Shared recipe/source identity is a narrow foundation, not a mandate
to complete every future transform first.

| Track | Accountable role | Earliest dependency | Owning brief |
| --- | --- | --- | --- |
| F1-F4: foundations | Processing/desktop engineer | Current source audit | [Engineering](engineering-follow-up.md) |
| H1-H3: browser remediation | Desktop engineer + ImageGuide counterpart | Shared import contract, source identity/confinement before effects | [Browser handoff](browser-handoff-plan.md) |
| A1-A3: agent execution | Processing/CLI engineer | A1 now; complete relevant identity/persistence before A2/A3 | [Agent execution](agent-execution-plan.md) |
| D1-D3: deliverables/checks | Processing/product owner | Existing recipes/jobs plus relevant foundations | [Delivery recipes](delivery-recipes.md) |
| S1-S3: supplier vertical slice | Integration owner + Studio counterpart | Confirmed scoped auth/intake; relevant local checks | [Supplier contract](supplier-portal-integration.md) |
| I1-I3: phone inputs | Processing engineer + packaging support | I1 evidence and decoder decision before implementation | [Phone ingestion](phone-image-ingestion-plan.md) |
| C1-C2: hosted/shared value | Studio integration owner + service owner | Confirmed native/service authority and observed demand | [Connected services](studio-connected-services.md) |

## S1-S3: keep the supplier loop executable

**S1: contract proof.** Status: proposed, needs a Studio counterpart. Next decision:
which existing services map to native sign-in, scoped assignment/requirement reads
and canonical intake. Record missing server work in its owning repository; do not
guess endpoints. No merchant key or unrelated paid workspace for authorized requests.

**S2: one real submission.** Status: blocked on S1 contracts. Pin workspace,
supplier, product/slot, source/output hashes, recipe/policy revisions and attempt
identity on the one pilot job. Browser fallback stays visible; generic Sirv folder
pairing is not the submission route.

**S3: correction and interruption recovery.** Status: blocked on S1 contracts.
Persist attempt IDs before sending; reconcile after acceptance, retain siblings,
restore pending work after restart. Show server states only when the server reports
them; revocation and policy changes trigger fresh server checks, never cached permission.

Acceptance is the existing supplier matrix: wrong recipient, revoked assignment,
changed policy, edited bytes, duplicate retry, cancellation races, partial batch,
restart and rejection/resubmission. Use fake services first and an authorized live
retailer pilot second. Include setup by someone other than the principal developer.
Success is reviewable work with less correction/support effort, not a browser visit
or compulsory installation. Roll back only the integration; keep local files and
accepted receipts.

## C1-C2 and deliberate deferrals

C1 is one contextual hosted operation under the connected-services design. Status:
blocked on the confirmed Studio ledger/provider contract. No bulk auto-retry or
sponsorship on legacy direct-call paths meanwhile; a discard is not automatically
free and a timeout never authorizes a paid rerun.

C2 is an explicit own-workspace save/share journey on canonical Studio assets.
Status: deferred until shared work is needed and canonical services support it.
Retailer scope and personal workspace stay distinct; no desktop PIM required.

Editor handoff, watched folders, native browser launch, MCP, CLI-only packaging,
large template catalogs, remote policy feeds and sponsorship remain separate
options. No watched-folder upload/spend without independently designed consent and
server controls. No weakened local models or artificial export caps to force C1/C2.

## Definition of ready and done

Before coding a slice, identify its real user/job, accountable person, read-first
files, counterpart contract, bounded result, exclusions, failure cases and evidence.
Do not open a general "build the workbench" implementation task. Each H/A/D/I brief
already supplies small PR boundaries; F and S should be similarly split when needed.

An implementation PR is done only when its actual changed behavior has evidence:
pinned Rust test/lint/format gates, black-box CLI checks where relevant, fixture
compatibility/migration and supported-OS filesystem tests. UI changes also require
real-window demonstration, keyboard/focus, narrow layout, cancelled/failed state
and restart. Connected changes require server failure injection and scoped pilot
proof; no live credentials or billable calls in ordinary regression tests.

Update this ledger in the same PR: status, baseline/result SHA, actual commands,
platforms, demonstrated journey, remaining limitation and next decision. A link
to code or the existence of a test is not a passing runtime result. Do not erase
prior acceptance criteria when marking a smaller subset source-present.

## Product evidence and expansion gates

Use the cohort-specific measures in [strategy](preflight-strategy.md) and the
retailer/service economics in [packaging](adoption-and-packaging.md). Agree the
threshold before observing outcomes. Track real job completion, repeat work among
users with another eligible job, active preparation time and support effort.
For browser work, distinguish mapped, exported, deployed and re-audited stages.
For hosted work, distinguish gross use from retained useful outcomes and net cost.

Any source loss, cross-tenant delivery, unauthorized upload/charge or unrecoverable
accepted operation blocks expansion of that path. Preserve the local product and
existing browser workflow as fallbacks. If a channel does not produce useful repeat
work, improve the observed bottleneck or defer it instead of adding more features.

## September 8 closeout checkpoint

This closeout records the reviewed integration tree after `origin/main` was
merged at `b5b57b3cda84437c1dcc436de58de325514f991d`. The approved local
foundations and D2 requirements work are present through `b6570f6`; the isolated
D3 preparation experiment was stopped and is not part of this tree.

| Slice | Landed evidence | Boundary at closeout |
| --- | --- | --- |
| F1 | Exact source/output identity, bounded consumed bytes, recipe processing revision and content-verified reuse in `b6570f6` | GUI preparation parity remains open under D3 |
| F2/F3 subset | Bounded imports, privacy preview, explicit saved-job choice and asynchronous ownership in `b6570f6`; F3 visibility and native export were demonstrated | Keyboard action focus still needs follow-up; no complete UI or all-platform claim |
| D2 | Strict local requirements snapshots and actual-output reports in `b6570f6`; use `press check <file-or-folder> --requirements-file <spec> [--json]` | Requirements are local and user-authored; required unsupported checks stay `not_checked`, so a report is not approval |
| Muse Spark fakehost | The fixture-only supplier and interrupted Studio rehearsal review was completed through `698e990` (supplier base `e447f41`) with explicit fake scripts and durable local attempts | No native auth, retailer service, hosted inference, billing or live pilot was performed |
| A2/A3 | Not landed; isolated checkpoint `codex/workbench-identity` at `d62dfd0` | Saved plans must not be presented as silently executable |
| D1 | Not landed; isolated checkpoint `codex/workbench-job-safety` at `fc9ad974` | The existing CLI target subset is not independent GUI target delivery |
| H1 | Not landed; extension checkpoint `codex/press-handoff` at `7216e32` | Local mapping/export work is not browser deployment or live re-audit proof |
| D3 | Not landed; preparation/parity work was stopped before integration | No maintained Google/eBay packs, policy catalog or pack approval is included |
| HEIF | Not landed; investigation only | No decoder, packaging or platform proof is included |

The D2 CLI check is bounded to local bytes and reports relative names, actual
content hashes, dimensions, format, bytes and observed evidence. The supported
AVIF path selects the native libavif dav1d backend and refuses nonidentity
`irot`/`imir`/`clap` transforms by name. It does not claim arbitrary backend,
orientation or all-platform support.

Root's final integration gates recorded `cargo test --locked` with 658 passed
and 2 ignored, and the updater variant with 674 passed and 3 ignored. The
all-targets/all-features clippy gate, `cargo fmt --check` and `git diff --check`
passed. An offline disconnected-container check converted a deterministic 8x8
PNG and passed the local requirements check; the wrong-format and wrong-dimension
fixture correctly failed for four WebP outputs. The frozen D2 binary hash was
`4020250913923ee82729f30f941445041950f1d5f55d256d678f536e6ca06c1d`. Logs and
JSON evidence remain under `ux/`; they are review artifacts, not release proof.

The F3 real-window proof uses local review artifacts under `ux/` (untracked in
this integration worktree): `ux/workbench-ui/f3-corrected-reveal-004.png` and
`ux/workbench-ui/exported-alpha.press-job.json`. Those artifacts prove the
reviewed Linux visibility/export path. Keyboard action focus remains unresolved,
and no macOS/Windows or complete UI claim follows.

### September 9 handoff review increment

The extension landed its Press export on `main` at `2e56550`. Its fixture
`test/fixtures/press-handoff.json` is copied byte for byte into
`tests/fixtures/` here, pinned to LF, and checked against the SHA-256 it landed
as, so a producer change that moves the shared contract fails on this side too.

On top of that, this branch adds the **session review** half of H2 and nothing
else. The window imports a report through the same bounded read and parser the
CLI uses, the user selects one folder, matching runs off the update thread, and
each resource is confirmed individually against the bytes on disk, with a
recheck that revokes a confirmation when those bytes change. The review writes
no job, recipe, setting or output, replaces no dataset, and starts no
conversion; it lives only as long as the session.

The CLI verdict `confirmed` is renamed to `path_match` (and the JSON summary
key with it) because it only ever meant "one exact supplied hint named one
existing file". No release tag contains the `handoff` command — `git tag
--contains 9f2031a` is empty — so this corrects an unreleased draft rather than
breaking a shipped contract, and `HandoffReport.schema_version` stays `1`. The
producer's own envelope schema is unaffected. `press skill` guidance is updated
to the labels the binary actually prints.

A valid report can be a mebibyte of preserved advisory fields with a warning for
each, so the card draws a bounded head of every list, previews each value with a
visible ellipsis, and discloses the counts it is not drawing; the pending report
keeps all of it. Source reads take one review-wide slot, so five hundred rows
cannot put five hundred bounded reads in flight, and the slot is released by the
read that finishes rather than by the review that started it. The card also
carries the folder walk's own diagnostics: a folder Press could not read
completely cannot show a resource to be absent, and every row that matched
nothing says so.

This is one increment, not H2. Preparing confirmed sources, durable task state
across a restart, deployment and re-audit — the rest of the
[H2 and H3 acceptance](browser-handoff-plan.md#implementation-prs-and-acceptance)
— all remain open, as does any native Windows or macOS proof.

### September 9 local-check label repair

`press handoff --root <dir> --deployed <dir>` reads a second folder on this
machine. It scanned that folder for files whose stem matched a mapping and whose
format and longest edge met the resource's constraints, then reported the result
as `deployed`, `deploy_summary.deployed` and a "verified against" heading. A
local directory listing cannot show that a website serves those bytes, that the
file is the reported image, that markup, viewport or saving model changed, or
that anything was re-audited, so those labels claimed more than the code does.

This corrects the labels, not the behaviour. The successful status is
`local_match`; the JSON report carries `local_root`, `local_checks` with the
matched `paths`, `local_summary` and a `local_evidence_scope` string stating the
boundary inside the document; the text heading reads `local file check under
<dir>` and repeats that scope on the next line. `--deployed` still parses and
still needs `--root`, and `--help` now calls it a local folder check with no
live site verification. `differs`, `missing`, `ambiguous`, the open findings
listing and the exit-1-on-gaps semantics are unchanged, and the check still runs
on automatic `path_match`/`candidate` mappings, which are name evidence and not
a person's confirmation. As with the `confirmed` → `path_match` rename this
corrects an unreleased draft — `git tag --contains 9f2031a` is still empty — so
`HandoffReport.schema_version` stays `1` and the producer's envelope schema is
untouched.

This is a truthfulness repair to one label set, not H3. Deployment, re-audit,
the local-export versus deployed distinction, changed viewport/model evidence
and the rest of
[H2 and H3 acceptance](browser-handoff-plan.md#implementation-prs-and-acceptance)
all remain open.

### Isolated checkpoints for tomorrow

| Worktree branch | Checkpoint | Closeout handling |
| --- | --- | --- |
| `codex/workbench-completion` | `b5b57b3` | Integration tree with this closeout pending review |
| `codex/workbench-requirements` | `326afb0` | D2 is at `b6570f6`; the later D3 error-shape WIP is not to be merged |
| `codex/workbench-identity` | `d62dfd0` | A2/A3 WIP; not landed |
| `codex/workbench-job-safety` | `fc9ad974` | D1 WIP; not landed |
| `codex/press-handoff` | `7216e32` | H1 WIP; not landed |

The closeout docs do not land those isolated checkpoints. No additional release,
marketplace, browser or cross-platform result is inferred.
