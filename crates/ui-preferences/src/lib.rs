//! Full 7-page Aegisub-style Preferences dialog (`egui 0.37`) bound to [`settings::AppSettings`]
//! and [`aegisub_config`], featuring injectable `%APPDATA%\Aegisub\config.json` import and live
//! theme / Eye Comfort preview.

#![forbid(unsafe_code)]
#![cfg_attr(not(test), deny(clippy::unwrap_used, clippy::expect_used))]

use aegisub_config::import_config_json;
use commands::CommandRegistry;
use egui::{ScrollArea, Ui};
use i18n_catalog::Catalog;
use settings::{AppSettings, HiddenTagMarker, ThemeMode};
use std::fs;
use std::path::{Path, PathBuf};
use theme::{BaseTheme, ThemeSpec};
use ui_theme::ThemeController;

/// Navigation pages matching Aegisub 3.5.1's Preferences tree (minus legacy Automation).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum PrefsPage {
    General,
    Audio,
    Video,
    InterfaceColours,
    Hotkeys,
    Backup,
    Advanced,
}

impl PrefsPage {
    /// All pages in sidebar display order.
    pub const ALL: [Self; 7] = [
        Self::General,
        Self::Audio,
        Self::Video,
        Self::InterfaceColours,
        Self::Hotkeys,
        Self::Backup,
        Self::Advanced,
    ];

    /// Fluent key for this page's sidebar label.
    #[must_use]
    pub const fn ftl_key(self) -> &'static str {
        match self {
            Self::General => "prefs-page-general",
            Self::Audio => "prefs-page-audio",
            Self::Video => "prefs-page-video",
            Self::InterfaceColours => "prefs-page-interface",
            Self::Hotkeys => "prefs-page-hotkeys",
            Self::Backup => "prefs-page-backup",
            Self::Advanced => "prefs-page-advanced",
        }
    }
}

/// Persistent UI state for the Preferences dialog.
#[derive(Debug, Clone)]
pub struct PreferencesDialogState {
    pub active_page: PrefsPage,
    pub aegisub_config_path: PathBuf,
    pub hotkey_filter: String,
    pub import_status: Option<String>,
}

impl Default for PreferencesDialogState {
    fn default() -> Self {
        Self {
            active_page: PrefsPage::General,
            aegisub_config_path: default_aegisub_config_path(),
            hotkey_filter: String::new(),
            import_status: None,
        }
    }
}

/// Resolves the default Aegisub `config.json` path (`%APPDATA%\Aegisub\config.json` on Windows,
/// `~/.aegisub/config.json` on Unix).
#[must_use]
pub fn default_aegisub_config_path() -> PathBuf {
    if let Ok(appdata) = std::env::var("APPDATA") {
        return PathBuf::from(appdata).join("Aegisub").join("config.json");
    }
    if let Ok(home) = std::env::var("HOME") {
        return PathBuf::from(home).join(".aegisub").join("config.json");
    }
    PathBuf::from("Aegisub").join("config.json")
}

/// Imports an Aegisub `config.json` file from `path` into `settings`, returning a summary string.
///
/// # Errors
/// Returns a human-readable error if the file cannot be read or parsed.
pub fn import_aegisub_settings_from_path(
    path: &Path,
    settings: &mut AppSettings,
) -> Result<String, String> {
    let raw =
        fs::read_to_string(path).map_err(|e| format!("Failed to read {}: {e}", path.display()))?;
    let imported =
        import_config_json(&raw).map_err(|e| format!("Invalid Aegisub config.json: {e}"))?;
    settings.import_from_aegisub(&imported);
    settings.validate_and_clamp();
    Ok(format!(
        "Imported Aegisub settings from {} (lead-in={}ms, lead-out={}ms)",
        path.display(),
        settings.audio.lead_in_ms,
        settings.audio.lead_out_ms
    ))
}

/// Outcome returned by [`show_preferences_dialog`].
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum PrefsDialogAction {
    None,
    ApplyAndClose,
    Cancel,
}

/// Renders the full 7-page Preferences dialog into `ui`, applying live theme previews to `theme_ctl`.
#[allow(clippy::too_many_lines)]
pub fn show_preferences_dialog(
    ui: &mut Ui,
    state: &mut PreferencesDialogState,
    settings: &mut AppSettings,
    theme_ctl: &mut ThemeController,
    commands: &CommandRegistry,
    catalog: &Catalog,
) -> PrefsDialogAction {
    let mut action = PrefsDialogAction::None;

    ui.horizontal(|ui| {
        ui.vertical(|ui| {
            ui.set_min_width(150.0);
            for page in PrefsPage::ALL {
                let label = catalog.get(page.ftl_key()).unwrap_or(page.ftl_key());
                if ui.selectable_label(state.active_page == page, label).clicked() {
                    state.active_page = page;
                }
            }
        });

        ui.separator();

        ui.vertical(|ui| {
            ui.set_min_width(420.0);
            match state.active_page {
                PrefsPage::General => {
                    ui.heading(catalog.get("prefs-page-general").unwrap_or("General"));
                    ui.horizontal(|ui| {
                        ui.label("Undo levels:");
                        ui.add(
                            egui::DragValue::new(&mut settings.general.undo_levels)
                                .range(10..=10_000),
                        );
                    });
                    ui.checkbox(&mut settings.general.check_for_updates, "Opt-in update check");
                    ui.separator();
                    let btn_label = catalog
                        .get("prefs-import-aegisub")
                        .unwrap_or("Import my Aegisub settings...");
                    if ui.button(btn_label).clicked() {
                        match import_aegisub_settings_from_path(&state.aegisub_config_path, settings)
                        {
                            Ok(msg) => state.import_status = Some(msg),
                            Err(err) => state.import_status = Some(err),
                        }
                    }
                    if let Some(ref st) = state.import_status {
                        ui.label(st);
                    }
                }
                PrefsPage::Audio => {
                    ui.heading(catalog.get("prefs-page-audio").unwrap_or("Audio"));
                    ui.horizontal(|ui| {
                        ui.label("Lead-in (ms):");
                        ui.add(
                            egui::DragValue::new(&mut settings.audio.lead_in_ms).range(0..=5_000),
                        );
                    });
                    ui.horizontal(|ui| {
                        ui.label("Lead-out (ms):");
                        ui.add(
                            egui::DragValue::new(&mut settings.audio.lead_out_ms).range(0..=5_000),
                        );
                    });
                    ui.checkbox(&mut settings.audio.spectrum, "Default to Spectrum mode");
                    ui.checkbox(&mut settings.audio.medusa_hotkeys, "Enable Medusa numpad hotkeys");
                    ui.checkbox(&mut settings.audio.auto_commit, "Auto-commit timing changes");
                    ui.separator();
                    ui.checkbox(
                        &mut settings.audio.auto_snap_keyframes,
                        "Auto-snap to keyframes",
                    );
                    ui.checkbox(
                        &mut settings.audio.snap_to_lines,
                        "Auto-snap to neighbouring line boundaries",
                    );
                    ui.horizontal(|ui| {
                        ui.label("Snap distance (px):");
                        ui.add(
                            egui::DragValue::new(&mut settings.audio.snap_distance_px)
                                .range(1..=64),
                        );
                    });
                }
                PrefsPage::Video => {
                    ui.heading(catalog.get("prefs-page-video").unwrap_or("Video"));
                    ui.checkbox(&mut settings.video.detached_video, "Detached video window");
                    ui.horizontal(|ui| {
                        ui.label("Fast jump step (frames):");
                        ui.add(
                            egui::DragValue::new(&mut settings.video.fast_jump_step)
                                .range(1..=1000),
                        );
                    });
                }
                PrefsPage::InterfaceColours => {
                    ui.heading(
                        catalog
                            .get("prefs-page-interface")
                            .unwrap_or("Interface / Colours"),
                    );
                    let mut changed = false;
                    ui.horizontal(|ui| {
                        if ui
                            .selectable_label(
                                settings.appearance.theme_mode == ThemeMode::Light,
                                "Light",
                            )
                            .clicked()
                        {
                            settings.appearance.theme_mode = ThemeMode::Light;
                            changed = true;
                        }
                        if ui
                            .selectable_label(
                                settings.appearance.theme_mode == ThemeMode::Dark,
                                "Dark",
                            )
                            .clicked()
                        {
                            settings.appearance.theme_mode = ThemeMode::Dark;
                            changed = true;
                        }
                        if ui
                            .selectable_label(
                                settings.appearance.theme_mode == ThemeMode::EyeComfortDark,
                                "Eye Comfort",
                            )
                            .clicked()
                        {
                            settings.appearance.theme_mode = ThemeMode::EyeComfortDark;
                            changed = true;
                        }
                    });
                    if ui
                        .add(
                            egui::Slider::new(
                                &mut settings.appearance.eye_comfort_strength,
                                0..=100,
                            )
                            .text("Eye Comfort strength")
                            .suffix("%"),
                        )
                        .changed()
                    {
                        changed = true;
                    }
                    if changed {
                        let spec = match settings.appearance.theme_mode {
                            ThemeMode::Light => ThemeSpec::Light,
                            ThemeMode::Dark | ThemeMode::System => ThemeSpec::Dark,
                            ThemeMode::EyeComfortLight => ThemeSpec::EyeComfort {
                                base: BaseTheme::Light,
                                strength: settings.appearance.eye_comfort_strength,
                            },
                            ThemeMode::EyeComfortDark => ThemeSpec::EyeComfort {
                                base: BaseTheme::Dark,
                                strength: settings.appearance.eye_comfort_strength,
                            },
                        };
                        let _ = theme_ctl.set_spec(spec);
                    }
                    ui.checkbox(
                        &mut settings.appearance.show_performance_overlay,
                        "Show performance overlay in status bar",
                    );
                    ui.separator();
                    ui.strong("Grid");
                    ui.horizontal(|ui| {
                        ui.label("Hidden tag marker:");
                        let current_label = match settings.interface.hidden_tag_marker {
                            HiddenTagMarker::AegisubCircle => "Aegisub circle",
                            HiddenTagMarker::SmallDot => "Small dot",
                            HiddenTagMarker::AegisubSun => "Aegisub sun",
                            HiddenTagMarker::None => "None",
                        };
                        egui::ComboBox::from_id_salt("prefs_hidden_tag_marker")
                            .selected_text(current_label)
                            .show_ui(ui, |ui| {
                                ui.selectable_value(
                                    &mut settings.interface.hidden_tag_marker,
                                    HiddenTagMarker::AegisubCircle,
                                    "Aegisub circle",
                                );
                                ui.selectable_value(
                                    &mut settings.interface.hidden_tag_marker,
                                    HiddenTagMarker::SmallDot,
                                    "Small dot",
                                );
                                ui.selectable_value(
                                    &mut settings.interface.hidden_tag_marker,
                                    HiddenTagMarker::AegisubSun,
                                    "Aegisub sun",
                                );
                                ui.selectable_value(
                                    &mut settings.interface.hidden_tag_marker,
                                    HiddenTagMarker::None,
                                    "None",
                                );
                            });
                    });
                    ui.checkbox(
                        &mut settings.interface.hide_karaoke_only_markers,
                        r"Hide markers for karaoke-only blocks ({\k \kf \ko \K} with nothing else)",
                    );
                }
                PrefsPage::Hotkeys => {
                    ui.heading(catalog.get("prefs-page-hotkeys").unwrap_or("Hotkeys"));
                    ui.text_edit_singleline(&mut state.hotkey_filter);
                    let q = state.hotkey_filter.to_ascii_lowercase();
                    ScrollArea::vertical().max_height(200.0).show(ui, |ui| {
                        for cmd in commands::COMMANDS.iter().take(64) {
                            if q.is_empty() || cmd.id.to_ascii_lowercase().contains(&q) {
                                ui.label(format!("{} — {}", cmd.id, cmd.ftl_name));
                            }
                        }
                        if !commands.conflicts().is_empty() {
                            ui.label(format!("Conflicts detected: {}", commands.conflicts().len()));
                        }
                    });
                }
                PrefsPage::Backup => {
                    ui.heading(
                        catalog
                            .get("prefs-page-backup")
                            .unwrap_or("Backup & Autosave"),
                    );
                    ui.checkbox(&mut settings.autosave.enabled, "Enable autosave");
                    ui.checkbox(&mut settings.autosave.backup_enabled, "Enable .bak backups");
                    ui.horizontal(|ui| {
                        ui.label("Autosave interval (s):");
                        ui.add(
                            egui::DragValue::new(&mut settings.autosave.interval_seconds)
                                .range(10..=3600),
                        );
                    });
                }
                PrefsPage::Advanced => {
                    ui.heading(catalog.get("prefs-page-advanced").unwrap_or("Advanced"));
                    ui.label(
                        "Native Rust tools & Karaoke Templater sandbox limits (no legacy Automation).",
                    );
                    ui.horizontal(|ui| {
                        ui.label("Audio cache RAM (MiB):");
                        ui.add(
                            egui::DragValue::new(&mut settings.audio.cache_ram_mb)
                                .range(32..=4096),
                        );
                    });
                }
            }
        });
    });

    ui.separator();
    ui.horizontal(|ui| {
        if ui.button(catalog.get("btn-ok").unwrap_or("OK")).clicked() {
            settings.validate_and_clamp();
            action = PrefsDialogAction::ApplyAndClose;
        }
        if ui
            .button(catalog.get("btn-cancel").unwrap_or("Cancel"))
            .clicked()
        {
            action = PrefsDialogAction::Cancel;
        }
    });

    action
}

#[cfg(test)]
mod tests {
    use super::*;
    use egui_kittest::Harness;
    use i18n_catalog::Locale;
    use ui_theme::FontConfig;

    #[test]
    fn import_aegisub_config_and_render_all_7_pages() {
        let dir = tempfile::tempdir().expect("tempdir");
        let cfg_path = dir.path().join("config.json");
        fs::write(
            &cfg_path,
            r#"{"Audio":{"Lead":{"IN":250,"OUT":350}},"Subtitle":{"Grid":{"Hide Overrides":"*"}}}"#,
        )
        .expect("write config.json");

        let mut settings = AppSettings::default();
        let msg = import_aegisub_settings_from_path(&cfg_path, &mut settings).expect("import");
        assert!(msg.contains("250"));
        assert_eq!(settings.audio.lead_in_ms, 250);
        assert_eq!(settings.audio.lead_out_ms, 350);

        let mut theme_ctl = ThemeController::new(ThemeSpec::Dark, FontConfig::default());
        let commands = CommandRegistry::new_default();
        let cat = Catalog::for_locale(Locale::En);
        let mut state = PreferencesDialogState {
            aegisub_config_path: cfg_path,
            ..Default::default()
        };

        for page in PrefsPage::ALL {
            state.active_page = page;
            let builder = Harness::builder().with_accessibility_check(false);
            let mut harness = builder.build_ui(|ui| {
                let _ = show_preferences_dialog(
                    ui,
                    &mut state,
                    &mut settings,
                    &mut theme_ctl,
                    &commands,
                    &cat,
                );
            });
            harness.run();
        }
    }
}
