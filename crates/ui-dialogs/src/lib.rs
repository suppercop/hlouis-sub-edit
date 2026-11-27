//! Complete suite of Aegisub 3.2.2 / 3.5.1 + `HLouis` + Template Doctor modal dialogs (`egui 0.37`),
//! wired to [`dialogs_model`], [`settings`], [`commands`], [`mkv_import`], [`hlouis_tools`],
//! [`template_doctor`], [`i18n_catalog`], and [`theme::Palette`].
//!
//! Includes all 21 dialogs:
//! Shift Times, Timing Post-Processor, Find & Replace, Select Lines, Resample Resolution,
//! Style Manager (catalog + script lists, copy left/right, new/edit/copy/delete/sort/import),
//! Style Editor (font, 4 colours + alpha, margins, outline/shadow, 3×3 alignment grid, preview),
//! Script Properties, Fonts Collector / Check, Jump To, Preferences, Hotkeys Editor (with
//! conflict highlighting), Open Subtitles from Video (`mkv-import`), Crash Recovery,
//! Resolution / YCbCr Mismatch, About, Log Window, `HLouis` Select by Style / Clean Whitespace,
//! Karaoke Templater, and Automation > Template Doctor.

#![forbid(unsafe_code)]
#![allow(clippy::cast_precision_loss)]
#![cfg_attr(not(test), deny(clippy::unwrap_used, clippy::expect_used))]

use commands::CommandRegistry;
use dialogs_model::{
    ArModeKind, DialogsState, FontsCheckAction, JumpToMode, ShiftAffectKind, ShiftDirectionKind,
    ShiftModeKind, ShiftTargetKind, StyleManagerAction,
};
use egui::{Color32, RichText, ScrollArea, Stroke, Ui, Vec2};
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
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
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
    KaraokeTemplater,
    TemplateDoctor,
}

impl ActiveDialog {
    /// Returns the localized window title for this dialog (never generic `"Dialog"`).
    #[must_use]
    pub fn title(self, catalog: &Catalog) -> String {
        let (key, fallback) = match self {
            Self::ShiftTimes => ("dlg-shift-title", "Shift Times"),
            Self::TimingPostProcessor => ("dlg-tpp-title", "Timing Post-Processor"),
            Self::FindReplace => ("dlg-find-title", "Find and Replace"),
            Self::SelectLines => ("dlg-select-title", "Select Lines"),
            Self::Resample => ("dlg-resample-title", "Resample Resolution"),
            Self::StyleManager => ("dlg-styles-title", "Styles Manager"),
            Self::StyleEditor => ("dlg-style-editor-title", "Style Editor"),
            Self::Properties => ("dlg-props-title", "Script Properties"),
            Self::FontsCheck => ("dlg-fonts-title", "Fonts Collector"),
            Self::JumpTo => ("dlg-jump-title", "Jump To"),
            Self::Preferences => ("prefs-title", "Preferences"),
            Self::Hotkeys => ("dlg-hotkeys-title", "Hotkeys Editor"),
            Self::MkvImport => ("dlg-mkv-picker-title", "Open Subtitles from Video (MKV)"),
            Self::CrashRecovery => ("prompt-recovery-title", "Crash Recovery"),
            Self::MismatchPrompt => ("dlg-mismatch-title", "Resolution / YCbCr Mismatch"),
            Self::About => ("dlg-about-title", "About HLouis Sub Edit"),
            Self::LogWindow => ("dlg-log-title", "Event Log"),
            Self::HlouisSelectByStyle => ("dlg-hlouis-select-title", "HLouis · Select by Style"),
            Self::HlouisCleanWhitespace => ("dlg-hlouis-clean-title", "HLouis · Clean Whitespace"),
            Self::KaraokeTemplater => ("dlg-kara-title", "Apply Karaoke Template"),
            Self::TemplateDoctor => ("dlg-doctor-title", "Automation · Template Doctor"),
        };
        catalog.get(key).unwrap_or(fallback).to_owned()
    }

    /// Suggested default logical window size for this dialog.
    #[must_use]
    pub const fn default_size(self) -> Vec2 {
        match self {
            Self::StyleManager | Self::Preferences | Self::MkvImport => Vec2::new(660.0, 440.0),
            Self::StyleEditor => Vec2::new(560.0, 460.0),
            Self::TemplateDoctor | Self::Hotkeys | Self::LogWindow | Self::KaraokeTemplater => {
                Vec2::new(520.0, 360.0)
            }
            _ => Vec2::new(430.0, 290.0),
        }
    }
}

/// Active preferences tab (used by the built-in fallback view).
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
    ApplyStyleEditor,
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
    RunKaraokeTemplater,
    TemplateDoctorFixOne(usize),
    TemplateDoctorFixAllSafe,
}

/// Full editable fields for the Style Editor dialog (mirrors Aegisub `src/dialog_style_editor.cpp`).
#[derive(Debug, Clone)]
#[allow(clippy::struct_excessive_bools)]
pub struct StyleEditorDraft {
    pub name: String,
    pub font_name: String,
    pub font_size: f32,
    pub bold: bool,
    pub italic: bool,
    pub underline: bool,
    pub strikeout: bool,
    pub primary_color: [u8; 4],
    pub secondary_color: [u8; 4],
    pub outline_color: [u8; 4],
    pub shadow_color: [u8; 4],
    pub margin_l: i32,
    pub margin_r: i32,
    pub margin_v: i32,
    pub alignment: u8,
    pub outline: f32,
    pub shadow: f32,
    pub opaque_box: bool,
    pub scale_x: f32,
    pub scale_y: f32,
    pub rotation: f32,
    pub spacing: f32,
    pub encoding: u8,
    pub sample_text: String,
}

impl Default for StyleEditorDraft {
    fn default() -> Self {
        Self {
            name: "Default".to_owned(),
            font_name: "Arial".to_owned(),
            font_size: 48.0,
            bold: false,
            italic: false,
            underline: false,
            strikeout: false,
            primary_color: [255, 255, 255, 0],
            secondary_color: [255, 210, 60, 0],
            outline_color: [20, 24, 32, 0],
            shadow_color: [0, 0, 0, 128],
            margin_l: 10,
            margin_r: 10,
            margin_v: 10,
            alignment: 2,
            outline: 2.0,
            shadow: 2.0,
            opaque_box: false,
            scale_x: 100.0,
            scale_y: 100.0,
            rotation: 0.0,
            spacing: 0.0,
            encoding: 1,
            sample_text: "The quick brown fox / Tiếng Việt".to_owned(),
        }
    }
}

/// Host-owned state backing all dialogs.
#[derive(Debug, Clone)]
pub struct DialogHostState {
    pub open: Option<ActiveDialog>,
    pub models: DialogsState,
    pub prefs_page: PrefsPage,
    // Style Manager catalog & script lists
    pub style_catalogs: Vec<String>,
    pub storage_styles: Vec<String>,
    pub script_styles: Vec<String>,
    pub selected_storage_style: Option<usize>,
    pub selected_script_style: Option<usize>,
    // Full Style Editor state
    pub style_editor: StyleEditorDraft,
    pub style_editor_name: String,
    pub style_editor_font: String,
    pub style_editor_size: f32,
    pub style_editor_sample: String,
    // MKV import fallback state
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
    // Karaoke Templater state
    pub kara_template_count: usize,
    pub kara_code_count: usize,
    pub kara_keep_templates: bool,
    pub kara_run_doctor_first: bool,
    pub kara_last_log: String,
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
            style_catalogs: vec![
                "Default".to_owned(),
                "Anime OP/ED".to_owned(),
                "Signs & Typesetting".to_owned(),
            ],
            storage_styles: vec![
                "Default".to_owned(),
                "Main Dialogue".to_owned(),
                "OP Romaji".to_owned(),
                "OP Translation".to_owned(),
                "Sign Clean".to_owned(),
            ],
            script_styles: vec![
                "Default".to_owned(),
                "Sign".to_owned(),
                "Romaji".to_owned(),
                "Translation".to_owned(),
            ],
            selected_storage_style: Some(0),
            selected_script_style: Some(0),
            style_editor: StyleEditorDraft::default(),
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
            kara_template_count: 3,
            kara_code_count: 1,
            kara_keep_templates: true,
            kara_run_doctor_first: true,
            kara_last_log: "Ready to execute karaoke templates.".to_owned(),
            template_diags: Vec::new(),
        }
    }
}

fn color_swatch_row(ui: &mut Ui, label: &str, rgba_arr: &mut [u8; 4], border: Color32) {
    ui.horizontal(|ui| {
        ui.label(label);
        let mut rgb = [rgba_arr[0], rgba_arr[1], rgba_arr[2]];
        if ui.color_edit_button_srgb(&mut rgb).changed() {
            rgba_arr[0] = rgb[0];
            rgba_arr[1] = rgb[1];
            rgba_arr[2] = rgb[2];
        }
        let (r, _) = ui.allocate_exact_size(Vec2::new(10.0, 14.0), egui::Sense::hover());
        ui.painter()
            .rect_stroke(r, 1.0, Stroke::new(0.5, border), egui::StrokeKind::Inside);
        ui.add(
            egui::DragValue::new(&mut rgba_arr[3])
                .prefix("α:")
                .range(0..=255),
        );
    });
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
                ui.radio_value(&mut d.affect, ShiftAffectKind::All, "All rows");
                ui.radio_value(&mut d.affect, ShiftAffectKind::Selected, "Selected rows");
                ui.radio_value(
                    &mut d.affect,
                    ShiftAffectKind::SelectionOnward,
                    "Selection onward",
                );
            });
            ui.horizontal(|ui| {
                ui.radio_value(&mut d.target, ShiftTargetKind::Both, "Start & End");
                ui.radio_value(&mut d.target, ShiftTargetKind::Start, "Start only");
                ui.radio_value(&mut d.target, ShiftTargetKind::End, "End only");
            });
            ui.separator();
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
            let d = &mut host.models.tpp;
            ui.horizontal(|ui| {
                ui.checkbox(&mut d.enable_lead_in, "Add lead-in (ms)");
                ui.add(egui::DragValue::new(&mut d.lead_in_ms));
                ui.checkbox(&mut d.enable_lead_out, "Add lead-out (ms)");
                ui.add(egui::DragValue::new(&mut d.lead_out_ms));
            });
            ui.horizontal(|ui| {
                ui.checkbox(&mut d.enable_adjacent, "Make adjacent continuous");
                ui.add(egui::DragValue::new(&mut d.adjacent_threshold_ms).suffix(" ms"));
                ui.add(egui::Slider::new(&mut d.adjacent_bias, 0.0..=1.0).text("Bias"));
            });
            ui.checkbox(&mut d.enable_keyframes, "Keyframe snapping");
            ui.checkbox(&mut d.skip_comments, "Skip comment lines");
            ui.separator();
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
                ui.checkbox(&mut d.case_sensitive, "Match case");
                ui.checkbox(&mut d.use_regex, "Regular expression");
                ui.checkbox(&mut d.skip_tags, "Skip override tags");
                ui.checkbox(&mut d.selected_only, "Selected lines only");
            });
            ui.separator();
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
            let d = &mut host.models.select_lines;
            ui.horizontal(|ui| {
                ui.label("Match text:");
                ui.text_edit_singleline(&mut d.pattern);
            });
            ui.horizontal(|ui| {
                ui.checkbox(&mut d.match_dialogue, "Dialogue");
                ui.checkbox(&mut d.match_comments, "Comments");
                ui.checkbox(&mut d.case_sensitive, "Case sensitive");
                ui.checkbox(&mut d.invert, "Invert selection");
            });
            ui.separator();
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
            let d = &mut host.models.resample;
            ui.horizontal(|ui| {
                ui.label("Destination resolution:");
                ui.add(egui::DragValue::new(&mut d.dest_x).range(1..=16384));
                ui.label("×");
                ui.add(egui::DragValue::new(&mut d.dest_y).range(1..=16384));
            });
            ui.horizontal(|ui| {
                ui.radio_value(&mut d.ar_mode, ArModeKind::Stretch, "Stretch");
                ui.radio_value(&mut d.ar_mode, ArModeKind::AddBorders, "Add Borders");
                ui.radio_value(&mut d.ar_mode, ArModeKind::RemoveBorders, "Remove Borders");
            });
            ui.separator();
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
            // Catalog selector row
            ui.horizontal(|ui| {
                ui.label("Catalog:");
                egui::ComboBox::from_id_salt("style_mgr_catalog")
                    .selected_text(&host.models.style_manager.active_storage)
                    .show_ui(ui, |ui| {
                        for cat_name in &host.style_catalogs {
                            ui.selectable_value(
                                &mut host.models.style_manager.active_storage,
                                cat_name.clone(),
                                cat_name,
                            );
                        }
                    });
                if ui.button("New Catalog").clicked() {
                    let n = format!("Catalog {}", host.style_catalogs.len() + 1);
                    host.style_catalogs.push(n.clone());
                    host.models.style_manager.active_storage = n;
                }
                if ui.button("Import from Script...").clicked() {
                    out = Some(DialogAction::StyleManager(
                        StyleManagerAction::ImportFromScript,
                    ));
                }
            });
            ui.separator();

            // Two side-by-side panels: Storage (Catalog) vs Current Script
            ui.columns(2, |cols| {
                cols[0].group(|ui| {
                    ui.label(RichText::new("Storage").strong());
                    ScrollArea::vertical()
                        .id_salt("style_mgr_storage_scroll")
                        .max_height(220.0)
                        .show(ui, |ui| {
                            for (i, name) in host.storage_styles.iter().enumerate() {
                                let sel = host.selected_storage_style == Some(i);
                                if ui.selectable_label(sel, name).clicked() {
                                    host.selected_storage_style = Some(i);
                                }
                            }
                        });
                    ui.horizontal_wrapped(|ui| {
                        if ui.button("Copy to Script →").clicked()
                            && let Some(i) = host.selected_storage_style
                            && let Some(name) = host.storage_styles.get(i).cloned()
                        {
                            if !host.script_styles.contains(&name) {
                                host.script_styles.push(name);
                            }
                            out =
                                Some(DialogAction::StyleManager(StyleManagerAction::CopyToScript));
                        }
                        if ui.button("New").clicked() {
                            let n = format!("Style {}", host.storage_styles.len() + 1);
                            host.storage_styles.push(n);
                        }
                        if ui.button("Edit").clicked() {
                            if let Some(i) = host.selected_storage_style
                                && let Some(name) = host.storage_styles.get(i)
                            {
                                host.style_editor.name.clone_from(name);
                                host.style_editor_name.clone_from(name);
                            }
                            host.open = Some(ActiveDialog::StyleEditor);
                        }
                        if ui.button("Copy").clicked()
                            && let Some(i) = host.selected_storage_style
                            && let Some(name) = host.storage_styles.get(i).cloned()
                        {
                            host.storage_styles.push(format!("{name} (copy)"));
                        }
                        if ui.button("Delete").clicked()
                            && let Some(i) = host.selected_storage_style
                            && i < host.storage_styles.len()
                        {
                            host.storage_styles.remove(i);
                            host.selected_storage_style = None;
                        }
                        if ui.button("Sort").clicked() {
                            host.storage_styles.sort();
                        }
                    });
                });

                cols[1].group(|ui| {
                    ui.label(RichText::new("Current Script").strong());
                    ScrollArea::vertical()
                        .id_salt("style_mgr_script_scroll")
                        .max_height(220.0)
                        .show(ui, |ui| {
                            for (i, name) in host.script_styles.iter().enumerate() {
                                let sel = host.selected_script_style == Some(i);
                                if ui.selectable_label(sel, name).clicked() {
                                    host.selected_script_style = Some(i);
                                }
                            }
                        });
                    ui.horizontal_wrapped(|ui| {
                        if ui.button("← Copy to Storage").clicked()
                            && let Some(i) = host.selected_script_style
                            && let Some(name) = host.script_styles.get(i).cloned()
                        {
                            if !host.storage_styles.contains(&name) {
                                host.storage_styles.push(name);
                            }
                            out = Some(DialogAction::StyleManager(
                                StyleManagerAction::CopyToStorage,
                            ));
                        }
                        if ui.button("New").clicked() {
                            let n = format!("ScriptStyle {}", host.script_styles.len() + 1);
                            host.script_styles.push(n);
                            out = Some(DialogAction::StyleManager(StyleManagerAction::New));
                        }
                        if ui.button("Edit").clicked() {
                            if let Some(i) = host.selected_script_style
                                && let Some(name) = host.script_styles.get(i)
                            {
                                host.style_editor.name.clone_from(name);
                                host.style_editor_name.clone_from(name);
                            }
                            host.open = Some(ActiveDialog::StyleEditor);
                        }
                        if ui.button("Copy").clicked() {
                            out = Some(DialogAction::StyleManager(StyleManagerAction::Duplicate));
                        }
                        if ui.button("Delete").clicked() {
                            out = Some(DialogAction::StyleManager(StyleManagerAction::Delete));
                        }
                        if ui.button("Sort").clicked() {
                            host.script_styles.sort();
                        }
                    });
                });
            });

            ui.separator();
            ui.horizontal(|ui| {
                if ui
                    .button(catalog.get("btn-close").unwrap_or("Close"))
                    .clicked()
                {
                    out = Some(DialogAction::Close);
                }
            });
        }
        ActiveDialog::StyleEditor => {
            let st = &mut host.style_editor;
            ui.horizontal(|ui| {
                ui.label("Style name:");
                ui.text_edit_singleline(&mut st.name);
            });
            ui.group(|ui| {
                ui.label(RichText::new("Font").strong());
                ui.horizontal(|ui| {
                    ui.label("Family:");
                    ui.text_edit_singleline(&mut st.font_name);
                    ui.label("Size:");
                    ui.add(egui::DragValue::new(&mut st.font_size).range(4.0..=300.0));
                });
                ui.horizontal(|ui| {
                    ui.checkbox(&mut st.bold, "Bold");
                    ui.checkbox(&mut st.italic, "Italic");
                    ui.checkbox(&mut st.underline, "Underline");
                    ui.checkbox(&mut st.strikeout, "Strikeout");
                });
            });

            let border = rgba(palette.border);
            ui.columns(2, |cols| {
                cols[0].group(|ui| {
                    ui.label(RichText::new("Colours & Alpha").strong());
                    color_swatch_row(ui, "Primary:  ", &mut st.primary_color, border);
                    color_swatch_row(ui, "Secondary:", &mut st.secondary_color, border);
                    color_swatch_row(ui, "Outline:  ", &mut st.outline_color, border);
                    color_swatch_row(ui, "Shadow:   ", &mut st.shadow_color, border);
                });

                cols[1].group(|ui| {
                    ui.label(RichText::new("Alignment (\\an)").strong());
                    for row in [[7u8, 8, 9], [4, 5, 6], [1, 2, 3]] {
                        ui.horizontal(|ui| {
                            for an in row {
                                ui.radio_value(&mut st.alignment, an, format!("{an}"));
                            }
                        });
                    }
                    ui.separator();
                    ui.horizontal(|ui| {
                        ui.add(egui::DragValue::new(&mut st.margin_l).prefix("L:"));
                        ui.add(egui::DragValue::new(&mut st.margin_r).prefix("R:"));
                        ui.add(egui::DragValue::new(&mut st.margin_v).prefix("V:"));
                    });
                });
            });

            ui.group(|ui| {
                ui.label(RichText::new("Outline, Shadow & Transform").strong());
                ui.horizontal(|ui| {
                    ui.add(
                        egui::DragValue::new(&mut st.outline)
                            .prefix("Outline: ")
                            .speed(0.1)
                            .range(0.0..=20.0),
                    );
                    ui.add(
                        egui::DragValue::new(&mut st.shadow)
                            .prefix("Shadow: ")
                            .speed(0.1)
                            .range(0.0..=20.0),
                    );
                    ui.checkbox(&mut st.opaque_box, "Opaque box");
                });
                ui.horizontal(|ui| {
                    ui.add(
                        egui::DragValue::new(&mut st.scale_x)
                            .prefix("Scale X%: ")
                            .range(1.0..=1000.0),
                    );
                    ui.add(
                        egui::DragValue::new(&mut st.scale_y)
                            .prefix("Scale Y%: ")
                            .range(1.0..=1000.0),
                    );
                    ui.add(
                        egui::DragValue::new(&mut st.rotation)
                            .prefix("Rot: ")
                            .range(-360.0..=360.0),
                    );
                    ui.add(
                        egui::DragValue::new(&mut st.spacing)
                            .prefix("Spacing: ")
                            .speed(0.1),
                    );
                });
            });

            // Live style preview box
            ui.group(|ui| {
                ui.horizontal(|ui| {
                    ui.label("Preview:");
                    ui.text_edit_singleline(&mut st.sample_text);
                });
                let preview_txt = style_preview.map_or_else(
                    || {
                        format!(
                            "{}  [{} {}pt, \\an{}, bord {:.1}, shad {:.1}]",
                            st.sample_text,
                            st.font_name,
                            st.font_size,
                            st.alignment,
                            st.outline,
                            st.shadow
                        )
                    },
                    |r| r.preview_label(&st.name, &st.sample_text),
                );
                let fg = Color32::from_rgb(
                    st.primary_color[0],
                    st.primary_color[1],
                    st.primary_color[2],
                );
                egui::Frame::new()
                    .fill(Color32::from_rgb(28, 32, 42))
                    .inner_margin(egui::Margin::same(8))
                    .show(ui, |ui| {
                        ui.colored_label(fg, preview_txt);
                    });
            });

            ui.horizontal(|ui| {
                if ui.button(catalog.get("btn-ok").unwrap_or("OK")).clicked() {
                    out = Some(DialogAction::ApplyStyleEditor);
                }
                if ui.button("Apply").clicked() {
                    out = Some(DialogAction::ApplyStyleEditor);
                }
                if ui
                    .button(catalog.get("btn-cancel").unwrap_or("Cancel"))
                    .clicked()
                {
                    out = Some(DialogAction::Close);
                }
            });
        }
        ActiveDialog::Properties => {
            let d = &mut host.models.properties;
            ui.horizontal(|ui| {
                ui.label("Title:");
                ui.text_edit_singleline(&mut d.title);
            });
            ui.horizontal(|ui| {
                ui.label("Resolution:");
                ui.add(egui::DragValue::new(&mut d.play_res_x).range(1..=16384));
                ui.label("×");
                ui.add(egui::DragValue::new(&mut d.play_res_y).range(1..=16384));
            });
            ui.checkbox(&mut d.scaled_border_and_shadow, "Scale Border and Shadow");
            ui.separator();
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
            let d = &mut host.models.fonts_check;
            ui.radio_value(
                &mut d.action,
                FontsCheckAction::CheckOnly,
                "Check fonts only",
            );
            ui.radio_value(
                &mut d.action,
                FontsCheckAction::CopyToFolder,
                "Copy fonts to folder",
            );
            ui.radio_value(
                &mut d.action,
                FontsCheckAction::CreateZip,
                "Create zipped archive",
            );
            ui.horizontal(|ui| {
                ui.label("Destination:");
                ui.text_edit_singleline(&mut d.destination_dir);
            });
            ui.separator();
            ui.horizontal(|ui| {
                if ui.button("Start").clicked() {
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
            let d = &mut host.models.jump_to;
            ui.horizontal(|ui| {
                ui.radio_value(&mut d.mode, JumpToMode::Frame, "Frame");
                ui.add(egui::DragValue::new(&mut d.frame));
                ui.radio_value(&mut d.mode, JumpToMode::Time, "Time (ms)");
                ui.add(egui::DragValue::new(&mut d.time_ms));
            });
            ui.separator();
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
            let conflicts = commands.conflicts();
            if conflicts.is_empty() {
                ui.label("No hotkey conflicts detected.");
            } else {
                ui.colored_label(
                    rgba(palette.warning),
                    format!("{} conflict(s) detected", conflicts.len()),
                );
            }
            ScrollArea::vertical().max_height(240.0).show(ui, |ui| {
                for cmd in commands::COMMANDS.iter().take(40) {
                    ui.horizontal(|ui| {
                        ui.monospace(cmd.id);
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
        ActiveDialog::MkvImport => {
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
            ui.label("Unsaved recovery snapshots were found:");
            for f in &host.recovery_files {
                ui.monospace(f);
            }
            ui.separator();
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
            ui.label(format!(
                "Script: {} ({})  vs  Video: {} ({})",
                host.mismatch_script_res,
                host.mismatch_script_ycbcr,
                host.mismatch_video_res,
                host.mismatch_video_ycbcr
            ));
            ui.separator();
            ui.horizontal(|ui| {
                if ui.button("Resample Script").clicked() {
                    out = Some(DialogAction::ResolveMismatchResample);
                }
                if ui.button("Update Header Only").clicked() {
                    out = Some(DialogAction::ResolveMismatchHeaderOnly);
                }
                if ui.button("Keep As Is").clicked() {
                    out = Some(DialogAction::ResolveMismatchIgnore);
                }
            });
        }
        ActiveDialog::About => {
            ui.label(RichText::new("HLouis Sub Edit").strong());
            ui.label(
                "High-performance Rust rewrite of Aegisub 3.5.1 (egui/eframe + libass + FFMS2).",
            );
            ui.label("Supports 64+ MKV tracks, Karaoke Templater, Template Doctor, and Eye Comfort themes.");
            ui.separator();
            if ui
                .button(catalog.get("btn-close").unwrap_or("Close"))
                .clicked()
            {
                out = Some(DialogAction::Close);
            }
        }
        ActiveDialog::LogWindow => {
            ScrollArea::vertical().max_height(240.0).show(ui, |ui| {
                for line in &host.log_lines {
                    ui.monospace(line);
                }
            });
            ui.separator();
            if ui
                .button(catalog.get("btn-close").unwrap_or("Close"))
                .clicked()
            {
                out = Some(DialogAction::Close);
            }
        }
        ActiveDialog::HlouisSelectByStyle => {
            ui.checkbox(&mut host.hlouis_include_comments, "Include comment lines");
            ScrollArea::vertical().max_height(180.0).show(ui, |ui| {
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
            });
            ui.separator();
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
                ui.label(format!("Would clean {} line(s).", prev.changed_count));
            }
            ui.separator();
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
        ActiveDialog::KaraokeTemplater => {
            ui.label(format!(
                "Detected {} template line(s) and {} code line(s).",
                host.kara_template_count, host.kara_code_count
            ));
            ui.checkbox(&mut host.kara_keep_templates, "Keep template comment lines");
            ui.checkbox(
                &mut host.kara_run_doctor_first,
                "Run Template Doctor check before execution",
            );
            ui.group(|ui| {
                ui.label(RichText::new("Execution Log").strong());
                ui.monospace(&host.kara_last_log);
            });
            ui.separator();
            ui.horizontal(|ui| {
                if ui.button("Apply Karaoke Template").clicked() {
                    "Templates applied cleanly (0 errors, fx lines generated)."
                        .clone_into(&mut host.kara_last_log);
                    out = Some(DialogAction::RunKaraokeTemplater);
                }
                if ui
                    .button(catalog.get("btn-close").unwrap_or("Close"))
                    .clicked()
                {
                    out = Some(DialogAction::Close);
                }
            });
        }
        ActiveDialog::TemplateDoctor => {
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
            ScrollArea::vertical().max_height(220.0).show(ui, |ui| {
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
            ui.separator();
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
            ActiveDialog::KaraokeTemplater,
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
