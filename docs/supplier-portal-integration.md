# Sirv Studio supplier-portal integration

Status: proposed; requires coordinated server work and a pilot. This document
specifies desired behavior, not existing desktop-compatible endpoints. See the
[roadmap](../ROADMAP.md) and [shared recipe contract](delivery-recipes.md).

## The useful workflow

A supplier chooses the connected retailer, sees assigned products and required
image slots, maps local files to those targets, prepares the requested deliverables,
reviews them, and submits. Press then shows the server's per-file validation and
review outcomes, including actionable correction reasons and a way to resubmit.

This is not the existing generic Sirv folder-pairing workflow, nor an extra Studio
AI tool. Uploading directly to a retailer's Sirv folder would bypass the supplier
intake and approval contract. The supplier integration must submit through the
portal's canonical intake path.

## Ownership boundary

**Studio owns** identity, organization and supplier scope, assignments, canonical
product/slot identity, resolved requirements, quotas, authoritative validation,
review/approval, and downstream delivery. It rechecks those facts at submission
finalization and at the operations that use them.

**Press owns** local source inspection, cached requirement display, deterministic
preflight that it can actually perform, recipe execution, mapping confirmation,
transfer progress, local retry state, and display of server receipts. A client-side
check cannot grant access, waive a requirement, approve an asset, or prove delivery.

Reuse Studio's canonical services rather than building a desktop-only alternate
review flow. Do not duplicate Studio's layered policy resolver in Rust. Expose a
resolved, versioned requirement snapshot for the exact supplier/product/slot;
Press interprets only its explicitly supported subset.

## Minimum integration

1. **Connect and load requirements.** Browser-assisted supplier sign-in; explicit
   retailer/workspace choice; assigned products, available slots, reference examples
   where permitted, and resolved requirements with revision and retrieval time.
2. **Map, prepare, and submit.** Filename/SKU matching with optional CSV mapping and
   manual correction; an explicit product/slot mapping preview; local checks;
   selected output review; per-file uploads with durable submission identifiers.
3. **Complete the feedback loop.** Server-reported validation/review state, correction
   reasons, and resubmission linked to the earlier submission. Resume or reconcile an
   interrupted job without creating duplicate assets or approvals.

These are one pilot-sized vertical slice, not permission to recreate the whole
supplier portal. Status can use bounded polling initially; realtime infrastructure
is not a prerequisite. The web portal remains available for occasional suppliers.

## Authentication and server API dependencies

Confirm the existing identity and programmatic-access contracts before implementation.
The presence of web routes or bearer authentication is not proof that a secure
native-client flow and its entitlements are ready.

Use narrowly scoped, revocable supplier authorization. A supplier must not need a
retailer admin key, merchant-wide Studio AI key, or client secret distributed in
Press. Store tokens in the platform credential store; never in exported presets,
logs, URLs, reports, or source-folder manifests. A custom URI handoff must not accept
arbitrary destinations or bearer tokens embedded in an untrusted link.

Before releasing the desktop client, Studio must provide or explicitly confirm:

- Supplier-scoped assignment and resolved-requirement reads, plus canonical mapping
  resolution. Display SKU/EAN as hints; do not invent product identity locally.
- Authorized upload initialization, bounded data transfer, idempotent finalization,
  cancellation/recovery, and per-file status/receipt reads.
- Current entitlement, quota, assignment, organization, media, and slot validation at
  finalization and retry. A stale desktop cache never substitutes for access checks.

Pin workspace and supplier identity to the job; changing accounts cannot redirect
an existing queue. Permission revocation blocks pending submission, even if local
preparation was completed offline. Validate the selected destination explicitly
before bytes leave the machine.

## Requirements and file identity

Cache resolved requirements with their revision and retrieval time so a supplier
can prepare offline. Label that snapshot as cached. Refresh before submission;
a changed mandatory rule yields a visible revalidation decision. Press must not
resolve an incompatible rule by silently dropping it from the checklist.

A mapping stores the server's canonical product and slot identity. Where an API
uses role and position rather than an opaque slot ID, retain those exact values
and the relevant template revision; do not guess that a filename is sufficient.
Ambiguous or unauthorized matches require correction, not best-effort uploading.

Record source identity, prepared-output identity, recipe fingerprint, policy
revision, and the explicit product/slot mapping with the local job. Store a hash of
the submitted bytes and the server receipt after acceptance. Do not treat two
identical images assigned to different slots as the same submission.

Requirement-driven preparation chooses which bytes to submit. A retailer requesting
a master must not receive a compressed marketplace derivative just because that was
the last selected preset. Preserve originals, label any lossy/depth/color changes,
and let the supplier inspect the exact file that will be uploaded.

## Retries, cancellations, and truthful state

Persist a client job ID and stable per-file attempt IDs before uploading. Reuse an
attempt ID for retries of the same bytes and mapping; changing content, target, or
recipe creates a distinct attempt linked to the existing submission where appropriate.
Server idempotency must include scope and distinguish resubmission from transport retry.

If a connection drops after server acceptance, query the attempt/receipt before
re-sending or declaring failure. Restart recovery restores named pending items, not
just an aggregate progress bar. Retry failed or unconfirmed items, not the entire
successful batch. Cancellation must retain the distinction between an accepted
submission, an upload still in flight, and work never submitted.

Display the server's outcome, not a guessed local sequence. Keep these meanings
separate: **Prepared locally**, **Transferred**, **Submission accepted**,
**Awaiting review**, **Approved/Rejected**, and **Delivered/Delivery failed** when the
server exposes them. An upload finishing is not an approval. A failed downstream
delivery does not imply the image was rejected in review.

Local preparation may work offline. Submission requires an authenticated, current
server decision. Automatic background uploads and watched-folder submission are
out of scope for the first integration.

## Pilot acceptance matrix

| Scenario | Required evidence |
| --- | --- |
| Happy path | Explicit mapping reaches the intended product/slot and becomes reviewable; exact output and receipt can be inspected. |
| Different retailer/account | Existing queue cannot change organizations or upload with another account's credentials. |
| Assignment revoked before finalization | Server refuses; Press preserves a named failure and local output. |
| Policy changed after local preparation | Server revision is visible and affected checks are repeated; no stale pass badge. |
| Unsupported visual check | Press says not checked/needs review; server policy remains authoritative. |
| Response lost after acceptance | Reconciliation finds the accepted attempt; retry creates no duplicate submission. |
| Partial batch and restart | Successful receipts survive; only failed or unconfirmed items need action. |
| Cancellation races with acceptance | UI reports actual retained submissions and unstarted items separately. |
| Edited file after preparation | Submitted bytes are reverified or the item is re-prepared; receipt does not describe stale content. |
| Rejection and resubmission | Corrected output links to the intended earlier submission and follows normal review policy. |
| Approval versus delivery failure | Both states remain inspectable; client never equates transfer success with published content. |

Release only after a real retailer pilot completes this loop with supplier-scoped
access and recovery evidence. Keep private tenant details, credentials, and server
operational findings out of this public repository.
