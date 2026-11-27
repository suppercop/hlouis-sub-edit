//! Offline GitHub Releases JSON parser, semver comparator, release-channel filter, and
//! remind-later / skip-version / rate-limit update-check state machine (no networking).

use serde_json::Value;
use std::cmp::Ordering;
use thiserror::Error;

/// Errors returned by version checking.
#[derive(Debug, Error)]
pub enum VersionError {
    /// Invalid GitHub releases JSON.
    #[error("releases JSON parse error: {0}")]
    Json(#[from] serde_json::Error),
    /// Invalid semver string.
    #[error("invalid semver string: {0}")]
    InvalidSemVer(String),
    /// The JSON was valid but is not a release object or an array of them.
    #[error("releases JSON has an unexpected shape (expected an array or object)")]
    UnexpectedShape,
}

/// Update release channel.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ReleaseChannel {
    Stable,
    PreRelease,
}

/// Parsed semantic version (`major.minor.patch[-pre]`).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SemVer {
    pub major: u64,
    pub minor: u64,
    pub patch: u64,
    pub pre: Option<String>,
}

impl SemVer {
    /// Parses `"v1.2.3"` or `"1.2.3-beta.1"`.
    ///
    /// # Errors
    /// Returns [`VersionError::InvalidSemVer`] if the string is not a valid semver triple.
    pub fn parse(raw: &str) -> Result<Self, VersionError> {
        let s = raw.trim().strip_prefix('v').unwrap_or(raw.trim());
        let (core, pre) = match s.split_once('-') {
            Some((c, p)) => (c, Some(p.to_owned())),
            None => (s, None),
        };
        let core_no_build = core.split_once('+').map_or(core, |(c, _)| c);
        let mut parts = core_no_build.split('.');
        let major = parts
            .next()
            .and_then(|p| p.parse::<u64>().ok())
            .ok_or_else(|| VersionError::InvalidSemVer(raw.to_owned()))?;
        let minor = parts
            .next()
            .and_then(|p| p.parse::<u64>().ok())
            .ok_or_else(|| VersionError::InvalidSemVer(raw.to_owned()))?;
        let patch = parts
            .next()
            .and_then(|p| p.parse::<u64>().ok())
            .ok_or_else(|| VersionError::InvalidSemVer(raw.to_owned()))?;
        if parts.next().is_some() {
            return Err(VersionError::InvalidSemVer(raw.to_owned()));
        }
        Ok(Self {
            major,
            minor,
            patch,
            pre,
        })
    }

    /// Returns `true` if this version has a pre-release suffix.
    #[must_use]
    pub const fn is_prerelease(&self) -> bool {
        self.pre.is_some()
    }
}

impl std::fmt::Display for SemVer {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "{}.{}.{}", self.major, self.minor, self.patch)?;
        if let Some(ref pre) = self.pre {
            write!(f, "-{pre}")?;
        }
        Ok(())
    }
}

impl Ord for SemVer {
    fn cmp(&self, other: &Self) -> Ordering {
        self.major
            .cmp(&other.major)
            .then_with(|| self.minor.cmp(&other.minor))
            .then_with(|| self.patch.cmp(&other.patch))
            .then_with(|| match (&self.pre, &other.pre) {
                (None, None) => Ordering::Equal,
                (None, Some(_)) => Ordering::Greater, // release > prerelease
                (Some(_), None) => Ordering::Less,
                (Some(a), Some(b)) => a.cmp(b),
            })
    }
}

impl PartialOrd for SemVer {
    fn partial_cmp(&self, other: &Self) -> Option<Ordering> {
        Some(self.cmp(other))
    }
}

/// One parsed release from GitHub Releases API JSON.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ReleaseInfo {
    pub version: SemVer,
    pub tag_name: String,
    pub name: String,
    pub html_url: String,
    pub body: String,
    pub prerelease: bool,
    pub draft: bool,
}

/// Parses a GitHub Releases API JSON array (or single release object) without networking.
///
/// # Errors
/// Returns [`VersionError::Json`] if `json` is invalid JSON.
pub fn parse_github_releases_json(json: &str) -> Result<Vec<ReleaseInfo>, VersionError> {
    parse_github_releases_with_skipped(json).map(|(releases, _)| releases)
}

/// Like [`parse_github_releases_json`] but also returns one reason per release entry that was
/// skipped (missing `tag_name`, tag that is not a semantic version) so the update check can tell
/// the user why a release did not show up.
///
/// # Errors
/// Returns [`VersionError::Json`] for invalid JSON and [`VersionError::UnexpectedShape`] when the
/// document is neither an array nor an object.
pub fn parse_github_releases_with_skipped(
    json: &str,
) -> Result<(Vec<ReleaseInfo>, Vec<String>), VersionError> {
    let val: Value = serde_json::from_str(json)?;
    let items: Vec<&Value> = match &val {
        Value::Array(arr) => arr.iter().collect(),
        Value::Object(_) => vec![&val],
        _ => return Err(VersionError::UnexpectedShape),
    };
    let mut out = Vec::with_capacity(items.len());
    let mut skipped = Vec::new();
    for (i, item) in items.into_iter().enumerate() {
        let Some(tag) = item.get("tag_name").and_then(Value::as_str) else {
            skipped.push(format!("release #{} has no tag_name", i + 1));
            continue;
        };
        let Ok(version) = SemVer::parse(tag) else {
            skipped.push(format!("release tag `{tag}` is not a valid version"));
            continue;
        };
        let draft = item.get("draft").and_then(Value::as_bool).unwrap_or(false);
        let prerelease = item
            .get("prerelease")
            .and_then(Value::as_bool)
            .unwrap_or_else(|| version.is_prerelease());
        let name = item
            .get("name")
            .and_then(Value::as_str)
            .unwrap_or(tag)
            .to_owned();
        let html_url = item
            .get("html_url")
            .and_then(Value::as_str)
            .unwrap_or("")
            .to_owned();
        let body = item
            .get("body")
            .and_then(Value::as_str)
            .unwrap_or("")
            .to_owned();
        out.push(ReleaseInfo {
            version,
            tag_name: tag.to_owned(),
            name,
            html_url,
            body,
            prerelease,
            draft,
        });
    }
    Ok((out, skipped))
}

/// Persistent update-check state supporting channel selection, skip-version, remind-later, and
/// rate limiting.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct UpdateCheckState {
    pub channel: ReleaseChannel,
    pub skipped_version: Option<SemVer>,
    pub remind_after_epoch_sec: u64,
    pub last_check_epoch_sec: u64,
    /// Minimum seconds between network checks (default 24 hours = `86_400`).
    pub min_check_interval_sec: u64,
}

impl Default for UpdateCheckState {
    fn default() -> Self {
        Self {
            channel: ReleaseChannel::Stable,
            skipped_version: None,
            remind_after_epoch_sec: 0,
            last_check_epoch_sec: 0,
            min_check_interval_sec: 86_400,
        }
    }
}

impl UpdateCheckState {
    /// Returns `true` if rate limit and remind-later window allow checking now.
    #[must_use]
    pub const fn should_check(&self, now_epoch_sec: u64) -> bool {
        now_epoch_sec >= self.remind_after_epoch_sec
            && now_epoch_sec.saturating_sub(self.last_check_epoch_sec)
                >= self.min_check_interval_sec
    }

    /// Defers prompts by `delay_sec` ("Remind me later").
    pub fn remind_later(&mut self, now_epoch_sec: u64, delay_sec: u64) {
        self.remind_after_epoch_sec = now_epoch_sec.saturating_add(delay_sec);
    }

    /// Marks `version` as permanently skipped ("Skip this version").
    pub fn skip_version(&mut self, version: SemVer) {
        self.skipped_version = Some(version);
    }

    /// Picks the newest eligible update strictly greater than `current` matching `self.channel`
    /// and not skipped.
    #[must_use]
    pub fn find_update<'a>(
        &self,
        current: &SemVer,
        releases: &'a [ReleaseInfo],
    ) -> Option<&'a ReleaseInfo> {
        releases
            .iter()
            .filter(|r| !r.draft)
            .filter(|r| self.channel == ReleaseChannel::PreRelease || !r.prerelease)
            .filter(|r| &r.version > current)
            .filter(|r| self.skipped_version.as_ref() != Some(&r.version))
            .max_by(|a, b| a.version.cmp(&b.version))
    }
}
