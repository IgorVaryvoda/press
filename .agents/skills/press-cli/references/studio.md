# Rehearse one hosted Studio operation

The bounded hosted client is a local rehearsal only. Use an explicit fixture with
`--fake`; there is no live service or billing authority:

```bash
press studio quote --tool upscale --image <file> --payer <id> --fake <script> --json
press studio accept --job <id> --image <file> --fake <script> --json
press studio status --job <id> --fake <script> --json
press studio cancel --job <id> --fake <script> --json
press studio reconcile --job <id> --fake <script> --json
press studio retrieve --job <id> --out <file> --fake <script> --json
```

The quote pins the input hash, payer, model, maximum credits and expiry. Acceptance
requires the same input bytes and persists `submitting` before the fixture call. A
transport or unknown result keeps that job inspectable; `reconcile` performs a lookup
by its client id and never redispatches it. Completed retrieval refuses to clobber an
existing destination. JSON reports `rehearsal: true` so fixture results cannot be read
as live work.
