# Press: prepare, verify and deliver product images

Updated 2026-09-08 against `77ab9191a8986604fa54f2f41a41863c33584302`
(v0.6.6). This is product direction, not an announcement that the planned
integrations exist. The [execution ledger](docs/workbench-execution-plan.md)
separates source-present work, unverified behavior and proposed work.

## The product and its place

```text
ImageGuide / extension: diagnose a delivery problem
Press: prepare local files, inspect outputs, explain checks and produce deliverables
Studio: coordinate shared products, access, requirements, review and delivery
```

**Product-image preflight** is the strategic direction: understand what a batch is
for, prepare the required outputs without losing the masters, and explain what
was actually checked. It is not a promise of marketplace acceptance. Keep the
simple folder optimizer as a useful entry point, not a compulsory product/SKU
wizard. Public messaging must distinguish today's local capabilities from future
submission and verification capabilities.

Press can also be valuable on its own. It need not convert every local user into
a Studio customer or become a separately monetized desktop business. Studio earns
adoption through hosted processing, shared work and managed delivery, including
when those services are used entirely inside Press.

See [strategy and distribution](docs/preflight-strategy.md) for positioning,
audience hypotheses, experiments and the commercial boundary.

## Four entrances, one local engine

| Entrance | First useful outcome | Next experiment |
| --- | --- | --- |
| ImageGuide site and extension | Turn an observed image problem into a local preparation task | File-based audit handoff, confirmed source mapping, explicit deployment and re-audit |
| Desktop discovery and referrals | Prepare a real folder without an account | Repeat the job with saved presets and two independent outputs |
| Agents and command-line workflows | Inspect, plan and perform authorized local work with machine-readable outcomes | Contract tests, then a saved plan/execution/receipt contract |
| Studio retailer invitations | Fulfill an assigned supplier task with less correction work | One scoped submission, interruption recovery and rejection/resubmission loop |

These are distribution hypotheses, not four simultaneous launch commitments.
Choose a small real job for each experiment; increase investment only where it
produces repeat use or measurable retailer value.

## What exists and what comes next

Personal recipes and local product-set job code already exist. Do not rebuild
W1/W2 from the September 5 design documents. Destination proof has also moved off
the conversion click handler. Source presence is not proof that every acceptance
journey works on every operating system. The execution ledger records the remaining
verification and implementation gaps with specific source references.

The default next work is:

1. Reconcile recipe/source/result identity, portable-job safety and CLI contract
   documentation. Preserve the existing output and restore boundary.
2. Test a minimal ImageGuide-to-Press handoff and a two-target local delivery job.
   Reuse the current preset and product-job models; neither needs a template catalog.
3. Complete the supplier vertical slice against confirmed Studio contracts, with
   authoritative requirements and durable receipts.
4. Add maintained requirement packs and contextual Studio services where observed
   work justifies them. Investigate phone-image ingestion in parallel when actual
   user folders contain unsupported HEIC/HEIF files.

This is not a new serial dependency chain. A committed supplier pilot takes
priority over speculative browser acquisition work. Its auth/intake discovery can
run alongside local work; it must not wait for multi-target export, an agent
protocol, a marketplace catalog or AI checkout. Each expansion consumes only the
foundation checks relevant to its own writes or public promises.

## Ownership and commercial boundary

Press owns local sources, work-in-progress grouping, supported preparation,
inspection, local checks and recovery. Studio owns canonical connected products,
assignments, authorization, policy resolution, billing, shared assets, review and
downstream delivery. A local product set is not a second PIM, and a cached policy
is not authority. A preset, report, deep link or plan cannot grant permission.

Keep capability, access, payer and destination separate. Ordinary authorized
supplier submission must not inherit an unrelated personal API subscription gate.
Optional processing in a supplier's own workspace has a separate purchase context;
retailer sponsorship remains a later server-enforced allowance, never a merchant
API key. Existing plans and entitlements must be confirmed with Studio rather
than bypassed in the client.

Useful local operations, presets, checks and exports should remain account-free.
Do not manufacture cloud demand with batch caps, watermarks, weakened local models
or an export paywall. No silent uploads, unapproved spending or repeated offers
after a user declines. Remote outages must not disable ordinary local work.

## Design and execution map

| Document | Owns |
| --- | --- |
| [Execution ledger and briefs](docs/workbench-execution-plan.md) | Current status, dependencies, bounded implementation PRs and proof |
| [Strategy and distribution](docs/preflight-strategy.md) | Positioning, audience experiments, channel order and success criteria |
| [Browser handoff](docs/browser-handoff-plan.md) | Audit import, safe matching, deployment boundary and re-audit |
| [Agent execution](docs/agent-execution-plan.md) | CLI compatibility, deterministic plans, local execution and receipts |
| [Delivery recipes](docs/delivery-recipes.md) | Presets, early multi-target exports, executable requirements and evidence |
| [Phone-image ingestion](docs/phone-image-ingestion-plan.md) | Bounded HEIC/HEIF investigation and platform acceptance |
| [Product workbench](docs/product-workbench.md) | Intended product-set experience and end-to-end journeys |
| [Supplier integration](docs/supplier-portal-integration.md) | Canonical intake, scoped access, receipts and resubmission |
| [Connected Studio services](docs/studio-connected-services.md) | Native authorization, explicit cost/payer and paid-job recovery |
| [Adoption and packaging](docs/adoption-and-packaging.md) | Proposed service boundary, retailer pilot, economics and rollback |
| [Engineering follow-up](docs/engineering-follow-up.md) | Source-backed foundation gaps and validation obligations |

The ledger supersedes old scheduling and blanket "not implemented" statements;
those design documents still own their detailed behavioral contracts. A shipped
subset does not make its whole design complete. Update the ledger with evidence
in the same PR that lands a slice.

## Deliberate limits

Do not build another DAM/PIM, approval engine, billing ledger, generic workflow
canvas, RAW developer or tethering stack. Do not start a codec-count race, an AI
catalog expansion, an MCP server or a template marketplace without an observed
job that needs it. A CLI-only package is an option if installation evidence calls
for it, not a reason to rewrite the application now.

Keep these outcomes distinct: locally prepared, technically checked, visually
reviewed, submitted, approved and delivered. Generation creates a candidate to
inspect, not permission to invent product details or approve an asset. Expand
because users need the next outcome, not because another tool can be added.
