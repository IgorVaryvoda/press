# Custom presets and marketplace export templates

Status: proposed. Entry point: [roadmap](../ROADMAP.md).

## First useful increment: save what already works

Introduce one versioned, strictly parsed recipe representation for the existing
format, lossy/lossless quality, maximum edge, and applicable encoder options.
Normalize the four built-in presets through it instead of maintaining another
settings-to-engine mapping beside `src/audit/panel.rs`.

The first UI is a preset selector with **Save current settings**, **Duplicate**,
**Rename**, **Delete**, **Import**, and **Export**. Changing a setting marks the
selection as modified; it must not silently edit the stored preset. Deleting a
preset does not delete generated files. Duplicate display names must not become
ambiguous identifiers. Store presets atomically outside the source folder.

Proposed CLI shape (not implemented): `press convert PATH --preset-file FILE`.
Explicit command-line settings override a personal recipe and are shown in the
resolved plan. Contradictory or unsupported options fail before writing. Do not
make headless commands depend on whichever preset the GUI last selected.

## Separate three concepts

| Concept | Owns | Does not own |
| --- | --- | --- |
| Requirements | Allowed media, dimensions, aspect ratio, byte limits, slot role, required checks, policy source/revision | Codec preferences, local paths, credentials, approval |
| Recipe | Supported transforms and their parameters; requested and effective settings | Authority to waive a required check |
| Destination binding | Local output root or an explicitly authorized supplier target | Portable credentials or arbitrary code |

Use plain typed data, not a graph editor or an executable transformation language.
The binding to a local folder or authenticated account is stored separately from
portable templates. Requirements, recipes, and template provenance can be parts
of one document without being conflated as one list of overridable settings.

Stable recipe IDs, schema version, recipe revision, name, and provenance are needed
from the first increment. Record a normalized recipe fingerprint with each output.
Include output-affecting options such as AVIF speed and the relevant processing
implementation revision; a display label such as `q80` is not a full recipe identity.
Avoid introducing arbitrary script steps, shell commands, or model downloads in
imported recipes.

## A template is more than a format button

A marketplace template should identify channel, region, category scope where
relevant, and image role (for example main image versus gallery). Distinguish
mandatory requirements from the application's recommended export settings.

Each published revision needs an official source URL, last-verification date,
effective-from date when applicable, supported check coverage, and template/engine
compatibility. Unknown required operations or unsupported schema versions must
fail with a reason, not be silently ignored. Unknown advisory metadata may be
preserved without being treated as a passed check.

Choose the first two or three templates from pilot demand. Candidate primary
sources checked on 2026-09-04:

- [Amazon product image guide](https://sellercentral.amazon.com/help/hub/reference/external/G1881)
  and its [public product-photo guidance](https://sell.amazon.com/blog/product-photos).
- [eBay picture policy](https://www.ebay.com/help/policies/listing-policies/picture-policy?id=4370).
- [Google Merchant Center image link specification](https://support.google.com/merchants/answer/6324350?hl=en-GB).

These are source leads, not a claim that a full executable specification has been
verified. Resolve conflicting official guidance, category exceptions, image roles,
and account-specific rules before publishing a template. Do not silently pick
whichever page offers the easiest threshold.

Effective dates are a real requirement: Google's current image-link guidance
announces a new minimum-size policy starting **2027-01-31**. A template must not
apply a future rule early as a current rejection, nor remain permanently pinned
to an old rule. Preview a template revision's changes before updating a saved job;
never alter an active run because a template update arrived.

Built-in updates are bundled with a release initially. A later remote update feed
requires integrity verification and the same strict data parser, not downloaded
executable logic. Users may fork built-in recipes without losing their provenance;
a fork is no longer advertised as the unchanged maintained template.

## Add capabilities only with truthful output verification

After basic preset persistence, add exact-canvas fit/pad, explicit background
compositing, naming patterns, and byte-budget encoding as separate tested increments.
Do not show a control as supported until the engine and preview implement it.

For a byte budget, use a bounded quality search with a quality floor. Resizing below
an agreed limit requires an explicit policy, not a hidden fallback. Failure to fit
is a named unsatisfied constraint. Never upscale merely to make a minimum-dimension
check green; distinguish source resolution from delivered dimensions and require an
explicit user decision for AI upscaling or other content-changing repairs.

Profile handling must distinguish preserving an ICC profile from converting pixels
to sRGB. Attaching an sRGB label to unchanged wide-gamut pixels is not conversion.
Likewise, white padding does not prove an existing background is white or that the
subject occupies the requested fraction of the frame.

Technical checks inspect the actual encoded output: content-derived format,
dimensions, alpha, byte count, naming, and supported profile/depth constraints.
Claims about accurate depiction, rights, logos, text, background, or framing require
the appropriate visual check or explicit human review. An AI opinion is not a
marketplace acceptance guarantee. Unsupported mandatory checks remain **Not checked**.

Use **Technical checks passed**, **Needs review**, or **Cannot meet requirements**,
not a universal **Marketplace approved** badge. Export the files plus a per-file
report identifying recipe/policy revision, checks performed, failures, and output
identity. Do not reduce the source master to a marketplace derivative merely to
feed a retailer who actually requested the master.

## Interaction and safety contract

Show one execution summary before committing: selected file count, effective
format/quality/depth, dimensions, destination, source handling, and local versus
remote execution. Explain a forced-lossless transparent WebP rather than making
the quality slider appear to do something it cannot.

A saved profile is data, not permission. Import must have bounded file size, field
lengths, names, and counts. Reject traversal in output naming; use the existing
safe output boundary and collision planner. Do not import credentials, executable
commands, automatic upload permission, or an absolute destination path from someone
else's template. A different selected output folder is a local decision.

Keep the same recipe snapshot throughout preview, conversion, export report, and
optional submission. Stale source content or a changed policy invalidates affected
work visibly. Settings changes after conversion do not retroactively relabel an
existing output as produced by the new recipe.

## Acceptance tests

| Case | Required outcome |
| --- | --- |
| Save, restart, edit, duplicate, delete | Persisted settings round-trip; modified copies remain distinct; deleting never removes outputs. |
| GUI versus CLI | Resolve the same explicit recipe; GUI preference state does not change a headless invocation. |
| Corrupt, oversized, future-schema import | Named refusal before any image or settings file is changed. |
| Unsupported mandatory rule | Remains unsatisfied/not checked; never appears as a pass. |
| Identical stem, Unicode names, case-insensitive destination | Stable collision-safe names and accurate result-to-source mapping. |
| Quality, size, codec-option, or source change | A recorded output is not falsely reused as the new job's verified result. |
| Third-party or edited output | Remains protected; no automatic overwrite justified by an unverified timestamp. |
| Too-small source or impossible byte budget | Explicit unmet requirement, not silent enlargement or over-compression. |
| New template revision or effective date | Existing run remains pinned; new run shows the applicable changed requirements. |
| Multi-target export, when added | Independent outputs and reports; one target's failure does not invalidate successful siblings. |

Start with persistent recipes, not a catalog service, a marketplace uploader, or a
new preset marketplace. One maintained data model should do the work.
