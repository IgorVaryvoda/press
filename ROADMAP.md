# Press: prepare, verify, deliver

Decision recorded 2026-09-04. This is a proposed implementation sequence, not a
list of features already shipped. Reference code: `583a1c1f` (0.4.4).

Press should turn a local folder into reviewed, delivery-ready assets. The next
product direction is **custom presets, maintained marketplace export templates,
and optional Sirv Studio supplier-portal submission**, built on one processing
contract rather than three separate feature engines.

```text
Choose requirements -> prepare locally -> inspect outputs -> export or submit
                                                            |
                                             Studio validation -> review -> delivery
```

Ordinary local work remains useful without an account. Studio owns supplier
access, product assignments, authoritative requirements, review, approval, and
downstream delivery. Press owns local preparation and an honest submission client.
A successful local check or upload is not approval.

## What this PR changes

- Moves manifest-backed output-name and backup planning off the UI thread while
  retaining the existing collision rules and immutable selection snapshot.
- Fences preflight results, progress, and completion with both the dataset and the
  conversion run's token. A superseded plan cannot start writing; a stopped current
  run still completes its normal stop-reporting path.
- Makes comparison refuse the lossless sample-depth reductions already refused by
  the disk writer, with a cross-path regression test. This is a narrow guard, not
  the complete shared preparation layer described below.

The initial output-context check still runs in the click handler. The new Rust
regression tests are included but were not executed in the authoring environment;
merge requires the local gates in [engineering follow-up](docs/engineering-follow-up.md).

No supplier connection, custom-preset editor, or marketplace template is shipped
by this PR. Those are the explicitly scoped next increments.

## Sequence and exit conditions

| Order | Deliverable | Exit condition |
| --- | --- | --- |
| 0 | Finish processing trust fixes | Preview, estimate, export, and prepared upload agree on supported transforms; a large/slow destination does not block input; existing recovery tests pass. |
| 1 | Save and reuse custom presets | Save current settings, select, duplicate, rename, delete, import, and export; settings survive restart; GUI and CLI resolve the same recipe. |
| 2 | Maintained marketplace templates | Two or three pilot-relevant channel/role templates use the same recipe model, have sourced and dated rules, and distinguish technical checks from visual review. |
| 3 | Supplier-portal pilot | One retailer and a small supplier cohort can load assignments and requirements, prepare, submit, recover interrupted work, and see authoritative per-file outcomes. |
| 4 | Repeated-job efficiency | Add multi-target exports or watched-folder preparation only after pilot evidence identifies repeated work worth automating. |

Start custom-preset design while fixing the engine, but do not build new output
promises on inconsistent validation. A committed supplier pilot can move the
integration forward in parallel; it must not bypass the processing or server-side
security gates. No dates are promised without implementation and customer evidence.

## One model, three sources

A built-in template supplies maintained requirements and a recommended recipe.
A personal preset saves the user's recipe. A connected retailer supplies its
current, server-resolved requirements and may recommend a recipe. All three use
the same supported operations and output verification.

**Requirements and preferences are not interchangeable.** A user can fork a
marketplace recipe, but cannot relax a retailer's mandatory requirements and keep
claiming that retailer's validation passed. Record the target policy revision and
the effective processing settings used for every result.

Implementation scope, import rules, UI, and acceptance cases are in
[delivery recipes](docs/delivery-recipes.md). Supplier boundaries, authentication
dependencies, and a minimum useful integration are in
[supplier-portal integration](docs/supplier-portal-integration.md).

## Product boundaries

Keep the folder and its deliverables central. The operation rail should summarize
selection, effective transform, destination, source handling, and whether bytes
will leave the machine. Local and hosted AI remain contextual repairs, not a second
product competing with batch preparation.

Do not build another DAM, PIM, review queue, workflow canvas, or general image
editor inside Press. Do not implement separate marketplace upload adapters merely
to export compliant-looking files: the first templates produce a local delivery
folder and report. Do not require Press installation for occasional suppliers;
the browser portal remains the lower-friction path.

Watch folders must never imply silent upload or approval. New formats, AI tools,
and integrations need a named user problem before taking priority over source
safety, repeatable results, or delivery reliability.

## Prove usefulness

Use real, consented folders from an initial cohort of about ten target users;
this is an observational pilot, not a statistical success claim. Record whether
they complete the first job without explanation, find the outputs, return for a
second job, and replace an existing preparation step.

For supplier work, compare against the browser-only workflow: first-pass technical
acceptance, corrections/resubmissions, time to a reviewable submission, support
interventions, and retry recovery. Count wrong-product or wrong-tenant assignment
as a release blocker, not an acceptable average. Collect this through consented
pilot observation and operational receipts; local folders need no mandatory
behavioral telemetry.
