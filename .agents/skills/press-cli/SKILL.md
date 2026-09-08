---
name: press-cli
description: Audit or convert local image files with the Press CLI. Use when an agent needs structured image metadata, optimization findings, or explicitly authorized local WebP, AVIF, JPEG XL, or JPEG conversion, or a resize that keeps each file's own format.
---

# Press CLI

Use `press` for local, scriptable image audits and conversion. It does not upload files in CLI mode.

## Inspect first

Run `press --help` if the installed command may differ from this skill. Use the read-only command before proposing or performing conversion:

```bash
press audit <file-or-folder> --json
```

Both commands walk subfolders by default and state that scope: `subfolders` in JSON, `subfolders included` or `subfolders excluded` on the text summary line. Pass `--no-subfolders` to read one folder only, which is how the Press window opens a folder by default.

Read `summary`, then inspect `files` for exact dimensions, bytes, bytes per pixel, `heavy`, and `mislabelled`. A zero-image result is not always an empty folder: `summary.heic_skipped` and `summary.camera_raw_skipped` count files that were found but never decoded, and both must be reported. A successful JSON command writes one document to stdout with `schema_version: 1`; diagnostics go to stderr.

Exit status `0` means the audit was complete. Status `1` means the JSON is still usable, but one or more paths could not be read; report the named `unreadable` or `walk_errors` entries.

## Convert only when authorized

Conversion writes into `optimized/` under the input folder and can replace outputs from an earlier run. Do not run it from a request to inspect, audit, compare, estimate, or recommend. Get explicit authorization to write files, then use one command:

```bash
press convert <file-or-folder> --format webp --quality 80 --json
press convert <file-or-folder> --format avif --quality 70 --max-edge 1600 --json
press convert <file-or-folder> --format jxl --lossless --json
press convert <file-or-folder> --format jpeg --quality 85 --json
press convert <file-or-folder> --format same --max-edge 1600 --json
press convert <file-or-folder> --output <dir> --json
press convert <file-or-folder> --skip-existing --json
press convert <file-or-folder> --dry-run --json
```

`--output <dir>` (short `-o`) writes the mirrored tree into that folder instead of `optimized/`. It is refused with exit status `2` when it is or contains the source folder or ends in a symlink. With `--json` the refusal is the schema-two error document (`output` null, `error` naming the reason, every file `failed`); without it, the reason is one stderr line plus per-file stdout lines.

`--skip-existing` leaves a source alone when its planned output is already current. Those files come back with `status` `skipped`, `skipped: true`, a named `reason`, and the size already on disk in `output_bytes`; they are counted in `summary.skipped` and never in `converted` or `failed`.

A recorded run first matches its recipe fingerprint and the source content hash: changed settings or edited bytes rebuild even when the timestamps look current. An output with no record skips on timestamps alone. Under this flag `summary.source_bytes` and `summary.output_bytes` cover only the files this run re-encoded, so they are not the size of the whole tree.

`--dry-run` writes nothing. A file that would be written comes back with `status` `planned` and the `planned_output` it would go to; `summary.converted` is `0`, and `summary.projected_bytes` holds the projected size of the whole run with `summary.projected_samples` real encodes behind it. The document carries `dry_run: true`. Report a projection as a projection, never as a measured saving.

A file whose destination or format is refused comes back from a dry run as `status` `failed` with the reason and a null `planned_output`, and the run exits `1`. A dry run cannot make the refusals that need the pixels: JPEG over a source with real transparency, an animated GIF, PNG, WebP or JPEG XL, and `--lossless` over a bit depth the format cannot keep. A clean dry run is not a promise that every file converts.

Use quality `1` through `100`. `--lossless` supports WebP and JPEG XL, not AVIF, JPEG, or `same`. `--max-edge` only downscales.

`--format jpeg` refuses a source with real transparency by name; JPEG has no alpha channel. `--format same` re-encodes each JPEG, PNG, WebP, AVIF, or JPEG XL source in its own format and keeps its file name and extension, so existing references keep working; use it with `--max-edge` for a resize-only run. Other formats, such as BMP or GIF, are refused by name under `same`.

Treat exit status `1` as a partial result, not as proof that nothing was written. Read each file's `status`, report named failures, and use each successful `output` path as the source of truth. Never claim savings from the requested settings alone; use `summary.source_bytes` and `summary.output_bytes` from the completed run.

Exit status `2` means the destination itself was refused before any file was converted. With `--json` that refusal is the schema-two error document; without it, the reason is one line on stderr — usually that `optimized` already exists as a file or a symlink. Report it; nothing was written.

Every file also carries `planned_output`, the name the plan gave it, whether or not this run wrote it. It is null when the plan itself was refused.

The report's `output` field is the canonical path of the folder that was written, so it can differ from the spelling of the target you passed when that path was reached through a link.

Every run appends one line to `.press-manifest.jsonl` in the output folder as each file lands, recording which source that output came from. The report's `manifest` field is its path. An output an earlier run wrote from a different source is never overwritten: that file gets a name of its own, such as `shot-jpg.webp`, so read each file's `output` rather than assuming the name.

## Replace the originals only when told to

`press convert <folder> --replace` writes each converted file beside its source and moves the original into `press-originals/` under the same folder. Nothing is deleted, but the folder the user gave you is rewritten, so treat it as a separate authorization from conversion itself. The report's `backup` field is the folder the originals moved into.

```bash
press convert <folder> --replace --format webp --quality 80 --json
press restore <folder>
```

`press restore` reads the manifest, moves every original back, and removes the file that replaced it. It works on a later run and on another machine, because the record is in the folder. It prints one `restored` line per file, names on stderr anything it could not put back, and exits `1` when any original stayed put.

## Map an ImageGuide report before converting it

`press handoff <report.json> [--root <dir>] [--json]` validates a findings report into a pending task and, with `--root`, resolves each resource against that folder. Import reads the report and the root's headers only: no network, no writes, no conversion.

Each resource gets one verdict: `confirmed` names its file, `candidate` names one plausible file, `ambiguous` lists several, `unmatched` names nothing, and `out_of_scope` points at another machine's layout. Convert only `confirmed` mappings, and only after the user picks them: a `candidate` is a resemblance, not an identification. Reported dimensions and measured bytes that disagree with the local file arrive as `notes`, not verdict changes. With `--deployed <dir>` each pinned-down mapping also verifies against the deployed tree (`deployed`, `differs`, `missing`, `ambiguous`); gaps exit `1`, and every finding stays open as page work.

## Update Press

Run `press update` to install the latest signed release. Self-updating works for the Press AppImage, macOS app, and Windows installer; use the package manager for other installs.

## Bundled copy

`press skill` prints this exact skill to stdout for installation or use outside the Press repository.
