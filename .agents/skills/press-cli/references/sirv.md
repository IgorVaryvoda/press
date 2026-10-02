# Sync a folder with Sirv

`press sirv` compares a local folder with a Sirv folder by relative name and byte size, and pushes or pulls what differs. It uses the Sirv API keys the user saved in the Press window. Without them every verb exits `2` and says so.

```bash
press sirv status <folder> --remote /photos --json
press sirv push <folder> --remote /photos --allow-upload --json
press sirv pull <folder> --remote /photos --json
```

`--remote` names the Sirv folder. Without it, Press uses the folder's pairing from the window, and exits `2` when there is none. Subfolders are compared unless `--no-subfolders` is given. `optimized/`, `press-originals/` and dot folders are never compared.

`status` reads both sides and writes nothing. The report lists `only_local`, `only_remote` and `different_size` keys and counts `same_size`. Size is the only comparison: `same_size` is not proof that the contents match.

`push` uploads the `only_local` files. **Files on Sirv are public.** Without `--allow-upload` the push exits `2` before any upload and names the number of files and their size. Ask the user before you add the flag, every time: consent is not remembered between runs. Each file planned as new is checked again just before its upload, and one that appeared on Sirv in the meantime is not overwritten. A file reached through a symlink is never sent.

`pull` downloads the `only_remote` files. It never replaces a local file unless `--replace-changed` is given.

`--replace-changed` adds the `different_size` files: on push it replaces them on Sirv, on pull it replaces them on this computer. Ask before you use it.

Each transferred file is in `files` with `status` `pushed`, `pulled` or `failed` and a named `error`. Exit `1` means at least one file failed, or the run stopped after five failures in a row (`stopped: true`) and left the rest untried. Add `--progress` to a `--json` run for one stderr line per file.
