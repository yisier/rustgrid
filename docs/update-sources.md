# Update sources: GitHub + CNB

Plan for replacing the GitHub-REST-based update check with a **static release manifest** that can
be served from both GitHub Releases and a **CNB (cnb.cool) mirror**, with a user-selectable
source and automatic fallback. Referenced design: DBX (`t8y2/dbx`), which ships the same idea on
Tauri's updater.

## 1. Why the current check fails (issue #61)

`crates/rustgrid-app/src/app/update.rs` drives everything through
`self_update::backends::github::Update`, which:

- lists releases with `GET https://api.github.com/repos/yisier1/rustgrid/releases?per_page=100`
  (unauthenticated: **60 requests/hour per egress IP**, shared behind NAT/CGNAT/proxies), and
- reads the changelog through `GET https://api.github.com/repos/yisier1/rustgrid/contents/<file>`
  (`fetch_repo_file`, second quota consumer).

GitHub answers 403 + "rate limit exceeded" once the shared quota is gone — exactly the screenshot in
the issue. `self_update` surfaces it as `RateLimited` and deliberately does not retry. Two more
problems compound it: GitHub's API is slow/unreliable from mainland China, and the archive download
itself goes to `objects.githubusercontent.com`.

Nothing can be fixed inside already-installed 0.1.13–0.1.18 binaries; the fix has to land in a new
release that stops using the REST API at all.

## 2. What DBX does (verified from its source)

- The updater reads a **static `latest.json`** (Tauri schema) that is attached as a release asset;
  endpoints are `https://dl.dbxio.com/releases/latest/latest.json` then
  `https://github.com/t8y2/dbx/releases/latest/download/latest.json`.
- The whole repo **and tags** are mirrored to CNB with a token
  (`https://cnb:<token>@cnb.cool/dbxio.com/dbx.git`, `git push --all/--tags --force`).
- `.github/scripts/sync-cnb-release.mjs` mirrors each GitHub release to CNB through the Open API:
  create/update the release, request an asset upload URL, `PUT` the bytes to object storage,
  `POST` the verify URL, optionally prune stale assets.
- The CNB-hosted `latest.json` **still contains GitHub asset URLs**; the app rewrites
  `https://github.com/t8y2/dbx/releases/download/<tag>/<file>` to
  `https://cnb.cool/dbxio.com/dbx/-/releases/download/<tag>/<file>` and keeps the official URL as a
  fallback candidate per download.
- The update source (官方 / CNB / AtomGit) is a persisted setting; the check endpoint becomes
  `https://cnb.cool/dbxio.com/dbx/-/releases/download/<tag>/latest.json` when CNB is chosen, and
  the download candidates are tried in order (CNB → own CDN → GitHub).
- DBX's LICENSE is Apache-2.0, so the sync script can be adapted with attribution.

Facts confirmed against the live platform (no auth required for public repos):

- `https://cnb.cool/<repo>/-/releases/latest/download/<asset>` resolves to the **latest** release's
  asset (stable URL; verified with DBX's `latest.json`, no redirect, `Content-Length` present).
- `https://api.cnb.cool/<repo>/-/releases/...` requires `Authorization: Bearer <token>` (401
  without) and `Accept: application/vnd.cnb.api+json`. **The app must never call it** — only CI does.
- The repo `yisier1/rustgrid` exists on CNB (public, created 2026-11-28) and is empty until the
  first mirror run pushes the branches and tags.

## 3. Design

### 3.1 Static manifests instead of the API

`self_update 1.3` (already in the tree) has a service-agnostic **`manifest` backend**: it fetches a
JSON document, parses releases, compares semver, selects the asset for `PLATFORM`/`BIN_NAME`,
downloads with progress, verifies `sha256` digests and runs the same extract/install pipeline —
including macOS **bundle mode** (`bundle_path_in_archive`). No API, no quota, one small HTTPS GET.

Add the features in the root `Cargo.toml`:

```toml
self_update = { version = "1.3", default-features = false, features = [
    "manifest", "checksums",   # new
    "ureq", "rustls", "native-certs",
    "archive-zip", "compression-zip-deflate",
    "archive-tar", "compression-tar-gz",
] }
```

(The `github` feature can stay as the last-resort fallback for releases that predate manifests.)

### 3.2 Where the manifests live

Two stable URLs, same JSON, one per source:

```rust
const GITHUB_MANIFEST: &str =
    "https://github.com/yisier1/rustgrid/releases/latest/download/latest.json";
const CNB_MANIFEST: &str =
    "https://cnb.cool/yisier1/rustgrid/-/releases/latest/download/latest.json";
```

`releases/latest/download/<asset>` is not the REST API: it is a plain redirect to the asset of the
newest non-draft release, with no rate limit. `latest.json` is attached to every release that has
the new workflow, so the URL is permanent.

Manifest body (`schema: 1`, exactly what `self_update` parses):

```json
{
  "schema": 1,
  "releases": [
    {
      "version": "0.1.19",
      "date": "2026-11-28T12:00:00Z",
      "assets": [
        {
          "name": "RustGrid-0.1.19-windows-x64.zip",
          "url": "https://github.com/yisier1/rustgrid/releases/download/v0.1.19/RustGrid-0.1.19-windows-x64.zip",
          "digest": "sha256:2cf24dba5fb0a30e26e83b2ac5b9e29e1b161e5c1fa7425e73043362938b9824"
        }
      ]
    }
  ]
}
```

- `version` is bare semver (no `v`), compared against `CARGO_PKG_VERSION`.
- Asset `name` must keep containing `RustGrid` and the `PLATFORM` tag
  (`windows-x64`, `macos-arm64`, …) — unchanged from today.
- `digest` makes a corrupted or tampered mirror fail before installing (`.verify_release_digest`).
- The **GitHub copy** points at GitHub asset URLs; the **CNB copy** is generated by rewriting the
  URL prefix to `https://cnb.cool/yisier1/rustgrid/-/releases/download/`. Writing the rewrite at
  release time (instead of in the app, as DBX does) keeps the client free of mirror-specific URL
  logic and lets each source's manifest be validated on its own.

### 3.3 Source selection and fallback

Implemented as two explicit sources in `crates/rustgrid-config/src/lib.rs` (`"github"` / `"cnb"`
in `settings.json`, default GitHub) and reordered into a try-list in `update.rs`:

```rust
pub enum UpdateSource {
    #[default]
    Github,
    Cnb,
}

fn manifest_urls(source: UpdateSource) -> [&'static str; 2] {
    match source {
        UpdateSource::Github => [GITHUB_MANIFEST, CNB_MANIFEST],
        UpdateSource::Cnb => [CNB_MANIFEST, GITHUB_MANIFEST],
    }
}
```

- **The chosen source is tried first, the other one second.** Both serve the same manifest, so a
  mirror outage, a missed upload or a blocked host never hides an update, and the choice cannot
  brick updates.
- Last resort only: if both manifests are missing (releases published before this change, or a
  manually created release), fall back to today's `github` backend. That path can still be rate
  limited, which is acceptable because it is never the normal path again.
- Check timeout is short (`10 s` per manifest); the install builder keeps a normal timeout because
  archives are tens of MB. `self_update`'s `RateLimited` is not retried; other transport errors
  continue to the next source.
- The install runs the same list from the chosen source onward, so a check from CNB downloads from
  CNB.

### 3.4 Release notes without the API

`fetch_repo_file(file, source)` reads the notes from plain files instead of `api.github.com`, in
the same fallback order (the contents API stays as the last resort):

- CNB: `https://cnb.cool/yisier1/rustgrid/-/git/raw/main/CHANGELOG.md` (and `CHANGELOG_zh.md`,
  `CHANGELOG_ru.md`; `Sync to CNB` mirrors the branches, so no extra release assets are needed).
- GitHub: `https://raw.githubusercontent.com/yisier1/rustgrid/main/CHANGELOG.md`.

`changelog::version_notes(version, language, fetch)` stays as it is; only the reader changes. This
removes the second REST call and keeps 选项 → 关于's 更新日志 working offline from CNB.

### 3.5 Code changes (file by file)

Implemented as described below.

`crates/rustgrid-app/src/app/update.rs`

1. `UpdateSource`, the manifest URL constants and `manifest_urls` / `changelog_urls`.
2. `check_latest(source, language)` tries each manifest:
   ```rust
   for url in manifest_urls(source) {
       let updater = Update::configure()
           .manifest_url(url)
           .bin_name(BIN_NAME)
           .target(PLATFORM)
           .current_version(APP_VERSION)
           .timeout(UPDATE_SOURCE_TIMEOUT)
           .build()?;
       let releases = updater.get_latest_release()?;
       if !releases.is_update_available()? { return Ok(None); }
       let release = releases.latest().cloned().expect("update is available");
       return Ok(Some((release.version().to_string(), release_notes(&release, language, source))));
   }
   ```
   with `github::Update` (`check_github_api`) as the final fallback and error aggregation.
3. `install_latest(source, progress)` tries `manifest::Update` per URL:
   ```rust
   builder.manifest_url(url)
       .bin_name(BIN_NAME).target(PLATFORM)
       .current_version(APP_VERSION)
       .verify_release_digest(true)
       .no_confirm(true).show_output(false)
       .show_download_progress(false)
       .progress_callback(..);
   configure_macos_layout_manifest(&mut builder);   // same bundle/bare-exec logic
   ```
   then `install_github_api` as the last resort. A download failure happens before the running
   binary is replaced, so trying the next source is safe.
4. `fetch_repo_file(file, source)` reads the raw repository files above, then the other source,
   then the contents API.
5. `UpdateStatus`, the progress channel, the titlebar button and the 关于 layout are unchanged.

`crates/rustgrid-config/src/lib.rs`

- `UpdateSource` (`Github` / `Cnb`, `"github"` / `"cnb"` in JSON).
- `AppSettings::update_source` with `#[serde(default)]` (old files read as GitHub), included in
  `load_settings`/`save_settings` and `Default`.
- `SETTINGS_VERSION` 18 → 19; no data rewrite, the default covers old files.

`crates/rustgrid-app/src/app/options.rs`

- The 关于 page's 更新源 dropdown is an entity on `AppView` (`update_source_combo`), built on the
  first Options open; the value is **staged** (`options_update_source`) like every other Options
  setting and applied by `apply_options` on 确定, so 取消 discards it.
- `options_about_body` (in `update.rs`) renders it under 检查更新: 更新源 + GitHub / CNB.

Locale keys (all three files: `en.yml`, `zh-CN.yml`, `ru.yml`): `about.update_source`.

### 3.6 CI: generate the manifest, mirror to CNB

Implemented in this repository:

- `tools/release/make-manifest.mjs` — writes the schema-1 manifest from an asset directory
  (sha256 per archive, GitHub or CNB URLs via `--base-url`). Node only, no extra action.
- `.github/workflows/release.yml` — the `release` job runs it into `dist/latest.json`, the **only**
  extra asset beside the six archives (the changelogs stay in the repository, so the release page
  remains a download list); then `softprops/action-gh-release` uploads `dist/*` as before.
- `.github/workflows/sync-cnb.yml` — a reusable workflow (also `workflow_dispatch`-able with a tag)
  doing the mirror:
  1. mirror branches and tags (`git push cnb` with a token URL, branches fetched into
     `refs/mirror/branches/*` first so a detached tag checkout still pushes them);
  2. `gh release download "$TAG"` for exactly the published assets, then copies the three changelogs
     into the upload directory — CNB-only assets kept for builds from before the raw-file reader
     (0.1.19 and older), whose CNB source looks for `CHANGELOG*.md` at the release's download
     prefix; the GitHub release never carries them;
  3. re-mint the manifest with
     `--base-url https://cnb.cool/yisier1/rustgrid/-/releases/download/<tag>/` (same digests);
  4. `node .github/scripts/sync-cnb-release.mjs` creates/updates the CNB release with
     `make_latest: "true"` and uploads every asset — `latest.json` **last**, so the `latest` alias
     never points at a release whose manifest references files that are still uploading;
  5. `curl`s the mirrored `latest.json` and prints its version and asset count.
- `.github/workflows/release.yml`'s `mirror-cnb` job (`needs: release`) calls the workflow for the
  pushed tag. GitHub's `continue-on-error` is **not supported on `uses:` jobs**, so a mirror failure
  turns that job red while the GitHub release stays published; re-run `Sync to CNB` from the Actions
  tab with the tag to repair it.

CNB Open API calls used by the script (all verified against `https://api.cnb.cool/swagger.json`):

| Step | Request |
| --- | --- |
| find release | `GET /{repo}/-/releases/tags/{tag}` |
| create release | `POST /{repo}/-/releases` `{tag_name,name,body,prerelease,make_latest:"true"}` |
| update release | `PATCH /{repo}/-/releases/{id}` |
| upload slot | `POST /{repo}/-/releases/{id}/asset-upload-url` `{asset_name,size,overwrite}` → `{upload_url,verify_url}` |
| upload bytes | `PUT <upload_url>` (`application/octet-stream`) |
| confirm | `POST <verify_url>` |
| prune stale | `DELETE /{repo}/-/releases/{id}/assets/{asset_id}` |

Headers: `Accept: application/vnd.cnb.api+json`, `Authorization: Bearer ${CNB_TOKEN}`.

Required repository setup (one-off, by the maintainer):

- the public repo `yisier1/rustgrid` on cnb.cool (already created),
- a CNB access token: 个人设置 → 访问令牌 (`https://cnb.cool/profile/token`) → 添加访问令牌, with
  the write scopes for this repository (代码仓库读写, which also covers releases) →
  GitHub secret `CNB_TOKEN`,
- optional repository variable `CNB_REPOSITORY` (default `yisier1/rustgrid`).

Back-fill an existing release (or retry a failed mirror): Actions → **Sync to CNB** → Run workflow
with the tag, e.g. `v0.1.18`. The Git login is `cnb` as the user name and the token as the password;
the same token is the `Bearer` value for the Open API.

## 4. Edge cases and mitigations

| Risk | Mitigation |
| --- | --- |
| CNB `latest` alias lags behind GitHub | Release is created with `make_latest: "true"`; per-release manifests are versioned, and picking GitHub (or its fallback) always shows the update |
| Mirror upload fails halfway | Job uploads `latest.json` last; clients fall back to the GitHub manifest, which always has every asset |
| CNB unavailable / blocked outside China | The chosen source is tried first and the other second after one short timeout, so GitHub still answers |
| Manifest missing on old releases | Final fallback to the current `github` backend (rate limited, but only in this case) |
| Check/install version drift (release published between the two steps) | Install compares `status.version()` with the version the check stored and reports a mismatch |
| Corrupted mirror asset | `sha256` digest in the manifest, `verify_release_digest(true)`, `checksums` feature |
| macOS bundle install | The manifest backend runs the same install pipeline; `configure_macos_layout` (bundle vs bare exe) is applied to the manifest builder unchanged |
| Token leakage | `CNB_TOKEN` is only a GitHub Actions secret; the app never authenticates against CNB |
| DBX code reuse | Apache-2.0; keep the copyright/attribution header in the adapted script |
| Already-installed 0.1.13–0.1.18 | Cannot be fixed in place; they need one successful manual download (CNB mirror link added to the release body/README) to reach the first manifest-based release |

## 5. Test plan

- Unit (`update.rs`): manifest JSON built by the CI script parses via
  `self_update::backends::manifest::parse_manifest`, digest fields survive, the chosen source's URL
  comes first from `manifest_urls`, and the notes reader maps a source to the expected URL.
- Ignored network test (`#[ignore]`): fetch both stable manifests, assert the version equals the
  newest tag and that a `RustGrid-<version>-<PLATFORM>.*` asset exists with a `sha256:` digest.
- Manual: 关于 → 检查更新 with `https_proxy` pointing at a dead endpoint for one source, confirm the
  other source still answers; install once from each source; macOS bundle and bare installs both
  stay intact.
- CI: `workflow_dispatch` the mirror job with an existing tag, then `curl` the stable CNB
  `latest.json`, the mirrored `CHANGELOG.md` raw file and one archive; verify `Content-Length` and
  digest.
- `cargo fmt --all`, `cargo build`, `cargo test --workspace`, `cargo clippy --workspace --all-targets
  -- -D warnings` before landing (per AGENTS.md).

## 6. Rollout

1. Repo exists at `https://cnb.cool/yisier1/rustgrid`; create the access token
   (`https://cnb.cool/profile/token`) and add it as the GitHub secret `CNB_TOKEN`
   (Settings → Secrets and variables → Actions → New repository secret).
2. The app and CI changes are in place; the first release after them writes `latest.json` and
   mirrors to CNB automatically (the maintainer decides the version; both changelogs get their
   section in the same commit).
3. Until the first manifest release exists, back-fill the current release by hand:
   Actions → **Sync to CNB** → Run workflow with `v0.1.18` (this mirrors the branches, tags and the
   0.1.18 archives, and mints a manifest for it). Builds that predate the manifest code still use the
   REST API, so they are unaffected by the mirror until they update once.
4. Reply to issue #61 with the CNB download links and a note that the next release no longer hits
   GitHub's REST API.
5. Follow-up (not in this change): add AtomGit/Gitee as extra `UpdateSource` values (the abstraction
   is source-agnostic), a beta/prerelease channel, and an optional `RUSTGRID_UPDATE_SOURCE`
   environment override for testing.

The Rust/settings/UI changes and the release workflow/mirror scripts in this document are
implemented; the remaining work is the one-off token configuration above and the release itself.
