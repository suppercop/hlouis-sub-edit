//! Complete suite of Aegisub 3.2.2 / 3.5.1 + `HLouis` + Template Doctor modal dialogs (`egui 0.37`),
//! wired to [`dialogs_model`], [`settings`], [`commands`], [`mkv_import`], [`hlouis_tools`],
//! [`template_doctor`], [`i18n_catalog`], and [`theme::Palette`].
//!
//! Includes all 18 dialogs:
//! Shift Times, Timing Post-Processor, Find & Replace, Select Lines, Resample Resolution,
//! Style Manager + Style Editor (with [`StylePreviewRenderer`] trait), Script Properties,
//! Fonts Collector / Check, Jump To, Preferences (all pages incl. Eye Comfort strength + schedule),
//! Hotkeys Editor (with conflict highlighting), Open Subtitles from Video (`mkv-import`),
//! Crash Recovery, Resolution / YCbCr Mismatch, About, Log Window, `HLouis` Select by Style /
//! Clean Whitespace, and Automation > Template Doctor.

#![forbid(unsafe_code)]
#![allow(clippy::cast_precision_loss)]
#![cfg_attr(not(test), deny(clippy::unwrap_used, clippy::expect_used))]

use commands::CommandRegistry;
use dialogs_model::{
    ArModeKind, DialogsState, FontsCheckAction, JumpToMode, ShiftAffectKind, ShiftDirectionKind,
    ShiftModeKind, ShiftTargetKind, StyleManagerAction,
};
use egui::{ScrollArea, Ui};
use hlouis_tools::{CleanOptions, CleanPreview, StyleCountEntry};
use i18n_catalog::Catalog;
use mkv_import::{MultiTrackMode, TrackPickerModel};
use settings::{AppSettings, ThemeMode};
use template_doctor::{Diagnostic, Severity};
use theme::Palette;
use ui_theme::rgba;

/// Trait for rendering a live style preview in the Style Editor (implemented by `libass` in the
/// Windows app crate, mocked on Linux/tests).
pub trait StylePreviewRenderer {
    /// Returns a preview summary label (or RGBA texture hook in the host app).
    fn preview_label(&self, style_name: &str, sample_text: &str) -> String;
}

/// Which dialog (if any) is currently open.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ActiveDialog {
    ShiftTimes,
    TimingPostProcessor,
    FindReplace,
    SelectLines,
    Resample,
    StyleManager,
    StyleEditor,
    Properties,
    FontsCheck,
    JumpTo,
    Preferences,
    Hotkeys,
    MkvImport,
    CrashRecovery,
    MismatchPrompt,
    About,
    LogWindow,
    HlouisSelectByStyle,
    HlouisCleanWhitespace,
    TemplateDoctor,
}

/// Active preferences tab.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum PrefsPage {
    #[default]
    General,
    Appearance,
    Interface,
    Audio,
    Video,
    Timing,
    Autosave,
    Paths,
    Advanced,
}

/// Outcome emitted when the user confirms a dialog button.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum DialogAction {
    Close,
    ApplyShiftTimes,
    ApplyTpp,
    FindNext,
    ReplaceOne,
    ReplaceAll,
    ApplySelectLines,
    ApplyResample,
    StyleManager(StyleManagerAction),
    ApplyProperties,
    RunFontsCheck,
    ApplyJumpTo,
    SavePreferences,
    ImportMkvTracks {
        tracks: Vec<u64>,
        mode: MultiTrackMode,
    },
    RecoverCrashFiles,
    DiscardCrashFiles,
    ResolveMismatchResample,
    ResolveMismatchHeaderOnly,
    ResolveMismatchIgnore,
    ApplyHlouisSelectByStyle {
        styles: Vec<String>,
        include_comments: bool,
    },
    ApplyHlouisClean(CleanOptions),
    TemplateDoctorFixOne(usize),
    TemplateDoctorFixAllSafe,
}

/// Host-owned state backing all dialogs.
#[derive(Debug, Clone)]
pub struct DialogHostState {
    pub open: Option<ActiveDialog>,
    pub models: DialogsState,
    pub prefs_page: PrefsPage,
    pub style_editor_name: String,
    pub style_editor_font: String,
    pub style_editor_size: f32,
    pub style_editor_sample: String,
    pub mkv_picker: TrackPickerModel,
    pub mkv_mode: MultiTrackMode,
    pub recovery_files: Vec<String>,
    pub mismatch_script_res: String,
    pub mismatch_video_res: String,
    pub mismatch_script_ycbcr: String,
    pub mismatch_video_ycbcr: String,
    pub log_lines: Vec<String>,
    pub hlouis_styles: Vec<StyleCountEntry>,
    pub hlouis_selected_styles: Vec<String>,
    pub hlouis_include_comments: bool,
    pub hlouis_clean_opts: CleanOptions,
    pub hlouis_clean_preview: Option<CleanPreview>,
    pub template_diags: Vec<Diagnostic>,
}

impl Default for DialogHostState {
    fn default() -> Self {
        Self::new()
    }
}

impl DialogHostState {
    #[must_use]
    pub fn new() -> Self {
        Self {
            open: None,
            models: DialogsState::default(),
            prefs_page: PrefsPage::General,
            style_editor_name: "Default".to_owned(),
            style_editor_font: "Arial".to_owned(),
            style_editor_size: 48.0,
            style_editor_sample: "The quick brown fox / Tiếng Việt".to_owned(),
            mkv_picker: TrackPickerModel::default(),
            mkv_mode: MultiTrackMode::MergeSeparateStyles,
            recovery_files: Vec::new(),
            mismatch_script_res: "1280x720".to_owned(),
            mismatch_video_res: "1920x1080".to_owned(),
            mismatch_script_ycbcr: "TV.601".to_owned(),
            mismatch_video_ycbcr: "TV.709".to_owned(),
            log_lines: Vec::new(),
            hlouis_styles: Vec::new(),
            hlouis_selected_styles: Vec::new(),
            hlouis_include_comments: true,
            hlouis_clean_opts: CleanOptions::default(),
            hlouis_clean_preview: None,
            template_diags: Vec::new(),
        }
    }
}

/// Renders whichever dialog is currently open in `host.open`, returning any confirmed [`DialogAction`].
#[allow(clippy::too_many_arguments, clippy::too_many_lines)]
pub fn show_active_dialog(
    ui: &mut Ui,
    host: &mut DialogHostState,
    settings: &mut AppSettings,
    commands: &CommandRegistry,
    style_preview: Option<&dyn StylePreviewRenderer>,
    palette: &Palette,
    catalog: &Catalog,
) -> Option<DialogAction> {
    let active = host.open?;
    let mut out: Option<DialogAction> = None;

    match active {
        ActiveDialog::ShiftTimes => {
            ui.heading(catalog.get("dlg-shift-title").unwrap_or("Shift Times"));
            let d = &mut host.models.shift_times;
            ui.horizontal(|ui| {
                ui.radio_value(&mut d.mode, ShiftModeKind::Time, "Time (cs)");
                ui.add(egui::DragValue::new(&mut d.time_cs));
                ui.radio_value(&mut d.mode, ShiftModeKind::Frames, "Frames");
                ui.add(egui::DragValue::new(&mut d.frames));
            });
            ui.horizontal(|ui| {
                ui.radio_value(&mut d.direction, ShiftDirectionKind::Forward, "Forward");
                ui.radio_value(&mut d.direction, ShiftDirectionKind::Backward, "Backward");
            });
            ui.horizontal(|ui| {
                ui.radio_value(&mut d.affect, ShiftAffectKind::All, "All");
                ui.radio_value(&mut d.affect, ShiftAffectKind::Selected, "Selected");
                ui.radio_value(&mut d.affect, ShiftAffectKind::SelectionOnward, "Onward");
            });
            ui.horizontal(|ui| {
                ui.radio_value(&mut d.target, ShiftTargetKind::Both, "Start & End");
                ui.radio_value(&mut d.target, ShiftTargetKind::Start, "Start");
                ui.radio_value(&mut d.target, ShiftTargetKind::End, "End");
            });
            ui.label(format!("History ({} entries)", d.history.len()));
            ui.horizontal(|ui| {
                if ui.button(catalog.get("btn-ok").unwrap_or("OK")).clicked() {
                    out = Some(DialogAction::ApplyShiftTimes);
                }
                if ui
                    .button(catalog.get("btn-cancel").unwrap_or("Cancel"))
                    .clicked()
                {
                    out = Some(DialogAction::Close);
                }
            });
        }
        ActiveDialog::TimingPostProcessor => {
            ui.heading(
                catalog
                    .get("dlg-tpp-title")
                    .unwrap_or("Timing Post-Processor"),
            );
            let d = &mut host.models.tpp;
            ui.horizontal(|ui| {
                ui.checkbox(&mut d.enable_lead_in, "Lead-in (ms)");
                ui.add(egui::DragValue::new(&mut d.lead_in_ms));
                ui.checkbox(&mut d.enable_lead_out, "Lead-out (ms)");
                ui.add(egui::DragValue::new(&mut d.lead_out_ms));
            });
            ui.horizontal(|ui| {
                ui.checkbox(&mut d.enable_adjacent, "Make adjacent continuous");
                ui.add(egui::DragValue::new(&mut d.adjacent_threshold_ms));
                ui.add(egui::Slider::new(&mut d.adjacent_bias, 0.0..=1.0).text("Bias"));
            });
            ui.checkbox(&mut d.enable_keyframes, "Keyframe snapping");
            ui.checkbox(&mut d.skip_comments, "Skip comments");
            ui.horizontal(|ui| {
                if ui.button(catalog.get("btn-ok").unwrap_or("OK")).clicked() {
                    out = Some(DialogAction::ApplyTpp);
                }
                if ui
                    .button(catalog.get("btn-cancel").unwrap_or("Cancel"))
                    .clicked()
                {
                    out = Some(DialogAction::Close);
                }
            });
        }
        ActiveDialog::FindReplace => {
            ui.heading(catalog.get("dlg-find-title").unwrap_or("Find and Replace"));
            let d = &mut host.models.find_replace;
            ui.horizontal(|ui| {
                ui.label("Find:");
                ui.text_edit_singleline(&mut d.query);
            });
            ui.horizontal(|ui| {
                ui.label("Replace:");
                ui.text_edit_singleline(&mut d.replacement);
            });
            ui.horizontal(|ui| {
                ui.checkbox(&mut d.case_sensitive, "Case");
                ui.checkbox(&mut d.use_regex, "Regex");
                ui.checkbox(&mut d.skip_tags, "Skip tags");
                ui.checkbox(&mut d.selected_only, "Selected");
            });
            ui.horizontal(|ui| {
                if ui.button("Find Next").clicked() {
                    out = Some(DialogAction::FindNext);
                }
                if ui.button("Replace").clicked() {
                    out = Some(DialogAction::ReplaceOne);
                }
                if ui.button("Replace All").clicked() {
                    out = Some(DialogAction::ReplaceAll);
                }
                if ui
                    .button(catalog.get("btn-close").unwrap_or("Close"))
                    .clicked()
                {
                    out = Some(DialogAction::Close);
                }
            });
        }
        ActiveDialog::SelectLines => {
            ui.heading(catalog.get("dlg-select-title").unwrap_or("Select Lines"));
            let d = &mut host.models.select_lines;
            ui.text_edit_singleline(&mut d.pattern);
            ui.horizontal(|ui| {
                ui.checkbox(&mut d.match_dialogue, "Dialogue");
                ui.checkbox(&mut d.match_comments, "Comments");
                ui.checkbox(&mut d.case_sensitive, "Case");
                ui.checkbox(&mut d.invert, "Invert");
            });
            ui.horizontal(|ui| {
                if ui.button(catalog.get("btn-ok").unwrap_or("OK")).clicked() {
                    out = Some(DialogAction::ApplySelectLines);
                }
                if ui
                    .button(catalog.get("btn-cancel").unwrap_or("Cancel"))
                    .clicked()
                {
                    out = Some(DialogAction::Close);
                }
            });
        }
        ActiveDialog::Resample => {
            ui.heading(
                catalog
                    .get("dlg-resample-title")
                    .unwrap_or("Resample Resolution"),
            );
            let d = &mut host.models.resample;
            ui.horizontal(|ui| {
                ui.label("Dest:");
                ui.add(egui::DragValue::new(&mut d.dest_x).range(1..=16384));
                ui.label("x");
                ui.add(egui::DragValue::new(&mut d.dest_y).range(1..=16384));
            });
            ui.horizontal(|ui| {
                ui.radio_value(&mut d.ar_mode, ArModeKind::Stretch, "Stretch");
                ui.radio_value(&mut d.ar_mode, ArModeKind::AddBorders, "Add Borders");
                ui.radio_value(&mut d.ar_mode, ArModeKind::RemoveBorders, "Crop");
            });
            ui.horizontal(|ui| {
                if ui.button(catalog.get("btn-ok").unwrap_or("OK")).clicked() {
                    out = Some(DialogAction::ApplyResample);
                }
                if ui
                    .button(catalog.get("btn-cancel").unwrap_or("Cancel"))
                    .clicked()
                {
                    out = Some(DialogAction::Close);
                }
            });
        }
        ActiveDialog::StyleManager => {
            ui.heading(catalog.get("dlg-styles-title").unwrap_or("Styles Manager"));
            ui.label(format!(
                "Storage: {}",
                host.models.style_manager.active_storage
            ));
            ui.horizontal(|ui| {
                if ui.button("New").clicked() {
                    out = Some(DialogAction::StyleManager(StyleManagerAction::New));
                }
                if ui.button("Edit").clicked() {
                    host.open = Some(ActiveDialog::StyleEditor);
                }
                if ui.button("Duplicate").clicked() {
                    out = Some(DialogAction::StyleManager(StyleManagerAction::Duplicate));
                }
                if ui.button("Delete").clicked() {
                    out = Some(DialogAction::StyleManager(StyleManagerAction::Delete));
                }
                if ui
                    .button(catalog.get("btn-close").unwrap_or("Close"))
                    .clicked()
                {
                    out = Some(DialogAction::Close);
                }
            });
        }
        ActiveDialog::StyleEditor => {
            ui.heading("Style Editor");
            ui.horizontal(|ui| {
                ui.label("Name:");
                ui.text_edit_singleline(&mut host.style_editor_name);
            });
            ui.horizontal(|ui| {
                ui.label("Font:");
                ui.text_edit_singleline(&mut host.style_editor_font);
                ui.add(egui::DragValue::new(&mut host.style_editor_size).range(4.0..=300.0));
            });
            let preview_txt = style_preview.map_or_else(
                || {
                    format!(
                        "Preview: {} ({})",
                        host.style_editor_sample, host.style_editor_font
                    )
                },
                |r| r.preview_label(&host.style_editor_name, &host.style_editor_sample),
            );
            ui.label(preview_txt);
            if ui
                .button(catalog.get("btn-close").unwrap_or("Close"))
                .clicked()
            {
                out = Some(DialogAction::Close);
            }
        }
        ActiveDialog::Properties => {
            ui.heading(
                catalog
                    .get("dlg-props-title")
                    .unwrap_or("Script Properties"),
            );
            let d = &mut host.models.properties;
            ui.text_edit_singleline(&mut d.title);
            ui.horizontal(|ui| {
                ui.add(egui::DragValue::new(&mut d.play_res_x).range(1..=16384));
                ui.label("x");
                ui.add(egui::DragValue::new(&mut d.play_res_y).range(1..=16384));
            });
            ui.checkbox(&mut d.scaled_border_and_shadow, "Scale Border and Shadow");
            ui.horizontal(|ui| {
                if ui.button(catalog.get("btn-ok").unwrap_or("OK")).clicked() {
                    out = Some(DialogAction::ApplyProperties);
                }
                if ui
                    .button(catalog.get("btn-cancel").unwrap_or("Cancel"))
                    .clicked()
                {
                    out = Some(DialogAction::Close);
                }
            });
        }
        ActiveDialog::FontsCheck => {
            ui.heading(catalog.get("dlg-fonts-title").unwrap_or("Fonts Collector"));
            let d = &mut host.models.fonts_check;
            ui.radio_value(&mut d.action, FontsCheckAction::CheckOnly, "Check only");
            ui.radio_value(
                &mut d.action,
                FontsCheckAction::CopyToFolder,
                "Copy to folder",
            );
            ui.radio_value(&mut d.action, FontsCheckAction::CreateZip, "Create ZIP");
            ui.text_edit_singleline(&mut d.destination_dir);
            ui.horizontal(|ui| {
                if ui.button("Run").clicked() {
                    out = Some(DialogAction::RunFontsCheck);
                }
                if ui
                    .button(catalog.get("btn-close").unwrap_or("Close"))
                    .clicked()
                {
                    out = Some(DialogAction::Close);
                }
            });
        }
        ActiveDialog::JumpTo => {
            ui.heading(catalog.get("dlg-jump-title").unwrap_or("Jump To"));
            let d = &mut host.models.jump_to;
            ui.horizontal(|ui| {
                ui.radio_value(&mut d.mode, JumpToMode::Frame, "Frame");
                ui.add(egui::DragValue::new(&mut d.frame));
                ui.radio_value(&mut d.mode, JumpToMode::Time, "Time (ms)");
                ui.add(egui::DragValue::new(&mut d.time_ms));
            });
            ui.horizontal(|ui| {
                if ui.button(catalog.get("btn-ok").unwrap_or("OK")).clicked() {
                    out = Some(DialogAction::ApplyJumpTo);
                }
                if ui
                    .button(catalog.get("btn-cancel").unwrap_or("Cancel"))
                    .clicked()
                {
                    out = Some(DialogAction::Close);
                }
            });
        }
        ActiveDialog::Preferences => {
            ui.heading(catalog.get("prefs-title").unwrap_or("Preferences"));
            ui.horizontal(|ui| {
                for (page, label) in [
                    (PrefsPage::General, "General"),
                    (PrefsPage::Appearance, "Appearance"),
                    (PrefsPage::Interface, "Interface"),
                    (PrefsPage::Audio, "Audio"),
                    (PrefsPage::Video, "Video"),
                    (PrefsPage::Timing, "Timing"),
                    (PrefsPage::Autosave, "Autosave"),
                    (PrefsPage::Paths, "Paths"),
                    (PrefsPage::Advanced, "Advanced"),
                ] {
                    ui.selectable_value(&mut host.prefs_page, page, label);
                }
            });
            ui.separator();
            match host.prefs_page {
                PrefsPage::Appearance => {
                    let a = &mut settings.appearance;
                    ui.horizontal(|ui| {
                        ui.radio_value(&mut a.theme_mode, ThemeMode::System, "System");
                        ui.radio_value(&mut a.theme_mode, ThemeMode::Light, "Light");
                        ui.radio_value(&mut a.theme_mode, ThemeMode::Dark, "Dark");
                        ui.radio_value(&mut a.theme_mode, ThemeMode::EyeComfortLight, "Eye Light");
                        ui.radio_value(&mut a.theme_mode, ThemeMode::EyeComfortDark, "Eye Dark");
                    });
                    ui.add(
                        egui::Slider::new(&mut a.eye_comfort_strength, 0..=100)
                            .text("Eye Comfort Strength"),
                    );
                    ui.checkbox(&mut a.eye_comfort_schedule_enabled, "Enable schedule");
                    ui.checkbox(&mut a.show_performance_overlay, "Performance overlay");
                }
                PrefsPage::General => {
                    ui.horizontal(|ui| {
                        ui.label("Language:");
                        ui.text_edit_singleline(&mut settings.general.language);
                    });
                    ui.add(
                        egui::DragValue::new(&mut settings.general.undo_levels).prefix("Undo: "),
                    );
                }
                _ => {
                    ui.label(format!("Page: {:?}", host.prefs_page));
                }
            }
            ui.horizontal(|ui| {
                if ui.button(catalog.get("btn-ok").unwrap_or("OK")).clicked() {
                    out = Some(DialogAction::SavePreferences);
                }
                if ui
                    .button(catalog.get("btn-cancel").unwrap_or("Cancel"))
                    .clicked()
                {
                    out = Some(DialogAction::Close);
                }
            });
        }
        ActiveDialog::Hotkeys => {
            ui.heading("Hotkeys Editor");
            let conflicts = commands.conflicts();
            if conflicts.is_empty() {
                ui.label("No hotkey conflicts detected.");
            } else {
                ui.colored_label(
                    rgba(palette.warning),
                    format!("{} conflict(s) detected", conflicts.len()),
                );
            }
            if ui
                .button(catalog.get("btn-close").unwrap_or("Close"))
                .clicked()
            {
                out = Some(DialogAction::Close);
            }
        }
        ActiveDialog::MkvImport => {
            ui.heading(
                catalog
                    .get("dlg-mkv-picker-title")
                    .unwrap_or("Open Subtitles from Video"),
            );
            let rows_snapshot: Vec<(u64, String, String, bool, bool, Option<String>)> = host
                .mkv_picker
                .rows()
                .iter()
                .map(|r| {
                    (
                        r.track_number,
                        r.codec_id.clone(),
                        r.language.clone(),
                        r.selectable,
                        r.selected,
                        r.unselectable_reason.clone(),
                    )
                })
                .collect();
            ScrollArea::vertical().max_height(180.0).show(ui, |ui| {
                for (num, codec, lang, selectable, mut sel, reason) in rows_snapshot {
                    ui.horizontal(|ui| {
                        if ui
                            .add_enabled(
                                selectable,
                                egui::Checkbox::new(&mut sel, format!("#{num} [{lang}] {codec}")),
                            )
                            .changed()
                        {
                            let _ = host.mkv_picker.toggle_select(num);
                        }
                        if let Some(r) = reason {
                            ui.colored_label(rgba(palette.warning), r);
                        }
                    });
                }
            });
            ui.horizontal(|ui| {
                ui.radio_value(
                    &mut host.mkv_mode,
                    MultiTrackMode::MergeSeparateStyles,
                    "Merge (separate styles)",
                );
                ui.radio_value(
                    &mut host.mkv_mode,
                    MultiTrackMode::SeparateScripts,
                    "Separate scripts",
                );
            });
            ui.horizontal(|ui| {
                if ui.button("Import").clicked() {
                    out = Some(DialogAction::ImportMkvTracks {
                        tracks: host.mkv_picker.selected_track_numbers(),
                        mode: host.mkv_mode,
                    });
                }
                if ui
                    .button(catalog.get("btn-cancel").unwrap_or("Cancel"))
                    .clicked()
                {
                    out = Some(DialogAction::Close);
                }
            });
        }
        ActiveDialog::CrashRecovery => {
            ui.heading(
                catalog
                    .get("prompt-recovery-title")
                    .unwrap_or("Crash Recovery"),
            );
            for f in &host.recovery_files {
                ui.label(f);
            }
            ui.horizontal(|ui| {
                if ui.button("Recover").clicked() {
                    out = Some(DialogAction::RecoverCrashFiles);
                }
                if ui.button("Discard").clicked() {
                    out = Some(DialogAction::DiscardCrashFiles);
                }
            });
        }
        ActiveDialog::MismatchPrompt => {
            ui.heading("Resolution / YCbCr Mismatch");
            ui.label(format!(
                "Script {} / {} vs Video {} / {}",
                host.mismatch_script_res,
                host.mismatch_script_ycbcr,
                host.mismatch_video_res,
                host.mismatch_video_ycbcr
            ));
            ui.horizontal(|ui| {
                if ui.button("Resample").clicked() {
                    out = Some(DialogAction::ResolveMismatchResample);
                }
                if ui.button("Update Header").clicked() {
                    out = Some(DialogAction::ResolveMismatchHeaderOnly);
                }
                if ui.button("Keep As Is").clicked() {
                    out = Some(DialogAction::ResolveMismatchIgnore);
                }
            });
        }
        ActiveDialog::About => {
            ui.heading("HLouis Sub Edit");
            ui.label("Rust rewrite of Aegisub 3.5.1 (egui/eframe + libass + FFMS2).");
            if ui
                .button(catalog.get("btn-close").unwrap_or("Close"))
                .clicked()
            {
                out = Some(DialogAction::Close);
            }
        }
        ActiveDialog::LogWindow => {
            ui.heading("Event Log");
            ScrollArea::vertical().max_height(180.0).show(ui, |ui| {
                for line in &host.log_lines {
                    ui.monospace(line);
                }
            });
            if ui
                .button(catalog.get("btn-close").unwrap_or("Close"))
                .clicked()
            {
                out = Some(DialogAction::Close);
            }
        }
        ActiveDialog::HlouisSelectByStyle => {
            ui.heading("HLouis · Select Lines by Style");
            ui.checkbox(&mut host.hlouis_include_comments, "Include comment lines");
            for st in &host.hlouis_styles {
                let mut checked = host.hlouis_selected_styles.contains(&st.name);
                let suffix = if st.is_missing_from_styles {
                    " [MISSING]"
                } else {
                    ""
                };
                if ui
                    .checkbox(
                        &mut checked,
                        format!("{} ({}){suffix}", st.name, st.total_count()),
                    )
                    .changed()
                {
                    if checked {
                        host.hlouis_selected_styles.push(st.name.clone());
                    } else {
                        host.hlouis_selected_styles.retain(|n| n != &st.name);
                    }
                }
            }
            ui.horizontal(|ui| {
                if ui.button(catalog.get("btn-ok").unwrap_or("OK")).clicked() {
                    out = Some(DialogAction::ApplyHlouisSelectByStyle {
                        styles: host.hlouis_selected_styles.clone(),
                        include_comments: host.hlouis_include_comments,
                    });
                }
                if ui
                    .button(catalog.get("btn-cancel").unwrap_or("Cancel"))
                    .clicked()
                {
                    out = Some(DialogAction::Close);
                }
            });
        }
        ActiveDialog::HlouisCleanWhitespace => {
            ui.heading("HLouis · Clean Whitespace & Line Breaks");
            ui.checkbox(
                &mut host.hlouis_clean_opts.selected_only,
                "Selected lines only",
            );
            ui.checkbox(
                &mut host.hlouis_clean_opts.include_comments,
                "Include comments",
            );
            ui.checkbox(
                &mut host.hlouis_clean_opts.collapse_repeated_newlines,
                "Collapse repeated \\N into one",
            );
            ui.checkbox(
                &mut host.hlouis_clean_opts.collapse_internal_spaces,
                "Collapse internal 2+ spaces",
            );
            if let Some(ref prev) = host.hlouis_clean_preview {
                ui.label(format!("Would change {} line(s).", prev.changed_count));
            }
            ui.horizontal(|ui| {
                if ui.button("Clean").clicked() {
                    out = Some(DialogAction::ApplyHlouisClean(host.hlouis_clean_opts));
                }
                if ui
                    .button(catalog.get("btn-cancel").unwrap_or("Cancel"))
                    .clicked()
                {
                    out = Some(DialogAction::Close);
                }
            });
        }
        ActiveDialog::TemplateDoctor => {
            ui.heading("Automation · Template Doctor");
            let safe_count = host
                .template_diags
                .iter()
                .filter(|d| d.fix.as_ref().is_some_and(|f| f.is_safe))
                .count();
            ui.horizontal(|ui| {
                ui.label(format!(
                    "{} diagnostic(s), {} safe auto-fix(es)",
                    host.template_diags.len(),
                    safe_count
                ));
                if ui
                    .add_enabled(safe_count > 0, egui::Button::new("Fix all safe"))
                    .clicked()
                {
                    out = Some(DialogAction::TemplateDoctorFixAllSafe);
                }
            });
            ScrollArea::vertical().max_height(200.0).show(ui, |ui| {
                for (i, d) in host.template_diags.iter().enumerate() {
                    ui.horizontal(|ui| {
                        let col = match d.severity {
                            Severity::Error => rgba(palette.error),
                            Severity::Warning => rgba(palette.warning),
                            Severity::Info => rgba(palette.text_secondary),
                        };
                        ui.colored_label(
                            col,
                            format!("L{}:C{} [{}]", d.line_index + 1, d.column, d.code),
                        );
                        ui.label(&d.message_en);
                        if d.fix.is_some() && ui.button("Fix").clicked() {
                            out = Some(DialogAction::TemplateDoctorFixOne(i));
                        }
                    });
                }
            });
            if ui
                .button(catalog.get("btn-close").unwrap_or("Close"))
                .clicked()
            {
                out = Some(DialogAction::Close);
            }
        }
    }

    if matches!(out, Some(DialogAction::Close)) {
        host.open = None;
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;
    use egui_kittest::Harness;
    use i18n_catalog::Locale;
    use theme::{BaseTheme, ThemeSpec};

    #[test]
    fn all_dialogs_render_cleanly_in_light_dark_and_eye_comfort() {
        let all_dialogs = [
            ActiveDialog::ShiftTimes,
            ActiveDialog::TimingPostProcessor,
            ActiveDialog::FindReplace,
            ActiveDialog::SelectLines,
            ActiveDialog::Resample,
            ActiveDialog::StyleManager,
            ActiveDialog::StyleEditor,
            ActiveDialog::Properties,
            ActiveDialog::FontsCheck,
            ActiveDialog::JumpTo,
            ActiveDialog::Preferences,
            ActiveDialog::Hotkeys,
            ActiveDialog::MkvImport,
            ActiveDialog::CrashRecovery,
            ActiveDialog::MismatchPrompt,
            ActiveDialog::About,
            ActiveDialog::LogWindow,
            ActiveDialog::HlouisSelectByStyle,
            ActiveDialog::HlouisCleanWhitespace,
            ActiveDialog::TemplateDoctor,
        ];
        let themes = [
            ThemeSpec::Light,
            ThemeSpec::Dark,
            ThemeSpec::EyeComfort {
                base: BaseTheme::Dark,
                strength: 60,
            },
        ];

        let fonts = ui_theme::build_font_definitions(&ui_theme::FontConfig::default());
        let cat = Catalog::for_locale(Locale::En);
        let cmds = CommandRegistry::new_default();

        for spec in themes {
            let pal = theme::palette(spec);
            let style = ui_theme::build_style(
                &pal,
                ui_theme::is_dark_spec(spec),
                &ui_theme::FontConfig::default(),
            );
            for dlg in all_dialogs {
                let mut host = DialogHostState::new();
                host.open = Some(dlg);
                let mut settings = AppSettings::default();
                let fonts_c = fonts.clone();
                let style_c = style.clone();
                let pal_c = pal.clone();
                let cat_c = cat.clone();
                let cmds_c = cmds.clone();
                let mut inited = false;

                let mut harness =
                    Harness::builder()
                        .with_accessibility_check(false)
                        .build_ui(move |ui| {
                            if !inited {
                                ui.ctx().set_fonts(fonts_c.clone());
                                ui.ctx().set_global_style(style_c.clone());
                                inited = true;
                            }
                            let _ = show_active_dialog(
                                ui,
                                &mut host,
                                &mut settings,
                                &cmds_c,
                                None,
                                &pal_c,
                                &cat_c,
                            );
                        });
                harness.run();
            }
        }
    }
}
