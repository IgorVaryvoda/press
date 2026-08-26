# Press UX evaluation loop

This folder turns a UI opinion into a repeatable task evaluation:

```text
scenario -> real Press captures -> four independent reviews -> judge
        -> one proposed change -> blind before/after comparison -> decision ledger
```

The evaluator changes no Rust source. Reviewers run with Codex's read-only sandbox,
cannot see one another's output, and return JSON constrained by a schema. The judge
may propose one change only when at least two reviewers identify the same user
problem from evidence.

## Run it

```bash
./scripts/ux-eval doctor
./scripts/ux-eval self-test
./scripts/ux-eval run audit-core
```

`run` builds the locked release binary, launches it in isolated headless Gamescope
windows, captures the sizes declared in `scenarios.json`, runs the reviewer panel,
and writes `report.md` under `ux/runs/<run-id>/`.

To review an existing capture or skip an unchanged build:

```bash
./scripts/ux-eval capture comparison-core --skip-build
./scripts/ux-eval review ux/runs/<run-id>
```

After implementing one accepted proposal, capture the same scenario again and run
a blind comparison:

```bash
./scripts/ux-eval compare ux/runs/<before> ux/runs/<after>
```

Record the maintainer verdict. Future panels receive the latest decisions as context:

```bash
./scripts/ux-eval decide ux/runs/<run-or-comparison> accept \
  --reason "The output decision now reads in one direction"
```

`PRESS_UX_REVIEW_MODEL` and `PRESS_UX_JUDGE_MODEL` optionally select models. With no
override, Codex uses the configured default.

## Evidence and privacy

Normal Press audits remain local, but this evaluator attaches its screenshots to the
configured Codex service. The bundled scenarios use repository images. An external
fixture is refused unless `--allow-external-fixture` is explicit; inspect client
imagery before allowing it.

Generated runs are ignored by Git. `decisions.jsonl` is append-only and intentionally
tracked once it exists: accepted and rejected outcomes are the system's durable taste
memory. Edit `principles.md` only for a rule that should constrain every future run.

The capture backend currently targets Linux Gamescope/PipeWire. Scenarios may send
keys, click fixed, edge-relative, or center-relative coordinates, type text, and wait. A launch with
`"copy": true` gives every viewport a fresh temporary fixture for write-path checks.
