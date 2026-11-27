//! Importer for Aegisub 3.5.1 `config.json` and `hotkey.json` into a typed settings model.
//!
//! Mirrors Aegisub `libaegisub/common/option.cpp`, `src/libresrc/default_config.json`, and
//! `libaegisub/common/hotkey.cpp`. Tolerates missing, unknown, or differently typed fields
//! without panicking, and collects unmapped hotkey commands for user inspection.

#![forbid(unsafe_code)]
#![cfg_attr(not(test), deny(clippy::unwrap_used, clippy::expect_used))]

use serde_json::Value;
use std::collections::BTreeMap;
use std::fs;
use std::path::Path;
use thiserror::Error;

/// Errors returned by `aegisub-config`.
#[derive(Debug, Error)]
pub enum ConfigImportError {
    /// Filesystem I/O failure.
    #[error("config I/O error: {0}")]
    Io(#[from] std::io::Error),
    /// Invalid JSON syntax.
    #[error("JSON parse error: {0}")]
    Json(#[from] serde_json::Error),
    /// Valid JSON whose top level is not an object, so nothing could be imported.
    #[error("{file} must contain a JSON object at the top level, found {found}")]
    NotAnObject {
        file: &'static str,
        found: &'static str,
    },
}

fn json_kind(v: &Value) -> &'static str {
    match v {
        Value::Null => "null",
        Value::Bool(_) => "a boolean",
        Value::Number(_) => "a number",
        Value::String(_) => "a string",
        Value::Array(_) => "an array",
        Value::Object(_) => "an object",
    }
}

/// RGBA color parsed from Aegisub's `rgb(r, g, b)`, `rgba(r, g, b, a)`, or `#RRGGBB` strings.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct RgbaColor {
    pub r: u8,
    pub g: u8,
    pub b: u8,
    pub a: u8,
}

impl RgbaColor {
    /// Creates an opaque RGB color.
    #[must_use]
    pub const fn rgb(r: u8, g: u8, b: u8) -> Self {
        Self { r, g, b, a: 255 }
    }

    /// Parses Aegisub color strings (`rgb(r, g, b)`, `rgba(r, g, b, a)`, `#RRGGBB`).
    #[must_use]
    pub fn parse_aegisub(raw: &str) -> Option<Self> {
        let trimmed = raw.trim();
        if let Some(inner) = trimmed
            .strip_prefix("rgba(")
            .and_then(|x| x.strip_suffix(')'))
        {
            let mut it = inner.split(',').map(str::trim);
            let r = it.next()?.parse::<u8>().ok()?;
            let g = it.next()?.parse::<u8>().ok()?;
            let b = it.next()?.parse::<u8>().ok()?;
            let a_str = it.next()?;
            let a = if let Ok(byte) = a_str.parse::<u8>() {
                byte
            } else if let Ok(frac) = a_str.parse::<f32>() {
                #[allow(clippy::cast_possible_truncation, clippy::cast_sign_loss)]
                {
                    (frac.clamp(0.0, 1.0) * 255.0).round() as u8
                }
            } else {
                return None;
            };
            return Some(Self { r, g, b, a });
        }
        if let Some(inner) = trimmed
            .strip_prefix("rgb(")
            .and_then(|x| x.strip_suffix(')'))
        {
            let mut it = inner.split(',').map(str::trim);
            let r = it.next()?.parse::<u8>().ok()?;
            let g = it.next()?.parse::<u8>().ok()?;
            let b = it.next()?.parse::<u8>().ok()?;
            return Some(Self::rgb(r, g, b));
        }
        if let Some(hex) = trimmed.strip_prefix('#') {
            if hex.len() == 6 {
                let r = u8::from_str_radix(&hex[0..2], 16).ok()?;
                let g = u8::from_str_radix(&hex[2..4], 16).ok()?;
                let b = u8::from_str_radix(&hex[4..6], 16).ok()?;
                return Some(Self::rgb(r, g, b));
            }
        }
        None
    }
}

/// Workspace layout preferences.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct LayoutSettings {
    pub language: String,
    pub show_toolbar: bool,
    pub show_video: bool,
    pub show_audio: bool,
    pub edit_box_font_face: String,
    pub edit_box_font_size: u32,
}

impl Default for LayoutSettings {
    fn default() -> Self {
        Self {
            language: "en_US".to_owned(),
            show_toolbar: true,
            show_video: true,
            show_audio: true,
            edit_box_font_face: "Consolas".to_owned(),
            edit_box_font_size: 11,
        }
    }
}

/// Grid column visibility toggles.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
#[allow(clippy::struct_excessive_bools)]
pub struct GridColumns {
    pub layer: bool,
    pub start: bool,
    pub end: bool,
    pub style: bool,
    pub actor: bool,
    pub effect: bool,
    pub margin_l: bool,
    pub margin_r: bool,
    pub margin_v: bool,
    pub cps: bool,
}

impl Default for GridColumns {
    fn default() -> Self {
        Self {
            layer: true,
            start: true,
            end: true,
            style: true,
            actor: true,
            effect: false,
            margin_l: false,
            margin_r: false,
            margin_v: false,
            cps: true,
        }
    }
}

/// Subtitle grid appearance & override-tag display settings.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct GridSettings {
    pub font_face: String,
    pub font_size: u32,
    pub hide_overrides_mode: i64,
    pub columns: GridColumns,
}

impl Default for GridSettings {
    fn default() -> Self {
        Self {
            font_face: "Tahoma".to_owned(),
            font_size: 9,
            hide_overrides_mode: 1,
            columns: GridColumns::default(),
        }
    }
}

/// Subtitle grid color palette imported from `Colour/Subtitle Grid`.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct GridColors {
    pub background: RgbaColor,
    pub selection: RgbaColor,
    pub comment: RgbaColor,
    pub selected_comment: RgbaColor,
    pub inframe: RgbaColor,
    pub standard_text: RgbaColor,
    pub collision_text: RgbaColor,
    pub header: RgbaColor,
    pub active_border: RgbaColor,
}

impl Default for GridColors {
    fn default() -> Self {
        Self {
            background: RgbaColor::rgb(255, 255, 255),
            selection: RgbaColor::rgb(206, 255, 231),
            comment: RgbaColor::rgb(216, 222, 245),
            selected_comment: RgbaColor::rgb(211, 238, 238),
            inframe: RgbaColor::rgb(255, 253, 234),
            standard_text: RgbaColor::rgb(0, 0, 0),
            collision_text: RgbaColor::rgb(255, 0, 0),
            header: RgbaColor::rgb(165, 207, 231),
            active_border: RgbaColor::rgb(45, 207, 42),
        }
    }
}

/// Video & subtitle renderer defaults.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct VideoSettings {
    pub provider: String,
    pub subtitle_provider: String,
    pub default_zoom: i64,
    pub fast_jump_step: i64,
    pub check_script_res: i64,
}

impl Default for VideoSettings {
    fn default() -> Self {
        Self {
            provider: "FFmpegSource".to_owned(),
            subtitle_provider: "libass".to_owned(),
            default_zoom: 7,
            fast_jump_step: 10,
            check_script_res: 2,
        }
    }
}

/// Audio display & timing defaults.
#[derive(Debug, Clone, PartialEq, Eq)]
#[allow(clippy::struct_excessive_bools)]
pub struct AudioSettings {
    pub provider: String,
    pub spectrum: bool,
    pub spectrum_quality: i64,
    pub spectrum_cutoff: i64,
    pub medusa_hotkeys: bool,
    pub auto_focus: bool,
    pub auto_commit: bool,
    pub lead_in_ms: i64,
    pub lead_out_ms: i64,
}

impl Default for AudioSettings {
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
        }
    }
}

/// Timing & post-processor defaults.
#[derive(Debug, Clone, Copy, PartialEq)]
#[allow(clippy::struct_excessive_bools)]
pub struct TimingSettings {
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

impl Default for TimingSettings {
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

/// Autosave & autobackup defaults.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct AutosaveSettings {
    pub enabled: bool,
    pub interval_seconds: u64,
    pub on_every_change: bool,
    pub backup_enabled: bool,
    pub autosave_dir: String,
    pub autobackup_dir: String,
}

impl Default for AutosaveSettings {
    fn default() -> Self {
        Self {
            enabled: true,
            interval_seconds: 60,
            on_every_change: false,
            backup_enabled: true,
            autosave_dir: "?user/autosave".to_owned(),
            autobackup_dir: "?user/autoback".to_owned(),
        }
    }
}

/// Canonical `hlouis-sub-edit` command IDs mapped from Aegisub `hotkey.json` command strings.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum CommandId {
    AppExit,
    FileOpen,
    FileSave,
    EditUndo,
    EditRedo,
    EditFindReplace,
    EditLineCopy,
    EditLineCut,
    EditLinePaste,
    EditLinePasteOver,
    EditLineDelete,
    EditLineDuplicate,
    EditLineSplitByKaraoke,
    TimeShift,
    AudioPlaySelection,
    AudioCommit,
    AudioGoTo,
    VideoPlay,
    VideoFrameNext,
    VideoFramePrev,
}

/// Key combination from `hotkey.json`.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct KeyCombo {
    pub modifiers: Vec<String>,
    pub key: String,
}

/// A mapped hotkey entry within an Aegisub context (`"Default"`, `"Subtitle Grid"`, etc.).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct MappedHotkey {
    pub command: CommandId,
    pub aegisub_name: String,
    pub combos: Vec<KeyCombo>,
}

/// An unrecognized or custom hotkey command preserved for reporting to the user.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct UnmappedHotkey {
    pub context: String,
    pub aegisub_name: String,
    pub combos: Vec<KeyCombo>,
}

/// Imported hotkey map grouped by context name.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct HotkeySettings {
    pub by_context: BTreeMap<String, Vec<MappedHotkey>>,
    pub unmapped: Vec<UnmappedHotkey>,
}

/// Complete typed settings imported from Aegisub 3.5.1 `config.json` + `hotkey.json`.
#[derive(Debug, Clone, Default, PartialEq)]
pub struct ImportedSettings {
    pub layout: LayoutSettings,
    pub grid: GridSettings,
    pub colors: GridColors,
    pub video: VideoSettings,
    pub audio: AudioSettings,
    pub timing: TimingSettings,
    pub autosave: AutosaveSettings,
    pub hotkeys: HotkeySettings,
    /// Top-level keys present in `config.json` that were not recognized (informational).
    pub unknown_top_level_keys: Vec<String>,
}

/// Imports both Aegisub `config.json` and `hotkey.json` from JSON strings.
///
/// # Errors
/// Returns [`ConfigImportError::Json`] if either string is not valid JSON.
pub fn import_aegisub_json(
    config_json: &str,
    hotkey_json: &str,
) -> Result<ImportedSettings, ConfigImportError> {
    let mut settings = import_config_json(config_json)?;
    settings.hotkeys = import_hotkey_json(hotkey_json)?;
    Ok(settings)
}

/// Imports both `config.json` and `hotkey.json` from paths on disk.
///
/// # Errors
/// Returns [`ConfigImportError`] on I/O or JSON parse failure.
pub fn import_aegisub_files(
    config_path: &Path,
    hotkey_path: &Path,
) -> Result<ImportedSettings, ConfigImportError> {
    let cfg_text = fs::read_to_string(config_path)?;
    let hk_text = fs::read_to_string(hotkey_path)?;
    import_aegisub_json(&cfg_text, &hk_text)
}

/// Imports only `config.json` into [`ImportedSettings`] (hotkeys left empty).
///
/// # Errors
/// Returns [`ConfigImportError::Json`] if `config_json` is invalid JSON and
/// [`ConfigImportError::NotAnObject`] if its top level is not an object.
pub fn import_config_json(config_json: &str) -> Result<ImportedSettings, ConfigImportError> {
    let root: Value = serde_json::from_str(config_json)?;
    if !root.is_object() {
        return Err(ConfigImportError::NotAnObject {
            file: "config.json",
            found: json_kind(&root),
        });
    }
    let mut out = ImportedSettings::default();

    if let Some(obj) = root.as_object() {
        for key in obj.keys() {
            if !matches!(
                key.as_str(),
                "App" | "Audio" | "Colour" | "Path" | "Subtitle" | "Timing" | "Tool" | "Video"
            ) {
                out.unknown_top_level_keys.push(key.clone());
            }
        }
    }

    // Layout & Autosave
    if let Some(v) = get_str(&root, &["App", "Language"]) {
        v.clone_into(&mut out.layout.language);
    }
    if let Some(v) = get_bool(&root, &["App", "Show Toolbar"]) {
        out.layout.show_toolbar = v;
    }
    if let Some(v) = get_bool(&root, &["App", "Show Video"]) {
        out.layout.show_video = v;
    }
    if let Some(v) = get_bool(&root, &["App", "Show Audio"]) {
        out.layout.show_audio = v;
    }
    if let Some(v) = get_str(&root, &["Subtitle", "Edit Box", "Font Face"]) {
        v.clone_into(&mut out.layout.edit_box_font_face);
    }
    if let Some(v) = get_u32(&root, &["Subtitle", "Edit Box", "Font Size"]) {
        out.layout.edit_box_font_size = v;
    }

    if let Some(v) = get_bool(&root, &["App", "Auto", "Save"]) {
        out.autosave.enabled = v;
    }
    if let Some(v) = get_u64(&root, &["App", "Auto", "Save Every Seconds"]) {
        out.autosave.interval_seconds = v;
    }
    if let Some(v) = get_bool(&root, &["App", "Auto", "Save on Every Change"]) {
        out.autosave.on_every_change = v;
    }
    if let Some(v) = get_bool(&root, &["App", "Auto", "Backup"]) {
        out.autosave.backup_enabled = v;
    }
    if let Some(v) = get_str(&root, &["Path", "Auto", "Save"]) {
        v.clone_into(&mut out.autosave.autosave_dir);
    }
    if let Some(v) = get_str(&root, &["Path", "Auto", "Backup"]) {
        v.clone_into(&mut out.autosave.autobackup_dir);
    }

    // Grid
    if let Some(v) = get_str(&root, &["Subtitle", "Grid", "Font Face"]) {
        v.clone_into(&mut out.grid.font_face);
    }
    if let Some(v) = get_u32(&root, &["Subtitle", "Grid", "Font Size"]) {
        out.grid.font_size = v;
    }
    if let Some(v) = get_i64(&root, &["Subtitle", "Grid", "Hide Overrides"]) {
        out.grid.hide_overrides_mode = v;
    }
    apply_grid_columns(&root, &mut out.grid.columns);
    apply_grid_colors(&root, &mut out.colors);
    apply_video_audio(&root, &mut out.video, &mut out.audio);
    apply_timing(&root, &mut out.timing);

    Ok(out)
}

/// Imports `hotkey.json` into [`HotkeySettings`], mapping known Aegisub commands to [`CommandId`]
/// and recording unknown/custom commands in `unmapped`.
///
/// # Errors
/// Returns [`ConfigImportError::Json`] if `hotkey_json` is invalid JSON.
pub fn import_hotkey_json(hotkey_json: &str) -> Result<HotkeySettings, ConfigImportError> {
    let root: Value = serde_json::from_str(hotkey_json)?;
    let mut out = HotkeySettings::default();

    let Some(contexts) = root.as_object() else {
        return Err(ConfigImportError::NotAnObject {
            file: "hotkey.json",
            found: json_kind(&root),
        });
    };

    for (ctx_name, ctx_val) in contexts {
        let Some(cmd_map) = ctx_val.as_object() else {
            continue;
        };
        let mut mapped_list = Vec::new();
        for (aegisub_cmd, bindings_val) in cmd_map {
            let combos = parse_combos(bindings_val);
            if let Some(command) = map_aegisub_command(aegisub_cmd) {
                mapped_list.push(MappedHotkey {
                    command,
                    aegisub_name: aegisub_cmd.clone(),
                    combos,
                });
            } else {
                out.unmapped.push(UnmappedHotkey {
                    context: ctx_name.clone(),
                    aegisub_name: aegisub_cmd.clone(),
                    combos,
                });
            }
        }
        if !mapped_list.is_empty() {
            out.by_context.insert(ctx_name.clone(), mapped_list);
        }
    }

    Ok(out)
}

fn apply_grid_columns(root: &Value, cols: &mut GridColumns) {
    if let Some(v) = get_bool(root, &["Subtitle", "Grid", "Column", "Layer"]) {
        cols.layer = v;
    }
    if let Some(v) = get_bool(root, &["Subtitle", "Grid", "Column", "Start"]) {
        cols.start = v;
    }
    if let Some(v) = get_bool(root, &["Subtitle", "Grid", "Column", "End"]) {
        cols.end = v;
    }
    if let Some(v) = get_bool(root, &["Subtitle", "Grid", "Column", "Style"]) {
        cols.style = v;
    }
    if let Some(v) = get_bool(root, &["Subtitle", "Grid", "Column", "Actor"]) {
        cols.actor = v;
    }
    if let Some(v) = get_bool(root, &["Subtitle", "Grid", "Column", "Effect"]) {
        cols.effect = v;
    }
    if let Some(v) = get_bool(root, &["Subtitle", "Grid", "Column", "MarginL"]) {
        cols.margin_l = v;
    }
    if let Some(v) = get_bool(root, &["Subtitle", "Grid", "Column", "MarginR"]) {
        cols.margin_r = v;
    }
    if let Some(v) = get_bool(root, &["Subtitle", "Grid", "Column", "MarginV"]) {
        cols.margin_v = v;
    }
    if let Some(v) = get_bool(root, &["Subtitle", "Grid", "Column", "CPS"]) {
        cols.cps = v;
    }
}

fn apply_grid_colors(root: &Value, c: &mut GridColors) {
    if let Some(col) = get_color(
        root,
        &["Colour", "Subtitle Grid", "Background", "Background"],
    ) {
        c.background = col;
    }
    if let Some(col) = get_color(
        root,
        &["Colour", "Subtitle Grid", "Background", "Selection"],
    ) {
        c.selection = col;
    }
    if let Some(col) = get_color(root, &["Colour", "Subtitle Grid", "Background", "Comment"]) {
        c.comment = col;
    }
    if let Some(col) = get_color(
        root,
        &["Colour", "Subtitle Grid", "Background", "Selected Comment"],
    ) {
        c.selected_comment = col;
    }
    if let Some(col) = get_color(root, &["Colour", "Subtitle Grid", "Background", "Inframe"]) {
        c.inframe = col;
    }
    if let Some(col) = get_color(root, &["Colour", "Subtitle Grid", "Standard"]) {
        c.standard_text = col;
    }
    if let Some(col) = get_color(root, &["Colour", "Subtitle Grid", "Collision"]) {
        c.collision_text = col;
    }
    if let Some(col) = get_color(root, &["Colour", "Subtitle Grid", "Header"]) {
        c.header = col;
    }
    if let Some(col) = get_color(root, &["Colour", "Subtitle Grid", "Active Border"]) {
        c.active_border = col;
    }
}

fn apply_video_audio(root: &Value, video: &mut VideoSettings, audio: &mut AudioSettings) {
    if let Some(v) = get_str(root, &["Video", "Provider"]) {
        v.clone_into(&mut video.provider);
    }
    if let Some(v) = get_str(root, &["Subtitle", "Provider"]) {
        v.clone_into(&mut video.subtitle_provider);
    }
    if let Some(v) = get_i64(root, &["Video", "Default Zoom"]) {
        video.default_zoom = v;
    }
    if let Some(v) = get_i64(root, &["Video", "Slider", "Fast Jump Step"]) {
        video.fast_jump_step = v;
    }
    if let Some(v) = get_i64(root, &["Video", "Check Script Res"]) {
        video.check_script_res = v;
    }

    if let Some(v) = get_str(root, &["Audio", "Provider"]) {
        v.clone_into(&mut audio.provider);
    }
    if let Some(v) = get_bool(root, &["Audio", "Spectrum"]) {
        audio.spectrum = v;
    }
    if let Some(v) = get_i64(root, &["Audio", "Spectrum Quality"]) {
        audio.spectrum_quality = v;
    }
    if let Some(v) = get_i64(root, &["Audio", "Spectrum Cutoff"]) {
        audio.spectrum_cutoff = v;
    }
    if let Some(v) = get_bool(root, &["Audio", "Medusa Timing Hotkeys"]) {
        audio.medusa_hotkeys = v;
    }
    if let Some(v) = get_bool(root, &["Audio", "Auto", "Focus"]) {
        audio.auto_focus = v;
    }
    if let Some(v) = get_bool(root, &["Audio", "Auto", "Commit"]) {
        audio.auto_commit = v;
    }
    if let Some(v) = get_i64(root, &["Audio", "Lead", "IN"]) {
        audio.lead_in_ms = v;
    }
    if let Some(v) = get_i64(root, &["Audio", "Lead", "OUT"]) {
        audio.lead_out_ms = v;
    }
}

fn apply_timing(root: &Value, timing: &mut TimingSettings) {
    if let Some(v) = get_i64(root, &["Timing", "Default Duration"]) {
        timing.default_duration_ms = v;
    }
    if let Some(v) = get_bool(
        root,
        &["Tool", "Timing Post Processor", "Enable", "Lead", "IN"],
    ) {
        timing.enable_lead_in = v;
    }
    if let Some(v) = get_bool(
        root,
        &["Tool", "Timing Post Processor", "Enable", "Lead", "OUT"],
    ) {
        timing.enable_lead_out = v;
    }
    if let Some(v) = get_bool(
        root,
        &["Tool", "Timing Post Processor", "Enable", "Adjacent"],
    ) {
        timing.enable_adjacent = v;
    }
    if let Some(v) = get_bool(
        root,
        &["Tool", "Timing Post Processor", "Enable", "Keyframe"],
    ) {
        timing.enable_keyframe = v;
    }
    if let Some(v) = get_i64(
        root,
        &["Tool", "Timing Post Processor", "Threshold", "Adjacent"],
    ) {
        timing.adjacent_threshold_ms = v;
    }
    if let Some(v) = get_f64(root, &["Tool", "Timing Post Processor", "Adjacent Bias"]) {
        timing.adjacent_bias = v;
    }
    if let Some(v) = get_i64(
        root,
        &[
            "Tool",
            "Timing Post Processor",
            "Threshold",
            "Key Start Before",
        ],
    ) {
        timing.key_start_before = v;
    }
    if let Some(v) = get_i64(
        root,
        &[
            "Tool",
            "Timing Post Processor",
            "Threshold",
            "Key Start After",
        ],
    ) {
        timing.key_start_after = v;
    }
    if let Some(v) = get_i64(
        root,
        &[
            "Tool",
            "Timing Post Processor",
            "Threshold",
            "Key End Before",
        ],
    ) {
        timing.key_end_before = v;
    }
    if let Some(v) = get_i64(
        root,
        &[
            "Tool",
            "Timing Post Processor",
            "Threshold",
            "Key End After",
        ],
    ) {
        timing.key_end_after = v;
    }
}

fn parse_combos(val: &Value) -> Vec<KeyCombo> {
    let Some(arr) = val.as_array() else {
        return Vec::new();
    };
    let mut out = Vec::with_capacity(arr.len());
    for item in arr {
        let Some(obj) = item.as_object() else {
            continue;
        };
        let Some(key) = obj.get("key").and_then(Value::as_str) else {
            continue;
        };
        let modifiers = obj
            .get("modifiers")
            .and_then(Value::as_array)
            .map_or_default(|m| {
                m.iter()
                    .filter_map(Value::as_str)
                    .map(ToOwned::to_owned)
                    .collect()
            });
        out.push(KeyCombo {
            modifiers,
            key: key.to_owned(),
        });
    }
    out
}

fn map_aegisub_command(name: &str) -> Option<CommandId> {
    match name {
        "app/exit" => Some(CommandId::AppExit),
        "subtitle/open" => Some(CommandId::FileOpen),
        "subtitle/save" => Some(CommandId::FileSave),
        "edit/undo" => Some(CommandId::EditUndo),
        "edit/redo" => Some(CommandId::EditRedo),
        "edit/find_replace" => Some(CommandId::EditFindReplace),
        "edit/line/copy" => Some(CommandId::EditLineCopy),
        "edit/line/cut" => Some(CommandId::EditLineCut),
        "edit/line/paste" => Some(CommandId::EditLinePaste),
        "edit/line/paste/over" => Some(CommandId::EditLinePasteOver),
        "edit/line/delete" => Some(CommandId::EditLineDelete),
        "edit/line/duplicate" => Some(CommandId::EditLineDuplicate),
        "edit/line/split/by_karaoke" => Some(CommandId::EditLineSplitByKaraoke),
        "time/shift" => Some(CommandId::TimeShift),
        "audio/play/selection" => Some(CommandId::AudioPlaySelection),
        "audio/commit" => Some(CommandId::AudioCommit),
        "audio/go_to" => Some(CommandId::AudioGoTo),
        "video/play" => Some(CommandId::VideoPlay),
        "video/frame/next" => Some(CommandId::VideoFrameNext),
        "video/frame/prev" => Some(CommandId::VideoFramePrev),
        _ => None,
    }
}

fn walk<'a>(root: &'a Value, path: &[&str]) -> Option<&'a Value> {
    let mut cur = root;
    for &seg in path {
        cur = cur.get(seg)?;
    }
    Some(cur)
}

fn get_str<'a>(root: &'a Value, path: &[&str]) -> Option<&'a str> {
    walk(root, path)?.as_str()
}

fn get_bool(root: &Value, path: &[&str]) -> Option<bool> {
    walk(root, path)?.as_bool()
}

fn get_i64(root: &Value, path: &[&str]) -> Option<i64> {
    walk(root, path)?.as_i64()
}

fn get_u64(root: &Value, path: &[&str]) -> Option<u64> {
    walk(root, path)?.as_u64()
}

fn get_u32(root: &Value, path: &[&str]) -> Option<u32> {
    u32::try_from(walk(root, path)?.as_u64()?).ok()
}

fn get_f64(root: &Value, path: &[&str]) -> Option<f64> {
    walk(root, path)?.as_f64()
}

fn get_color(root: &Value, path: &[&str]) -> Option<RgbaColor> {
    RgbaColor::parse_aegisub(get_str(root, path)?)
}

/// Summary report produced when importing an Aegisub 3.5.1 config + hotkey directory.
#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub struct AegisubImportReport {
    pub config_loaded: bool,
    pub hotkeys_loaded: bool,
    pub mapped_hotkeys: usize,
    pub unmapped_hotkeys: Vec<UnmappedHotkey>,
    pub unmapped_keys: Vec<String>,
}

/// Detects the user's existing Aegisub config directory (`%APPDATA%\Aegisub` on Windows or
/// `~/.aegisub` / `$XDG_CONFIG_HOME/aegisub` on Unix) if `config.json` or `hotkey.json` exists.
#[must_use]
pub fn detect_aegisub_config_dir() -> Option<std::path::PathBuf> {
    let mut candidates = Vec::new();
    if let Some(appdata) = std::env::var_os("APPDATA") {
        candidates.push(std::path::PathBuf::from(appdata).join("Aegisub"));
    }
    if let Some(xdg) = std::env::var_os("XDG_CONFIG_HOME") {
        candidates.push(std::path::PathBuf::from(xdg).join("aegisub"));
    }
    if let Some(home) = std::env::var_os("HOME") {
        candidates.push(std::path::PathBuf::from(home).join(".aegisub"));
    }
    candidates
        .into_iter()
        .find(|d| d.join("config.json").is_file() || d.join("hotkey.json").is_file())
}

/// Imports `config.json` and optional `hotkey.json` from `dir`, returning both the typed
/// [`ImportedSettings`] and an [`AegisubImportReport`] listing any unmapped hotkeys or keys.
///
/// # Errors
/// Returns [`ConfigImportError`] if reading or parsing `config.json` or `hotkey.json` fails.
pub fn import_aegisub_dir(
    dir: &Path,
) -> Result<(ImportedSettings, AegisubImportReport), ConfigImportError> {
    let cfg_path = dir.join("config.json");
    let hk_path = dir.join("hotkey.json");
    let mut report = AegisubImportReport::default();

    let mut imported = if cfg_path.is_file() {
        let raw = fs::read_to_string(&cfg_path)?;
        report.config_loaded = true;
        import_config_json(&raw)?
    } else {
        ImportedSettings::default()
    };

    if hk_path.is_file() {
        let raw_hk = fs::read_to_string(&hk_path)?;
        let hk = import_hotkey_json(&raw_hk)?;
        report.hotkeys_loaded = true;
        report.mapped_hotkeys = hk.by_context.values().map(Vec::len).sum();
        report.unmapped_hotkeys.clone_from(&hk.unmapped);
        imported.hotkeys = hk;
    }

    Ok((imported, report))
}

#[cfg(test)]
mod tests {
    use super::*;

    const CONFIG_FIXTURE: &str = include_str!("../tests/fixtures/config.json");
    const HOTKEY_FIXTURE: &str = include_str!("../tests/fixtures/hotkey.json");

    #[test]
    fn imports_aegisub_config_and_hotkeys_tolerating_unknown_fields() {
        let imported = import_aegisub_json(CONFIG_FIXTURE, HOTKEY_FIXTURE).expect("import");

        assert_eq!(imported.layout.edit_box_font_face, "Consolas");
        assert_eq!(imported.grid.font_face, "Tahoma");
        assert_eq!(imported.grid.font_size, 9);
        assert!(imported.grid.columns.cps);
        assert!(!imported.grid.columns.effect);
        assert_eq!(imported.colors.selection, RgbaColor::rgb(206, 255, 231));
        assert_eq!(imported.colors.collision_text, RgbaColor::rgb(255, 0, 0));
        assert_eq!(imported.audio.lead_in_ms, 200);
        assert_eq!(imported.timing.default_duration_ms, 2000);
        assert_eq!(imported.autosave.interval_seconds, 60);
        assert_eq!(
            imported.unknown_top_level_keys,
            vec!["UnknownCustomSection"]
        );

        // Mapped hotkeys
        let grid_hks = imported
            .hotkeys
            .by_context
            .get("Subtitle Grid")
            .expect("grid context");
        assert!(grid_hks
            .iter()
            .any(|h| h.command == CommandId::EditLineDuplicate && h.combos[0].key == "D"));

        // Unmapped custom automation macro preserved in unmapped list
        assert_eq!(imported.hotkeys.unmapped.len(), 1);
        assert_eq!(
            imported.hotkeys.unmapped[0].aegisub_name,
            "automation/lua/custom_macro_xyz"
        );
    }

    #[test]
    fn handles_empty_or_partial_json_gracefully() {
        let imported = import_aegisub_json("{}", "{}").expect("empty");
        assert_eq!(imported.autosave.interval_seconds, 60);
        assert!(imported.hotkeys.by_context.is_empty());
        assert_eq!(imported.hotkeys.unmapped.len(), 0);
    }

    #[test]
    fn non_object_json_is_an_error_not_empty_settings() {
        assert!(matches!(
            import_config_json("[]"),
            Err(ConfigImportError::NotAnObject {
                file: "config.json",
                ..
            })
        ));
        let err = import_hotkey_json("42").unwrap_err();
        assert!(err.to_string().contains("hotkey.json"));
    }
}
