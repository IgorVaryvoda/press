# Save, execute and reconcile a local plan

Use a saved plan when the selected files, output mapping and effective settings must be reviewed before conversion. Creation binds an explicit source and output root, snapshots bounded source bytes and writes only the plan file:

```bash
press plan --root <source-dir> --output <output-dir> --plan <plan.json> --format webp --quality 80 --json
press execute <plan.json> --root <source-dir> --output <output-dir> --continue-unstarted --json
press reconcile <plan.json> --root <source-dir> --output <output-dir> --json
```

The plan is portable data. It contains relative source/output names, source SHA-256 identities, collision mappings, resolved settings and the engine revision; it does not contain roots, credentials, shell commands or permission. Every relative name in it must be a portable one: a literal backslash, a drive or alternate-stream marker, a reserved Windows device name, a control character and a trailing dot or space are refused rather than retargeted on the machine that opens the plan. Execution requires both roots again and refuses a changed source, changed mapping, unsafe boundary, incompatible recipe or edited destination. It never uses `--replace`.

Source identity and metadata come from one bounded read: dimensions, container and encoded depth are measured from the same bytes the SHA-256 covers, and a file that changes between the walk and that read is refused instead of described by two reads that never agreed. A source that resolves outside the bound source root is refused before its bytes are consumed, whatever its hash.

Each target's `out` is a namespace, not authority. Two targets may not share or nest their namespaces, resolve to one folder through a link, or plan two sources onto one destination; those are refused before any effect. At the destination, a saved plan does not inherit ordinary conversion's permission to overwrite an unrecorded output because it is older than its source: a new, unmanaged, externally changed or differently produced file is preserved and the item fails. Ordinary `press convert` behaviour is unchanged.

`--continue-unstarted` runs only items that have not started. `--retry-failed` is a separate explicit action for failed items, and `--cancel` records pending items as cancelled without encoding. `--reinstate-cancelled` is its undo: it puts cancelled items back as unstarted and encodes nothing, so a cancellation is a decision you can take back rather than a state only a deleted run file escapes. `execute` takes exactly one of the four. `reconcile` inspects the manifest and installed output hashes, repairs receipts after an interruption and never re-encodes. Successful siblings stay written when another item fails. One lock covers a whole `execute`, `reconcile` or `--cancel` command for a given plan and root pair; a second concurrent command is refused by name rather than reading a half-written state. Plan, execute and reconcile JSON uses schema version `1`: exit `0` is complete, exit `1` is a partial run with named item states, and exit `2` is a refusal that wrote nothing — an invalid plan, an invalid binding, an invalid invocation, or a plan another command is holding — with a named top-level `error`. A run that starts and then cannot continue, because its state would not persist, exits `1` and carries the same `error` field beside the items it did write; read the items, not the code alone. The exit code survives a closed pipe, so `press execute … --json | head` still reports what the run did. A plan holds at least one source: a folder whose images none of them decode is refused at `plan`, never reported as a finished run of nothing.

## Review a plan against a requirements snapshot

`plan`, `execute` and `reconcile` also take `--requirements-file <local-spec>`, the same bounded snapshot `press check` reads:

```bash
press plan --root <source-dir> --output <output-dir> --plan <plan.json> --requirements-file <local-spec> --json
press execute <plan.json> --root <source-dir> --output <output-dir> --continue-unstarted --requirements-file <local-spec> --json
```

The plan records only which snapshot it was reviewed against — id, revision, provenance, effective interval, last verified date and a digest of the document. The rules themselves stay in their own file, so a copied plan cannot carry weakened checks. Execution must supply that same document again: a missing file, or a different one under the same id and revision, exits `2` with a named error and writes nothing.

Each written output is then checked against its actual bytes. The item carries a `requirements` object with `effective`, `all_required_pass` and the per-rule results, and the run carries `counts.requirements_failed`. A snapshot outside its effective interval leaves every check `not_applicable`, and an `unsupported` constraint stays `not_checked`; neither can pass. **An output that was written but whose required checks did not pass is a partial run with exit `1`, not a success** — and re-running does not encode it again to look for a different answer.
