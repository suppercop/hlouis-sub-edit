//! Optional, privacy-respecting update checker against the GitHub Releases API of
//! `suppercop/hlouis-sub-edit`, with strict opt-in gating, timeout/offline safety, injectable
//! HTTP transport for deterministic testing, and an egui release-notes viewer.

#![forbid(unsafe_code)]
#![cfg_attr(not(test), deny(clippy::unwrap_used, clippy::expect_used))]

use egui::{ScrollArea, Ui};
use i18n_catalog::Catalog;
use serde::Deserialize;
use std::time::Duration;
use thiserror::Error;

/// Canonical GitHub Releases endpoint for `suppercop/hlouis-sub-edit`.
pub const RELEASES_API_URL: &str =
    "https://api.github.com/repos/suppercop/hlouis-sub-edit/releases/latest";

/// Default network timeout for update checks.
pub const DEFAULT_TIMEOUT: Duration = Duration::from_secs(5);

/// Minimal `SemVer` (`major.minor.patch`) parsed with zero heap allocations.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord)]
pub struct SemVer {
    pub major: u32,
    pub minor: u32,
    pub patch: u32,
}

impl SemVer {
    /// Parses `"v1.2.3"` or `"1.2.3-beta.1"` (strips leading `v`/`V` and pre-release suffix)
    /// with **0 heap allocations**.
    #[must_use]
    pub fn parse(raw: &str) -> Option<Self> {
        let trimmed = raw.trim().trim_start_matches(['v', 'V']);
        let core = trimmed.split(['-', '+']).next()?;
        let mut parts = core.split('.');
        let major = parts.next()?.parse::<u32>().ok()?;
        let minor = parts.next()?.parse::<u32>().ok()?;
        let patch = parts.next().unwrap_or("0").parse::<u32>().ok()?;
        Some(Self {
            major,
            minor,
            patch,
        })
    }
}

/// Errors produced by [`check_for_updates`].
#[derive(Debug, Clone, Error, PartialEq, Eq)]
pub enum UpdateError {
    #[error("update checks are disabled (opt-in required)")]
    OptInDisabled,
    #[error("network/offline error: {0}")]
    Transport(String),
    #[error("invalid GitHub release JSON: {0}")]
    InvalidPayload(String),
    #[error("invalid version tag: {0}")]
    InvalidVersion(String),
}

/// Parsed release metadata returned by GitHub's `/releases/latest` endpoint.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ReleaseInfo {
    pub version: SemVer,
    pub tag_name: String,
    pub name: String,
    pub html_url: String,
    pub body: String,
    pub published_at: String,
}

/// Result of comparing the running app version against the latest GitHub release.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum UpdateStatus {
    UpToDate {
        current: SemVer,
    },
    UpdateAvailable {
        current: SemVer,
        latest: ReleaseInfo,
    },
}

/// Injectable HTTP transport trait (enables 100% deterministic offline unit tests and lets
/// the desktop shell plug in its TLS/HTTP client with strict timeout and no tracking headers).
pub trait ReleaseTransport {
    /// Fetches raw JSON text from `url` within `timeout`.
    ///
    /// # Errors
    /// Returns [`UpdateError::Transport`] on timeout, DNS/offline failure, or non-2xx HTTP status.
    fn fetch_latest_json(&self, url: &str, timeout: Duration) -> Result<String, UpdateError>;
}

/// Offline-safe transport stub that always returns [`UpdateError::Transport`] without touching
/// the network (safe default when no network backend is wired).
#[derive(Debug, Default, Clone, Copy)]
pub struct OfflineTransport;

impl ReleaseTransport for OfflineTransport {
    fn fetch_latest_json(&self, _url: &str, _timeout: Duration) -> Result<String, UpdateError> {
        Err(UpdateError::Transport("offline mode".to_owned()))
    }
}

#[derive(Deserialize)]
struct GitHubReleaseDto {
    tag_name: String,
    #[serde(default)]
    name: String,
    #[serde(default)]
    html_url: String,
    #[serde(default)]
    body: String,
    #[serde(default)]
    published_at: String,
}

/// Checks for updates against [`RELEASES_API_URL`] only when `opt_in_enabled` is `true`.
///
/// # Errors
/// Returns [`UpdateError::OptInDisabled`] when `opt_in_enabled == false`, or transport/parse errors.
pub fn check_for_updates(
    current: SemVer,
    opt_in_enabled: bool,
    timeout: Duration,
    transport: &dyn ReleaseTransport,
) -> Result<UpdateStatus, UpdateError> {
    if !opt_in_enabled {
        return Err(UpdateError::OptInDisabled);
    }
    let raw = transport.fetch_latest_json(RELEASES_API_URL, timeout)?;
    let dto: GitHubReleaseDto =
        serde_json::from_str(&raw).map_err(|e| UpdateError::InvalidPayload(e.to_string()))?;
    let latest_ver = SemVer::parse(&dto.tag_name)
        .ok_or_else(|| UpdateError::InvalidVersion(dto.tag_name.clone()))?;

    if latest_ver > current {
        Ok(UpdateStatus::UpdateAvailable {
            current,
            latest: ReleaseInfo {
                version: latest_ver,
                tag_name: dto.tag_name,
                name: dto.name,
                html_url: dto.html_url,
                body: dto.body,
                published_at: dto.published_at,
            },
        })
    } else {
        Ok(UpdateStatus::UpToDate { current })
    }
}

/// UI state for the egui update / release-notes dialog.
#[derive(Debug, Clone, Default)]
pub struct UpdateDialogState {
    pub opt_in: bool,
    pub status: Option<Result<UpdateStatus, String>>,
}

/// Renders the egui Check for Updates & Release Notes viewer.
pub fn show_update_dialog(
    ui: &mut Ui,
    state: &mut UpdateDialogState,
    current: SemVer,
    transport: &dyn ReleaseTransport,
    catalog: &Catalog,
) {
    ui.heading(catalog.get("update-title").unwrap_or("Check for Updates"));
    ui.checkbox(
        &mut state.opt_in,
        catalog
            .get("update-opt-in")
            .unwrap_or("Enable optional update checks (GitHub Releases)"),
    );
    ui.horizontal(|ui| {
        if ui
            .add_enabled(
                state.opt_in,
                egui::Button::new(catalog.get("update-check-now").unwrap_or("Check Now")),
            )
            .clicked()
        {
            state.status = Some(
                check_for_updates(current, state.opt_in, DEFAULT_TIMEOUT, transport)
                    .map_err(|e| e.to_string()),
            );
        }
        ui.label(format!(
            "Current version: {}.{}.{}",
            current.major, current.minor, current.patch
        ));
    });

    ui.separator();
    match &state.status {
        None => {
            ui.label("No update check performed yet.");
        }
        Some(Err(err)) => {
            ui.label(format!("Unable to check updates: {err}"));
        }
        Some(Ok(UpdateStatus::UpToDate { .. })) => {
            ui.label("You are running the latest version.");
        }
        Some(Ok(UpdateStatus::UpdateAvailable { latest, .. })) => {
            ui.strong(format!(
                "New version available: {} ({})",
                latest.name, latest.tag_name
            ));
            if !latest.html_url.is_empty() {
                ui.hyperlink_to("Open GitHub Release", &latest.html_url);
            }
            ScrollArea::vertical().max_height(220.0).show(ui, |ui| {
                ui.label(&latest.body);
            });
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use egui_kittest::Harness;
    use i18n_catalog::Locale;

    struct MockTransport {
        response: Result<String, UpdateError>,
    }
    impl ReleaseTransport for MockTransport {
        fn fetch_latest_json(&self, url: &str, _timeout: Duration) -> Result<String, UpdateError> {
            assert_eq!(url, RELEASES_API_URL);
            self.response.clone()
        }
    }

    #[test]
    fn opt_in_gating_update_detection_offline_and_viewer() {
        let mock = MockTransport {
            response: Ok(r###"{
                "tag_name": "v0.3.0",
                "name": "HLouis Sub Edit 0.3.0",
                "html_url": "https://github.com/suppercop/hlouis-sub-edit/releases/tag/v0.3.0",
                "body": "## What's New\n- Karaoke Templater (LuaJIT)\n- 64+ track MKV importer",
                "published_at": "2026-11-27T12:00:00Z"
            }"###
                .to_owned()),
        };

        let cur = SemVer::parse("0.2.0").expect("semver");
        // Opt-in disabled -> blocked
        assert_eq!(
            check_for_updates(cur, false, DEFAULT_TIMEOUT, &mock),
            Err(UpdateError::OptInDisabled)
        );
        // Opt-in enabled -> update available
        let status = check_for_updates(cur, true, DEFAULT_TIMEOUT, &mock).expect("ok");
        assert!(matches!(status, UpdateStatus::UpdateAvailable { .. }));
        // Same or newer version -> up to date
        let newer = SemVer::parse("v0.3.0").expect("semver");
        assert_eq!(
            check_for_updates(newer, true, DEFAULT_TIMEOUT, &mock).expect("ok"),
            UpdateStatus::UpToDate { current: newer }
        );
        // Offline safe
        assert!(check_for_updates(cur, true, DEFAULT_TIMEOUT, &OfflineTransport).is_err());

        // Render egui release-notes viewer
        let mut state = UpdateDialogState {
            opt_in: true,
            status: Some(Ok(status)),
        };
        let cat = Catalog::for_locale(Locale::En);
        let builder = Harness::builder().with_accessibility_check(false);
        let mut harness = builder.build_ui(|ui| {
            show_update_dialog(ui, &mut state, cur, &mock, &cat);
        });
        harness.run();
    }
}
