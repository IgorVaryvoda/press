# Processing trust: current baseline and follow-up

Updated 2026-09-05 against Press
`ec9794bad7a6115a2fe1cd8313ca623ae6420699`. Source inspection is not a reproduced
runtime result. The [roadmap](../ROADMAP.md) owns direction and the
[execution plan](workbench-execution-plan.md) owns feature sequencing.

## Preserve what has already landed

The original [PR #3](https://github.com/IgorVaryvoda/press/pull/3) is merged. It moved
manifest-backed output/backup planning onto the background executor and added
run-token plus dataset ownership checks. The initial destination-context proof
was not part of that off-thread move. Its comparison depth guard was an interim fix.

Since then, `convert::check_lossless_depth` is shared by processing paths, including
comparison and Studio preparation; the estimate path also has the shared check.
Studio preparation retains the extracted ICC profile when encoding lossless, lossy
and resized copies, and counts the resulting bytes against its upload limit.
Accepted original bytes keep their passthrough path. Do not rebuild either fix.

Reference changes:
[shared depth work](https://github.com/IgorVaryvoda/press/commit/46e4bf5d3fd5f03c7fb16b12cfaade1af7c0ab3d)
and [ICC preservation](https://github.com/IgorVaryvoda/press/commit/ec9794bad7a6115a2fe1cd8313ca623ae6420699).
The source includes new regression coverage; this documentation pass did not execute
it and does not claim end-to-end color correctness or complete preparation parity.

`main.rs::queue_run` already compares recorded format, quality and maximum edge for
recognized managed outputs. Unrecorded or externally edited outputs keep legacy
skip behavior. Extend full source/recipe identity rather than creating another
settings-aware skip mechanism from an outdated README or review paragraph.

## Remaining implementation increments

### 1. Complete a shared recipe/result contract

The shared depth predicate is not the whole requested/effective transform contract.
Make supported format, resize, alpha, depth, profile handling, warnings and refusal
consistent across preview, estimates, export and prepared upload. Extend the existing
boundary; do not add a second encoder path. Inspect actual outputs and name any files
excluded from estimates rather than projecting success across unsupported work.

Cover 8/16-bit RGB and grayscale, float sources, opaque alpha, real cutout edges,
animated containers and genuine ICC profiles. Profile transport tests are useful but
not proof of color-managed rendering. Preserving a profile is not converting pixels
to sRGB. Keep refusal of unsupported lossless depth changes explicit; any future lossy
fallback needs a separate informed decision, not a silent reinterpretation.

Exit: accepted/refused requests and effective transformations agree across entry
points, with realistic fixtures and no writes or uploads before required consent.
Do not weaken current high-depth refusal to make a new preset appear supported.

### 2. Finish asynchronous planning and freshness

Revalidate and move any remaining destination-context filesystem work off the click
handler with an explicit planning phase. Preserve prior results if the destination
is refused. Cancellation, a changed dataset and a replaced run must not leave stuck
busy state or permit a stale plan to start writes.

`compare::Key` has conversion settings and path rather than complete source revision.
Add freshness checks outside rendering and at job boundaries; do not stat every
visible row on each frame. Include all output-affecting options, including AVIF
speed and processing revision, in the recipe identity. Keep edited and unrecorded
outputs protected rather than blindly overwriting them after a settings change.

Exit: edited/deleted/same-name-replaced sources, recipe changes, invalid/slow output
folders, cancel and restart have truthful states. The workbench's source/master,
deliverable and remote-attempt identities reuse this foundation.

### 3. Honest labels and useful scope

Recheck built-in labels while converting them to the common recipe model. `Resize
only` must not conceal recompression; `Pixel-perfect` must not claim unsupported
source-depth preservation. Explain effective forced-lossless behavior for transparent
WebP. A convenient default is not a recipient's validated requirement.

Keep selected count, effective settings, destination, original handling and local/cloud
execution legible together. Verify keyboard/focus, filter/selection interaction,
narrow windows, failed-file recovery and scale-aware thumbnail rendering with the
actual app. Do not redesign the shell merely to expose more features.

### 4. Aggregate memory and ownership

Retain format-aware workers and existing thumbnail/estimate/prefetch budgets. Add
admission by estimated decoded memory where needed, including transient buffers,
rather than assuming a worker count alone bounds all input sizes. Header estimates
are not a guarantee that arbitrary decoders cannot allocate more. Measure peak RSS
on a named reference machine with large images and rapid interaction.

Keep the single crate. Extract product-job state, shared recipe planning and service
lifecycle only as ownership requires them; do not let every new responsibility expand
Audit. GUI and CLI may report differently but must share safety and transform policy.
No microservices, database replacement or generic execution graph is required.

## Validation obligations

This docs update changes no Rust code and makes no claim about current native test
results. For implementation changes, run the pinned-toolchain gates:

```sh
cargo test --locked
cargo clippy --locked --all-targets -- -D warnings
cargo fmt --check
```

Exercise the real window: stop during slow planning, change/reopen the dataset,
check ordinary conversion and replace/restore, and compare/export unsupported
lossless source depths. Add fake-service tests for upload preparation; production
credentials and billable Studio calls are not regression fixtures. Preserve sources,
name failures and record actual results, not just the existence of tests.

The wider [connected-services plan](studio-connected-services.md) adds separate
native-auth, spend and server-recovery gates. A new product view does not imply those
services are ready; a passing local codec test does not certify supplier authorization.
