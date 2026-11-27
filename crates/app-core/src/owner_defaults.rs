//! First-run detection and ask-before-importing flow for Aegisub 3.5.1 `config.json` +
//! `hotkey.json` into [`crate::EditorApp`].

use crate::{AppError, EditorApp};
use aegisub_config::{AegisubImportReport, detect_aegisub_config_dir, import_aegisub_dir};
use std::path::{Path, PathBuf};

/// Pending first-run import prompt state (never silently overwrites user settings).
#[derive(Debug, Clone, Default)]
pub struct FirstRunImportState {
    pub detected_dir: Option<PathBuf>,
    pub prompt_open: bool,
    pub last_report: Option<AegisubImportReport>,
}

impl EditorApp {
    /// Checks for an existing Aegisub 3.5.1 config directory (using `override_dir` if provided,
    /// otherwise `%APPDATA%\Aegisub` / `~/.aegisub`) and opens the ask-before-importing prompt.
    pub fn check_first_run_aegisub_import(&mut self, override_dir: Option<&Path>) -> bool {
        let found = override_dir
            .map(Path::to_path_buf)
            .filter(|d| d.join("config.json").is_file() || d.join("hotkey.json").is_file())
            .or_else(detect_aegisub_config_dir);
        if let Some(dir) = found {
            self.first_run_import.detected_dir = Some(dir);
            self.first_run_import.prompt_open = true;
            true
        } else {
            false
        }
    }

    /// Accepts the pending first-run Aegisub import, applying columns, fonts, colours, audio/video
    /// options, timing defaults, autosave, and hotkeys by context, and returning the report of
    /// mapped + unmapped items.
    pub fn accept_aegisub_import(&mut self) -> Result<AegisubImportReport, AppError> {
        let dir =
            self.first_run_import.detected_dir.clone().ok_or_else(|| {
                AppError::Settings("no Aegisub config directory detected".to_owned())
            })?;
        let (imported, report) =
            import_aegisub_dir(&dir).map_err(|e| AppError::Settings(e.to_string()))?;

        self.shell.settings.import_from_aegisub(&imported);
        self.shell.settings.validate_and_clamp();
        self.apply_settings_to_shell();

        // Sync grid Layer column + audio spectrum + layout directly to live shell widgets.
        self.shell.grid.model.columns.layer = self.shell.settings.interface.show_col_layer;
        self.shell.audio.spectrum_mode = self.shell.settings.audio.spectrum;
        self.shell.audio.auto_commit = self.shell.settings.audio.auto_commit;

        self.first_run_import.prompt_open = false;
        self.first_run_import.last_report = Some(report.clone());
        self.shell.status_message = format!(
            "Imported Aegisub config ({} hotkeys mapped, {} unmapped)",
            report.mapped_hotkeys,
            report.unmapped_hotkeys.len()
        );
        Ok(report)
    }

    /// Declines the first-run Aegisub import without changing current settings.
    pub fn decline_aegisub_import(&mut self) {
        self.first_run_import.prompt_open = false;
    }
}
