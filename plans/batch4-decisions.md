# Batch 4 — Decision memos

Three findings from the 2026-08-22 audit are real but not plannable until the
maintainer makes the call each one hinges on. Recorded here so they stop
re-appearing in every future improve pass without progress.

## D — Push ships originals, never `optimized/` output

**State**: `run_push` uploads the source file. The converted WebP/AVIF under
`root/optimized/` is never pushed, even when it exists.
Deferred from batch 2 as "a design decision for the maintainer".

**The call**: what does Sirv sync *for*? Two coherent answers:

1. **Backup/sync of the library** — pushing originals is correct. Then the
   current behaviour is right and only the button label ("Push N new") could
   clarify scope. Cost: nothing.
2. **Publishing web-ready assets** — push should offer `optimized/`, since
   that is the app's whole output. Cost: a second key namespace on Sirv
   (`optimized/…`), ambiguity about which side "changed" compares, and a
   product question about whether the CDN should serve sources at all.

**Recommendation**: if the pairing exists to publish to a website, answer 2;
otherwise leave as-is and rename nothing. Do not build both modes.

## E — Size-only sync classification

**State**: `sirv::classify` compares byte sizes; equal-size different-content
reads "synced". Documented trade-off in batch 2's rejected list: "revisit
after 029 lands if users hit it". 029 has now landed (forced push/pull gives
the user a manual escape hatch for a false "synced"), which lowers the
urgency further.

**The call**: content hashing (even a cheap xxhash of local bytes vs a
remote-provided checksum) requires Sirv to expose hashes — its REST listing
does not, so true content comparison means downloading every file, which
defeats the sync. The realistic middle ground: hash local files and store the
hash alongside the remote copy on upload (a sibling `.sha256` key or a
manifest file). Cost: doubles small-file count on the remote, adds manifest
write logic, and only helps pairs created after the change.

**Recommendation**: not worth it while size-collisions remain hypothetical.
Revisit only when a user actually reports a false "synced".

## B — Auto-install updates without asking

**State**: `update::install_if_available` downloads and installs any signed
newer release unconditionally, in the background, at launch. Plan 032 makes
the outcome visible but deliberately keeps auto-install.

**The call**: is silent auto-update acceptable for this app?

- **For keeping it**: signatures verify integrity (`updater.pub`); the app is
  pre-alpha and fast-moving, so users benefit from not thinking about
  updates; there is no settings surface yet to put a toggle in.
- **Against**: an install writes to disk mid-session with no consent; a bad
  release channel mistake propagates instantly; enterprise/paranoid users
  reasonably expect opt-in.

**Recommendation**: keep auto-install while pre-alpha, revisit when a
settings page exists (the natural place for an "updates: automatic / ask"
control). If a release ever ships data-format changes that older versions
cannot read back, switch to ask-first before then.

**New evidence (2026-08-23)**: on macOS, when replacing the `.app` hits
`PermissionDenied` (admin-owned /Applications, managed Macs),
cargo-packager-updater 0.2.3 escalates via
`osascript … with administrator privileges` — an unexplained admin-password
dialog during startup (the check runs on a detached thread, racing window
creation, so it can appear before, beside, or after the first frame). That
is a materially worse failure mode than the memo weighed. Raise the
priority of "ask-first" when revisiting; plan 035 only stops non-bundle
installs.
