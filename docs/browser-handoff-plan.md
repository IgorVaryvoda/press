# ImageGuide to Press: diagnose, prepare, deploy, re-audit

Proposed implementation, 2026-09-08. Owner role: Press desktop engineer with an
ImageGuide extension/site counterpart. No changes to those other repositories or
new desktop protocol handler are included in this docs PR.

## Baseline and first slice

Press already audits local folders and has portable product jobs. ImageGuide's
extension already exports JSON and Markdown; its report code declares schema v4
and separates resources from their page usages. Its existing `lib/handoff.js` is a
popup-to-full-audit handoff, not a desktop integration. Read
[extension README](https://github.com/IgorVaryvoda/imageguide-extension/blob/main/README.md)
and [report implementation](https://github.com/IgorVaryvoda/imageguide-extension/blob/main/lib/report.js)
before designing an adapter. Recheck their revision in the implementation PR.

**First delivery: an explicit report file import, not native messaging.**
The producer exports a sanitized, bounded handoff; Press imports it as a pending
local task and asks the user to select the source folder. No account, daemon,
localhost listener, browser cookies or native host installation is needed.
The file suffix and envelope schema must be agreed with the producer; do not
advertise a `.pressjob` association or a new CLI flag before implementing it.

A raw existing report is not automatically safe for redistribution. Make export
redaction visible; test the same producer fixture in the consumer. Keep a normal
report/download path when Press is missing or cannot read that schema.

## Minimum handoff contract

The proposed envelope contains its own schema, producer/report/model revisions,
observation time, a task ID, selected resources and their usage evidence. Per
resource, retain an opaque ID, approved URL/path hints, observed dimensions and
size provenance, relevant findings and desired output constraints. Preserve
unknown values as unknown; do not turn estimated bytes into measurements.

Imported page text is untrusted data, including any text an agent reads. It must
never become instructions to execute commands, access another folder or upload.
Set bounded payload, resource, usage and string counts in a shared fixture contract;
reject oversize data before allocating the full parse. Reject unknown required
schema semantics rather than silently changing their meaning.

Drop credentials, URL user-info, fragments and sensitive query values by default.
Keep dimension/format query hints only through an explicit allowlist and export
preview; redaction can make resources ambiguous, which must remain visible.
Private page paths can be sensitive too: disclose them and allow removal. Do not
claim that a report is anonymous merely because cookies are absent.

No image bytes, absolute local paths, executable instructions, output destinations,
access grants or upload authorization belong in this initial handoff. An imported
origin label is provenance, not proof of trusted authorship or authority.

## Matching is the hard part

The user chooses each local source root. Search only that root, with traversal,
symlink/junction escape and platform path checks before reading selected sources.
Use exact supplied mappings first; filename/path/dimension matches are candidates.
A content hash can prove identical bytes only when both sides actually measured
the same bytes. A resized CDN derivative is not its local master, and perceptual
similarity is not proof of product identity.

Show **confirmed**, **candidate**, **ambiguous**, **unmatched** and **out of scope**.
Require explicit confirmation of candidate mappings before conversion. Duplicate
basenames, hashed build names, transformed CDN URLs and multiple usage sizes must
not collapse into a guessed source. Do not fetch arbitrary report URLs on import.
Remote download would need a separately designed, consented and bounded network
path; it is not necessary for this local-source pilot.

The current portable-job exporter retains a base-path hint and reserved binding
fields. Do not reuse that export unchanged as a public handoff. Apply the portable
boundary in [engineering follow-up](engineering-follow-up.md), including stripping
or explicitly rebinding connected context and removing machine-specific hints.

## Keep the remediation scope honest

| Finding class | Press outcome | What remains outside this slice |
| --- | --- | --- |
| Excess dimensions or suitable format change | Prepare and inspect a confirmed local derivative | Publish the file and update asset references |
| Multiple responsive sizes | Show the usages; prepare supported independent targets | Correct `srcset`, `sizes` and server negotiation |
| Missing dimensions, alt attribute or loading issue | Preserve as a named markup task | Inventing alt text, editing the website or claiming a codec fixed it |
| Delivery/cache/measurement uncertainty | Retain evidence and unknowns | Proving field performance or server configuration from a local export |

Produce an output mapping and a human-readable deployment checklist beside the
local receipt. Leave originals untouched. The user deploys through their existing
CMS/repository workflow; Press does not rewrite a website or publish files in the
first slice. Re-audit only after deployment and associate results with the original
task. Different viewport, selected variant or saving-model revision can prevent a
valid before/after comparison; report that rather than inventing measured savings.

## Implementation PRs and acceptance

| PR | Small result | Required evidence |
| --- | --- | --- |
| H1: schema and file adapter | Sanitized producer fixture imports to a pending task | Round-trip schema checks, redaction, future-schema/oversize refusal, no network or writes on import |
| H2: source mapping and preparation | One real page's selected findings map to a user-selected folder | Duplicate/hashed names, CDN variants, ambiguous matches, root escape attempts and changed sources; no unconfirmed conversion |
| H3: deployment and re-audit | Files, output mapping and checklist reconnect to the original observations | Local-export versus deployed distinction, changed viewport/model, unresolved markup, unmatched resources and cancelled work |

Name accountable people in those PRs. H1/H2 depend on safe parsing, path confinement
and source identity, not on Supplier Portal, paid AI or a template catalog. Use
synthetic fixtures and consented real pages; keep private reports out of the public
repository. Measure attempted handoffs, confirmed mappings, completed jobs and
post-deployment re-audits separately.

## Later launch convenience

Only after file-based jobs complete reliably, compare explicit OS file association,
a registered app link and Chrome native messaging. Keep the file path as fallback.
Native messaging requires a permission and installed host configuration with allowed
extension origins; it is not a free deep-link shortcut. See
[Chrome's official native messaging documentation](https://developer.chrome.com/docs/extensions/develop/concepts/native-messaging),
checked 2026-09-08. App links may locate a pending task but never carry credentials
or start writes/spending. Do not put a private report in a public URL.

Rollback is simple: hide the new handoff entry point while retaining normal browser
reports and Press's local workflow. A failed import must not modify existing jobs,
source files, saved credentials or an active conversion.
