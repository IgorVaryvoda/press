# Recipes, multiple deliverables and executable requirements

Updated 2026-09-08 against `77ab9191a8986604fa54f2f41a41863c33584302`.
The [execution ledger](workbench-execution-plan.md) owns status and order.
This updates the September 4 proposal without replacing its safety/authority
boundary. New transforms, multi-target execution and policy packs remain proposed.

## Preserve the landed recipe work

`src/recipe.rs` already defines schema-versioned personal recipes, strict parsing,
identity/revision, built-ins, persistence and a settings fingerprint. The UI in
`src/audit/recipe_actions.rs` supports saving/updating, renaming, importing and
exporting. `src/main.rs` exposes `--preset-file`. Do not implement another preset
library. Verify round-trip and GUI/CLI behavior against this source.

Recipe fingerprints exist but do not yet constitute a complete source/engine/
policy/output identity. Extend the existing model with explicit compatibility and
migration tests. Display labels are not canonical numeric settings. Record resolved
defaults, output-affecting codec options and processing revision, while preserving
old records for recovery. Imported settings cannot acquire authority.

## Keep four concepts separate

| Concept | Owns | Must not imply |
| --- | --- | --- |
| Requirements | Allowed media, dimensions, roles, byte limits, policy provenance/revision, mandatory/advisory checks | A preference or a local permission to waive the recipient's rules |
| Recipe | Supported transforms and requested/effective settings | Approval, access or arbitrary executable steps |
| Destination binding | A locally selected output root or independently authorized remote recipient | Portable credentials or permission inherited from an import |
| Receipt | Actual source/output identity, settings, checks and outcomes | A tamper-proof certificate, marketplace acceptance or server approval |

Use typed data and known handlers, not an executable DSL or workflow graph.
Unknown required operations/schema semantics fail closed with a reason; unsupported
checks remain Not checked, not Passed. Allow advisory metadata only under an
explicit schema compatibility rule.

## D1: one master, two useful local targets

Bring this ahead of an extensive marketplace catalog. First example: a byte-for-byte
master copy in one target folder and a 1600px website derivative in another. A
verified passthrough copy is a proposed operation, not something to fake using
`Keep`/`same`, which re-encodes. Alternatively test two already supported encoding
targets first. Do not label either example marketplace-compliant without a policy.

Extend `Job.target_recipe` through a versioned migration to independent targets;
existing single-target jobs must retain their meaning. Keep product grouping
optional. Pin each target's recipe snapshot, requirements when present and output
namespace. Make source -> target -> actual output visible in one execution summary.
The first UI may support two targets while the model allows bounded repetition.

Generate each target from the selected master revision, not another target's
compressed output. Plan names across the complete selection and target roots.
Refuse overlapping/unsafe destinations and protect originals, including unselected
files. Use separate folders by default and the existing output-safety boundary.
Do not use replace mode for this initial multi-target flow.

Keep success/failure per item and target. An impossible target must not erase a
successful sibling. Retry failed and regenerate outdated are separate actions.
Changing a recipe or source invalidates only affected deliverables; changing a
display name does not pretend the bytes changed. Never relabel previous results
using whatever controls happen to be selected now.

Exit: reopen the job, inspect both real files, change one input, regenerate only
affected outputs, fail/cancel one target and recover without touching the other.
Measure repeated-job preparation effort before adding a larger target catalog.

## D2: requirements and receipts for one real destination

Start with one retailer's resolved technical requirements or a user-authored local
specification. Keep their provenance distinct. Studio resolves its policy layers;
Press interprets a supported snapshot rather than reimplementing that resolver.
Mandatory server requirements remain authoritative at submission.

A pack should identify stable policy/revision, target and role, category/region
scope where relevant, source URL or authorized issuer, last verification,
effective interval, supported engine/check versions and mandatory versus advisory
rules. Define reference fixtures for the supported subset. Requirements are
portable data, but private retailer metadata is exported only with authorization.

Check the actual encoded output: content-derived format, dimensions, alpha, bytes,
name and supported profile/depth constraints. Record the source/output hashes,
recipe and processing revision, policy revision, each check's evidence, observed
value and result. Keep pass, fail, not checked, not applicable and needs review
separate. A required unknown prevents a claim that all technical requirements pass.

Images plus a JSON receipt and a human-readable report are the useful deliverable.
A raw local filesystem path or signed asset URL should not leak into a shareable
report. Local evidence can be edited; the receiving service must revalidate bytes.
A product may have all required roles present yet still fail technical checks or
human review. Do not compress those states into an invented readiness percentage.

## D3: maintained packs and supported preparation

Choose two or three maintained marketplace/channel targets from actual demand.
Assign a maintainer and define what happens when verification is stale. Each
published revision must link to official policy, date its verification and state
coverage, exceptions and effective dates. A forked personal recipe is not the
unchanged maintained policy. Do not silently update an active run or saved job;
show changed requirements and revalidate before a new submission.

For example, Google's official [image-link guidance](https://support.google.com/merchants/answer/6324350?hl=en-GB)
announces a minimum of 500 x 500 pixels from **2027-01-31**, verified 2026-09-08.
That is a concrete need for effective-dated checks, not permission to reject every
smaller image before then or declare every larger image acceptable. Role, surface
and category exceptions require a complete source review before publishing a pack.

Candidate policy sources also include [Amazon's product-image guidance](https://sell.amazon.com/blog/product-photos)
and [eBay's picture policy](https://www.ebay.com/help/policies/listing-policies/picture-policy?id=4370).
These are implementation research leads, not newly verified executable specifications.
Bundle initial pack updates with releases; a remote feed later requires integrity
verification, strict parsing and an explicit update decision, not downloaded code.

Add exact-canvas fit/pad, explicit compositing, naming and bounded byte-budget
encoding as separate tested increments. No control is supported until preparation,
preview and actual output verification agree. Use a quality floor and bounded
search for a byte budget; impossible constraints remain named failures. Never
silently resize below an agreed minimum or upscale to manufacture a technical pass.

Preserving ICC metadata is different from converting pixels to sRGB. White padding
is not background removal or proof of a white background. Subject occupancy,
accurate depiction, logos, text and usage rights need appropriate evidence/human
review. Retain or explicitly account for relevant provenance metadata; do not
present generated content as an untouched original. A hosted repair produces an
inspectable candidate, not guaranteed compliance.

## Acceptance and rollout

| Case | Required outcome |
| --- | --- |
| Existing recipe/job/manifest | Migration preserves its meaning and restore data; deleting a preset never deletes output |
| Malformed/future/oversized import | Bounded refusal before writes; no executable steps, credentials or imported absolute output root |
| Duplicate stems, Unicode/case differences, overlapping targets | Collision-safe mappings or named refusal; no lost original |
| Source, numeric option, engine or policy revision changes | No false reuse of an old verified result |
| Byte budget impossible or mandatory check unsupported | Explicit failure/Not checked, not an approval badge |
| Current versus future policy | Apply the appropriate revision and expose stale/changed policy |
| Wide-gamut/deep/transparent input | Consistent preparation, preview and output evidence; explicit unsupported cases |
| Partial target failure, restart or edited output | Successful siblings retained; ownership rechecked before reuse/retry |

D1 needs source/recipe/output identity and safe persistence, not D2/D3 or Studio.
D2 can also supply a pilot-required policy before D1. The supplier pilot must not
wait for a catalog. Direct marketplace publishing, a preset marketplace and remote
policy execution remain out of scope. Record native and CLI proof in each code PR.
