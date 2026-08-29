---
name: press-cli
description: Audit or convert local image files with the Press CLI. Use when an agent needs structured image metadata, optimization findings, or explicitly authorized local WebP, AVIF, or JPEG XL conversion.
---

# Press CLI

Use `press` for local, scriptable image audits and conversion. It does not upload files in CLI mode.

## Inspect first

Run `press --help` if the installed command may differ from this skill. Use the read-only command before proposing or performing conversion:

```bash
press audit <file-or-folder> --json
```

Read `summary`, then inspect `files` for exact dimensions, bytes, bytes per pixel, `heavy`, and `mislabelled`. A successful JSON command writes one document to stdout with `schema_version: 1`; diagnostics go to stderr.

Exit status `0` means the audit was complete. Status `1` means the JSON is still usable, but one or more paths could not be read; report the named `unreadable` or `walk_errors` entries.

## Convert only when authorized

Conversion writes into `optimized/` under the input folder and can replace outputs from an earlier run. Do not run it from a request to inspect, audit, compare, estimate, or recommend. Get explicit authorization to write files, then use one command:

```bash
press convert <file-or-folder> --format webp --quality 80 --json
press convert <file-or-folder> --format avif --quality 70 --max-edge 1600 --json
press convert <file-or-folder> --format jxl --lossless --json
```

Use quality `1` through `100`. `--lossless` supports WebP and JPEG XL, not AVIF. `--max-edge` only downscales.

Treat exit status `1` as a partial result, not as proof that nothing was written. Read each file's `status`, report named failures, and use each successful `output` path as the source of truth. Never claim savings from the requested settings alone; use `summary.source_bytes` and `summary.output_bytes` from the completed run.

## Update Press

Run `press update` to install the latest signed release. Self-updating works for the Press AppImage, macOS app, and Windows installer; use the package manager for other installs.

## Bundled copy

`press skill` prints this exact skill to stdout for installation or use outside the Press repository.
