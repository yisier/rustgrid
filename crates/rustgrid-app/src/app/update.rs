//! Update detection and self-upgrade from a static release manifest served by GitHub Releases or
//! its CNB mirror (see `docs/update-sources.md` and `.github/workflows/release.yml`).
//!
//! [`self_update`]'s **manifest** backend drives the whole flow: read `latest.json`, compare its
//! semver against this build's `CARGO_PKG_VERSION`, download the portable archive for the running
//! target, verify its sha256 and replace the running executable in place. 选项 → 关于 picks the
//! source (更新源: GitHub / CNB); the chosen host is tried first and the other one second, so a
//! mirror outage never hides an update. GitHub's rate-limited REST API is only a fallback for
//! releases published before the manifest existed. Detection runs once shortly after launch; a newer
//! release surfaces an update button in the titlebar, and 关于 offers the same check plus a manual
//! install.
//!
//! The asset names produced by the release workflow must contain the running platform tag (see
//! [`PLATFORM`]), otherwise `self_update` cannot pick the right download. The macOS archives carry an
//! app bundle rather than a bare executable, so installing one picks between `self_update`'s two
//! layouts — see [`configure_macos_layout`].

use super::options::{section_label, setting_row, settings_card};
use super::*;
use gpui_kit::base::SelectableText;
use std::io::Read as _;
use std::time::Duration;

/// The GitHub repository whose Releases are checked.
const REPO_OWNER: &str = "yisier";
const REPO_NAME: &str = "rustgrid";

/// The CNB mirror of the release page (see `docs/update-sources.md`).
const CNB_REPO: &str = "yisier1/rustgrid";

/// Stable manifest URLs. `releases/latest/download/<file>` is a plain redirect to the newest
/// release's asset — not the REST API, so checking never consumes GitHub's 60/hour quota and both
/// sources can be tried in any order.
const GITHUB_MANIFEST: &str =
    "https://github.com/yisier/rustgrid/releases/latest/download/latest.json";
const CNB_MANIFEST: &str =
    "https://cnb.cool/yisier1/rustgrid/-/releases/latest/download/latest.json";

/// How long a manifest fetch may take before the next source is tried.
const UPDATE_SOURCE_TIMEOUT: Duration = Duration::from_secs(10);

/// The manifest URLs to try: the chosen source first, the other one as a fallback, so a mirror
/// outage or a missed upload never hides an update.
fn manifest_urls(source: UpdateSource) -> [&'static str; 2] {
    match source {
        UpdateSource::Github => [GITHUB_MANIFEST, CNB_MANIFEST],
        UpdateSource::Cnb => [CNB_MANIFEST, GITHUB_MANIFEST],
    }
}

/// The changelog URLs for one file, in the same order as [`manifest_urls`]. Both are plain file
/// reads of the repository's default branch (`Sync to CNB` mirrors the branches and tags), so the
/// notes need no extra release assets.
fn changelog_urls(source: UpdateSource, file: &str) -> [String; 2] {
    let github = format!("https://raw.githubusercontent.com/{REPO_OWNER}/{REPO_NAME}/main/{file}");
    let cnb = format!("https://cnb.cool/{CNB_REPO}/-/git/raw/main/{file}");
    match source {
        UpdateSource::Github => [github, cnb],
        UpdateSource::Cnb => [cnb, github],
    }
}

/// The executable's base name. `self_update` appends the platform suffix (`.exe` on Windows) and
/// uses the result both to select the asset and to find the binary inside the archive.
const BIN_NAME: &str = "RustGrid";

/// The macOS app bundle inside the macOS archives (see `packaging/macos`).
#[cfg(target_os = "macos")]
const APP_BUNDLE: &str = "RustGrid.app";

/// This build's version, from the workspace `Cargo.toml`. It is compared against the release tag
/// (with its leading `v` stripped) to decide whether an update exists.
pub(super) const APP_VERSION: &str = env!("CARGO_PKG_VERSION");

/// The platform tag this build downloads. It is a plain substring of the asset names minted by
/// `.github/workflows/release.yml` (e.g. `RustGrid-0.0.1-windows-x64.zip`); `self_update` matches
/// assets by substring, so any stable tag works — it need not be a Rust target triple.
pub(super) const PLATFORM: &str = if cfg!(target_os = "windows") {
    if cfg!(target_arch = "x86_64") {
        "windows-x64"
    } else {
        "windows-arm64"
    }
} else if cfg!(target_os = "macos") {
    if cfg!(target_arch = "x86_64") {
        "macos-x64"
    } else {
        "macos-arm64"
    }
} else if cfg!(target_arch = "x86_64") {
    "linux-x64"
} else {
    "linux-arm64"
};

/// macOS only: point the updater at the right member of the macOS archive.
///
/// macOS releases ship `RustGrid.app` (see `packaging/macos`), while the Windows/Linux archives hold
/// the bare executable at their root. Running from inside a bundle, the **whole bundle** is swapped
/// through `self_update`'s bundle mode, so the Info.plist and the icon are replaced together with
/// the binary instead of going stale against it. An install that predates the bundle — a bare
/// `RustGrid` file, i.e. anyone updating from an older release layout — has no `.app` to swap, so it
/// extracts the executable from inside the archive instead; the update still lands, the install just
/// stays bundle-less.
#[cfg(target_os = "macos")]
fn configure_macos_layout(builder: &mut self_update::backends::github::UpdateBuilder) {
    if running_from_app_bundle() {
        builder.bundle_path_in_archive(APP_BUNDLE);
    } else {
        builder.bin_path_in_archive(format!("{APP_BUNDLE}/Contents/MacOS/{BIN_NAME}"));
    }
}

/// [`configure_macos_layout`] for the manifest backend, which installs the same way.
#[cfg(target_os = "macos")]
fn configure_macos_layout_manifest(builder: &mut self_update::backends::manifest::UpdateBuilder) {
    if running_from_app_bundle() {
        builder.bundle_path_in_archive(APP_BUNDLE);
    } else {
        builder.bin_path_in_archive(format!("{APP_BUNDLE}/Contents/MacOS/{BIN_NAME}"));
    }
}

/// Whether this executable lives inside a `.app` bundle. Bundle mode resolves its install path from
/// the nearest `.app` ancestor of `current_exe()`, so this is exactly the condition under which it
/// can be used.
#[cfg(target_os = "macos")]
fn running_from_app_bundle() -> bool {
    let Ok(exe) = std::env::current_exe() else {
        return false;
    };
    exe.ancestors().any(|path| {
        path.extension()
            .is_some_and(|extension| extension.eq_ignore_ascii_case("app"))
    })
}

/// The state of the update check / install, driving the titlebar button and the 关于 page.
#[derive(Clone, Default)]
pub(super) enum UpdateStatus {
    /// No check has run yet.
    #[default]
    Idle,
    /// A check is in flight.
    Checking,
    /// The running version is the newest release.
    UpToDate,
    /// A newer release exists; its version is in `AppView::update_version`.
    Available,
    /// The newer release is downloading / being installed.
    Downloading,
    /// Installed; the process is about to relaunch.
    Installed,
    /// The check or install failed (the message is shown in 关于).
    Failed(String),
}

impl UpdateStatus {
    pub(super) fn is_available(&self) -> bool {
        matches!(self, UpdateStatus::Available)
    }

    /// Whether a check or install is running, so a second action must be ignored.
    pub(super) fn is_busy(&self) -> bool {
        matches!(self, UpdateStatus::Checking | UpdateStatus::Downloading)
    }
}

/// Blocking: ask the chosen source for the newest release and return its version and notes when it
/// is strictly newer than the running build. Runs on the tokio blocking pool via
/// `Runtime::spawn_blocking`.
///
/// The sources serve the same static `latest.json` manifest (see [`manifest_urls`]); the REST API
/// path only covers releases published before the manifest existed, because it is rate limited.
fn check_latest(
    source: UpdateSource,
    language: LanguageSetting,
) -> Result<Option<(String, Option<String>)>, String> {
    let mut errors = Vec::new();
    for url in manifest_urls(source) {
        match check_manifest(url, source, language) {
            Ok(found) => return Ok(found),
            Err(error) => errors.push(error),
        }
    }
    match check_github_api(language) {
        Ok(found) => Ok(found),
        Err(error) => {
            errors.push(error);
            Err(errors.join("; "))
        }
    }
}

/// One source's manifest: the newest release as it should be shown, or `None` when this build is
/// current.
fn check_manifest(
    url: &str,
    source: UpdateSource,
    language: LanguageSetting,
) -> Result<Option<(String, Option<String>)>, String> {
    let updater = self_update::backends::manifest::Update::configure()
        .manifest_url(url)
        .bin_name(BIN_NAME)
        .target(PLATFORM)
        .current_version(APP_VERSION)
        .timeout(UPDATE_SOURCE_TIMEOUT)
        .build()
        .map_err(|error| format!("{url}: {error}"))?;
    let releases = updater
        .get_latest_release()
        .map_err(|error| format!("{url}: {error}"))?;
    if !releases
        .is_update_available()
        .map_err(|error| error.to_string())?
    {
        return Ok(None);
    }
    let release = releases
        .latest()
        .cloned()
        .ok_or_else(|| format!("{url}: the manifest lists no newer release"))?;
    let version = release.version().to_string();
    // The notes are read from the changelog for the UI's language (see `release_notes`), so 关于 can
    // show what installing would bring without a second check.
    let notes = release_notes(&release, language, source);
    Ok(Some((version, notes)))
}

/// The REST fallback for releases published before `latest.json` existed. Unauthenticated GitHub
/// allows ~60 requests/hour per egress IP, so this is only reached when both manifests are missing.
fn check_github_api(language: LanguageSetting) -> Result<Option<(String, Option<String>)>, String> {
    let updater = self_update::backends::github::Update::configure()
        .repo_owner(REPO_OWNER)
        .repo_name(REPO_NAME)
        .bin_name(BIN_NAME)
        .target(PLATFORM)
        .current_version(APP_VERSION)
        .build()
        .map_err(|error| error.to_string())?;
    let release = updater
        .is_update_available()
        .map_err(|error| error.to_string())?;
    Ok(release.map(|release| {
        let version = release.version().to_string();
        let notes = release_notes(&release, language, UpdateSource::Github);
        (version, notes)
    }))
}

/// The newer release's notes as 关于 should show them: the release's section out of the repository
/// changelog for the UI's language, falling back to English when that language has no changelog of
/// its own (see [`changelog::version_notes`]).
///
/// The release body itself is not a changelog any more — the release workflow publishes links to
/// the per-language files (see `.github/workflows/release.yml`), which keeps the body small as
/// languages are added. A hand-written release body (an older release, or a repository fetch that
/// failed) is still shown; the workflow's link-only body is not.
fn release_notes(
    release: &self_update::Release,
    language: LanguageSetting,
    source: UpdateSource,
) -> Option<String> {
    release_notes_with(release, language, |file| {
        // A lagging mirror serves its previous release's changelog, which does not document this
        // version yet; require the section so the reader falls through to the other source instead.
        fetch_repo_file_for(file, source, Some(release.version()))
    })
}

/// [`release_notes`] with the file reader injected, so the body fallback can be tested offline.
fn release_notes_with(
    release: &self_update::Release,
    language: LanguageSetting,
    fetch: impl Fn(&str) -> Option<String>,
) -> Option<String> {
    changelog::version_notes(release.version(), language, fetch).or_else(|| {
        release
            .body()
            .map(str::trim)
            .filter(|body| !body.is_empty() && !changelog::is_changelog_links(body))
            .map(str::to_string)
    })
}

/// Blocking: a file's text from the chosen source, then the other source, then GitHub's contents
/// API as a last resort.
///
/// Both sources serve the changelogs as plain repository files, so the normal paths consume no API
/// quota and need no release assets. Only failures that leave the panel without notes return
/// `None`.
pub(super) fn fetch_repo_file(file: &str, source: UpdateSource) -> Option<String> {
    fetch_repo_file_for(file, source, None)
}

/// [`fetch_repo_file`] with an optional version the file must document; a mirror that lags one
/// release still serves the previous changelog, so the version check moves on to the next source.
fn fetch_repo_file_for(file: &str, source: UpdateSource, version: Option<&str>) -> Option<String> {
    for url in changelog_urls(source, file) {
        let Some(text) = fetch_text(&url).filter(|text| !text.trim().is_empty()) else {
            continue;
        };
        if version.is_some_and(|version| changelog::version_section(&text, version).is_none()) {
            continue;
        }
        return Some(text);
    }
    fetch_contents_api(file)
}

/// A plain text file over HTTPS, with the User-Agent GitHub requires.
fn fetch_text(url: &str) -> Option<String> {
    use self_update::http_client::{HttpClient, UreqClient, header};
    let mut headers = self_update::http_client::HeaderMap::new();
    headers.insert(
        header::USER_AGENT,
        format!("{BIN_NAME}/{APP_VERSION}").parse().ok()?,
    );
    let client = UreqClient::default();
    let response = client
        .get(url, &headers, Some(Duration::from_secs(20)))
        .ok()?;
    let mut text = String::new();
    response.body().read_to_string(&mut text).ok()?;
    Some(text)
}

/// The pre-manifest release path: a file's text through GitHub's contents API.
///
/// `Accept: application/vnd.github.raw` makes the endpoint answer with the file itself rather than
/// its base64 JSON envelope, and leaving `?ref` off resolves to the repository's default branch,
/// whose changelog documents every released version — including one released before the tag carried
/// a changelog at all. Unauthenticated requests share a ~60/hour quota, so this is only a fallback.
fn fetch_contents_api(file: &str) -> Option<String> {
    use self_update::http_client::{HttpClient, UreqClient, header};
    let url = format!("https://api.github.com/repos/{REPO_OWNER}/{REPO_NAME}/contents/{file}");
    // GitHub rejects a request without a User-Agent, and the media type must be asked for by name.
    let mut headers = self_update::http_client::HeaderMap::new();
    headers.insert(
        header::USER_AGENT,
        format!("{BIN_NAME}/{APP_VERSION}").parse().ok()?,
    );
    headers.insert(header::ACCEPT, "application/vnd.github.raw".parse().ok()?);
    let client = UreqClient::default();
    let response = client
        .get(&url, &headers, Some(Duration::from_secs(20)))
        .ok()?;
    let mut text = String::new();
    response.body().read_to_string(&mut text).ok()?;
    Some(text)
}

/// Blocking: download and install the newest release, returning the installed version. The caller
/// relaunches the process afterwards.
///
/// Every chunk the transport reads is forwarded over `progress` as
/// `(bytes_downloaded_so_far, total_bytes)` so the UI can show a live progress bar; `total_bytes`
/// is `None` when the server sends no `Content-Length` (the bar then runs indeterminate). The
/// callback is `Fn` and the sender is `Send + Sync`, so it can be moved into `self_update`.
fn install_latest(
    source: UpdateSource,
    progress: tokio::sync::mpsc::UnboundedSender<(u64, Option<u64>)>,
) -> Result<String, String> {
    let mut errors = Vec::new();
    for url in manifest_urls(source) {
        match install_manifest(url, progress.clone()) {
            Ok(version) => return Ok(version),
            // A failed attempt stops before the running binary is replaced, so the next source is
            // safe to try: this is how a mirror with a missing or half-uploaded asset falls back to
            // the canonical GitHub release.
            Err(error) => errors.push(format!("{url}: {error}")),
        }
    }
    match install_github_api(progress) {
        Ok(version) => Ok(version),
        Err(error) => {
            errors.push(error);
            Err(errors.join("; "))
        }
    }
}

/// Install the newest release from one static manifest.
fn install_manifest(
    url: &str,
    progress: tokio::sync::mpsc::UnboundedSender<(u64, Option<u64>)>,
) -> Result<String, String> {
    let mut builder = self_update::backends::manifest::Update::configure();
    builder
        .manifest_url(url)
        .bin_name(BIN_NAME)
        .target(PLATFORM)
        .current_version(APP_VERSION)
        // The manifest carries a sha256 per archive: a truncated mirror download fails here instead
        // of being installed.
        .verify_release_digest(true)
        // Non-interactive: never print to stdio or block on stdin (this is a GUI process).
        .no_confirm(true)
        .show_output(false)
        .show_download_progress(false)
        .progress_callback(move |downloaded, total| {
            // A closed receiver (the About window / app is gone) just drops the sample.
            let _ = progress.send((downloaded, total));
        });
    // Only the install path cares about the archive's shape: detection just matches the asset name.
    #[cfg(target_os = "macos")]
    configure_macos_layout_manifest(&mut builder);

    let updater = builder.build().map_err(|error| error.to_string())?;
    let status = updater.update().map_err(|error| error.to_string())?;
    Ok(status.version().to_string())
}

/// The pre-manifest install path: GitHub Releases through the REST API. Only reached when neither
/// manifest exists, because the API is rate limited (~60 requests/hour per egress IP).
fn install_github_api(
    progress: tokio::sync::mpsc::UnboundedSender<(u64, Option<u64>)>,
) -> Result<String, String> {
    let mut builder = self_update::backends::github::Update::configure();
    builder
        .repo_owner(REPO_OWNER)
        .repo_name(REPO_NAME)
        .bin_name(BIN_NAME)
        .target(PLATFORM)
        .current_version(APP_VERSION)
        .no_confirm(true)
        .show_output(false)
        .show_download_progress(false)
        .progress_callback(move |downloaded, total| {
            let _ = progress.send((downloaded, total));
        });
    #[cfg(target_os = "macos")]
    configure_macos_layout(&mut builder);

    let updater = builder.build().map_err(|error| error.to_string())?;
    let status = updater.update().map_err(|error| error.to_string())?;
    Ok(status.version().to_string())
}

/// Human-readable byte count for the download progress caption (e.g. `12.3 MB`).
pub(super) fn format_bytes(bytes: u64) -> String {
    const UNITS: [&str; 4] = ["B", "KB", "MB", "GB"];
    let mut value = bytes as f64;
    let mut unit = 0;
    while value >= 1024.0 && unit < UNITS.len() - 1 {
        value /= 1024.0;
        unit += 1;
    }
    if unit == 0 {
        format!("{bytes} B")
    } else {
        format!("{value:.1} {}", UNITS[unit])
    }
}

/// The download progress as a 0–100 percentage, when the server sent a positive `Content-Length`.
pub(super) fn progress_percent(downloaded: u64, total: Option<u64>) -> Option<u32> {
    match total {
        Some(total) if total > 0 => {
            Some(((downloaded.min(total) as f64 / total as f64) * 100.0).round() as u32)
        }
        _ => None,
    }
}

/// Relaunch the freshly replaced executable and exit this process. The child is marked with
/// `RUSTGRID_RESTARTED`, which makes the single-instance guard wait (see `single_instance`) for this
/// process to exit and release its lock before it decides another copy is running.
fn relaunch() {
    if let Ok(exe) = std::env::current_exe() {
        let _ = std::process::Command::new(exe)
            .env("RUSTGRID_RESTARTED", "1")
            .spawn();
    }
    std::process::exit(0);
}

impl AppView {
    /// Start a background update check. Deliberately does not notify: the startup call runs during
    /// `render` (where a frame is already in progress) and the completion callback notifies.
    ///
    /// `manual` (the 关于 button) reports failures in the UI; the quiet startup check swallows them
    /// (a missing network or a repository with no releases yet must not raise a red flag on launch).
    pub(super) fn check_for_updates(&mut self, manual: bool, cx: &mut Context<'_, Self>) {
        if self.update_status.is_busy() {
            return;
        }
        self.update_status = UpdateStatus::Checking;
        let runtime = self.runtime.clone();
        let language = self.language;
        let source = self.update_source;
        cx.spawn(async move |this, cx| {
            let result = runtime
                .spawn_blocking(move || check_latest(source, language))
                .await;
            let _ = this.update(cx, move |app, cx| {
                match result {
                    Ok(Ok(Some((version, notes)))) => {
                        app.update_version = Some(version);
                        app.update_notes = notes;
                        app.update_status = UpdateStatus::Available;
                    }
                    Ok(Ok(None)) => {
                        app.update_version = None;
                        app.update_notes = None;
                        app.update_status = UpdateStatus::UpToDate;
                    }
                    Ok(Err(error)) => {
                        app.fail_update_check(manual, error);
                    }
                    Err(error) => {
                        app.fail_update_check(manual, error.to_string());
                    }
                }
                cx.notify();
            });
        })
        .detach();
    }

    /// Re-read an available release's notes in the UI's new language after a language change.
    ///
    /// The notes are fetched in the language they are shown in (see [`changelog::version_notes`]),
    /// so keeping the previous language's text would leave 选项 → 关于 half-translated. Nothing
    /// happens without an available release, and a failed refetch keeps the old notes rather than
    /// blanking a panel that was already showing something.
    pub(super) fn refresh_update_notes(&mut self, cx: &mut Context<'_, Self>) {
        let Some(version) = self.update_version.clone() else {
            return;
        };
        let language = self.language;
        let source = self.update_source;
        let runtime = self.runtime.clone();
        let fetch_version = version.clone();
        cx.spawn(async move |this, cx| {
            let fetched = runtime
                .spawn_blocking(move || {
                    changelog::version_notes(&fetch_version, language, |file| {
                        fetch_repo_file(file, source)
                    })
                })
                .await;
            let _ = this.update(cx, move |app, cx| {
                if app.language != language
                    || app.update_version.as_deref() != Some(version.as_str())
                {
                    return;
                }
                if let Ok(Some(notes)) = fetched {
                    app.update_notes = Some(notes);
                    cx.notify();
                }
            });
        })
        .detach();
    }

    /// Record a failed check: a manual check keeps the error for 关于; a quiet startup check resets
    /// to idle instead.
    fn fail_update_check(&mut self, manual: bool, error: String) {
        self.update_status = if manual {
            UpdateStatus::Failed(error)
        } else {
            UpdateStatus::Idle
        };
    }

    /// Download and install the newest release, then relaunch into it. Ignored while busy.
    ///
    /// The blocking install reports each read chunk over an unbounded channel; a second task
    /// drains it into `update_progress` so the titlebar and the 关于 page reflect the download
    /// live. The channel closes when the install finishes, which also ends the drain task.
    pub(super) fn install_update(&mut self, cx: &mut Context<'_, Self>) {
        if self.update_status.is_busy() || matches!(self.update_status, UpdateStatus::Installed) {
            return;
        }
        self.update_status = UpdateStatus::Downloading;
        self.update_progress = None;
        cx.notify();
        let runtime = self.runtime.clone();
        let source = self.update_source;
        let (progress_tx, mut progress_rx) =
            tokio::sync::mpsc::unbounded_channel::<(u64, Option<u64>)>();

        // Drain download progress into the UI while the install runs on the blocking pool.
        cx.spawn(async move |this, cx| {
            while let Some((downloaded, total)) = progress_rx.recv().await {
                let _ = this.update(cx, |app, cx| {
                    app.update_progress = Some((downloaded, total));
                    cx.notify();
                });
            }
        })
        .detach();

        cx.spawn(async move |this, cx| {
            let result = runtime
                .spawn_blocking(move || install_latest(source, progress_tx))
                .await;
            match result {
                Ok(Ok(version)) => {
                    let _ = this.update(cx, move |app, cx| {
                        app.update_version = Some(version);
                        app.update_status = UpdateStatus::Installed;
                        app.update_progress = None;
                        cx.notify();
                    });
                    // The on-disk executable has been replaced; hand over to it.
                    relaunch();
                }
                Ok(Err(error)) => {
                    let _ = this.update(cx, move |app, cx| {
                        app.update_status = UpdateStatus::Failed(error);
                        app.update_progress = None;
                        cx.notify();
                    });
                }
                Err(error) => {
                    let message = error.to_string();
                    let _ = this.update(cx, move |app, cx| {
                        app.update_status = UpdateStatus::Failed(message);
                        app.update_progress = None;
                        cx.notify();
                    });
                }
            }
        })
        .detach();
    }

    /// The human-readable status shown in 选项 → 关于.
    fn update_status_text(&self) -> String {
        match &self.update_status {
            UpdateStatus::Idle => String::new(),
            UpdateStatus::Checking => t!("about.checking").to_string(),
            UpdateStatus::UpToDate => t!("about.up_to_date").to_string(),
            UpdateStatus::Available => t!(
                "about.available",
                version = self.update_version.clone().unwrap_or_default()
            )
            .to_string(),
            UpdateStatus::Downloading => t!("about.downloading").to_string(),
            UpdateStatus::Installed => t!("about.installed").to_string(),
            UpdateStatus::Failed(error) => t!("about.failed", error = error.clone()).to_string(),
        }
    }

    /// The titlebar's update affordance, shown only when a check found a newer release (or an
    /// install is running / just finished). Returns an empty element when there is nothing to show.
    pub(super) fn titlebar_update_button(
        &self,
        theme: Theme,
        cx: &mut Context<'_, Self>,
    ) -> AnyElement {
        // `label` is `None` for the available state: the update affordance is just a compact
        // download icon, with the version available on hover.
        let (icon, label, color) = match &self.update_status {
            UpdateStatus::Available => ("icons/update.svg", None, theme.brand),
            UpdateStatus::Downloading => {
                let label = match self
                    .update_progress
                    .and_then(|(downloaded, total)| progress_percent(downloaded, total))
                {
                    Some(percent) => {
                        t!("update.downloading_percent", percent = percent).to_string()
                    }
                    None => t!("update.downloading").to_string(),
                };
                ("icons/refresh.svg", Some(label), theme.text_muted)
            }
            UpdateStatus::Installed => (
                "icons/check.svg",
                Some(t!("update.restarting").to_string()),
                theme.brand,
            ),
            UpdateStatus::Failed(_) => (
                "icons/warning_mark.svg",
                Some(t!("update.retry").to_string()),
                theme.danger,
            ),
            UpdateStatus::Idle | UpdateStatus::Checking | UpdateStatus::UpToDate => {
                return div().into_any_element();
            }
        };
        let busy = matches!(
            self.update_status,
            UpdateStatus::Downloading | UpdateStatus::Installed
        );
        let tooltip = match &self.update_version {
            Some(version) => t!("about.available", version = version.clone()).to_string(),
            None => t!("update.button").to_string(),
        };
        let mut button = div()
            .id("titlebar-update")
            .flex()
            .flex_row()
            .items_center()
            .justify_center()
            .gap_1()
            .h(px(26.0))
            .rounded_full()
            .when(label.is_some(), |style| style.px_2())
            .when(label.is_none(), |style| style.w(px(26.0)))
            .text_size(px(12.0))
            .text_color(rgb(color))
            .when(!busy, move |style| {
                style
                    .cursor_pointer()
                    .hover(move |style| style.bg(rgb(theme.button_hover_bg)))
            })
            .on_click(cx.listener(|this, _event, _window, cx| {
                if this.update_status.is_available() {
                    this.install_update(cx);
                } else if matches!(this.update_status, UpdateStatus::Failed(_)) {
                    this.check_for_updates(true, cx);
                    cx.notify();
                }
            }))
            .child(
                svg()
                    .path(icon)
                    .w(px(16.0))
                    .h(px(16.0))
                    .flex_none()
                    .text_color(rgb(color)),
            );
        if let Some(label) = label {
            button = button.child(label);
        }
        button.tooltip(ui::text_tooltip(tooltip)).into_any_element()
    }

    /// The 关于 page: app identity, current version, the manual update controls and the changelog.
    pub(super) fn options_about_body(&self, cx: &mut Context<'_, Self>) -> impl IntoElement {
        let theme = self.theme;
        let status_color = if matches!(self.update_status, UpdateStatus::Failed(_)) {
            theme.danger
        } else {
            theme.text_muted
        };

        let mut actions = div()
            .flex()
            .flex_row()
            .items_center()
            .gap_2()
            .flex_none()
            .child(self.dialog_button(
                "about-check",
                t!("about.check").to_string(),
                false,
                cx.listener(|this, _event, _window, cx| {
                    this.check_for_updates(true, cx);
                    cx.notify();
                }),
            ));
        if self.update_status.is_available() {
            actions = actions.child(self.dialog_button(
                "about-install",
                t!("about.install").to_string(),
                true,
                cx.listener(|this, _event, _window, cx| this.install_update(cx)),
            ));
        }

        // The status doubles as the row's description: error text wraps here instead of pushing the
        // buttons out, and a running download's progress bar sits under it.
        let mut status = div()
            .flex()
            .flex_col()
            .gap_1()
            .flex_1()
            .min_w(px(0.0))
            .child(
                div()
                    .text_size(px(12.0))
                    .text_color(rgb(status_color))
                    .child(self.update_status_text()),
            );
        if let Some(progress) = self.download_progress_element(theme) {
            status = status.child(div().w_full().max_w(px(360.0)).child(progress));
        }

        let source = match self.update_source_combo.clone() {
            Some(combo) => combo.into_any_element(),
            None => div().into_any_element(),
        };

        div()
            .flex()
            .flex_col()
            .gap_3()
            .w_full()
            // Identity: the logo beside the name, version and one-line description, so the page does
            // not open with a tall centred column and two empty halves.
            .child(
                settings_card(theme).child(
                    div()
                        .flex()
                        .flex_row()
                        .items_center()
                        .gap_4()
                        .w_full()
                        .px(px(20.0))
                        .py(px(18.0))
                        .child(
                            img(ImageSource::Resource(Resource::Embedded("logo.png".into())))
                                .w(px(48.0))
                                .h(px(48.0))
                                .rounded(px(12.0))
                                .flex_none(),
                        )
                        .child(
                            div()
                                .flex()
                                .flex_col()
                                .gap(px(5.0))
                                .min_w(px(0.0))
                                .child(
                                    div()
                                        .flex()
                                        .flex_row()
                                        .items_center()
                                        .gap_2()
                                        .child(
                                            div()
                                                .text_size(px(17.0))
                                                .font_weight(FontWeight::BOLD)
                                                .child(t!("app.title").to_string()),
                                        )
                                        .child(
                                            div()
                                                .px(px(7.0))
                                                .py(px(1.0))
                                                .rounded_full()
                                                .bg(rgb(theme.header_bg))
                                                .text_size(px(11.0))
                                                .text_color(rgb(theme.text_muted))
                                                .child(
                                                    t!("about.version", version = APP_VERSION)
                                                        .to_string(),
                                                ),
                                        ),
                                )
                                .child(
                                    div()
                                        .text_size(px(12.5))
                                        .text_color(rgb(theme.text_muted))
                                        .child(t!("about.description").to_string()),
                                ),
                        ),
                ),
            )
            .child(section_label(theme, t!("about.update").to_string()))
            .child(
                settings_card(theme)
                    .child(setting_row(
                        "about-source",
                        theme,
                        t!("about.update_source").to_string(),
                        None,
                        source,
                        true,
                    ))
                    .child(
                        div()
                            .flex()
                            .flex_row()
                            .items_center()
                            .gap_3()
                            .w_full()
                            .px(px(14.0))
                            .py(px(11.0))
                            .child(status)
                            .child(actions),
                    ),
            )
            // The newer release's changelog — and only while there is one to describe (see
            // `changelog_panel`).
            .when_some(self.changelog_panel(theme), |element, panel| {
                element.child(panel)
            })
    }

    /// The changelog at the foot of 关于: what installing the newer release would bring, in the UI's
    /// own language (so a 简体中文界面 shows the Chinese changelog).
    ///
    /// `None` — the section stays off the page — when there is nothing to describe: this build is
    /// the newest release, or the newer release documents itself in neither place (see
    /// [`changelog`]). It never falls back to this build's own entry, which would label the running
    /// version as the update.
    fn changelog_panel(&self, theme: Theme) -> Option<AnyElement> {
        let notes = changelog::notes(self.update_notes.as_deref());
        if notes.is_empty() {
            return None;
        }
        let mut lines = div().flex().flex_col().gap_2().w_full();
        // The button's copy text is the whole section; a drag can still select only what it covers.
        let full_text = notes
            .iter()
            .map(|note| match note {
                changelog::NoteLine::Version { version, date } => match date {
                    Some(date) => format!("{version}  {date}"),
                    None => version.clone(),
                },
                changelog::NoteLine::Group(group) => group.clone(),
                changelog::NoteLine::Bullet(item) => format!("• {item}"),
                changelog::NoteLine::Text(text) => text.clone(),
            })
            .collect::<Vec<_>>()
            .join("\n");
        for (order, note) in notes.into_iter().enumerate() {
            let order = order as u64;
            // Each line is its own selection participant with a document order, so a drag can
            // cross lines and Ctrl+C copies the whole selected range in reading order.
            let (text, size, weight, color, spaced) = match note {
                changelog::NoteLine::Version { version, date } => (
                    match date {
                        Some(date) => format!("{version}  {date}"),
                        None => version,
                    },
                    13.0,
                    FontWeight::SEMIBOLD,
                    theme.text,
                    false,
                ),
                changelog::NoteLine::Group(group) => {
                    (group, 12.0, FontWeight::MEDIUM, theme.brand, true)
                }
                changelog::NoteLine::Bullet(item) => (
                    format!("• {item}"),
                    12.0,
                    FontWeight::NORMAL,
                    theme.text,
                    false,
                ),
                changelog::NoteLine::Text(text) => {
                    (text, 12.0, FontWeight::NORMAL, theme.text, false)
                }
            };
            let mut line = div()
                .w_full()
                .text_size(px(size))
                .font_weight(weight)
                .text_color(rgb(color))
                .cursor_text();
            if spaced {
                line = line.mt_1();
            }
            lines = lines.child(
                line.child(
                    SelectableText::new(("about-changelog-line", order), text)
                        .document_order(order)
                        .selection_color(rgba((theme.grid_selection_bg << 8) | 0x59).into()),
                ),
            );
        }
        Some(
            div()
                .flex()
                .flex_col()
                .gap_2()
                .w_full()
                .child(
                    div()
                        .flex()
                        .flex_row()
                        .items_center()
                        .justify_between()
                        .w_full()
                        .child(section_label(theme, t!("about.changelog").to_string()))
                        .child(ui::popup_button(
                            "about-changelog-copy",
                            t!("about.copy_changelog").to_string(),
                            false,
                            theme,
                            move |_event, _window, cx| {
                                cx.write_to_clipboard(ClipboardItem::new_string(full_text.clone()));
                            },
                        )),
                )
                .child(
                    div()
                        .id("about-changelog")
                        .flex()
                        .flex_col()
                        .w_full()
                        .max_h(px(280.0))
                        .p_3()
                        .rounded(px(8.0))
                        .border_1()
                        .border_color(rgb(theme.border))
                        .bg(rgb(theme.dialog_face))
                        .overflow_y_scroll()
                        .child(lines),
                )
                .into_any_element(),
        )
    }

    /// The live download progress bar and byte caption for the 关于 page. `None` unless an install
    /// is running; the bar is indeterminate until the server's `Content-Length` gives a percentage.
    fn download_progress_element(&self, theme: Theme) -> Option<AnyElement> {
        if !matches!(self.update_status, UpdateStatus::Downloading) {
            return None;
        }
        let (downloaded, total) = self.update_progress.unwrap_or((0, None));
        let percent = progress_percent(downloaded, total);
        let caption = match (total, percent) {
            (Some(total), Some(percent)) if total > 0 => t!(
                "about.progress",
                percent = percent,
                done = format_bytes(downloaded),
                total = format_bytes(total)
            )
            .to_string(),
            _ => t!("about.progress_unknown", done = format_bytes(downloaded)).to_string(),
        };
        let bar = gpui_kit::component::progress::Progress::new("about-update-progress")
            .with_size(gpui_kit::component::Size::XSmall)
            .color(rgb(theme.brand))
            .value(percent.map(|percent| percent as f32).unwrap_or(0.0))
            // No total yet: run the indeterminate sliding animation (the byte caption still
            // advances), so the UI never looks frozen while the archive streams in.
            .loading(percent.is_none());
        Some(
            div()
                .flex()
                .flex_col()
                .gap_2()
                .w_full()
                .child(bar)
                .child(
                    div()
                        .text_size(px(12.0))
                        .text_color(rgb(theme.text_muted))
                        .child(caption),
                )
                .into_any_element(),
        )
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// A release as GitHub hands one to `check_latest`: a version and its body.
    fn release(body: &str) -> self_update::Release {
        let mut builder = self_update::Release::builder();
        builder
            .name("v0.0.11")
            .version("0.0.11")
            .date("2026-10-15")
            .body(body);
        builder.build().expect("0.0.11 is a release")
    }

    #[test]
    fn a_link_only_release_body_is_not_shown_as_notes() {
        // The release job now publishes only links to the changelogs (see release.yml); 关于 must
        // not render them as notes. A hand-written body still says something worth reading when no
        // changelog documents the version (the injected reader finds nothing here).
        let links = format!(
            "{}\n### Changelog\n\n- [English](https://example.com/CHANGELOG.md)\n",
            changelog::CHANGELOG_LINKS_MARKER
        );
        assert_eq!(
            release_notes_with(&release(&links), LanguageSetting::En, |_| None),
            None
        );

        let hand_written = "## What's Changed\n\n* fix a thing\n";
        assert_eq!(
            release_notes_with(&release(hand_written), LanguageSetting::En, |_| None).as_deref(),
            Some(hand_written.trim())
        );
    }

    #[test]
    fn an_update_source_tries_the_chosen_host_first() {
        // The 关于 choice only reorders the two hosts: whichever one is unreachable or still
        // uploading, the other still answers, so a mirror problem never hides an update.
        assert_eq!(
            manifest_urls(UpdateSource::Cnb),
            [CNB_MANIFEST, GITHUB_MANIFEST]
        );
        assert_eq!(
            manifest_urls(UpdateSource::Github),
            [GITHUB_MANIFEST, CNB_MANIFEST]
        );
        let cnb = changelog_urls(UpdateSource::Cnb, "CHANGELOG_zh.md");
        assert!(
            cnb[0].starts_with("https://cnb.cool/yisier1/rustgrid/-/releases/latest/download/")
        );
        assert!(cnb[1].starts_with("https://raw.githubusercontent.com/yisier/rustgrid/main/"));
    }

    #[test]
    #[ignore = "network: reads the live GitHub and CNB release manifests"]
    fn the_live_manifests_are_readable() {
        // Both sources serve the same schema-1 manifest, which is the path every new release goes
        // through (see `docs/update-sources.md`). One host being unreachable is exactly what the
        // fallback is for, so at least one must answer; a fetched release must be newer than this
        // build (`None` just means it already is the newest).
        let mut answered = 0;
        let mut errors = Vec::new();
        for (name, url, source) in [
            ("GitHub", GITHUB_MANIFEST, UpdateSource::Github),
            ("CNB", CNB_MANIFEST, UpdateSource::Cnb),
        ] {
            match check_manifest(url, source, LanguageSetting::En) {
                Ok(found) => {
                    answered += 1;
                    if let Some((version, _)) = found {
                        assert_ne!(
                            version, APP_VERSION,
                            "{name}: an offered release must be newer than this build"
                        );
                    }
                }
                Err(error) => errors.push(format!("{name}: {error}")),
            }
        }
        assert!(
            answered > 0,
            "no update manifest was readable: {}",
            errors.join("; ")
        );
    }

    #[test]
    fn the_release_manifest_parses_like_the_ci_script_writes_it() {
        // `tools/release/make-manifest.mjs` writes this schema-1 document; both sources only differ
        // in the asset URL prefix. Parsing it here keeps the script and the app from drifting.
        let manifest = r#"{
          "schema": 1,
          "releases": [{
            "version": "0.1.19",
            "date": "2026-11-28T00:00:00Z",
            "assets": [
              {"name": "RustGrid-0.1.19-windows-x64.zip",
               "url": "https://cnb.cool/yisier1/rustgrid/-/releases/download/v0.1.19/RustGrid-0.1.19-windows-x64.zip",
               "digest": "sha256:2cf24dba5fb0a30e26e83b2ac5b9e29e1b161e5c1fa7425e73043362938b9824"},
              {"name": "RustGrid-0.1.19-linux-x64.tar.gz",
               "url": "https://cnb.cool/yisier1/rustgrid/-/releases/download/v0.1.19/RustGrid-0.1.19-linux-x64.tar.gz",
               "digest": "sha256:2cf24dba5fb0a30e26e83b2ac5b9e29e1b161e5c1fa7425e73043362938b9824"}
            ]
          }]
        }"#;
        let releases = self_update::backends::manifest::parse_manifest(
            manifest,
            "https://example.net/latest.json",
        )
        .expect("the CI manifest parses");
        assert_eq!(releases.len(), 1);
        assert_eq!(releases[0].version(), "0.1.19");
        assert_eq!(releases[0].assets().len(), 2);
        assert!(releases[0].assets()[0].digest().is_some());
    }

    #[test]
    #[ignore = "network: fetches the repository's changelogs from the GitHub API"]
    fn a_release_reads_its_notes_from_the_repository_changelog() {
        // Every release reads the version's section from the changelog in the UI language, English
        // standing in where a language has no translated file yet — regardless of what the release
        // body says. 0.0.11 predates the link-only bodies and carries only GitHub's auto-generated
        // line.
        let release = release(
            "**Full Changelog**: https://github.com/yisier/rustgrid/compare/v0.0.10...v0.0.11",
        );
        for language in [
            LanguageSetting::En,
            LanguageSetting::ZhCn,
            LanguageSetting::Ru,
        ] {
            let notes = release_notes(&release, language, UpdateSource::Github)
                .unwrap_or_else(|| panic!("{language:?}: the repository documents 0.0.11"));
            assert!(
                notes.starts_with("## [0.0.11]"),
                "{language:?}: 关于 must show 0.0.11's entry, got {notes}"
            );
        }
    }
}
