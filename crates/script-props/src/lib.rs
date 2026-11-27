//! Aegisub 3.5.1 Script Properties dialog logic and resolution / YCbCr mismatch decision engine
//! (`src/dialog_properties.cpp` and `src/video_controller.cpp`).
//!
//! Reads/writes all standard `[Script Info]` properties, computes "from video" resolution updates,
//! implements Aegisub's resolution-mismatch & YCbCr-mismatch prompt rules, and produces reversible
//! [`ScriptInfoEdit`] patches against [`ass_core::ScriptFile`].

#![forbid(unsafe_code)]
#![cfg_attr(not(test), deny(clippy::unwrap_used, clippy::expect_used))]

use ass_core::{EditCommand, ScriptFile};
use thiserror::Error;

/// Errors returned by `script-props`.
#[derive(Debug, Error, PartialEq, Eq)]
pub enum ScriptPropsError {
    /// Script has no `[Script Info]` section.
    #[error("script has no [Script Info] section")]
    MissingScriptInfo,
    /// Underlying `ass-core` edit failed.
    #[error("ass-core edit failed: {0}")]
    Core(String),
}

/// ASS `WrapStyle` (`0..=3`, matches libass / Aegisub `src/dialog_properties.cpp`).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum WrapStyle {
    /// `0`: Smart wrapping, top line wider.
    #[default]
    SmartTopWider = 0,
    /// `1`: End-of-line word wrapping (only `\N` breaks).
    EndOfLine = 1,
    /// `2`: No word wrapping (`\n` and `\N` both break).
    NoWrap = 2,
    /// `3`: Smart wrapping, bottom line wider.
    SmartBottomWider = 3,
}

impl WrapStyle {
    /// Parses `"0"`..`"3"` into [`WrapStyle`].
    #[must_use]
    pub fn from_str_ass(s: &str) -> Self {
        match s.trim() {
            "1" => Self::EndOfLine,
            "2" => Self::NoWrap,
            "3" => Self::SmartBottomWider,
            _ => Self::SmartTopWider,
        }
    }

    /// Formats as ASS integer string.
    #[must_use]
    pub const fn as_str(self) -> &'static str {
        match self {
            Self::SmartTopWider => "0",
            Self::EndOfLine => "1",
            Self::NoWrap => "2",
            Self::SmartBottomWider => "3",
        }
    }
}

/// Standard ASS `YCbCr Matrix` header values (`src/dialog_properties.cpp`).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum YCbCrMatrix {
    #[default]
    None,
    Tv601,
    Pc601,
    Tv709,
    Pc709,
    TvFcc,
    PcFcc,
    Tv240m,
    Pc240m,
}

impl YCbCrMatrix {
    /// Parses an ASS `YCbCr Matrix` value string.
    #[must_use]
    pub fn from_ass(s: &str) -> Self {
        match s.trim() {
            "TV.601" => Self::Tv601,
            "PC.601" => Self::Pc601,
            "TV.709" => Self::Tv709,
            "PC.709" => Self::Pc709,
            "TV.FCC" => Self::TvFcc,
            "PC.FCC" => Self::PcFcc,
            "TV.240M" => Self::Tv240m,
            "PC.240M" => Self::Pc240m,
            _ => Self::None,
        }
    }

    /// Formats as canonical ASS header value.
    #[must_use]
    pub const fn as_str(self) -> &'static str {
        match self {
            Self::None => "None",
            Self::Tv601 => "TV.601",
            Self::Pc601 => "PC.601",
            Self::Tv709 => "TV.709",
            Self::Pc709 => "PC.709",
            Self::TvFcc => "TV.FCC",
            Self::PcFcc => "PC.FCC",
            Self::Tv240m => "TV.240M",
            Self::Pc240m => "PC.240M",
        }
    }
}

/// Complete Script Properties model (`src/dialog_properties.cpp`).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ScriptProperties {
    pub title: String,
    pub original_script: String,
    pub original_translation: String,
    pub original_editing: String,
    pub original_timing: String,
    pub synch_point: String,
    pub updated_by: String,
    pub update_details: String,
    pub play_res_x: u32,
    pub play_res_y: u32,
    pub layout_res_x: Option<u32>,
    pub layout_res_y: Option<u32>,
    pub wrap_style: WrapStyle,
    pub scaled_border_and_shadow: bool,
    pub ycbcr_matrix: YCbCrMatrix,
    pub kerning: bool,
}

impl Default for ScriptProperties {
    /// Defaults for a new script matching Aegisub 3.5.1.
    fn default() -> Self {
        Self {
            title: "Default Aegisub file".to_owned(),
            original_script: String::new(),
            original_translation: String::new(),
            original_editing: String::new(),
            original_timing: String::new(),
            synch_point: String::new(),
            updated_by: String::new(),
            update_details: String::new(),
            play_res_x: 0,
            play_res_y: 0,
            layout_res_x: None,
            layout_res_y: None,
            wrap_style: WrapStyle::SmartTopWider,
            scaled_border_and_shadow: true,
            ycbcr_matrix: YCbCrMatrix::None,
            kerning: false,
        }
    }
}

impl ScriptProperties {
    /// Reads all Script Properties from a [`ScriptFile`]'s `[Script Info]` section.
    #[must_use]
    pub fn from_script(script: &ScriptFile) -> Self {
        Self::from_script_with_warnings(script).0
    }

    /// Like [`ScriptProperties::from_script`] but also reports every resolution value that was
    /// present yet unparseable (and therefore replaced by a default).
    #[must_use]
    pub fn from_script_with_warnings(script: &ScriptFile) -> (Self, Vec<String>) {
        let mut warnings = Vec::new();
        let mut props = Self::default();
        let Some(info) = script.script_info() else {
            warnings.push("the script has no [Script Info] section; defaults were used".to_owned());
            return (props, warnings);
        };
        for key in ["PlayResX", "PlayResY", "LayoutResX", "LayoutResY"] {
            if let Some(v) = info.get(key) {
                if v.trim().parse::<u32>().is_err() {
                    warnings.push(format!(
                        "`{key}: {v}` is not a valid resolution; it was ignored"
                    ));
                }
            }
        }
        if let Some(v) = info.get("Title") {
            v.clone_into(&mut props.title);
        }
        if let Some(v) = info.get("Original Script") {
            v.clone_into(&mut props.original_script);
        }
        if let Some(v) = info.get("Original Translation") {
            v.clone_into(&mut props.original_translation);
        }
        if let Some(v) = info.get("Original Editing") {
            v.clone_into(&mut props.original_editing);
        }
        if let Some(v) = info.get("Original Timing") {
            v.clone_into(&mut props.original_timing);
        }
        if let Some(v) = info.get("Synch Point") {
            v.clone_into(&mut props.synch_point);
        }
        if let Some(v) = info.get("Script Updated By") {
            v.clone_into(&mut props.updated_by);
        }
        if let Some(v) = info.get("Update Details") {
            v.clone_into(&mut props.update_details);
        }
        if let Some(v) = info
            .get("PlayResX")
            .and_then(|s| s.trim().parse::<u32>().ok())
        {
            props.play_res_x = v;
        }
        if let Some(v) = info
            .get("PlayResY")
            .and_then(|s| s.trim().parse::<u32>().ok())
        {
            props.play_res_y = v;
        }
        props.layout_res_x = info
            .get("LayoutResX")
            .and_then(|s| s.trim().parse::<u32>().ok());
        props.layout_res_y = info
            .get("LayoutResY")
            .and_then(|s| s.trim().parse::<u32>().ok());
        if let Some(v) = info.get("WrapStyle") {
            props.wrap_style = WrapStyle::from_str_ass(v);
        }
        if let Some(v) = info.get("ScaledBorderAndShadow") {
            props.scaled_border_and_shadow = v.trim().eq_ignore_ascii_case("yes");
        }
        if let Some(v) = info.get("YCbCr Matrix") {
            props.ycbcr_matrix = YCbCrMatrix::from_ass(v);
        }
        if let Some(v) = info.get("Kerning") {
            props.kerning = v.trim().eq_ignore_ascii_case("yes");
        }
        (props, warnings)
    }

    /// Sets `PlayResX`/`PlayResY` from the loaded video's dimensions ("From video" button).
    pub fn set_from_video(&mut self, video_w: u32, video_h: u32) {
        self.play_res_x = video_w;
        self.play_res_y = video_h;
    }

    /// Computes a minimal reversible [`ScriptInfoEdit`] diffing `self` against `script`.
    ///
    /// # Errors
    /// Returns [`ScriptPropsError::MissingScriptInfo`] if `[Script Info]` is absent.
    pub fn build_edit(&self, script: &ScriptFile) -> Result<ScriptInfoEdit, ScriptPropsError> {
        let info = script
            .script_info()
            .ok_or(ScriptPropsError::MissingScriptInfo)?;
        let mut changes = Vec::new();

        let mut push_if_changed = |key: &'static str, new_val: String| {
            let old = info.get(key).map(ToOwned::to_owned);
            if old.as_deref() != Some(new_val.as_str()) {
                changes.push(ScriptInfoChange {
                    key,
                    before: old,
                    after: new_val,
                });
            }
        };

        push_if_changed("Title", self.title.clone());
        if !self.original_script.is_empty() || info.get("Original Script").is_some() {
            push_if_changed("Original Script", self.original_script.clone());
        }
        if !self.original_translation.is_empty() || info.get("Original Translation").is_some() {
            push_if_changed("Original Translation", self.original_translation.clone());
        }
        if !self.original_editing.is_empty() || info.get("Original Editing").is_some() {
            push_if_changed("Original Editing", self.original_editing.clone());
        }
        if !self.original_timing.is_empty() || info.get("Original Timing").is_some() {
            push_if_changed("Original Timing", self.original_timing.clone());
        }
        if !self.synch_point.is_empty() || info.get("Synch Point").is_some() {
            push_if_changed("Synch Point", self.synch_point.clone());
        }
        if !self.updated_by.is_empty() || info.get("Script Updated By").is_some() {
            push_if_changed("Script Updated By", self.updated_by.clone());
        }
        if !self.update_details.is_empty() || info.get("Update Details").is_some() {
            push_if_changed("Update Details", self.update_details.clone());
        }
        if self.play_res_x > 0 {
            push_if_changed("PlayResX", self.play_res_x.to_string());
        }
        if self.play_res_y > 0 {
            push_if_changed("PlayResY", self.play_res_y.to_string());
        }
        if let Some(lx) = self.layout_res_x {
            push_if_changed("LayoutResX", lx.to_string());
        }
        if let Some(ly) = self.layout_res_y {
            push_if_changed("LayoutResY", ly.to_string());
        }
        push_if_changed("WrapStyle", self.wrap_style.as_str().to_owned());
        push_if_changed(
            "ScaledBorderAndShadow",
            if self.scaled_border_and_shadow {
                "yes".to_owned()
            } else {
                "no".to_owned()
            },
        );
        push_if_changed("YCbCr Matrix", self.ycbcr_matrix.as_str().to_owned());
        if self.kerning || info.get("Kerning").is_some() {
            push_if_changed(
                "Kerning",
                if self.kerning {
                    "yes".to_owned()
                } else {
                    "no".to_owned()
                },
            );
        }

        Ok(ScriptInfoEdit { changes })
    }
}

/// Single key/value mutation in `[Script Info]`.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ScriptInfoChange {
    pub key: &'static str,
    pub before: Option<String>,
    pub after: String,
}

/// Reversible atomic edit for `[Script Info]` properties.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct ScriptInfoEdit {
    pub changes: Vec<ScriptInfoChange>,
}

impl ScriptInfoEdit {
    /// Converts this edit into an undoable [`ass_core::EditCommand`] (a `Batch` of
    /// `SetScriptInfo`), so header edits go through the normal undo/redo history.
    ///
    /// Returns `None` when there is nothing to change.
    #[must_use]
    pub fn to_command(&self) -> Option<EditCommand> {
        if self.changes.is_empty() {
            return None;
        }
        let cmds: Vec<EditCommand> = self
            .changes
            .iter()
            .map(|ch| EditCommand::SetScriptInfo {
                key: ch.key.to_owned(),
                before: ch.before.clone(),
                after: Some(ch.after.clone()),
            })
            .collect();
        Some(EditCommand::Batch(cmds))
    }

    /// Applies all changes forward to `script` via [`EditCommand::SetScriptInfo`].
    ///
    /// # Errors
    /// Returns [`ScriptPropsError::MissingScriptInfo`] if `[Script Info]` is absent.
    pub fn apply(&self, script: &mut ScriptFile) -> Result<(), ScriptPropsError> {
        if script.script_info().is_none() {
            return Err(ScriptPropsError::MissingScriptInfo);
        }
        if let Some(cmd) = self.to_command() {
            cmd.apply(script)
                .map_err(|e| ScriptPropsError::Core(e.to_string()))?;
        }
        Ok(())
    }

    /// Reverts all changes back to their prior values (keys that did not exist are removed).
    ///
    /// # Errors
    /// Returns [`ScriptPropsError::MissingScriptInfo`] if `[Script Info]` is absent.
    pub fn revert(&self, script: &mut ScriptFile) -> Result<(), ScriptPropsError> {
        if script.script_info().is_none() {
            return Err(ScriptPropsError::MissingScriptInfo);
        }
        if let Some(cmd) = self.to_command() {
            cmd.revert(script)
                .map_err(|e| ScriptPropsError::Core(e.to_string()))?;
        }
        Ok(())
    }
}

/// Configured policy for `Video/Check Script Res` (`src/video_controller.cpp`).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ResCheckPolicy {
    /// `0`: Never prompt or modify.
    Never,
    /// `1`: Ask user what to do.
    Ask,
    /// `2`: Automatically set to video resolution (without resampling).
    SetToVideo,
    /// `3`: Automatically resample script to video resolution.
    ResampleToVideo,
}

/// Decision emitted when video resolution differs from `PlayResX`/`PlayResY`.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ResMismatchDecision {
    NoAction,
    PromptUser {
        script_w: u32,
        script_h: u32,
        video_w: u32,
        video_h: u32,
    },
    SetToVideo {
        video_w: u32,
        video_h: u32,
    },
    ResampleToVideo {
        video_w: u32,
        video_h: u32,
    },
}

/// Evaluates Aegisub's resolution-mismatch rule on video open (`src/video_controller.cpp`).
#[must_use]
pub fn check_resolution_mismatch(
    props: &ScriptProperties,
    video_w: u32,
    video_h: u32,
    script_has_events: bool,
    policy: ResCheckPolicy,
) -> ResMismatchDecision {
    if video_w == 0 || video_h == 0 {
        return ResMismatchDecision::NoAction;
    }
    if props.play_res_x == video_w && props.play_res_y == video_h {
        return ResMismatchDecision::NoAction;
    }
    // Unset PlayRes (0x0) or empty new script: silently adopt video resolution as Aegisub does.
    if (props.play_res_x == 0 && props.play_res_y == 0) || !script_has_events {
        return ResMismatchDecision::SetToVideo { video_w, video_h };
    }
    match policy {
        ResCheckPolicy::Never => ResMismatchDecision::NoAction,
        ResCheckPolicy::Ask => ResMismatchDecision::PromptUser {
            script_w: props.play_res_x,
            script_h: props.play_res_y,
            video_w,
            video_h,
        },
        ResCheckPolicy::SetToVideo => ResMismatchDecision::SetToVideo { video_w, video_h },
        ResCheckPolicy::ResampleToVideo => {
            ResMismatchDecision::ResampleToVideo { video_w, video_h }
        }
    }
}

/// Decision emitted when video color matrix differs from `YCbCr Matrix` header.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum YCbCrMismatchDecision {
    NoAction,
    SetHeader(YCbCrMatrix),
    PromptUser {
        script_matrix: YCbCrMatrix,
        video_matrix: YCbCrMatrix,
    },
}

/// Evaluates Aegisub's YCbCr matrix mismatch rule (`src/video_controller.cpp`).
#[must_use]
pub fn check_ycbcr_mismatch(
    props: &ScriptProperties,
    video_matrix: YCbCrMatrix,
) -> YCbCrMismatchDecision {
    if video_matrix == YCbCrMatrix::None || props.ycbcr_matrix == video_matrix {
        return YCbCrMismatchDecision::NoAction;
    }
    if props.ycbcr_matrix == YCbCrMatrix::None {
        return YCbCrMismatchDecision::SetHeader(video_matrix);
    }
    YCbCrMismatchDecision::PromptUser {
        script_matrix: props.ycbcr_matrix,
        video_matrix,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn properties_roundtrip_revert_and_mismatch_decisions() {
        let mut script = ass_core::parse_ass(
            "[Script Info]\nTitle: Old\nPlayResX: 1280\nPlayResY: 720\nWrapStyle: 0\nScaledBorderAndShadow: yes\nYCbCr Matrix: TV.601\n",
        )
        .value;

        let mut props = ScriptProperties::from_script(&script);
        assert_eq!(props.play_res_x, 1280);
        assert_eq!(props.ycbcr_matrix, YCbCrMatrix::Tv601);

        assert_eq!(
            check_resolution_mismatch(&props, 1920, 1080, true, ResCheckPolicy::Ask),
            ResMismatchDecision::PromptUser {
                script_w: 1280,
                script_h: 720,
                video_w: 1920,
                video_h: 1080
            }
        );
        assert_eq!(
            check_ycbcr_mismatch(&props, YCbCrMatrix::Tv709),
            YCbCrMismatchDecision::PromptUser {
                script_matrix: YCbCrMatrix::Tv601,
                video_matrix: YCbCrMatrix::Tv709
            }
        );

        props.set_from_video(1920, 1080);
        props.ycbcr_matrix = YCbCrMatrix::Tv709;
        props.kerning = true;
        let edit = props.build_edit(&script).expect("edit");
        edit.apply(&mut script).expect("apply");
        assert_eq!(ScriptProperties::from_script(&script).play_res_x, 1920);
        assert!(ScriptProperties::from_script(&script).kerning);

        edit.revert(&mut script).expect("revert");
        assert_eq!(ScriptProperties::from_script(&script).play_res_x, 1280);
    }

    #[test]
    fn edit_goes_through_undo_history() {
        let mut script = ass_core::parse_ass(
            "[Script Info]
Title: Old
PlayResX: 1280
PlayResY: 720
",
        )
        .value;
        let mut props = ScriptProperties::from_script(&script);
        props.set_from_video(1920, 1080);
        props.kerning = true;
        let edit = props.build_edit(&script).expect("edit");
        let cmd = edit.to_command().expect("changes");
        let mut history = ass_core::UndoHistory::default();
        history.execute(&mut script, cmd).expect("execute");
        assert_eq!(ScriptProperties::from_script(&script).play_res_x, 1920);
        assert!(history.undo(&mut script).expect("undo"));
        let after_undo = ScriptProperties::from_script(&script);
        assert_eq!(after_undo.play_res_x, 1280);
        assert!(!after_undo.kerning);
        assert!(script
            .script_info()
            .is_some_and(|i| i.get("Kerning").is_none()));
        assert!(history.redo(&mut script).expect("redo"));
        assert_eq!(ScriptProperties::from_script(&script).play_res_x, 1920);
    }

    #[test]
    fn invalid_resolution_is_reported() {
        let script = ass_core::parse_ass(
            "[Script Info]
PlayResX: wide
PlayResY: 480
",
        )
        .value;
        let (props, warnings) = ScriptProperties::from_script_with_warnings(&script);
        assert_eq!(props.play_res_y, 480);
        assert_eq!(warnings.len(), 1, "{warnings:?}");
        assert!(warnings[0].contains("PlayResX"));
    }
}
