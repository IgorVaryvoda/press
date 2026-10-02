# Run Sirv Studio tools

`press studio` reaches the live Sirv Studio service with the API key the user saved in the Press window. Without a key every live verb exits `2` and says so.

```bash
press studio credits --json
press studio run --tool background-removal --image <file> --allow-upload --allow-spend --json
press studio run --tool background-replace --image <file> --prompt "a white studio floor" --allow-upload --allow-spend --json
press studio alt-text --url https://example.sirv.com/chair.jpg --allow-spend --json
```

`credits` reads the balance and spends nothing.

`run` sends one image to Studio, writes the result into the same `optimized/` folder a conversion would use (or `--output <dir>`), and reports `output`, `credits_used` and `credits_left`. The tools are `background-removal`, `background-replace`, `upscale` (2x), `image-to-image` and `product-lifestyle`. The last three that change a scene need `--prompt`. **`run` uploads the image and spends credits.** Without both `--allow-upload` and `--allow-spend` it exits `2` before anything is sent, and names the flags that are missing. Ask the user before you add them, every time. Consent is not remembered between runs.

Studio quotes no price before the work and reports the charge after it. Report `credits_used` and `credits_left` from the result; do not promise a cost in advance. A file over Studio's 20 MB limit that only fits as a lossy or smaller copy is refused by name: make that copy with `press convert` first, and run the copy.

`alt-text` has Studio read a public https URL, such as a Sirv CDN URL, and returns `alt_text`. Nothing is uploaded from this computer, but it spends credits, so it needs `--allow-spend`.

## Rehearse one hosted operation

The quote, accept, status, cancel, reconcile and retrieve verbs are a local rehearsal only. Use an explicit fixture with
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
