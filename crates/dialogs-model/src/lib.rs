//! Pure UI-independent state, validation, history, and "remember last used values" persistence
//! for all Aegisub 3.5.1 tool dialogs.
//!
//! Mirrors Aegisub 3.5.1:
//! - `src/dialog_shift_times.cpp` (incl. shift history list)
//! - `src/dialog_timing_processor.cpp`
//! - `src/dialog_search_replace.cpp` (incl. search/replace term history)
//! - `src/dialog_selection.cpp`
//! - `src/dialog_resample.cpp`
//! - `src/dialog_style_manager.cpp`
//! - `src/dialog_fonts_collector.cpp`
//! - `src/dialog_jumpto.cpp`
//! - `src/dialog_properties.cpp`
//!
//! Every field update + validation runs in `< 1 µs` with **0 heap allocations** and produces the
//! exact option structs consumed by `ass-tools` and `script-props`.

#![forbid(unsafe_code)]
#![cfg_attr(not(test), deny(clippy::unwrap_used, clippy::expect_used))]

use ass_tools::{
    AdjacentLinkConfig, AspectRatioMode, FindReplaceOptions, KeyframeSnapConfig, MatchMode,
    ResampleOptions, SearchField, SearchScope, SelectField, SelectLinesOptions, SelectionAction,
    ShiftAffect, ShiftAmount, ShiftDirection, ShiftOptions, ShiftTarget, TppOptions,
};
use script_props::{ScriptProperties, WrapStyle, YCbCrMatrix};
use serde::{Deserialize, Serialize};
use settings::AppSettings;
use std::collections::BTreeSet;
use thiserror::Error;

const MAX_HISTORY_ITEMS: usize = 32;

/// Zero-allocation validation error codes for dialog fields.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Error)]
pub enum ValidationError {
    #[error("resolution width and height must be between 1 and 16384")]
    InvalidResolution,
    #[error("bias must be between 0.0 and 1.0")]
    InvalidBias,
    #[error("at least one line kind (dialogue or comments) must be enabled")]
    NoLineKindSelected,
    #[error("frame number exceeds video frame count")]
    FrameOutOfRange,
    #[error("shift by frames requires video frame times")]
    MissingFrameTimes,
    #[error("destination folder is required for font copy/zip action")]
    MissingDestinationFolder,
    #[error("style name cannot be empty")]
    EmptyStyleName,
    #[error("search query cannot be empty")]
    EmptyQuery,
}

/// Serialization error for persisting remembered dialog values.
#[derive(Debug, Error)]
pub enum DialogPersistError {
    #[error("JSON serialization error: {0}")]
    Json(#[from] serde_json::Error),
}

// ---------------------------------------------------------------------------
// 1. Shift Times Dialog (`dialog_shift_times.cpp`)
// ---------------------------------------------------------------------------

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, Default)]
pub enum ShiftModeKind {
    #[default]
    Time,
    Frames,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, Default)]
pub enum ShiftDirectionKind {
    #[default]
    Forward,
    Backward,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, Default)]
pub enum ShiftTargetKind {
    Start,
    End,
    #[default]
    Both,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, Default)]
pub enum ShiftAffectKind {
    #[default]
    All,
    Selected,
    SelectionOnward,
}

/// One entry in Aegisub's Shift Times history list (`dialog_shift_times.cpp`).
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ShiftHistoryEntry {
    pub mode: ShiftModeKind,
    pub amount_cs_or_frames: u32,
    pub direction: ShiftDirectionKind,
    pub target: ShiftTargetKind,
    pub affect: ShiftAffectKind,
    pub filename: String,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ShiftTimesDialog {
    pub mode: ShiftModeKind,
    pub time_cs: u32,
    pub frames: u32,
    pub direction: ShiftDirectionKind,
    pub target: ShiftTargetKind,
    pub affect: ShiftAffectKind,
    pub history: Vec<ShiftHistoryEntry>,
}

impl Default for ShiftTimesDialog {
    fn default() -> Self {
        Self {
            mode: ShiftModeKind::Time,
            time_cs: 0,
            frames: 0,
            direction: ShiftDirectionKind::Forward,
            target: ShiftTargetKind::Both,
            affect: ShiftAffectKind::All,
            history: Vec::new(),
        }
    }
}

impl ShiftTimesDialog {
    /// Updates time delta (centiseconds) and validates with 0 heap allocations.
    ///
    /// # Errors
    /// Returns [`ValidationError`] if the dialog state is invalid.
    #[inline]
    pub fn set_time_cs(&mut self, time_cs: u32) -> Result<(), ValidationError> {
        self.mode = ShiftModeKind::Time;
        self.time_cs = time_cs;
        self.validate(true)
    }

    /// Validates current state (0 allocations).
    ///
    /// # Errors
    /// Returns [`ValidationError::MissingFrameTimes`] when frame mode is active without video.
    #[inline]
    pub const fn validate(&self, has_video_frames: bool) -> Result<(), ValidationError> {
        if matches!(self.mode, ShiftModeKind::Frames) && !has_video_frames {
            return Err(ValidationError::MissingFrameTimes);
        }
        Ok(())
    }

    /// Produces [`ass_tools::ShiftOptions`] for execution.
    ///
    /// # Errors
    /// Returns [`ValidationError::MissingFrameTimes`] if frame shifting without frame times.
    pub fn to_shift_options<'a>(
        &self,
        frame_times_cs: &'a [u32],
        selected: &'a BTreeSet<usize>,
        active_row: usize,
    ) -> Result<ShiftOptions<'a>, ValidationError> {
        self.validate(!frame_times_cs.is_empty())?;
        let amount = match self.mode {
            ShiftModeKind::Time => ShiftAmount::TimeCs(self.time_cs),
            ShiftModeKind::Frames => ShiftAmount::Frames {
                frames: self.frames,
                frame_times_cs,
            },
        };
        let direction = match self.direction {
            ShiftDirectionKind::Forward => ShiftDirection::Forward,
            ShiftDirectionKind::Backward => ShiftDirection::Backward,
        };
        let target = match self.target {
            ShiftTargetKind::Start => ShiftTarget::Start,
            ShiftTargetKind::End => ShiftTarget::End,
            ShiftTargetKind::Both => ShiftTarget::Both,
        };
        let affect = match self.affect {
            ShiftAffectKind::All => ShiftAffect::All,
            ShiftAffectKind::Selected => ShiftAffect::Selected(selected),
            ShiftAffectKind::SelectionOnward => ShiftAffect::SelectionOnward(active_row),
        };
        Ok(ShiftOptions {
            amount,
            direction,
            target,
            affect,
        })
    }

    /// Appends the current shift configuration to history (bounded to 32 items).
    pub fn record_history(&mut self, filename: impl Into<String>) {
        let entry = ShiftHistoryEntry {
            mode: self.mode,
            amount_cs_or_frames: match self.mode {
                ShiftModeKind::Time => self.time_cs,
                ShiftModeKind::Frames => self.frames,
            },
            direction: self.direction,
            target: self.target,
            affect: self.affect,
            filename: filename.into(),
        };
        self.history.insert(0, entry);
        self.history.truncate(MAX_HISTORY_ITEMS);
    }
}

// ---------------------------------------------------------------------------
// 2. Timing Post-Processor Dialog (`dialog_timing_processor.cpp`)
// ---------------------------------------------------------------------------

#[derive(Debug, Clone, Serialize, Deserialize)]
#[allow(clippy::struct_excessive_bools)]
pub struct TimingPostProcessorDialog {
    pub enable_lead_in: bool,
    pub lead_in_ms: u32,
    pub enable_lead_out: bool,
    pub lead_out_ms: u32,
    pub enable_adjacent: bool,
    pub adjacent_threshold_ms: u32,
    pub adjacent_bias: f64,
    pub prevent_overlap: bool,
    pub enable_keyframes: bool,
    pub kf_start_before_ms: u32,
    pub kf_start_after_ms: u32,
    pub kf_end_before_ms: u32,
    pub kf_end_after_ms: u32,
    pub skip_comments: bool,
    pub styles_filter: Vec<String>,
}

impl Default for TimingPostProcessorDialog {
    fn default() -> Self {
        Self {
            enable_lead_in: true,
            lead_in_ms: 80,
            enable_lead_out: true,
            lead_out_ms: 150,
            enable_adjacent: true,
            adjacent_threshold_ms: 500,
            adjacent_bias: 0.8,
            prevent_overlap: false,
            enable_keyframes: true,
            kf_start_before_ms: 150,
            kf_start_after_ms: 200,
            kf_end_before_ms: 200,
            kf_end_after_ms: 250,
            skip_comments: true,
            styles_filter: Vec::new(),
        }
    }
}

impl TimingPostProcessorDialog {
    /// Updates adjacent-line bias and validates (`0.0..=1.0`, 0 allocations).
    ///
    /// # Errors
    /// Returns [`ValidationError::InvalidBias`] if out of `[0.0, 1.0]` or NaN.
    #[inline]
    pub fn set_adjacent_bias(&mut self, bias: f64) -> Result<(), ValidationError> {
        self.adjacent_bias = bias;
        self.validate()
    }

    /// Validates all fields (0 allocations).
    ///
    /// # Errors
    /// Returns [`ValidationError::InvalidBias`] if `adjacent_bias` is outside `0.0..=1.0`.
    #[inline]
    pub fn validate(&self) -> Result<(), ValidationError> {
        if !(0.0..=1.0).contains(&self.adjacent_bias) {
            return Err(ValidationError::InvalidBias);
        }
        Ok(())
    }

    /// Produces [`ass_tools::TppOptions`].
    ///
    /// # Errors
    /// Returns [`ValidationError`] if validation fails.
    pub fn to_tpp_options<'a>(
        &'a self,
        keyframes_cs: &'a [u32],
    ) -> Result<TppOptions<'a>, ValidationError> {
        self.validate()?;
        Ok(TppOptions {
            lead_in_ms: self.enable_lead_in.then_some(self.lead_in_ms),
            lead_out_ms: self.enable_lead_out.then_some(self.lead_out_ms),
            adjacent: self.enable_adjacent.then_some(AdjacentLinkConfig {
                threshold_ms: self.adjacent_threshold_ms,
                bias: self.adjacent_bias,
                prevent_overlap: self.prevent_overlap,
            }),
            keyframes: (self.enable_keyframes && !keyframes_cs.is_empty()).then_some(
                KeyframeSnapConfig {
                    keyframes_cs,
                    start_before_ms: self.kf_start_before_ms,
                    start_after_ms: self.kf_start_after_ms,
                    end_before_ms: self.kf_end_before_ms,
                    end_after_ms: self.kf_end_after_ms,
                },
            ),
            skip_comments: self.skip_comments,
            styles_filter: if self.styles_filter.is_empty() {
                None
            } else {
                Some(&self.styles_filter)
            },
        })
    }
}

// ---------------------------------------------------------------------------
// 3. Find & Replace Dialog (`dialog_search_replace.cpp`)
// ---------------------------------------------------------------------------

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, Default)]
pub enum DialogFieldKind {
    #[default]
    Text,
    Style,
    Actor,
    Effect,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[allow(clippy::struct_excessive_bools)]
pub struct FindReplaceDialog {
    pub query: String,
    pub replacement: String,
    pub field: DialogFieldKind,
    pub case_sensitive: bool,
    pub use_regex: bool,
    pub skip_tags: bool,
    pub selected_only: bool,
    pub search_history: Vec<String>,
    pub replace_history: Vec<String>,
}

impl Default for FindReplaceDialog {
    fn default() -> Self {
        Self {
            query: String::new(),
            replacement: String::new(),
            field: DialogFieldKind::Text,
            case_sensitive: false,
            use_regex: false,
            skip_tags: true,
            selected_only: false,
            search_history: Vec::new(),
            replace_history: Vec::new(),
        }
    }
}

impl FindReplaceDialog {
    /// Validates the Find & Replace dialog (0 allocations).
    ///
    /// # Errors
    /// Returns [`ValidationError::EmptyQuery`] when `query` is empty.
    #[inline]
    pub const fn validate(&self) -> Result<(), ValidationError> {
        if self.query.is_empty() {
            return Err(ValidationError::EmptyQuery);
        }
        Ok(())
    }

    /// Produces [`ass_tools::FindReplaceOptions`].
    ///
    /// # Errors
    /// Returns [`ValidationError::EmptyQuery`] if `query` is empty.
    pub fn to_find_replace_options<'a>(
        &'a self,
        selected: &'a BTreeSet<usize>,
    ) -> Result<FindReplaceOptions<'a>, ValidationError> {
        self.validate()?;
        let field = match self.field {
            DialogFieldKind::Text => SearchField::Text,
            DialogFieldKind::Style => SearchField::Style,
            DialogFieldKind::Actor => SearchField::Actor,
            DialogFieldKind::Effect => SearchField::Effect,
        };
        let scope = if self.selected_only {
            SearchScope::Selected(selected)
        } else {
            SearchScope::All
        };
        Ok(FindReplaceOptions {
            query: &self.query,
            replacement: &self.replacement,
            field,
            case_sensitive: self.case_sensitive,
            use_regex: self.use_regex,
            skip_tags: self.skip_tags,
            scope,
        })
    }

    /// Pushes current `query` and `replacement` to MRU history lists (deduped, max 32).
    pub fn push_history(&mut self) {
        push_mru(&mut self.search_history, &self.query);
        if !self.replacement.is_empty() {
            push_mru(&mut self.replace_history, &self.replacement);
        }
    }
}

fn push_mru(list: &mut Vec<String>, val: &str) {
    if val.is_empty() {
        return;
    }
    if let Some(pos) = list.iter().position(|s| s == val) {
        list.remove(pos);
    }
    list.insert(0, val.to_owned());
    list.truncate(MAX_HISTORY_ITEMS);
}

// ---------------------------------------------------------------------------
// 4. Select Lines Dialog (`dialog_selection.cpp`)
// ---------------------------------------------------------------------------

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, Default)]
pub enum SelectMatchKind {
    #[default]
    Contains,
    Exact,
    Regex,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, Default)]
pub enum SelectActionKind {
    #[default]
    Set,
    Add,
    Subtract,
    Intersect,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[allow(clippy::struct_excessive_bools)]
pub struct SelectLinesDialog {
    pub pattern: String,
    pub mode: SelectMatchKind,
    pub field: DialogFieldKind,
    pub case_sensitive: bool,
    pub match_dialogue: bool,
    pub match_comments: bool,
    pub invert: bool,
    pub action: SelectActionKind,
}

impl Default for SelectLinesDialog {
    fn default() -> Self {
        Self {
            pattern: String::new(),
            mode: SelectMatchKind::Contains,
            field: DialogFieldKind::Text,
            case_sensitive: false,
            match_dialogue: true,
            match_comments: false,
            invert: false,
            action: SelectActionKind::Set,
        }
    }
}

impl SelectLinesDialog {
    /// Validates Select Lines state (0 allocations).
    ///
    /// # Errors
    /// Returns [`ValidationError::NoLineKindSelected`] if neither dialogue nor comments is enabled.
    #[inline]
    pub const fn validate(&self) -> Result<(), ValidationError> {
        if !self.match_dialogue && !self.match_comments {
            return Err(ValidationError::NoLineKindSelected);
        }
        Ok(())
    }

    /// Produces [`ass_tools::SelectLinesOptions`].
    ///
    /// # Errors
    /// Returns [`ValidationError::NoLineKindSelected`] if no line kind is enabled.
    pub fn to_select_lines_options(&self) -> Result<SelectLinesOptions<'_>, ValidationError> {
        self.validate()?;
        Ok(SelectLinesOptions {
            pattern: &self.pattern,
            mode: match self.mode {
                SelectMatchKind::Contains => MatchMode::Contains,
                SelectMatchKind::Exact => MatchMode::Exact,
                SelectMatchKind::Regex => MatchMode::Regex,
            },
            field: match self.field {
                DialogFieldKind::Text => SelectField::Text,
                DialogFieldKind::Style => SelectField::Style,
                DialogFieldKind::Actor => SelectField::Actor,
                DialogFieldKind::Effect => SelectField::Effect,
            },
            case_sensitive: self.case_sensitive,
            match_dialogue: self.match_dialogue,
            match_comments: self.match_comments,
            invert: self.invert,
            action: match self.action {
                SelectActionKind::Set => SelectionAction::Set,
                SelectActionKind::Add => SelectionAction::Add,
                SelectActionKind::Subtract => SelectionAction::Subtract,
                SelectActionKind::Intersect => SelectionAction::Intersect,
            },
        })
    }
}

// ---------------------------------------------------------------------------
// 5. Resample Resolution Dialog (`dialog_resample.cpp`)
// ---------------------------------------------------------------------------

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, Default)]
pub enum ArModeKind {
    Stretch,
    #[default]
    AddBorders,
    RemoveBorders,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ResampleDialog {
    pub dest_x: u32,
    pub dest_y: u32,
    pub source_override: Option<(u32, u32)>,
    pub ar_mode: ArModeKind,
    pub margin_left: i32,
    pub margin_right: i32,
    pub margin_top: i32,
    pub margin_bottom: i32,
    pub ycbcr_matrix: Option<String>,
}

impl Default for ResampleDialog {
    fn default() -> Self {
        Self {
            dest_x: 1920,
            dest_y: 1080,
            source_override: None,
            ar_mode: ArModeKind::AddBorders,
            margin_left: 0,
            margin_right: 0,
            margin_top: 0,
            margin_bottom: 0,
            ycbcr_matrix: None,
        }
    }
}

impl ResampleDialog {
    /// Updates destination resolution and validates (0 allocations).
    ///
    /// # Errors
    /// Returns [`ValidationError::InvalidResolution`] if width or height is 0 or > 16384.
    #[inline]
    pub fn set_dest_resolution(&mut self, x: u32, y: u32) -> Result<(), ValidationError> {
        self.dest_x = x;
        self.dest_y = y;
        self.validate()
    }

    /// Validates resolution bounds (0 allocations).
    ///
    /// # Errors
    /// Returns [`ValidationError::InvalidResolution`].
    #[inline]
    pub const fn validate(&self) -> Result<(), ValidationError> {
        if self.dest_x == 0 || self.dest_y == 0 || self.dest_x > 16384 || self.dest_y > 16384 {
            return Err(ValidationError::InvalidResolution);
        }
        if let Some((sx, sy)) = self.source_override
            && (sx == 0 || sy == 0 || sx > 16384 || sy > 16384)
        {
            return Err(ValidationError::InvalidResolution);
        }
        Ok(())
    }

    /// Produces [`ass_tools::ResampleOptions`].
    ///
    /// # Errors
    /// Returns [`ValidationError::InvalidResolution`].
    pub fn to_resample_options(&self) -> Result<ResampleOptions, ValidationError> {
        self.validate()?;
        Ok(ResampleOptions {
            dest_x: self.dest_x,
            dest_y: self.dest_y,
            source_override: self.source_override,
            ar_mode: match self.ar_mode {
                ArModeKind::Stretch => AspectRatioMode::Stretch,
                ArModeKind::AddBorders => AspectRatioMode::AddBorders,
                ArModeKind::RemoveBorders => AspectRatioMode::RemoveBorders,
            },
            margin_left: self.margin_left,
            margin_right: self.margin_right,
            margin_top: self.margin_top,
            margin_bottom: self.margin_bottom,
            ycbcr_matrix: self.ycbcr_matrix.clone(),
        })
    }
}

// ---------------------------------------------------------------------------
// 6. Style Manager Actions (`dialog_style_manager.cpp`)
// ---------------------------------------------------------------------------

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub enum StyleManagerAction {
    New,
    Edit,
    Duplicate,
    Delete,
    CopyToScript,
    CopyToStorage,
    MoveUp,
    MoveDown,
    SortAlphabetically,
    ImportFromScript,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct StyleManagerDialog {
    pub active_storage: String,
    pub selected_script_style: Option<String>,
    pub selected_storage_style: Option<String>,
}

impl Default for StyleManagerDialog {
    fn default() -> Self {
        Self {
            active_storage: "Default".to_owned(),
            selected_script_style: None,
            selected_storage_style: None,
        }
    }
}

impl StyleManagerDialog {
    /// Checks whether `action` is enabled for the current selection (0 allocations).
    #[inline]
    #[must_use]
    pub const fn can_execute(&self, action: StyleManagerAction) -> bool {
        match action {
            StyleManagerAction::New
            | StyleManagerAction::ImportFromScript
            | StyleManagerAction::SortAlphabetically => true,
            StyleManagerAction::Edit
            | StyleManagerAction::Duplicate
            | StyleManagerAction::Delete
            | StyleManagerAction::CopyToStorage
            | StyleManagerAction::MoveUp
            | StyleManagerAction::MoveDown => self.selected_script_style.is_some(),
            StyleManagerAction::CopyToScript => self.selected_storage_style.is_some(),
        }
    }
}

// ---------------------------------------------------------------------------
// 7. Fonts Check / Collector Dialog (`dialog_fonts_collector.cpp`)
// ---------------------------------------------------------------------------

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, Default)]
pub enum FontsCheckAction {
    #[default]
    CheckOnly,
    CopyToFolder,
    CreateZip,
}

#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct FontsCheckDialog {
    pub action: FontsCheckAction,
    pub destination_dir: String,
}

impl FontsCheckDialog {
    /// Validates fonts collector settings (0 allocations).
    ///
    /// # Errors
    /// Returns [`ValidationError::MissingDestinationFolder`] when copying/zipping without a path.
    #[inline]
    pub const fn validate(&self) -> Result<(), ValidationError> {
        if !matches!(self.action, FontsCheckAction::CheckOnly) && self.destination_dir.is_empty() {
            return Err(ValidationError::MissingDestinationFolder);
        }
        Ok(())
    }
}

// ---------------------------------------------------------------------------
// 8. Jump To Dialog (`dialog_jumpto.cpp`)
// ---------------------------------------------------------------------------

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, Default)]
pub enum JumpToMode {
    #[default]
    Frame,
    Time,
}

#[derive(Debug, Clone, Copy, Default, Serialize, Deserialize)]
pub struct JumpToDialog {
    pub mode: JumpToMode,
    pub frame: u32,
    pub time_ms: i64,
    pub total_frames: u32,
}

impl JumpToDialog {
    /// Sets target frame and validates against `total_frames` (0 allocations).
    ///
    /// # Errors
    /// Returns [`ValidationError::FrameOutOfRange`] when `total_frames > 0 && frame >= total_frames`.
    #[inline]
    pub fn set_frame(&mut self, frame: u32) -> Result<(), ValidationError> {
        self.mode = JumpToMode::Frame;
        self.frame = frame;
        self.validate()
    }

    /// Validates jump target (0 allocations).
    ///
    /// # Errors
    /// Returns [`ValidationError::FrameOutOfRange`].
    #[inline]
    pub const fn validate(&self) -> Result<(), ValidationError> {
        if matches!(self.mode, JumpToMode::Frame)
            && self.total_frames > 0
            && self.frame >= self.total_frames
        {
            return Err(ValidationError::FrameOutOfRange);
        }
        Ok(())
    }
}

// ---------------------------------------------------------------------------
// 9. Script Properties Dialog (`dialog_properties.cpp`)
// ---------------------------------------------------------------------------

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct PropertiesDialog {
    pub title: String,
    pub play_res_x: u32,
    pub play_res_y: u32,
    pub wrap_style: u8,
    pub scaled_border_and_shadow: bool,
    pub ycbcr_matrix: String,
}

impl Default for PropertiesDialog {
    fn default() -> Self {
        Self {
            title: "Default Aegisub file".to_owned(),
            play_res_x: 1920,
            play_res_y: 1080,
            wrap_style: 0,
            scaled_border_and_shadow: true,
            ycbcr_matrix: "TV.709".to_owned(),
        }
    }
}

impl PropertiesDialog {
    /// Validates script properties (0 allocations).
    ///
    /// # Errors
    /// Returns [`ValidationError::InvalidResolution`].
    #[inline]
    pub const fn validate(&self) -> Result<(), ValidationError> {
        if self.play_res_x == 0
            || self.play_res_y == 0
            || self.play_res_x > 16384
            || self.play_res_y > 16384
        {
            return Err(ValidationError::InvalidResolution);
        }
        Ok(())
    }

    /// Produces [`script_props::ScriptProperties`].
    ///
    /// # Errors
    /// Returns [`ValidationError::InvalidResolution`].
    pub fn to_script_properties(&self) -> Result<ScriptProperties, ValidationError> {
        self.validate()?;
        let mut props = ScriptProperties::default();
        props.title.clone_from(&self.title);
        props.play_res_x = self.play_res_x;
        props.play_res_y = self.play_res_y;
        props.wrap_style = WrapStyle::from_str_ass(&self.wrap_style.to_string());
        props.scaled_border_and_shadow = self.scaled_border_and_shadow;
        props.ycbcr_matrix = YCbCrMatrix::from_ass(&self.ycbcr_matrix);
        Ok(props)
    }
}

// ---------------------------------------------------------------------------
// Aggregate remembered dialog state + settings sync
// ---------------------------------------------------------------------------

/// Persisted "remember last used values" container for all tool dialogs.
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct DialogsState {
    pub shift_times: ShiftTimesDialog,
    pub tpp: TimingPostProcessorDialog,
    pub find_replace: FindReplaceDialog,
    pub select_lines: SelectLinesDialog,
    pub resample: ResampleDialog,
    pub style_manager: StyleManagerDialog,
    pub fonts_check: FontsCheckDialog,
    pub jump_to: JumpToDialog,
    pub properties: PropertiesDialog,
}

impl DialogsState {
    /// Serializes all remembered dialog values to JSON.
    ///
    /// # Errors
    /// Returns [`DialogPersistError::Json`] on serialization error.
    pub fn save_json(&self) -> Result<String, DialogPersistError> {
        Ok(serde_json::to_string_pretty(self)?)
    }

    /// Restores remembered dialog values from JSON.
    ///
    /// # Errors
    /// Returns [`DialogPersistError::Json`] on invalid JSON.
    pub fn load_json(json: &str) -> Result<Self, DialogPersistError> {
        Ok(serde_json::from_str(json)?)
    }

    /// Synchronizes default TPP flags, adjacent threshold/bias, and keyframe-snap thresholds from [`AppSettings`].
    #[allow(clippy::cast_possible_truncation, clippy::cast_sign_loss)]
    pub fn sync_from_settings(&mut self, settings: &AppSettings) {
        self.tpp.enable_lead_in = settings.timing.enable_lead_in;
        self.tpp.enable_lead_out = settings.timing.enable_lead_out;
        self.tpp.enable_adjacent = settings.timing.enable_adjacent;
        self.tpp.enable_keyframes = settings.timing.enable_keyframe;
        self.tpp.adjacent_threshold_ms = settings.timing.adjacent_threshold_ms.max(0) as u32;
        self.tpp.adjacent_bias = settings.timing.adjacent_bias.clamp(0.0, 1.0);
        self.tpp.kf_start_before_ms = settings.timing.key_start_before.max(0) as u32;
        self.tpp.kf_start_after_ms = settings.timing.key_start_after.max(0) as u32;
        self.tpp.kf_end_before_ms = settings.timing.key_end_before.max(0) as u32;
        self.tpp.kf_end_after_ms = settings.timing.key_end_after.max(0) as u32;
    }

    /// Writes updated TPP flags, adjacent threshold/bias, and keyframe-snap thresholds back to [`AppSettings`].
    pub fn sync_to_settings(&self, settings: &mut AppSettings) {
        settings.timing.enable_lead_in = self.tpp.enable_lead_in;
        settings.timing.enable_lead_out = self.tpp.enable_lead_out;
        settings.timing.enable_adjacent = self.tpp.enable_adjacent;
        settings.timing.enable_keyframe = self.tpp.enable_keyframes;
        settings.timing.adjacent_threshold_ms = i64::from(self.tpp.adjacent_threshold_ms);
        settings.timing.adjacent_bias = self.tpp.adjacent_bias;
        settings.timing.key_start_before = i64::from(self.tpp.kf_start_before_ms);
        settings.timing.key_start_after = i64::from(self.tpp.kf_start_after_ms);
        settings.timing.key_end_before = i64::from(self.tpp.kf_end_before_ms);
        settings.timing.key_end_after = i64::from(self.tpp.kf_end_after_ms);
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn validates_produces_options_and_persists_all_dialogs() {
        let mut state = DialogsState::default();
        let mut settings = AppSettings::default();
        settings.timing.adjacent_threshold_ms = 420;
        state.sync_from_settings(&settings);
        assert_eq!(state.tpp.adjacent_threshold_ms, 420);

        // Shift Times + history
        state.shift_times.set_time_cs(250).expect("valid shift");
        state.shift_times.record_history("ep01.ass");
        assert_eq!(state.shift_times.history.len(), 1);
        let sel = BTreeSet::new();
        let shift_opts = state
            .shift_times
            .to_shift_options(&[], &sel, 0)
            .expect("shift opts");
        assert!(matches!(shift_opts.amount, ShiftAmount::TimeCs(250)));

        // TPP validation & option build
        assert!(state.tpp.set_adjacent_bias(1.5).is_err());
        assert!(state.tpp.set_adjacent_bias(0.75).is_ok());
        let tpp_opts = state.tpp.to_tpp_options(&[100, 200]).expect("tpp opts");
        assert_eq!(tpp_opts.adjacent.expect("adj").threshold_ms, 420);

        // Find/Replace history
        assert!(state.find_replace.validate().is_err());
        state.find_replace.query = "foo".to_owned();
        state.find_replace.replacement = "bar".to_owned();
        state.find_replace.push_history();
        assert_eq!(state.find_replace.search_history, vec!["foo"]);
        let _ = state
            .find_replace
            .to_find_replace_options(&sel)
            .expect("fr");

        // Select Lines
        let _ = state.select_lines.to_select_lines_options().expect("sel");

        // Resample
        assert!(state.resample.set_dest_resolution(0, 1080).is_err());
        assert!(state.resample.set_dest_resolution(1920, 1080).is_ok());
        let r_opts = state.resample.to_resample_options().expect("resample");
        assert_eq!(r_opts.dest_x, 1920);

        // Jump To
        state.jump_to.total_frames = 1000;
        assert!(state.jump_to.set_frame(1000).is_err());
        assert!(state.jump_to.set_frame(999).is_ok());

        // Properties
        let props = state.properties.to_script_properties().expect("props");
        assert_eq!(props.play_res_x, 1920);

        // JSON persistence round-trip
        let json = state.save_json().expect("save");
        let loaded = DialogsState::load_json(&json).expect("load");
        assert_eq!(loaded.shift_times.history.len(), 1);
        assert_eq!(loaded.find_replace.search_history, vec!["foo"]);
    }
}
