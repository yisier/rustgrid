//! Update detection and self-upgrade from GitHub Releases (see `.github/workflows/release.yml`).
//!
//! [`self_update`] (GitHub backend, ureq) drives the whole flow: list the repository's releases,
//! compare their semver against this build's `CARGO_PKG_VERSION`, download the portable archive for
//! the running target, extract the executable and replace the running one in place. Detection runs
//! once shortly after launch; a newer release surfaces an update button in the titlebar, and
//! 选项 → 关于 offers the same check plus a manual install.
//!
//! The asset names produced by the release workflow must contain the running platform tag (see
//! [`PLATFORM`]), otherwise `self_update` cannot pick the right download.

use super::*;

/// The GitHub repository whose Releases are checked.
const REPO_OWNER: &str = "yisier";
const REPO_NAME: &str = "rustgrid";

/// The executable's base name. `self_update` appends the platform suffix (`.exe` on Windows) and
/// uses the result both to select the asset and to find the binary inside the archive.
const BIN_NAME: &str = "RustGrid";

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

/// Blocking: ask GitHub for the newest release and return its version when it is strictly newer
/// than the running build. Runs on the tokio blocking pool via `Runtime::spawn_blocking`.
fn check_latest() -> Result<Option<String>, String> {
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
    Ok(release.map(|release| release.version().to_string()))
}

/// Blocking: download and install the newest release, returning the installed version. The caller
/// relaunches the process afterwards.
fn install_latest() -> Result<String, String> {
    let status = self_update::backends::github::Update::configure()
        .repo_owner(REPO_OWNER)
        .repo_name(REPO_NAME)
        .bin_name(BIN_NAME)
        .target(PLATFORM)
        .current_version(APP_VERSION)
        // Non-interactive: never print to stdio or block on stdin (this is a GUI process).
        .no_confirm(true)
        .show_output(false)
        .show_download_progress(false)
        .build()
        .map_err(|error| error.to_string())?
        .update()
        .map_err(|error| error.to_string())?;
    Ok(status.version().to_string())
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
        cx.spawn(async move |this, cx| {
            let result = runtime.spawn_blocking(check_latest).await;
            let _ = this.update(cx, move |app, cx| {
                match result {
                    Ok(Ok(Some(version))) => {
                        app.update_version = Some(version);
                        app.update_status = UpdateStatus::Available;
                    }
                    Ok(Ok(None)) => {
                        app.update_version = None;
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
    pub(super) fn install_update(&mut self, cx: &mut Context<'_, Self>) {
        if self.update_status.is_busy() || matches!(self.update_status, UpdateStatus::Installed) {
            return;
        }
        self.update_status = UpdateStatus::Downloading;
        cx.notify();
        let runtime = self.runtime.clone();
        cx.spawn(async move |this, cx| {
            let result = runtime.spawn_blocking(install_latest).await;
            match result {
                Ok(Ok(version)) => {
                    let _ = this.update(cx, move |app, cx| {
                        app.update_version = Some(version);
                        app.update_status = UpdateStatus::Installed;
                        cx.notify();
                    });
                    // The on-disk executable has been replaced; hand over to it.
                    relaunch();
                }
                Ok(Err(error)) => {
                    let _ = this.update(cx, move |app, cx| {
                        app.update_status = UpdateStatus::Failed(error);
                        cx.notify();
                    });
                }
                Err(error) => {
                    let message = error.to_string();
                    let _ = this.update(cx, move |app, cx| {
                        app.update_status = UpdateStatus::Failed(message);
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
            UpdateStatus::Downloading => (
                "icons/refresh.svg",
                Some(t!("update.downloading").to_string()),
                theme.text_muted,
            ),
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

    /// The 关于 page: app identity, current version and the manual update controls.
    pub(super) fn options_about_body(&self, cx: &mut Context<'_, Self>) -> impl IntoElement {
        let theme = self.theme;
        let status_color = if matches!(self.update_status, UpdateStatus::Failed(_)) {
            theme.danger
        } else {
            theme.text_muted
        };

        let mut row = div()
            .flex()
            .flex_row()
            .items_center()
            .gap_3()
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
            row = row.child(self.dialog_button(
                "about-install",
                t!("about.install").to_string(),
                true,
                cx.listener(|this, _event, _window, cx| this.install_update(cx)),
            ));
        }

        div()
            .flex()
            .flex_col()
            .gap_4()
            .w_full()
            .child(
                div()
                    .flex()
                    .flex_row()
                    .items_center()
                    .gap_3()
                    .child(
                        img(ImageSource::Resource(Resource::Embedded("logo.png".into())))
                            .w(px(48.0))
                            .h(px(48.0))
                            .flex_none(),
                    )
                    .child(
                        div()
                            .flex()
                            .flex_col()
                            .gap_1()
                            .child(
                                div()
                                    .text_size(px(16.0))
                                    .font_weight(FontWeight::MEDIUM)
                                    .child(t!("app.title").to_string()),
                            )
                            .child(
                                div()
                                    .text_size(px(12.0))
                                    .text_color(rgb(theme.text_muted))
                                    .child(t!("about.version", version = APP_VERSION).to_string()),
                            ),
                    ),
            )
            .child(
                div()
                    .text_size(px(12.0))
                    .text_color(rgb(theme.text_muted))
                    .child(t!("about.description").to_string()),
            )
            .child(row)
            .child(
                div()
                    .text_size(px(12.0))
                    .text_color(rgb(status_color))
                    .child(self.update_status_text()),
            )
    }
}
