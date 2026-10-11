# Product-set jobs

A job groups a folder's images by product. The Press window saves jobs; the command line reads them. Each product has views (roles) such as `main` and `detail`, and each view holds mapped files. A job can also have delivery targets: a saved recipe and a folder of its own for each.

```bash
press job list --json
press job show <id> --json
press convert <folder> --job <id> --json
press plan --root <folder> --output <dir> --plan <plan.json> --job <id> --json
```

`job list` names every saved job with its product and target counts; `skipped` names job files that do not parse. `job show` gives each product's `complete` flag, each view's `status` (`ready`, `changed`, `missing` or `unmapped`), and each mapped file's planned `outputs`. A product is complete when every required view has its files on disk. Both are read-only.

`--job` converts the folder as usual, with two changes. A mapped file is written as `<SKU>_<role>.<ext>` directly in the output folder, numbered `-1`, `-2` when a view holds several files, and once per view when one file serves two views. The SKU hint names the product, else its name. Unmapped files keep their mirrored names. When the job has delivery targets, they are the run's targets, exactly as `--target` would make them, so the report carries one `targets` section per target. `--job` refuses `--target`, `--replace`, and explicit `--format`, `--quality` or `--preset-file` when a target's recipe already answers them.

`plan --job` records each file's output name in the plan, so `execute` writes the same names. A saved plan has one output per file and target: a file that serves two views is refused by name at `plan`; use `convert --job` for it.

An unknown job id exits `2` and writes nothing.
