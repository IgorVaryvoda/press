# Studio services inside Press

Status: proposed, 2026-09-05. No new authentication, billing or sponsorship API is
claimed to exist. See the [roadmap](../ROADMAP.md), [workbench](product-workbench.md)
and existing [supplier intake contract](supplier-portal-integration.md).

## The boundary is service value, not the browser

Keep local processing useful. Studio may provide hosted image operations, shared
assets/review and managed supplier workflows through Press. A user need not switch
to the web to become a service customer. Complex administration and checkout may
open the browser with an explicit, context-preserving return path.

The existing `src/studio.rs` image-processing client is a starting integration, not
proof of native supplier sign-in, spend attribution, durable hosted jobs or delegated
retailer allowances. Reuse Studio's canonical services and ledger; do not construct
a second billing system in Rust or equate a generic AI API key with supplier access.

## Four independent questions for every action

| Question | Owner and behavior |
| --- | --- |
| Can this build perform the operation? | Press's supported handlers and format/model capabilities |
| May this actor use it on these assets? | Current server permissions, entitlement and content-access checks |
| Who pays for the hosted work? | One explicit authorized payer and, when applicable, allowance |
| Where is the result stored or submitted? | An explicit destination and a separate authorized action |

A recipe is not permission to upload or spend. An entitlement is not a price. A
credit balance is not consent. A payer does not automatically gain access to the
source, result, or a retailer's catalog. Keeping a result locally does not mean it
has been approved or published.

Persist these distinctions with the operation. Switching visible workspaces must
not change existing jobs' credentials, payer, recipient, prompt, or output owner.
Cross-workspace processing requires explicit source-access/export authorization;
being a member of both workspaces is not permission to silently copy assets.

## Capability and offer UX

Use a small server capability response mapped to known client handlers, with
operation/version, accepted input, permission/entitlement outcome and quote support.
Treat this as a proposed contract, not a new endpoint name. Unknown handlers remain
unavailable; the response cannot deliver executable UI, scripts or arbitrary URLs.
Keep invalid/stale capability data from turning into a request to purchase access.

Display **Local** or **Studio cloud** at the action. Local operation stays selected
unless the user chooses hosted processing; hardware limitations should be explained,
not presented as a fabricated subscription requirement. Cloud-required tasks can
remain visible with an explanation, but dismissing an offer suppresses repetition
for that task/context. No blocking promotion at normal local export completion.

At confirmation show selected file count, exact input/candidate, operation/model,
any prepared-upload changes, target workspace/storage behavior, payer, maximum
credits and possible cancellation charges. Get the quote before uploading image
bytes when possible. If inspection requires upload to quote, disclose that upload
separately; a quotation step must not secretly run paid inference.

A repair offered from a rejection should explain which supported issue it addresses.
Do not promise marketplace acceptance, guaranteed quality improvement, or unchanged
product identity from generation. Return a candidate to the same inspection context.

## Supplier access is not a personal paid account

The supplier pilot should authorize assigned-product reads, requirements, ordinary
submission/status and resubmission through the retailer relationship. It must not
require a personal paid Studio workspace or merchant admin credentials. Retailer
quota/access refusals name the actual problem and offer retry/contact paths; they
must not masquerade as a prompt for the supplier to purchase an unrelated plan.

For optional hosted work, the user can select their own authorized Studio workspace.
A retailer-paid allowance is a later mode. Never fall back from an exhausted retailer
allowance to personal credits, or the reverse, without a new explicit decision.
Disable the paid action while attribution is ambiguous; local work continues.

## Native authorization contract

Use system-browser authorization-code sign-in with PKCE (S256), registered redirect
handling and state/issuer checks; do not embed a password-collecting webview or ship
a client secret. Apply refresh-token rotation or sender constraint for public clients.
These choices follow [RFC 8252](https://www.rfc-editor.org/info/rfc8252/) and
[RFC 9700](https://www.rfc-editor.org/info/rfc9700/), checked 2026-09-05.

Studio must confirm the supported native flow, resource/audience scope and entitlement
behavior before a client implementation is advertised. Use least-privilege, revocable
credentials, OS credential storage and safe handling for refresh races. When a secure
store is unavailable, offer session-only authorization rather than silent plaintext
persistence. Existing API-key users need an explicit migration path that does not
unexpectedly break local work or delete working credentials.

Invitation and browser-return links may identify a request, not convey permission or
trigger upload/spend. Accept only registered application routes and trusted service
origins; authenticate and resolve canonical state after opening. Logout clears tokens
and private cached remote metadata, stops unstarted remote work and preserves the
user's local source files. An already accepted server job must be reconciled with its
original identity after reauthorization, not replayed in a new workspace.

## Quote, execution and charge lifecycle

The first paid slice supports one payer per confirmed job. A mixed-payer selection
must be separated before confirmation. Reuse Studio's existing ledger and operation
records; add missing behavior to those services rather than inventing a desktop ledger.

| Step | Required behavior |
| --- | --- |
| Resolve | Check actor, source access, supported operation and payer; do not infer any of them from the currently visible tab |
| Quote | Server returns a bounded, expiring quote tied to input identity, parameters/model revision, payer and cancellation policy |
| Confirm | User approves the shown maximum; persist a stable client attempt ID before transmitting execution |
| Accept | Server atomically authorizes/reserves permitted spend and records idempotent acceptance before paid dispatch |
| Process | Client records the server job ID; status and billing remain server-owned |
| Settle | Server settles at most the approved maximum, releases unused reservation and records a reconcilable receipt |
| Inspect | Fetch the same result, keep/discard locally or explicitly submit/share; this is not another paid execution |

Input, prompt, model, payer or price changes require re-quoting. Duplicate clicks
and network retries reuse the same accepted attempt identity. A timeout after server
acceptance triggers lookup/reconciliation, not a new job. Exactly-once client requests
cannot be assumed: use durable idempotency, provider recovery where supported, and
an explicit unresolved state when downstream execution cannot yet be determined.
Do not blindly dispatch again after an ambiguous provider timeout.

A lost result download retries retrieval, not inference or billing. A successful
hosted result discarded by the user is not automatically free. A technical failure,
partial batch or cancellation follows the policy disclosed before execution; show
actual settled, reserved and refunded amounts distinctly. Do not claim immediate
refunds or free cancellation while the server/provider outcome is unresolved.

Proposed terminal display outcomes include complete, partial, failed and cancelled,
plus nonterminal **Outcome being reconciled**. A displayed Stop request means
cancellation was requested until the server confirms it. Unstarted files remain
unstarted; completed siblings and receipts remain available. Server recovery must
settle/release orphaned reservations even if the desktop never reconnects.

Existing direct AI operations need not be removed to write this design. Do not add
bulk auto-retry, sponsorship or claims of recoverable quoted billing to those paths
until the server contract exists. Any legacy path keeps accurate current disclosures.

## Retailer sponsorship: a later delegated allowance

A merchant administrator may explicitly authorize an allowance for selected suppliers,
operations and assigned products/slots, with expiry and per-job/period budget caps.
The supplier gets an opaque grant reference and a scoped service decision, never
a merchant API key or unrestricted credit wallet. Server authorization and reservation
must check the grant, assignment and remaining budget together under concurrency.

Bind use to the authorized input and task. Do not allow an unrelated lifestyle job
to spend an allowance intended for correcting a retailer rejection. Revocation stops
new acceptance; accepted work follows its original reservation/cancellation policy.
Two simultaneous requests cannot both spend the final allowance. Repeated retries
cannot create multiple charges. Exhaustion leaves local/export/submission paths
available where their independent permissions still allow them.

The retailer's usage view should expose authorized operational receipts, not private
supplier-workspace assets or prompts. The supplier sees **Paid by this retailer**
and the applicable cap. Sponsorship is not needed for the first supplier pilot or
for independently paid Studio actions.

## Data custody and sharing

Upload only explicitly selected inputs. Disclose hosted processing, relevant service
retention/deletion terms and processing providers before use; do not invent a claim
that bytes are never stored or immediately deleted. Retain existing provenance
metadata where supported and disclose intentional changes. Generated content does
not silently become an original or an approved retailer submission.

Shared-library save and Share for review are separate consented actions. Confirm
workspace and audience; default to restricted access. No public review link by
accident. The client may display canonical comments and statuses, but Studio remains
the permission and review authority. Remote outage must not prevent local export or
opening existing local deliverables. Keep secrets, filenames, prompts and signed
asset URLs out of product analytics and ordinary diagnostics.

## Acceptance obligations

| Scenario | Evidence required before enabling the corresponding feature |
| --- | --- |
| Fresh installation, no account/network | Supported local preparation/export works; no sign-in wall |
| Supplier invited by retailer | Can perform authorized normal submission without personal paid workspace |
| Declined cloud action | No image upload or billable call; the local task remains usable |
| Quote expires or selection changes | Explicit new quote, not a silent larger charge |
| Account or recipient switched mid-job | Original actor/payer/target pinned; no cross-tenant write |
| Concurrent final allowance requests | Total accepted reservation cannot exceed the server budget |
| Double click, timeout or restart | One accepted logical attempt and reconcilable charge; not duplicate jobs |
| Provider outcome unknown | Reconcile or escalate without blind redispatch or a false completed/failed claim |
| Cancel/partial failure/discard | Actual job and charge states shown independently; no invented refund |
| Output download fails | Retrieve the existing result without running the operation again |
| Rejected generated result | Source and candidate history survive; no automatic submission or approval |
| Removed access or logout | No new remote work; reauthorization and canonical recovery for accepted work |

Use fake/loopback services, deterministic receipts and failure injection for these
cases. Live hosted tests need separately approved accounts, inputs and a hard spend
budget. This plan does not authorize production charges or account changes.
