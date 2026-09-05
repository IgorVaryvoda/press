# Press: a product-image workbench, connected to Studio

Direction updated 2026-09-05. These are design and implementation plans, not
announcements of shipped features or approved pricing. Code baseline:
`ec9794bad7a6115a2fe1cd8313ca623ae6420699` (0.4.4).

**Supplier Portal is the first strong use case, not the ceiling.** Press should
feel complete for one person preparing product images. Studio becomes valuable
when that person needs hosted processing, shared assets, collaboration, or a
managed supplier-to-retailer workflow. Using Studio entirely inside Press still
counts as using Studio; a browser visit is not the conversion objective.

## Product promise

```text
Images -> product sets -> requirements -> prepare -> inspect -> local deliverables
                                                           -> supplier submission
                                                           -> optional Studio service
```

Keep quick folder conversion intact. Product grouping is an optional layer over
that job, not onboarding that every screenshot must pass through. Give users a
useful local tool without account requirements, artificial batch caps, watermarks,
weakened models, or an export paywall. Monetize additional service value, not
avoidable frustration.

## Three connected opportunities

| Opportunity | Press experience | Studio value |
| --- | --- | --- |
| Supplier preparation | Assigned products and slots, local checks, corrections, submission and status | Authoritative requirements, access, review, approval and delivery |
| Independent product-image work | Product sets, consistent presentation, custom presets and marketplace export packages | Optional hosted repair/generation, then the user's own shared workspace |
| Repeated team workflows | Reusable deliverables, selected editor handoff, later monitored export folders | Shared masters, review, provenance and managed automation |

A supplier does not have to buy a separate Studio subscription merely to fulfill
an authorized retailer request. Optional processing in the supplier's own workspace
is a separate purchase context. Retailer-sponsored processing is a later, explicitly
budgeted service, not permission to use a merchant-wide API key.

## Ownership and commercial boundary

Press owns local sources, work-in-progress product sets, recipes, deterministic
checks it can actually perform, output inspection and transfer recovery. Studio
owns canonical connected products, assignments, policies, authorization, billing,
shared assets, review and delivery. Local grouping may show products without
becoming a second PIM; cached server state is never new authority.

Keep capabilities, access, payer and destination distinct. A hosted repair can run
inside Press without silently saving the user's whole folder to a cloud library.
A submitted file is not an approved file, and approval is not delivery.

## Implementation order

| Stage | Smallest useful result | Release condition |
| --- | --- | --- |
| Foundation | Preserve landed trust fixes; complete validated recipe/result identity and asynchronous planning | Local behavior remains safe and consistent across preview, estimate and export |
| Personal recipes and product sets | Save/reuse settings; map images to local product roles and show missing items | A job survives restart without requiring a cloud account |
| Supplier-first pilot | One retailer, scoped sign-in, exact requirements, preparation, submit and correction loop | Correct tenant/product/slot, recoverable partial batches, browser fallback |
| Contextual Studio services | A relevant repair or generation action with explicit upload and payer/cost confirmation | Recoverable job and charge, no duplicate spend or hidden account fallback |
| Broader deliverables | A small maintained marketplace set and independent outputs from one master | Real repeat demand; per-target verification without overwriting other outputs |
| Team and repeat-work expansion | Own workspace, shared review; later editor/watch-folder conveniences and sponsorship | Measured value and supportable operating costs, not feature-count growth |

Supplier access-contract work can run alongside local recipe work. Do not delay a
committed supplier pilot to build an extensive marketplace catalog, sponsorship,
or a general creative suite. Hosted-work planning may proceed in parallel, but
supplier submission must not depend on purchasing hosted AI. The executable slices,
dependencies and proof obligations are in the [execution plan](docs/workbench-execution-plan.md).

## Design map

| Document | Owns |
| --- | --- |
| [Product workbench](docs/product-workbench.md) | Users, product-set model, information architecture and end-to-end examples |
| [Delivery recipes](docs/delivery-recipes.md) | Custom presets, maintained marketplace requirements and output verification |
| [Supplier integration](docs/supplier-portal-integration.md) | Canonical intake, assignment scope, submission receipts and resubmission |
| [Connected Studio services](docs/studio-connected-services.md) | Contextual actions, native authorization, payer separation and paid-job recovery |
| [Adoption and packaging](docs/adoption-and-packaging.md) | Retailer-led and supplier-led adoption, proposed commercial boundaries and pilot measures |
| [Execution plan](docs/workbench-execution-plan.md) | Ordered implementation slices, accountable role, dependency and acceptance evidence |
| [Engineering follow-up](docs/engineering-follow-up.md) | Current processing baseline and remaining trust work |

This update broadens the earlier delivery-only framing. The existing recipe and
supplier contracts remain in force. This roadmap owns cross-document order;
commercial proposals live in packaging; detailed service authority lives in the
connected-services plan. No document promises a price, an available API, or an
implementation deadline that has not been verified.

## Already landed versus still proposed

[PR #3](https://github.com/IgorVaryvoda/press/pull/3) is merged. Subsequent source
changes share lossless-depth checks and retain ICC profiles in prepared Studio
uploads. The engineering follow-up records those changes so they are not rebuilt.
This documentation update does not constitute native runtime validation.

Product-set jobs, a custom-preset editor, maintained marketplace templates, supplier
sign-in, quoted/recoverable paid jobs and sponsorship are planned here; their
existence must not be inferred from existing generic Sirv or Studio AI actions.

## Deliberate limits

Do not build a second DAM/PIM, approval engine, billing ledger, generic workflow
canvas, full RAW developer, or camera-tethering stack in Press. Exposing authorized
Studio state in a desktop view is allowed; inventing a rival source of truth is not.
Local and hosted model choices should reflect capability and evidence, not a
manufactured quality gap. Advanced cloud administration can remain on the web.

No silent uploads, auto-spending watched folders, account-required local presets,
or nagging after a user declines Studio. Validate product accuracy after generation;
missing technical dimensions do not justify inventing product details or approving
an image. Expand when users need the next outcome, not when another tool is available.
