# Processing trust: review follow-up

Status: code changes plus an ordered remaining backlog. Reference: Press
`583a1c1f` (2026-09-04). A source observation is not a reproduced runtime failure.
The full feature direction is in the [roadmap](../ROADMAP.md).

## Included in this PR

`src/audit/convert_job.rs` now does manifest loading, output-name planning, and
backup-chain lookup on the background executor. Planning still uses a snapshot of
all audited paths, including unselected originals. Before starting writes it checks
both dataset generation and run-token identity; progress and completion use the
same ownership check. Stop during planning reaches the existing stopped-run path
without starting an encode after the stop is observed.

The initial `Output::context` check remains synchronous. This PR is not a claim
that every preflight filesystem operation is now off-thread, and cannot interrupt
an operating-system metadata call already in progress.

`src/compare.rs` now applies the disk writer's existing lossless-depth restrictions
before directly encoding a comparison. The cross-path test covers a 16-bit PNG,
with and without a cached preview, at full and reduced dimensions, and confirms
that refused work writes nothing. A separate predicate test covers supported
integer inputs and JPEG XL's float restriction.

This is a tactical comparison guard. It does not make estimation or Studio upload
preparation share the same validation layer yet. Existing comparison errors also
still use the current generic failure UI.

## Correct one finding before adding work

`main.rs::queue_run` already compares recorded format, quality, and maximum edge
when `Record::installed` identifies the recorded output. Changing those settings
rebuilds such a managed output. An unrecorded or externally edited output retains
the older timestamp-based skip behavior. Do not implement a second settings-aware
skip mechanism based on the older README paragraph or the original review.

The remaining work is fuller source/recipe identity, safe legacy handling, and
updating documentation to describe this existing distinction accurately.

## Next engineering increments

### 1. Share preparation and validation before expanding presets

Move transform validation to the common encoding/preparation boundary used by
comparison, estimation, export, and Studio preparation. The validated result should
record requested versus effective format, sample depth, alpha policy, resize,
profile handling, warnings, and structured failure. Do not re-decode or write a
file just to discover whether a transformation is permitted.

Remove the tactical comparison guard only when this shared path owns the same
restriction. Keep cross-path tests so a future bypass cannot quietly return.

Exit: the same source and request receives the same acceptance/refusal and effective
transform across callers. Cover 8/16-bit RGB and grayscale, float input, genuine ICC
profiles, real transparency, opaque alpha, and animated containers. Pixel-preserving
encoding is not a promise of correctly color-managed screen rendering; test those
separately. Estimates must not silently omit unsupported files and then project
success across them.

### 2. Fix prepared Studio upload fidelity

In `studio.rs::prepare_upload_using`, the conversion fallback currently discards the
decoded profile and calls the low-level encoder for a purported lossless WebP copy.
Accepted original bytes take a different passthrough path. Preserve that distinction;
do not unnecessarily re-encode ordinary accepted originals.

Preserve the ICC profile on a prepared copy, or explicitly transform pixels when a
supported color conversion was selected. Refuse unsupported lossless depth changes,
or obtain an explicit lossy/depth-reduction decision through a clearly labeled path.
Do not turn a failed lossless attempt into an unannounced lossy upload. Include actual
profile bytes in size-limit checks and preserve source files throughout.

Exit: passthrough bytes are identical; an oversized tagged image retains its profile
or records an explicit transformation; 16-bit fallback cannot masquerade as lossless;
no upload occurs before any required confirmation. Use a loopback service and fixtures,
not production credentials or billable Studio calls, for these regression tests.

### 3. Finish asynchronous preflight and source/recipe identity

Move the initial output-context proof off-thread with an explicit planning phase;
retain prior results if that destination is refused. Cancellation and source changes
must retire planning without leaving busy state stuck or starting a stale write.

`compare::Key` currently has path and conversion settings but no source revision.
Add freshness checks outside rendering, then revalidate at job boundaries. Do not
add `stat()` calls to every row render or assume a filename uniquely identifies its
current contents. Include all output-affecting recipe settings, including AVIF
speed, in managed-output identity. Keep edited and unrecorded outputs protected.

Exit: edits, deletion, same-name replacement, recipe changes, revoked/invalid output
folders, and a stopped preflight yield truthful results. Exercise slow destinations
and a restart; compare before/after behavior on the same hardware.

### 4. Make operation labels and scope honest

In `audit/panel.rs`, the `Resize only` preset keeps format but still recompresses at
quality 80. Rename it to describe that behavior, or implement a true unchanged-file
path when resizing is unnecessary. Scope `Pixel-perfect` to supported input types
and make forced lossless encoding visible. A general `Recommended` preset is a
convenient default, not proof it meets a marketplace's requirements.

Keep selection, effective settings, destination, original handling, and local/cloud
execution legible together. Verify keyboard focus, selection/filter interaction,
failed-file recovery, narrow windows, and scale-aware thumbnails on real screens.
Do not redesign the shell without task evidence.

### 5. Bound aggregate memory, not just worker count

Retain the format-aware worker limits and existing cache budgets. Add admission by
estimated decoded bytes across active conversion/preview/estimate work, with a named
refusal or explicitly limited path for an oversized image. Codec working buffers
and transient copies count too. Header-based estimates are admission hints, not a
claim that an untrusted decoder can never allocate more.

Exit: representative large images and rapid preview/slider changes do not create an
unbounded set of stale decodes. Record peak RSS and cancellation behavior on a named
reference machine; do not infer performance solely from source constants.

### 6. Extract ownership only where it removes duplicated behavior

Keep the single crate. Extract a common validated conversion plan and reusable job
semantics as the recipe work needs them. Let audit/session state own selection and
navigation; keep remote credentials and transfer state with their respective
integration. GUI and CLI may report progress differently, but must not develop
different file-safety or transformation policies. No framework rewrite, service
split, or generic workflow graph is needed.

## Validation and merge gate

The six added Rust regression tests have not been executed for this PR. See the
PR validation record for authoring-environment limitations. Before merge, run the
complete suite with the pinned toolchain and native dependencies, not only the
new tests:

```sh
cargo test --locked
cargo clippy --locked --all-targets -- -D warnings
cargo fmt --check
```

Before merge, also exercise a real window: start a large-folder conversion, stop
while planning, reopen a different dataset, and confirm there are no stale writes,
wrong row associations, or stuck controls. Compare a 16-bit PNG as lossless WebP
and then request its export; both must refuse without creating an output. Recheck
ordinary 8-bit conversion and replace/restore flows.

Do not report GitHub Actions status, model-generated UX opinions, a syntax scan,
or the presence of test code as successful local or native runtime validation.
