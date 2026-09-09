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
| F2/F3 subset | Bounded imports, privacy preview, explicit saved-job choice and asynchronous ownership in `b6570f6`; F3 visibility and native export were demonstrated, and the export review's keyboard lifecycle followed on `codex/workbench-export-focus` at `1b520fe`, proved in a native Linux window | Linux only; macOS and Windows stay with native CI, and no complete UI claim follows |
| D2 | Strict local requirements snapshots and actual-output reports in `b6570f6`; use `press check <file-or-folder> --requirements-file <spec> [--json]` | Requirements are local and user-authored; required unsupported checks stay `not_checked`, so a report is not approval |
| Muse Spark fakehost | The fixture-only supplier and interrupted Studio rehearsal review was completed through `698e990` (supplier base `e447f41`) with explicit fake scripts and durable local attempts | No native auth, retailer service, hosted inference, billing or live pilot was performed |
| A2/A3 | Not landed; isolated checkpoint `codex/workbench-identity` at `d62dfd0` | Saved plans must not be presented as silently executable |
| D1 | Not landed; isolated checkpoint `codex/workbench-job-safety` at `fc9ad974` | The existing CLI target subset is not independent GUI target delivery |
| H1 | Not landed; extension checkpoint `codex/press-handoff` at `7216e32` | Local mapping/export work is not browser deployment or live re-audit proof |
| D3 | Not landed at that checkpoint; the first bounded preparation-parity increment followed on `codex/workbench-preparation-parity` (see the September 9 section below) | No maintained Google/eBay packs, policy catalog or pack approval is included |
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
reviewed Linux visibility/export path, and no macOS/Windows or complete UI claim
follows.

The keyboard follow-up on `codex/workbench-export-focus` puts the export review
in charge of its own keys. Opening Export from the job menu anchors the review's
focus handle on a wrapper that is a tab group but not itself a tab stop, then
steps to the next tab stop on a frame callback, so the keyboard lands on the
real Export button with its own focus ring and its native Enter and Space —
gpui-component's `Button` renders its own keyed focus handle and discards a
caller's, so `track_focus` on the button alone does nothing. Tab reaches Cancel,
unmodified Escape from either button closes the review, and Enter, Space and
Escape stop at the review instead of reaching the list behind it. Cancel, Escape
and a completed save all hand the keyboard back to the list. Four GPUI tests
drive the real dropdown, the delivered frame and whole key presses, because
`simulate_keystrokes` sends only the key down while a button activates on the
key up.

Root independently re-ran the gates on that commit — 666 passed by default and
682 with the updater feature, plus all-targets/all-features clippy and
`cargo fmt --check` — and reviewed the code before it was proved in a native
window rather than argued from tests. Root ran the normal `cargo build --locked`
debug app, not a release build, from source `1b520fe`, binary SHA-256
`6223460b66958e1b3377359c3652791c417c5a3804941bb5d7517a7634c33eb5`, at 800x600
under an isolated Gamescope session with a private X11 display and DBus.
Choosing the Alpha job explicitly and pressing Up then Enter in the job dropdown
opened the review with a visible focus ring on Export. Return opened the real
native Save dialog; cancelling that dialog left the review up and Export still
usable. Tab visibly reached Cancel, Enter there closed the review, and Escape
closed it too. An explicit Save wrote the 647-byte Alpha catalog portable job,
SHA-256 `c2750313dc9a46d0742546be824a8bd13854d4b4b75c1e28ae18d58a89aa23e9`, and
closed the review, after which Down moved the audit cursor again. Root reviewed
the screenshots; they and `proof.json` stay under `ux/` in the workbench
worktree as review artifacts, untracked here and not release proof. The proof is
Linux only: macOS and Windows remain the separate native CI gate.

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

### September 9 CI repair (branch `codex/workbench-ci-resume`)

Three tests that pass on Linux failed on the remote runners at base `8131e2c`:
`audit::tests::replace_results_compare_against_the_backup_original` on both
macOS and Windows, and `crash::tests::crash_prompt_is_modal_and_uses_exact_visible_copy`
plus `requirements::tests::receipt_report_contains_relative_names_and_actual_hashes_only`
on Windows only.

| Failure | Source | Fix |
| --- | --- | --- |
| Receipt names | `src/requirements.rs` | `relative_name` rejected every native Windows separator as a literal backslash. Validated components are now joined with `/`; a backslash inside a unix component, a non-UTF-8 name and traversal stay refused. |
| Replace comparison | `src/audit/media.rs` | `comparison_source` stripped a canonical root off an entry that carries the walk's spelling and, on failure, joined the absolute leftover onto the backup root — which silently returned the moved-away original. It now falls back to the entry's canonical spelling and never joins an absolute leftover. `src/audit/tests.rs` gained an actual encoded output with distinct geometry and a unix symlinked-root case that reproduces the macOS/Windows split locally. |
| Crash prompt | `src/crash.rs` | The dialog's entrance animation runs on the wall clock, which the test executor does not advance, so `debug_bounds` handed back geometry the next frame had already invalidated. The prompt tests now render animations settled; the explicit Not now close, Escape staying modal and the absence of handoff on dismissal are unchanged. |

Local verification on Linux with `CARGO_TARGET_DIR` inside this worktree, all
exit code 0: focused tests for all three failures, `cargo test --locked`
(625 passed, 2 ignored, plus 32/2/3 integration tests), `cargo test --locked
--features updater` (641 passed, 3 ignored), `cargo clippy --locked --all-targets
--all-features -- -D warnings`, `cargo fmt --check` and `git diff --check`. Logs
are under `ux/resume-ci-*.log`.

Only Linux was executed here. The macOS and Windows behaviour is argued from the
remote logs and from a locally reproduced equivalent, not observed: native OS
proof stays pending on the next remote CI run.

### September 9 D3 preparation parity, first bounded increment (branch `codex/workbench-preparation-parity`)

Base `1bb9f65`. This increment is shared-source preparation, comparison, estimate
and writer parity only. No maintained pack, policy catalog, new recipe transform
or field, GUI target delivery, H2/H3 or HEIF work is included, and none of it is
implied. Full D3 packs and transforms remain pending.

| Flaw at base | Change |
| --- | --- |
| `compare::build` could re-encode a cached BGRA8 `Preview` and read the source ICC profile on a separate path, so a `Keep`, grayscale, deep or tagged source could be quoted at a size the writer never produces | `Preview` is display-only: its `profile`/`decoded` fields, the BGRA read-back and the preview-to-encode argument are gone. Every comparison prepares the source itself and converts to RGBA/BGRA only after the encode. `scan::icc_profile`, which existed for that shortcut, is deleted |
| Four copies of decode → resize → budget → resolve `Same` → depth check → encode, in the writer, the comparison, the window estimate and the CLI dry run | One `convert::prepare`/`convert::encode_prepared` pair. `prepare` returns the existing `scan::DecodedSource` after one `MaxEdge::apply` and the shared budget check; `encode_prepared` resolves `Same` from the container those same bytes were identified as, applies the shared lossless-depth verdict, and encodes at an explicit speed. No raw snapshot is retained beside the decoded pixels |
| A completed `Pair` was cached and built ahead on a size-and-mtime key, which a same-length rewrite inside one filesystem tick does not move | `CachedMedia::Pair` and `take_cached_pair` are removed and compare-mode lookahead is disabled. `Pair` now carries the `SourceIdentity` it consumed, plus the installed output's identity for a written comparison, and `Pair::confirm` re-reads both on the background executor before the result lands. `Key` stays a cheap miss; a stale build takes the existing reopen path, not a fake decode failure. The preview cache and preview lookahead are unchanged |
| `compare::Key` captured an AVIF speed the encoder then re-read from the process dial | Speed is captured once per request, per estimate and per run, and passed to `encode_with_speed`. A recorded write takes it from its own `Stamp` via `Stamp::avif_speed()`, so the bytes and the manifest line describing them cannot come from two reads of a setting that moved in between |
| The estimate held decoded pixels keyed on path and max edge alone | It holds `Arc<scan::DecodedSource>` and checks the held identity against the bytes on disk before reuse; a mismatch prepares the current bytes and replaces the entry. The cache mutex is released before that hash, because the window prunes the same map on the main thread |

Regressions added, each against actual writer output rather than the shared
helper compared with itself: grayscale JPEG, deep tagged PNG and transparency all
`Keep`/convert through `convert::convert_to` and must match the comparison's
quoted size and geometry; an AVIF run must write the bytes of the speed its own
`Stamp` claims while the process dial says otherwise, and its manifest line must
record that speed; a same-length rewrite under the original mtime must leave
`Key::fresh` true while `Pair::confirm` refuses, and the GUI estimate must reject
the held decode and project the length of a real `convert_to` output of the bytes
now on disk, which is proved to differ from the replaced bytes' output. The
existing compare lookahead test was rewritten to state the contract that now
holds — every comparison is built from its own source — rather than deleted.

Cost of the disabled completed-pair cache, stated as work rather than as a
number. Before this change, stepping to a comparison that had been built ahead,
or reopening one at unchanged settings, showed it with no encoding at all. Now
every comparison runs a full decode, encode and decode of the output, once per
open and once per arrow step, and the same file opened twice is encoded twice.
Nothing else regressed: previews are still cached and still decoded ahead, and
conversion runs are untouched. The work is worst where the encoder is slowest,
which is AVIF, and grows with the pixel count. That is the price of not handing
over pixels on the strength of a size and a timestamp, and it is stated rather
than hidden.

Normal-run written bytes are unchanged, so no recipe fingerprint or processing
revision is bumped: the encoder inputs are identical, and for an unconfigured or
unchanged setting `Stamp::avif_speed()` is the same value `avif::speed()` returned
at the same point. Only the ordering of `Same` resolution against the pixel-budget
check moved, which changes which name a doubly-invalid file fails under, not any
output.

Local Linux gates, `CARGO_TARGET_DIR` inside this worktree, `TMPDIR` under
`ux/test-tmp-d3`, all exit code 0: `cargo test --locked` (634 passed, 2 ignored,
plus 32/2/3 integration), `cargo test --locked --features updater` (650 passed,
3 ignored, plus 32/2/3), `cargo clippy --locked --all-targets --all-features --
-D warnings`, `cargo fmt --check`, `git diff --check`. Logs are under
`ux/d3-preparation/`. No flake was seen in these runs.

Only Linux was executed. No native macOS or Windows result is claimed, and the
native UI proof for this increment is root's after review.

#### Round 2 corrections

Formal round 1 blocked `e6fedd0` on two findings; both are addressed here.

The estimate and the CLI projection checked the identity of the pixels they
sampled only *before* encoding. An encode is not instant — AVIF is seconds — so a
source rewritten while its own sample ran could contribute a size measured from
pixels the file no longer held, and a freshly prepared sample had the same gap
because preparation is its only check. Both samplers now answer through one
shared `audit::sample_encode`, which re-reads the source and compares the exact
consumed `SourceIdentity` *after* the encoder returns, on the worker that did the
encoding. A mismatch is `Unknown`, not `Refused`: nothing about the recipe was
rejected, so the slice borrows the average instead of being taken out of the
total as a file the run would write nothing for. A projection standing on nothing
but such samples returns `None`, so the window publishes no estimate rather than a
stale one. Generation and dataset guards are untouched, the cache mutex is still
released before any disk read, and no cache framework or second representation
was added — the check is the `SourceIdentity::matches_path` already used
elsewhere. The comparison view already confirmed after encoding through
`Pair::confirm`, and the writer already re-checks at install, so neither changed.

The regression proves both arms against the actual disk writer: the same prepared
source left alone yields exactly the bytes `convert::convert_to` puts on disk,
and the same prepared source with the file rewritten underneath it — same length,
original mtime, so no stat can see it — yields `Unknown` and projects nothing,
while a genuine lossless-depth refusal still yields `Refused`. Removing the
post-encode check makes it fail. It is a direct call of the shared sampler with
the rewrite landing between preparation and the verdict, which is the whole window
that check exists for; the test executor runs a sample task to completion without
an interleaving point, so a mutation timed inside the encoder itself would have
needed either a sleep or a production seam that exists only for tests, and neither
was added. The earlier same-length/same-mtime-before-scheduling regression and its
writer oracle are unchanged.

The round 1 cost figures were withdrawn. The paragraph above previously quoted
about 0.16 s per AVIF image, obtained by subtracting a `--dry-run` invocation as
process and scan overhead. That subtraction is wrong: a dry run samples and
encodes as part of its own work, so what was subtracted was not overhead, and the
whole-folder conversion timings it was taken from do not measure the comparison
view at all. `ux/d3-preparation/avif-cost.log` is left in place as a record of
that run but is not evidence for any comparison-cost claim; the same figures in
the `e6fedd0` commit message stand as published history and are corrected here and
in this round's commit message rather than rewritten. The tradeoff is now stated
as the work that is actually repeated.

Gates for this round, `CARGO_TARGET_DIR` inside this worktree, `TMPDIR` under
`ux/test-tmp-d3`, run one at a time, logs under `ux/d3-preparation-r2/`, all exit
code 0: `cargo test --locked` (635 passed, 2 ignored, plus 32/2/3 integration),
`cargo test --locked --features updater` (651 passed, 3 ignored, plus 32/2/3),
`cargo clippy --locked --all-targets --all-features -- -D warnings`,
`cargo fmt --check`, `git diff --check`. No flake was seen.

Still Linux only. Round 1's native Linux comparison proof and CLI parity figures
are root's, recorded under `ux/resume-d3-root-proof`; nothing here claims a macOS
or Windows result, parent CI repair is still pending, and this has not landed.
