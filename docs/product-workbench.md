# Product-image workbench design

Status: proposed, 2026-09-05. Entry point: [roadmap](../ROADMAP.md).
The [execution plan](workbench-execution-plan.md) owns sequencing. This describes
the intended experience, not features already present in Press.

## Who comes first

The first user is a supplier preparing images for a retailer using Studio Supplier
Portal. Their task is to supply the right product views, understand exceptions,
submit, and correct rejected work. The retailer is often the paying customer.

The expansion user is the same supplier, brand, photographer or agency preparing
images for additional buyers, a website, or marketplaces. They should be able to
reuse local skills and recipes without connecting every destination to Studio.
A later team user needs shared masters, review and coordinated delivery; that is
an opportunity to connect Studio, not to build another desktop catalog database.

## Keep the existing small job small

Opening a file or folder still leads to inspection and conversion. No compulsory
project creation, SKU form, sign-in, tour or cloud-library import. **Organize as a
product set** is an additional action. A supplier invitation can instead open a
scoped assignment view after authentication, with a normal browser path retained.

Use one work area with progressive detail, not three separate applications:

| Surface | Main object and action | Important boundary |
| --- | --- | --- |
| Files | Source images; inspect, select, prepare | Existing folder workflow stays usable offline |
| Product sets | A product's required views and mapped files; resolve gaps | Local role coverage is not server readiness or approval |
| Deliverables | Actual outputs per target; inspect, export or submit | Target and recipe are pinned to the output, not the current controls |

These are information groups, not a mandate for three new full-screen tabs. Extend
the current list/gallery, preview and rail incrementally. At a narrow window keep
the active object, selected scope and commit action visible; use a details drawer
rather than multiple permanent sidebars. Remote status and jobs use a single
recoverable activity view, not one progress overlay per integration.

## A deliberately small local job model

Use these concepts when implementing, without treating this table as a final wire
schema or requiring a new database framework:

| Concept | Minimum identity and state |
| --- | --- |
| Work job | Stable local ID, source roots, ordered items, revision and local settings |
| Product set | Stable local ID, human SKU/name hint, required local roles; optional server binding |
| Source revision | File reference, observed metadata and content identity at processing/submission boundaries |
| Deliverable | Source revision, product role, target, recipe/policy revisions, actual output identity and checks |
| Remote attempt | Explicit account/recipient context, durable operation ID and server receipt/status |

Scope a connected binding by workspace, supplier and canonical product/slot
identity. A SKU is a display/matching hint, never a global identifier. Duplicate
SKUs across buyers must remain distinct. Multiple images with equal hashes may
still serve different roles or recipients. Bind mappings explicitly; do not create
canonical products on the server merely because a filename resembles a SKU.

Store restart state locally with a versioned format and atomic persistence; keep
credentials in the OS credential store. Reuse the output manifest for its existing
recovery purpose rather than turning it into a credential store or global catalog.
The exact persistence implementation is a small implementation decision, not a reason
to introduce a distributed sync engine.

An explicit portable job export contains relative references and user-approved
metadata, not machine-specific absolute paths, tokens, signed URLs or customer
contact details. Imported jobs never auto-upload, run executables or acquire authority.
Opening another person's job requires rebinding destinations and revalidating access.

A missing source is a relink problem. A changed source invalidates affected checks
and deliverables visibly. Old outputs remain identifiable historical results, not
files to silently erase. Reopening a job must not unexpectedly replay hosted work.

## Product sets and consistency

Offer filename/folder hints, CSV mapping and manual correction. Show ambiguous and
unmapped files before committing. Permit main/detail/lifestyle roles in a personal
job, but let a connected retailer's resolved slot contract define required roles.
A local rename or reordered thumbnail must not silently remap a submitted item.

Show missing required views, duplicate assignments and technical mismatches at
product level. Count **roles supplied** separately from **technical checks passed**,
**needs visual review**, and the server's review state. No synthetic universal
readiness percentage. Unknown policy or unsupported checks remain unknown.

Consistency work includes exact canvas, fit/pad, explicit background compositing,
consistent naming, and limits shared across a batch as the engine gains support.
Do not describe white padding as background removal, increased dimensions as new
detail, or guessed subject bounds as confirmed framing. Content-changing repairs
produce candidates that need inspection, not automatic acceptance.

## One master, several deliverables

A source/master revision can produce a retailer-requested master copy, a website
image and marketplace derivatives. Each target has its own recipe, requirements,
output namespace and report. Never generate a new target by recompressing another
target's derivative unless the user explicitly selected that derivative as source.

Default to separate output folders and stable collision-safe names. Failures are
per target/item: one impossible byte budget must not erase successful outputs.
**Regenerate outdated outputs** and **Retry failed items** are different actions.
Local preparation is not publication; a marketplace template exports a package,
not an undeclared connection to a marketplace account.

See [delivery recipes](delivery-recipes.md) for versioned rules, technical versus
visual checks and import safety. Maintained marketplace templates and personal
recipes use the same engine, while mandatory retailer rules remain authoritative.

## Contextual Studio, not a mandatory tools catalog

| User situation | Useful optional action | The user should understand |
| --- | --- | --- |
| Local cutout is unsatisfactory | Compare a hosted background-removal candidate | Which file leaves the machine, the payer and quoted cost |
| A lifestyle role needs an image | Generate a scene using the selected product | Generation may alter product details; inspect before assigning |
| A supplier rejection names an issue | Apply a supported repair or open the explanation | Repair, resubmission, approval and delivery are separate |
| A colleague needs to review deliverables | Share selected results in Studio | Who will receive access and which workspace stores the assets |
| Repeated jobs need common masters | Connect/create the user's own workspace | This is not automatic membership in a retailer's organization |

Keep the local option visible where it can do the job. An unmet requirement may
need a new photograph rather than an AI upsell. Generated outputs need scrutiny
for geometry, labels, color, materials and other product-identifying details.
Never promise improved quality solely because processing is hosted.

The [connected-services contract](studio-connected-services.md) owns authentication,
cost, disclosure and recovery. A user may complete a paid Studio operation without
leaving Press. **Keep** accepts a candidate locally; it does not approve, publish,
or automatically refund/charge a completed hosted job.

## Three acceptance journeys

**Supplier, no personal Studio purchase.** Open the retailer assignment, map a
folder, see an absent detail view, correct it, prepare and inspect, submit. Interrupt
the transfer and restart. Successful receipts survive; the remainder is reconciled.
View a rejection and resubmit the corrected revision to the same product/slot.
The free local job is still useful when the network or retailer access is unavailable.

**Independent supplier.** Open the same source folder without a retailer connection,
create local product sets and save a recipe. Produce website and marketplace
packages. Change one source and regenerate only affected deliverables. No account,
cloud upload, watermark or commercial nag is required to complete this journey.

**Connected enhancement.** Select one image, choose a hosted action, see destination
and payer separately, confirm an authoritative maximum cost, inspect the result and
keep it. Closing the app after submission does not lose the job or charge it twice.
Connecting a shared workspace later is explicit; nothing migrates from a retailer
workspace merely because the same person belongs to both.

## Later conveniences and exclusions

Editor handoff should launch a user-selected application safely and recheck saved
revisions. Prefer a derived working copy; clearly disclose an explicit in-place edit.
Imported recipes must never supply shell commands. Watched export folders should
first detect stable, complete files and offer local preparation, with pause and
per-folder scope; no silent uploads, approval, deletion mirroring or paid jobs.

Defer tethering, RAW development, broad video/3D creation, global deduplication,
a recipe marketplace and a general workflow canvas until a named customer need
justifies a separate decision. These are expansion candidates, not prerequisites
for a useful supplier tool.
