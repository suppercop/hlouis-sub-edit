//! Typed application preferences model covering Aegisub 3.5.1 preference pages plus
//! `hlouis-sub-edit` additions (theme / Eye Comfort strength & schedule, performance overlay,
//! layout), with JSON schema versioning/migrations, validation & clamping, unknown-key
//! preservation, `aegisub-config` import, and key-based change notifications.
//!
//! Mirrors Aegisub `src/preferences.cpp` and `src/libresrc/default_config.json`.

#![forbid(unsafe_code)]
#![cfg_attr(not(test), deny(clippy::unwrap_used, clippy::expect_used))]

use aegisub_config::ImportedSettings;
use serde::{Deserialize, Serialize};
use serde_json::{Map, Value};
use std::collections::HashMap;
use thiserror::Error;

/// Current JSON schema version for `settings.json`.
pub const CURRENT_SCHEMA_VERSION: u32 = 2;

/// Errors returned by `settings`.
#[derive(Debug, Error)]
pub enum SettingsError {
    /// JSON serialization or deserialization failure.
    #[error("settings JSON error: {0}")]
    Json(#[from] serde_json::Error),
    /// Filesystem I/O failure.
    #[error("settings I/O error: {0}")]
    Io(#[from] std::io::Error),
}

/// Top-level theme mode preference.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, Default)]
pub enum ThemeMode {
    #[default]
    System,
    Light,
    Dark,
    EyeComfortLight,
    EyeComfortDark,
}

/// General preference page.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(default)]
pub struct GeneralPrefs {
    pub language: String,
    pub check_for_updates: bool,
    pub max_recent_files: u32,
    pub undo_levels: u32,
}

impl Default for GeneralPrefs {
    fn default() -> Self {
        Self {
            language: "en_US".to_owned(),
            check_for_updates: true,
            max_recent_files: 16,
            undo_levels: 200,
        }
    }
}

/// Appearance & Eye Comfort preference page.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(default)]
pub struct AppearancePrefs {
    pub theme_mode: ThemeMode,
    pub eye_comfort_strength: u8,
    pub eye_comfort_schedule_enabled: bool,
    /// Schedule start in minutes from midnight (default 20:00 = 1200).
    pub eye_comfort_from_min: u16,
    /// Schedule end in minutes from midnight (default 06:00 = 360).
    pub eye_comfort_to_min: u16,
    pub show_performance_overlay: bool,
    pub grid_font_face: String,
    pub grid_font_size: u32,
    pub edit_box_font_face: String,
    pub edit_box_font_size: u32,
}

impl Default for AppearancePrefs {
    fn default() -> Self {
        Self {
            theme_mode: ThemeMode::System,
            eye_comfort_strength: 60,
            eye_comfort_schedule_enabled: false,
            eye_comfort_from_min: 1200,
            eye_comfort_to_min: 360,
            show_performance_overlay: false,
            grid_font_face: "Tahoma".to_owned(),
            grid_font_size: 9,
            edit_box_font_face: "Consolas".to_owned(),
            edit_box_font_size: 11,
        }
    }
}

/// Interface & grid preference page.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(default)]
#[allow(clippy::struct_excessive_bools)]
pub struct InterfacePrefs {
    pub show_toolbar: bool,
    pub show_video: bool,
    pub show_audio: bool,
    pub hide_overrides_mode: u8,
    pub show_col_actor: bool,
    pub show_col_effect: bool,
    pub show_col_margins: bool,
    pub show_col_cps: bool,
}

impl Default for InterfacePrefs {
    fn default() -> Self {
        Self {
            show_toolbar: true,
            show_video: true,
            show_audio: true,
            hide_overrides_mode: 1,
            show_col_actor: false,
            show_col_effect: false,
            show_col_margins: false,
            show_col_cps: true,
        }
    }
}

/// Audio preference page.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(default)]
#[allow(clippy::struct_excessive_bools)]
pub struct AudioPrefs {
    pub provider: String,
    pub spectrum: bool,
    pub spectrum_quality: u8,
    pub spectrum_cutoff: i64,
    pub medusa_hotkeys: bool,
    pub auto_focus: bool,
    pub auto_commit: bool,
    pub lead_in_ms: i64,
    pub lead_out_ms: i64,
    pub cache_ram_mb: u32,
}

impl Default for AudioPrefs {
    fn default() -> Self {
        Self {
            provider: "FFmpegSource".to_owned(),
            spectrum: true,
            spectrum_quality: 1,
            spectrum_cutoff: 0,
            medusa_hotkeys: false,
            auto_focus: true,
            auto_commit: true,
            lead_in_ms: 200,
            lead_out_ms: 300,
            cache_ram_mb: 512,
        }
    }
}

/// Video preference page.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(default)]
pub struct VideoPrefs {
    pub provider: String,
    pub subtitle_provider: String,
    pub default_zoom_idx: u8,
    pub fast_jump_step: i64,
    pub check_script_res: u8,
    pub detached_video: bool,
}

impl Default for VideoPrefs {
    fn default() -> Self {
        Self {
            provider: "FFmpegSource".to_owned(),
            subtitle_provider: "libass".to_owned(),
            default_zoom_idx: 7,
            fast_jump_step: 10,
            check_script_res: 2,
            detached_video: false,
        }
    }
}

/// Timing & karaoke preference page.
#[derive(Debug, Clone, Copy, PartialEq, Serialize, Deserialize)]
#[serde(default)]
#[allow(clippy::struct_excessive_bools)]
pub struct TimingPrefs {
    pub default_duration_ms: i64,
    pub enable_lead_in: bool,
    pub enable_lead_out: bool,
    pub enable_adjacent: bool,
    pub enable_keyframe: bool,
    pub adjacent_threshold_ms: i64,
    pub adjacent_bias: f64,
    pub key_start_before: i64,
    pub key_start_after: i64,
    pub key_end_before: i64,
    pub key_end_after: i64,
}

impl Default for TimingPrefs {
    fn default() -> Self {
        Self {
            default_duration_ms: 2000,
            enable_lead_in: true,
            enable_lead_out: true,
            enable_adjacent: true,
            enable_keyframe: true,
            adjacent_threshold_ms: 300,
            adjacent_bias: 1.0,
            key_start_before: 6,
            key_start_after: 5,
            key_end_before: 15,
            key_end_after: 10,
        }
    }
}

/// Autosave & autobackup preference page.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(default)]
pub struct AutosavePrefs {
    pub enabled: bool,
    pub interval_seconds: u64,
    pub on_every_change: bool,
    pub backup_enabled: bool,
    pub keep_count: usize,
}

impl Default for AutosavePrefs {
    fn default() -> Self {
        Self {
            enabled: true,
            interval_seconds: 60,
            on_every_change: false,
            backup_enabled: true,
            keep_count: 10,
        }
    }
}

/// Paths preference page.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(default)]
pub struct PathsPrefs {
    pub autosave_dir: String,
    pub autobackup_dir: String,
    pub styles_catalog_dir: String,
}

impl Default for PathsPrefs {
    fn default() -> Self {
        Self {
            autosave_dir: "?user/autosave".to_owned(),
            autobackup_dir: "?user/autoback".to_owned(),
            styles_catalog_dir: "?user/catalog".to_owned(),
        }
    }
}

/// Advanced preference page.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(default)]
pub struct AdvancedPrefs {
    pub worker_threads: u16,
    pub hardware_decode: bool,
    pub log_max_entries: usize,
}

impl Default for AdvancedPrefs {
    fn default() -> Self {
        Self {
            worker_threads: 0,
            hardware_decode: true,
            log_max_entries: 10_000,
        }
    }
}

/// Root settings struct with schema versioning and unknown-field preservation.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct AppSettings {
    pub schema_version: u32,
    pub general: GeneralPrefs,
    pub appearance: AppearancePrefs,
    pub interface: InterfacePrefs,
    pub audio: AudioPrefs,
    pub video: VideoPrefs,
    pub timing: TimingPrefs,
    pub autosave: AutosavePrefs,
    pub paths: PathsPrefs,
    pub advanced: AdvancedPrefs,
    /// Unknown top-level fields from newer/custom versions preserved verbatim on save.
    #[serde(flatten)]
    pub unknown: Map<String, Value>,
}

impl Default for AppSettings {
    fn default() -> Self {
        Self {
            schema_version: CURRENT_SCHEMA_VERSION,
            general: GeneralPrefs::default(),
            appearance: AppearancePrefs::default(),
            interface: InterfacePrefs::default(),
            audio: AudioPrefs::default(),
            video: VideoPrefs::default(),
            timing: TimingPrefs::default(),
            autosave: AutosavePrefs::default(),
            paths: PathsPrefs::default(),
            advanced: AdvancedPrefs::default(),
            unknown: Map::new(),
        }
    }
}

fn diff_json(
    path: &str,
    before: &serde_json::Value,
    after: &serde_json::Value,
    out: &mut Vec<String>,
) {
    use serde_json::Value;
    match (before, after) {
        (Value::Object(old), Value::Object(new)) => {
            for (key, old_val) in old {
                let child = if path.is_empty() {
                    key.clone()
                } else {
                    format!("{path}.{key}")
                };
                if let Some(new_val) = new.get(key) {
                    diff_json(&child, old_val, new_val, out);
                }
            }
        }
        _ if before != after => out.push(format!("{path}: {before} -> {after}")),
        _ => {}
    }
}

impl AppSettings {
    /// Loads settings from JSON, applies schema migrations, and clamps out-of-range values.
    ///
    /// # Errors
    /// Returns [`SettingsError::Json`] if `json` is syntactically invalid.
    pub fn load_json(json: &str) -> Result<Self, SettingsError> {
        let mut settings: Self = serde_json::from_str(json)?;
        settings.migrate();
        settings.validate_and_clamp();
        Ok(settings)
    }

    /// Like [`AppSettings::load_json`] but also lists every value that migration or clamping
    /// changed (`"general.undo_levels: 5 -> 10"`), so the UI can tell the user their config
    /// was adjusted instead of silently rewriting it.
    ///
    /// # Errors
    /// Returns [`SettingsError::Json`] if `json` is syntactically invalid.
    pub fn load_json_with_report(json: &str) -> Result<(Self, Vec<String>), SettingsError> {
        let mut settings: Self = serde_json::from_str(json)?;
        let before = serde_json::to_value(&settings)?;
        settings.migrate();
        settings.validate_and_clamp();
        let after = serde_json::to_value(&settings)?;
        let mut changes = Vec::new();
        diff_json("", &before, &after, &mut changes);
        Ok((settings, changes))
    }

    /// Serializes settings to pretty JSON (preserving unknown keys).
    ///
    /// # Errors
    /// Returns [`SettingsError::Json`] if serialization fails.
    pub fn save_json(&self) -> Result<String, SettingsError> {
        Ok(serde_json::to_string_pretty(self)?)
    }

    /// Applies forward schema migrations (e.g. v1 -> v2).
    pub fn migrate(&mut self) {
        if self.schema_version < 2 {
            // v1 -> v2: ensure eye_comfort_strength defaults to 60 if unset/0 on EyeComfort mode.
            if matches!(
                self.appearance.theme_mode,
                ThemeMode::EyeComfortLight | ThemeMode::EyeComfortDark
            ) && self.appearance.eye_comfort_strength == 0
            {
                self.appearance.eye_comfort_strength = 60;
            }
            self.schema_version = CURRENT_SCHEMA_VERSION;
        }
    }

    /// Clamps all numeric settings into safe bounds.
    pub fn validate_and_clamp(&mut self) {
        self.general.max_recent_files = self.general.max_recent_files.clamp(1, 64);
        self.general.undo_levels = self.general.undo_levels.clamp(10, 10_000);
        self.appearance.eye_comfort_strength = self.appearance.eye_comfort_strength.min(100);
        self.appearance.eye_comfort_from_min %= 1440;
        self.appearance.eye_comfort_to_min %= 1440;
        self.appearance.grid_font_size = self.appearance.grid_font_size.clamp(6, 72);
        self.appearance.edit_box_font_size = self.appearance.edit_box_font_size.clamp(6, 72);
        self.interface.hide_overrides_mode = self.interface.hide_overrides_mode.min(2);
        self.audio.spectrum_quality = self.audio.spectrum_quality.min(3);
        self.audio.lead_in_ms = self.audio.lead_in_ms.clamp(0, 10_000);
        self.audio.lead_out_ms = self.audio.lead_out_ms.clamp(0, 10_000);
        self.audio.cache_ram_mb = self.audio.cache_ram_mb.clamp(32, 4096);
        self.video.default_zoom_idx = self.video.default_zoom_idx.min(20);
        self.video.fast_jump_step = self.video.fast_jump_step.clamp(1, 1000);
        self.video.check_script_res = self.video.check_script_res.min(3);
        self.timing.default_duration_ms = self.timing.default_duration_ms.clamp(100, 60_000);
        self.timing.adjacent_bias = self.timing.adjacent_bias.clamp(0.0, 1.0);
        self.autosave.interval_seconds = self.autosave.interval_seconds.clamp(5, 3600);
        self.autosave.keep_count = self.autosave.keep_count.clamp(1, 200);
        self.advanced.log_max_entries = self.advanced.log_max_entries.clamp(100, 500_000);
    }

    /// Imports preferences from an [`aegisub_config::ImportedSettings`] snapshot.
    pub fn import_from_aegisub(&mut self, imported: &ImportedSettings) {
        imported
            .layout
            .language
            .clone_into(&mut self.general.language);
        self.interface.show_toolbar = imported.layout.show_toolbar;
        self.interface.show_video = imported.layout.show_video;
        self.interface.show_audio = imported.layout.show_audio;
        imported
            .layout
            .edit_box_font_face
            .clone_into(&mut self.appearance.edit_box_font_face);
        self.appearance.edit_box_font_size = imported.layout.edit_box_font_size;
        imported
            .grid
            .font_face
            .clone_into(&mut self.appearance.grid_font_face);
        self.appearance.grid_font_size = imported.grid.font_size;
        self.interface.hide_overrides_mode =
            u8::try_from(imported.grid.hide_overrides_mode.clamp(0, 2)).unwrap_or(1);
        self.interface.show_col_actor = imported.grid.columns.actor;
        self.interface.show_col_effect = imported.grid.columns.effect;
        self.interface.show_col_cps = imported.grid.columns.cps;
        self.audio.spectrum = imported.audio.spectrum;
        self.audio.lead_in_ms = imported.audio.lead_in_ms;
        self.audio.lead_out_ms = imported.audio.lead_out_ms;
        self.audio.auto_focus = imported.audio.auto_focus;
        self.audio.auto_commit = imported.audio.auto_commit;
        self.timing.default_duration_ms = imported.timing.default_duration_ms;
        self.autosave.enabled = imported.autosave.enabled;
        self.autosave.interval_seconds = imported.autosave.interval_seconds;
        self.autosave.on_every_change = imported.autosave.on_every_change;
        self.autosave.backup_enabled = imported.autosave.backup_enabled;
        imported
            .autosave
            .autosave_dir
            .clone_into(&mut self.paths.autosave_dir);
        imported
            .autosave
            .autobackup_dir
            .clone_into(&mut self.paths.autobackup_dir);
        self.validate_and_clamp();
    }
}

/// Callback type invoked when a subscribed preference key changes.
pub type ChangeListener = Box<dyn FnMut(&AppSettings)>;

/// Live settings holder with key-based change subscriptions.
#[derive(Default)]
pub struct SettingsStore {
    current: AppSettings,
    listeners: HashMap<String, Vec<ChangeListener>>,
}

impl SettingsStore {
    /// Creates a store wrapping `initial`.
    #[must_use]
    pub fn new(mut initial: AppSettings) -> Self {
        initial.validate_and_clamp();
        Self {
            current: initial,
            listeners: HashMap::new(),
        }
    }

    /// Borrows the active settings.
    #[must_use]
    pub const fn get(&self) -> &AppSettings {
        &self.current
    }

    /// Registers a listener for a dot-delimited preference key (e.g. `"appearance.theme_mode"`).
    pub fn subscribe(&mut self, key: impl Into<String>, listener: ChangeListener) {
        self.listeners.entry(key.into()).or_default().push(listener);
    }

    /// Mutates settings, re-validates, and fires listeners registered for `changed_key`.
    pub fn update<F>(&mut self, changed_key: &str, f: F)
    where
        F: FnOnce(&mut AppSettings),
    {
        f(&mut self.current);
        self.current.validate_and_clamp();
        if let Some(cbs) = self.listeners.get_mut(changed_key) {
            for cb in cbs {
                cb(&self.current);
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::cell::Cell;
    use std::rc::Rc;

    #[test]
    fn load_migrate_clamp_preserve_unknown_and_notify() {
        let v1_json = r#"{
            "schema_version": 1,
            "appearance": { "theme_mode": "EyeComfortDark", "eye_comfort_strength": 0, "grid_font_size": 999 },
            "autosave": { "interval_seconds": 1 },
            "future_plugin": { "enabled": true }
        }"#;
        let loaded = AppSettings::load_json(v1_json).expect("load");
        assert_eq!(loaded.schema_version, CURRENT_SCHEMA_VERSION);
        assert_eq!(loaded.appearance.eye_comfort_strength, 60);
        assert_eq!(loaded.appearance.grid_font_size, 72);
        assert_eq!(loaded.autosave.interval_seconds, 5);
        assert!(loaded.unknown.contains_key("future_plugin"));

        let saved = loaded.save_json().expect("save");
        assert!(saved.contains("future_plugin"));

        let hits = Rc::new(Cell::new(0u32));
        let hits_clone = Rc::clone(&hits);
        let mut store = SettingsStore::new(loaded);
        store.subscribe(
            "appearance.theme_mode",
            Box::new(move |_| hits_clone.set(hits_clone.get() + 1)),
        );
        store.update("appearance.theme_mode", |s| {
            s.appearance.theme_mode = ThemeMode::Light;
        });
        assert_eq!(hits.get(), 1);
    }

    #[test]
    fn clamped_values_are_reported() {
        let mut s = AppSettings::default();
        s.general.undo_levels = 5;
        let json = s.save_json().unwrap();
        let (loaded, changes) = AppSettings::load_json_with_report(&json).unwrap();
        assert_eq!(loaded.general.undo_levels, 10);
        assert!(
            changes.iter().any(|c| c.starts_with("general.undo_levels")),
            "{changes:?}"
        );
        let (_, none) =
            AppSettings::load_json_with_report(&AppSettings::default().save_json().unwrap())
                .unwrap();
        assert!(none.is_empty(), "{none:?}");
    }
}
