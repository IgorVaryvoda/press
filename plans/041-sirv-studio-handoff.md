# Plan 041: Open synced Sirv images directly in Sirv AI Studio

> **Executor instructions**: Follow this plan step by step. Run every
> verification command and confirm the expected result before moving to the
> next step. If anything in the "STOP conditions" section occurs, stop and
> report — do not improvise. When done, update the status row for this plan
> in `plans/README.md` — unless a reviewer dispatched you and told you they
> maintain the index.
>
> **Drift check (run first)**:
> `git diff --stat 86a3ebc..HEAD -- src/sirv.rs src/audit/mod.rs src/audit/sirv_actions.rs src/audit/compare_view.rs src/audit/tests.rs README.md plans/README.md`
> If any in-scope file changed since this plan was written, compare the
> "Current state" excerpts against the live code before proceeding; on a
> mismatch, treat it as a STOP condition.

## Status

- **Priority**: P1
- **Effort**: M
- **Risk**: MED
- **Depends on**: none
- **Category**: direction
- **Planned at**: commit `86a3ebc`, 2026-08-25

## Why this matters

ImageGuide already audits local images and pairs their folder with Sirv, but a
user who decides an image needs creative work must find it again in Studio.
The first useful companion slice is one truthful handoff: a byte-identical
remote image opens in Studio's Image to Image tool with that Sirv CDN image
preloaded. It uses the existing Sirv credentials and browser, proves the
desktop-to-Studio contract, and does not add a second AI client or API key
before native execution is needed.

## Current state

- `README.md:86-97` defines the current boundary: pairing provides push/pull
  sync, while normal audit, comparison, and conversion make no Sirv request.

```text
Open the Sirv folder browser, choose a remote folder, and pair it with the current
local folder. ImageGuide then shows files that exist on only one side or have a
different byte size.
...
No Sirv request happens as part of a normal audit, comparison, or conversion.
```

- `src/audit/mod.rs:377-387` keeps remote listing state separate from the
  paired folder and client. There is no public CDN host in the pairing.

```rust
enum Listing {
    Walking,
    Failed(String),
    Ready(HashMap<String, sirv::Node>),
}

struct SirvPairing {
    dir: String,
    files: Listing,
    client: Arc<parking_lot::Mutex<sirv::Client>>,
}
```

- `src/audit/sirv_actions.rs:227-298` already performs the recursive walk on
  `cx.background_executor()` and rejects stale results with
  `dataset_generation` plus `sirv_pairing_generation`. CDN-host discovery must
  ride this same job and landing gate; never make it a render-time request.
- `src/audit/media.rs:138-147` is the existing source of truth for sync state:
  `Same`, `Changed`, or `OnlyLocal`, based on relative path and byte size.
  Opening Studio is safe only for `Same`; `Changed` would open stale remote
  bytes while the comparison shows newer local bytes.
- `src/audit/compare_view.rs:271-348` renders the comparison top bar. The
  existing `compare-convert` and `compare-close` buttons are the placement
  pattern for one new `compare-edit-studio` action.

```rust
.child({
    let target_count = self.target_count();
    Button::new("compare-convert")
        .primary()
        .small()
        // ...
})
.child(
    Button::new("compare-close")
        .small()
        .icon(IconName::Close)
        // ...
)
```

- `src/sirv.rs:420-445` owns the authenticated, blocking Sirv client and its
  token cache. Its injected API base under `cfg(test)` is the existing seam
  for a local HTTP test; do not add a mocking dependency.
- `src/sirv.rs:113-127` already percent-encodes query values, including `/`.
  Reuse it for the Studio `image=` value. A CDN path needs a second small
  helper that encodes each path segment but preserves `/` separators.
- `src/sirv.rs:805-817` deliberately ignores the legacy `studio_key` because
  it was display-only. Do not restore it for this browser handoff.
- The official Sirv account endpoint is
  `GET https://api.sirv.com/v2/account`; its documented response contains a
  host such as `"cdnURL": "demo.sirv.com"`:
  <https://apidocs.sirv.com/>. Use `cdnURL`, not the account alias, so a
  configured CDN domain remains authoritative.
- Studio's companion route is
  `https://dev.sirv.studio/tools/image-to-image`. As of 2026-08-25 it accepts
  `?image=<percent-encoded-public-image-url>` and preserves that query through
  its authenticated redirect. This was verified against Sirv AI Studio
  `origin/dev` at `d2f56d1053`; live-verify it before implementation because
  it is an external contract.
- The locked GPUI dependency exposes `cx.open_url(&url)`, and
  `IconName::ExternalLink` is present. No dependency is needed.

## Commands you will need

| Purpose | Command | Expected on success |
|---|---|---|
| Drift | `git diff --stat 86a3ebc..HEAD -- src/sirv.rs src/audit/mod.rs src/audit/sirv_actions.rs src/audit/compare_view.rs src/audit/tests.rs README.md plans/README.md` | empty, or reviewed and confirmed equivalent before work starts |
| Build | `cargo build --release --locked` | exit 0 |
| Tests | `cargo test --locked` | all non-ignored tests pass; screenshot test remains ignored |
| Lint | `cargo clippy --all-targets -- -D warnings` | exit 0 |
| Format | `cargo fmt --check` | exit 0 |
| Real app | `cargo run --release --locked -- /path/to/a/local-folder-paired-with-sirv` | comparison opens; the Studio action follows the states in Step 3 |

## Suggested executor toolkit

- Read `.Codex/napkin.md` before UI proof. The ignored GPUI screenshot test is
  broken on this Linux host; launch the real release binary, control its
  Hyprland window (or isolated Gamescope session), capture it with `grim`, and
  inspect the PNG.
- Use the official Sirv REST reference above for `/v2/account`. Do not infer
  the response from old ImageGuide code.

## Scope

**In scope** (the only files you should modify):

- `src/sirv.rs`
- `src/audit/mod.rs`
- `src/audit/sirv_actions.rs`
- `src/audit/compare_view.rs`
- `src/audit/tests.rs`
- `README.md`
- `plans/README.md` (status row only)

**Out of scope**:

- Native calls to `api.sirv.studio`, Studio credit accounting, polling, or a
  restored `studio_key`.
- Uploading a local-only or changed image automatically. Existing explicit
  Push/Push changed controls own remote writes.
- Pulling a Studio result back into the local folder or comparison cache.
- Persistent local-to-remote pairings, a transfer-preview redesign, metadata,
  marketplace checks, dynamic-imaging controls, 360-spin tooling, and saved
  workflows. They are recorded under "Deferred companion roadmap" below.
- A generic provider abstraction, tool registry, workflow engine, new URL
  crate, or any new dependency.
- Changes to the size-based sync definition.

## Git workflow

- Branch: `improve/041-sirv-studio-handoff`
- Keep one logical implementation commit; this repo uses conventional commit
  subjects, for example `feat: open synced images in Sirv Studio`.
- Do not push, open a PR, merge, or modify remote Sirv files unless the
  operator separately instructs it.

## Steps

### Step 1: Resolve and cache the account CDN host

In `src/sirv.rs`:

1. Add `cdn_host: Option<String>` to `Client`; initialize it to `None` in
   `Client::new`. A client belongs to one credential set, so this is the
   smallest correct cache and is retired when credentials change.
2. Add `Client::cdn_host(&mut self) -> Result<String, Error>`:
   - return the cached value when present;
   - otherwise make an authenticated `GET {api}/v2/account` using the same
     `authenticated`/`bearer` pattern as `readdir`;
   - deserialize only `#[serde(rename = "cdnURL")] cdn_url: String` and ignore
     other account fields;
   - validate the value before using it as a host: non-empty and ASCII
     alphanumeric, `.` or `-` only. Reject a scheme, slash, port, whitespace,
     query, or fragment as an `Error { status: 0, ... }`;
   - cache and return the validated host.
3. Add two pure helpers:
   - `public_url(cdn_host, remote_filename)` validates the host, strips only
     the remote filename's leading `/`, percent-encodes each UTF-8 path segment
     with existing `encode_path`, rejoins with `/`, and returns
     `https://{cdn_host}/{encoded-path}`;
   - `studio_image_to_image_url(public_url)` returns
     `https://dev.sirv.studio/tools/image-to-image?image={encode_path(public_url)}`.
4. Keep all HTTP blocking. The caller added in Step 2 will run it on the
   existing background executor.

Add focused `src/sirv.rs` tests:

- `a_public_url_preserves_folders_and_encodes_segments`: spaces, `#`, `%`,
  and a non-ASCII filename are encoded while folder separators remain `/`.
- `an_account_cdn_host_comes_from_the_authenticated_endpoint`: extend the
  existing local `TcpListener` pattern to serve token then account responses;
  assert `/v2/account`, its Bearer header, and the parsed `cdnURL`.
- Call `cdn_host()` twice in that test while the fixture serves the account
  response once; the second result must come from the cache.
- `an_invalid_cdn_host_is_rejected`: cover at least a scheme and a slash.
- `a_studio_url_encodes_the_public_image`: assert the exact final URL, not
  only that it contains the source.

**Verify**: `cargo test --locked` -> all non-ignored tests pass, including
the new URL/account tests, then
`cargo fmt --check` -> exit 0.

### Step 2: Carry CDN readiness beside, not inside, listing readiness

In `src/audit/mod.rs`, add a small state enum beside `Listing`:

```rust
enum CdnHost {
    Loading,
    Failed(String),
    Ready(String),
}
```

Add `cdn_host: CdnHost` to `SirvPairing`.

In `src/audit/sirv_actions.rs`:

1. `pair_sirv` initializes a new pairing with `CdnHost::Loading`.
2. `adopt_new_credentials` replaces the client, resets both
   `files = Listing::Walking` and `cdn_host = CdnHost::Loading`, then uses the
   existing pairing-generation invalidation and re-walk.
3. Extend the background work in `walk_sirv_pairing` to resolve
   `client.cdn_host()` and walk the paired folder under the same client lock.
   Return both results independently. A CDN-host failure must become
   `CdnHost::Failed(reason)` without turning a successful file walk into
   `Listing::Failed`; sync must keep working when Studio handoff does not.
4. Apply both results only after the existing dataset/pairing generation
   check. If the cancellable walk returns `Ok(None)`, land neither result.
   Preserve the existing `sirv_walk_cancel` cleanup and listing error text.
5. Update the two direct `SirvPairing` constructors in `src/audit/tests.rs`
   with explicit CDN state. Do not add `Default` merely to make tests shorter.

Add or extend a test so changing credentials leaves the pairing with
`CdnHost::Loading`, proving an old account's cached host cannot survive a
credential swap. The authenticated endpoint test from Step 1 covers request
shape; do not build a second HTTP fixture in the GPUI test module.

**Verify**: `cargo test --locked new_credentials_retire_the_old_listing` ->
pass; `cargo test --locked` -> all non-ignored tests pass.

### Step 3: Put one truthful Studio action in the comparison bar

In `src/audit/sirv_actions.rs`, add a side-effect-free
`studio_url_for(&self, entry: &scan::Entry) -> Result<String, String>` (use the
actual in-scope `Entry` import/name if already imported). It must:

1. Require a pairing and `Listing::Ready`.
2. Get the relative key with `sirv::relative_key` and find its remote node.
3. Require `sirv::classify(entry.bytes, Some(node)) == SyncState::Same`.
   Return distinct user-facing reasons for local-only and changed images;
   changed must say to push the changed image first.
4. Require `CdnHost::Ready`; keep the stored failure reason in the disabled
   explanation and say when host discovery is still loading.
5. Build the public URL from the walked node's normalized absolute
   `filename`, then build the Studio URL with the helpers from Step 1.

In `src/audit/compare_view.rs`, place a small
`Button::new("compare-edit-studio")` between Convert and Close:

- label: `Edit in Studio`;
- icon: `IconName::ExternalLink`;
- enabled only when `studio_url_for(entry)` returns `Ok(url)`;
- enabled tooltip: `Open this synced image in Sirv AI Studio`;
- disabled tooltip: the exact `Err(reason)` from the helper;
- click: `cx.open_url(&url)` only. It performs no network or disk work and
  does not close the comparison.

Add focused tests in `src/audit/tests.rs` around the pure availability method:

- a same-size remote node plus ready CDN host returns the exact Studio URL;
- a changed remote node is rejected, even though it exists;
- a local-only image is rejected;
- a failed CDN-host lookup retains its named reason.

Do not test the operating system's browser. The exact URL helper and state
tests own logic; the real-app check in Step 4 owns integration.

**Verify**: `cargo test --locked studio` -> all new Studio tests pass;
`cargo clippy --all-targets -- -D warnings` -> exit 0.

### Step 4: Document and prove the handoff in the real app

Update `README.md`'s Optional Sirv sync section with two restrained facts:

- a synced image's comparison has **Edit in Studio**;
- new or changed local bytes must be pushed explicitly before the action can
  open that exact image.

Do not describe native AI generation, result import, or persistent pairing as
shipped.

For live proof, use a local folder paired with a Sirv folder containing:

- one byte-identical image;
- one local-only or byte-different image.

Run the release app at both the 760x560 minimum and a normal 1100x720 window.
Open each comparison and verify:

1. the top bar does not overlap or clip its filename, conversion summary,
   Convert, Studio, and Close controls;
2. the identical image enables **Edit in Studio**;
3. the other image disables it and names the Push prerequisite;
4. clicking the enabled action opens the default browser on
   `dev.sirv.studio`, survives login redirect if needed, and preloads the
   exact paired image;
5. the desktop comparison stays open.

Capture and inspect both window sizes with the real-app flow in
`.Codex/napkin.md`. Do not use the ignored screenshot test as proof.

**Verify**: `cargo build --release --locked`, `cargo test --locked`,
`cargo clippy --all-targets -- -D warnings`, and `cargo fmt --check` all exit
0; real-app proof satisfies all five checks above.

## Test plan

- `src/sirv.rs`: exact CDN URL encoding, host validation, authenticated
  `/v2/account` parsing, one-client cache behavior, exact Studio query URL.
- `src/audit/tests.rs`: ready/same, changed, local-only, failed-host, and
  credential-change state. Follow the existing direct `SirvPairing` and
  `sirv::Node` construction around lines 503-608 and 174-248.
- No test may call Sirv or Studio over the public network. HTTP tests use the
  injected API base and local `TcpListener`; external behavior is the manual
  live-app gate only.
- Full verification: `cargo test --locked` -> all non-ignored tests pass.

## Done criteria

- [ ] `cargo build --release --locked` exits 0.
- [ ] `cargo test --locked` exits 0 with all new URL/state tests passing.
- [ ] `cargo clippy --all-targets -- -D warnings` exits 0.
- [ ] `cargo fmt --check` exits 0.
- [ ] `/v2/account` is called only from existing background Sirv work and its
      `cdnURL` is cached per `Client`.
- [ ] A CDN-host failure does not hide or fail a successful sync listing.
- [ ] **Edit in Studio** opens only a `Same` remote image; changed and
      local-only images explain the explicit Push prerequisite.
- [ ] The opened URL is HTTPS, uses the account's documented `cdnURL`, keeps
      remote folder separators, and percent-encodes path/query data.
- [ ] Real-app screenshots at 760x560 and 1100x720 were visually inspected;
      the live Studio route preloaded the exact image.
- [ ] `Cargo.toml`/`Cargo.lock` are unchanged and `studio_key` remains unused.
- [ ] No files outside the in-scope list are modified (`git status`).
- [ ] `plans/README.md` marks 041 DONE only after the live proof passes.

## STOP conditions

Stop and report back; do not improvise if:

- An in-scope current-state excerpt no longer matches after the drift check.
- The live Studio route no longer accepts or preserves `?image=<URL>`.
- The authenticated `/v2/account` response no longer supplies a usable
  `cdnURL` host.
- Correct behavior appears to require a Studio API key, automatic upload,
  result import, a settings-schema change, or any new dependency.
- Adding CDN discovery would make a discovery failure fail the existing Sirv
  listing or transfer path.
- `cx.open_url` is unavailable in the locked GPUI revision.
- A verification fails twice after one reasonable correction.
- No authenticated Sirv/Studio setup is available for live proof. Leave the
  row TODO or mark it BLOCKED with that exact reason; do not claim DONE from
  unit tests alone.

## Maintenance notes

- The handoff deliberately opens only `SyncState::Same`. If ImageGuide later
  gains immutable remote revisions or direct local upload to Studio, revisit
  that rule with an explicit source indicator; do not silently open stale
  remote bytes.
- `cdnURL` is account data, not settings. Keep it with the credential-bound
  `Client` and reset it when credentials change.
- Reviewers should scrutinize URL construction, host validation, independent
  listing/CDN failure states, and the stale-generation gate.

### Deferred companion roadmap

This plan records the larger direction without prebuilding it. Create a new
numbered plan only when the maintainer selects one of these slices:

1. Persist local-folder/Sirv-folder pairing and show a pre-transfer summary
   (`upload`, `replace`, `download`, `conflict`) before writes.
2. Add more browser handoffs only where Studio has a stable preload contract:
   background removal, upscale, lifestyle/image-to-image, review, and Generate.
3. Add one native Studio API vertical slice only after browser handoff usage
   proves the need: operation-specific input, credit quote/confirmation,
   background job, cancel/poll, and result loaded into the existing comparison
   view before an explicit save or Push. Do not start with a generic workflow
   engine.
4. Carry provenance and commerce metadata with derivatives: source Sirv path,
   operation/model, prompt, dimensions, and approval state; then add
   marketplace preflight for aspect ratio, dimensions, format, transparency,
   weight, naming, and missing views.
5. Add a delivery lab that emits Sirv dynamic-imaging URLs and compares
   responsive widths/formats/quality without downloading originals again.
6. Add 360-spin readiness (sequence gaps, inconsistent dimensions/crop,
   duplicate or outlier frames) before spin creation.
7. Add saved/repeatable workflows only when the public Studio API supports the
   required operations and real users repeat the same chain.
