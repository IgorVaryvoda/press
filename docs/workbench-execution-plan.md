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
No runtime test was executed for this documentation refresh.

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
