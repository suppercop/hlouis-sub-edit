//! Aegisub 3.5.1 Style Editor & Style Manager editing logic
//! (`src/dialog_style_editor.cpp` and `src/dialog_style_manager.cpp`).
//!
//! Handles all 23 V4+ Style fields, `&HAABBGGRR` colour/alpha conversion, border styles `1/3/4`,
//! encodings including `-1`, clamping/validation, atomic `apply` with rename propagation via
//! [`styles_catalog::rename_style`], copy/duplicate/delete with in-use warnings, and sorting.

#![forbid(unsafe_code)]
#![cfg_attr(not(test), deny(clippy::unwrap_used, clippy::expect_used))]

use ass_core::{AssStyle, EditCommand, ScriptFile};
use std::collections::HashSet;
use thiserror::Error;

/// Errors returned by `style-editor`.
#[derive(Debug, Error, PartialEq, Eq)]
pub enum StyleEditorError {
    /// Script has no `[V4+ Styles]` section.
    #[error("script has no [V4+ Styles] section")]
    MissingStylesSection,
    /// Style name cannot be empty after trimming/sanitizing.
    #[error("style name cannot be empty")]
    EmptyName,
    /// Target style was not found.
    #[error("style not found: {0}")]
    StyleNotFound(String),
    /// Duplicate style name conflict.
    #[error("style name already exists: {0}")]
    DuplicateName(String),
    /// Underlying `styles-catalog` rename error.
    #[error("rename propagation error: {0}")]
    Rename(String),
}

/// ASS V4+ colour (`&HAABBGGRR`, where `alpha == 0` is opaque and `255` is fully transparent).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct AssColor {
    pub r: u8,
    pub g: u8,
    pub b: u8,
    /// ASS transparency byte (`0x00` = opaque, `0xFF` = transparent).
    pub alpha: u8,
}

impl AssColor {
    /// Parses `&HAABBGGRR` (or `&HBBGGRR`) strings from `[V4+ Styles]`.
    #[must_use]
    pub fn parse_ass(raw: &str) -> Option<Self> {
        let stripped = raw
            .trim()
            .strip_prefix("&H")
            .or_else(|| raw.trim().strip_prefix("&h"))?;
        let hex = stripped.trim_end_matches('&');
        let val = u32::from_str_radix(hex, 16).ok()?;
        let alpha = u8::try_from((val >> 24) & 0xFF).ok()?;
        let b = u8::try_from((val >> 16) & 0xFF).ok()?;
        let g = u8::try_from((val >> 8) & 0xFF).ok()?;
        let r = u8::try_from(val & 0xFF).ok()?;
        Some(Self { r, g, b, alpha })
    }

    /// Formats as canonical ASS `&HAABBGGRR`.
    #[must_use]
    pub fn to_ass_string(self) -> String {
        format!(
            "&H{:02X}{:02X}{:02X}{:02X}",
            self.alpha, self.b, self.g, self.r
        )
    }
}

/// Supported ASS border styles (`1` = outline + shadow, `3` = opaque box, `4` = libass shadow box).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum BorderStyle {
    OutlineAndShadow = 1,
    OpaqueBox = 3,
    ShadowBox = 4,
}

impl BorderStyle {
    /// Converts an ASS integer (`1`, `3`, `4`) into [`BorderStyle`], defaulting to `1`.
    #[must_use]
    pub const fn from_i32(v: i32) -> Self {
        match v {
            3 => Self::OpaqueBox,
            4 => Self::ShadowBox,
            _ => Self::OutlineAndShadow,
        }
    }
}

/// Editable draft of an [`AssStyle`] inside the Style Editor dialog.
#[derive(Debug, Clone, PartialEq)]
#[allow(clippy::struct_excessive_bools)]
pub struct StyleDraft {
    pub name: String,
    pub fontname: String,
    pub fontsize: f64,
    pub primary_color: AssColor,
    pub secondary_color: AssColor,
    pub outline_color: AssColor,
    pub back_color: AssColor,
    pub bold: bool,
    pub italic: bool,
    pub underline: bool,
    pub strike_out: bool,
    pub scale_x: f64,
    pub scale_y: f64,
    pub spacing: f64,
    pub angle: f64,
    pub border_style: BorderStyle,
    pub outline: f64,
    pub shadow: f64,
    pub alignment: u8,
    pub margin_l: i32,
    pub margin_r: i32,
    pub margin_v: i32,
    /// Windows charset encoding (`-1` = auto / default, `0` = ANSI, `1` = Default, `128` = Shift-JIS, `163` = Vietnamese, etc.).
    pub encoding: i32,
    /// Preview sample text shown in the Style Editor preview pane.
    pub preview_text: String,
}

impl StyleDraft {
    /// Initializes a [`StyleDraft`] from an existing [`AssStyle`].
    #[must_use]
    pub fn from_style(style: &AssStyle) -> Self {
        Self {
            name: style.name().to_owned(),
            fontname: style.fontname().to_owned(),
            fontsize: style.fontsize(),
            primary_color: AssColor::parse_ass(style.primary_colour()).unwrap_or(AssColor {
                r: 255,
                g: 255,
                b: 255,
                alpha: 0,
            }),
            secondary_color: AssColor::parse_ass(style.secondary_colour()).unwrap_or(AssColor {
                r: 255,
                g: 0,
                b: 0,
                alpha: 0,
            }),
            outline_color: AssColor::parse_ass(style.outline_colour()).unwrap_or(AssColor {
                r: 0,
                g: 0,
                b: 0,
                alpha: 0,
            }),
            back_color: AssColor::parse_ass(style.back_colour()).unwrap_or(AssColor {
                r: 0,
                g: 0,
                b: 0,
                alpha: 0,
            }),
            bold: style.bold() != 0,
            italic: style.italic() != 0,
            underline: style.underline() != 0,
            strike_out: style.strike_out() != 0,
            scale_x: style.scale_x(),
            scale_y: style.scale_y(),
            spacing: style.spacing(),
            angle: style.angle(),
            border_style: BorderStyle::from_i32(style.border_style()),
            outline: style.outline(),
            shadow: style.shadow(),
            alignment: u8::try_from(style.alignment().clamp(1, 9)).unwrap_or(2),
            margin_l: style.margin_l(),
            margin_r: style.margin_r(),
            margin_v: style.margin_v(),
            encoding: style.encoding(),
            preview_text: "The quick brown fox jumps over the lazy dog. Tiếng Việt!".to_owned(),
        }
    }

    /// Sanitizes and clamps all fields into valid ASS ranges.
    ///
    /// # Errors
    /// Returns [`StyleEditorError::EmptyName`] if `name` is empty after stripping commas/whitespace.
    pub fn validate_and_clamp(&mut self) -> Result<(), StyleEditorError> {
        let cleaned = self.name.replace(',', "");
        cleaned.trim().clone_into(&mut self.name);
        if self.name.is_empty() {
            return Err(StyleEditorError::EmptyName);
        }
        if self.fontname.trim().is_empty() {
            "Arial".clone_into(&mut self.fontname);
        } else {
            self.fontname = self.fontname.replace(',', "");
        }
        self.fontsize = self.fontsize.clamp(1.0, 1000.0);
        self.scale_x = self.scale_x.clamp(0.0, 10_000.0);
        self.scale_y = self.scale_y.clamp(0.0, 10_000.0);
        self.spacing = self.spacing.clamp(-1000.0, 1000.0);
        self.angle = self.angle.clamp(-3600.0, 3600.0);
        self.outline = self.outline.clamp(0.0, 500.0);
        self.shadow = self.shadow.clamp(0.0, 500.0);
        self.alignment = self.alignment.clamp(1, 9);
        self.margin_l = self.margin_l.clamp(0, 9999);
        self.margin_r = self.margin_r.clamp(0, 9999);
        self.margin_v = self.margin_v.clamp(0, 9999);
        self.encoding = self.encoding.clamp(-1, 255);
        Ok(())
    }

    /// Writes draft fields onto an existing [`AssStyle`].
    pub fn write_into(&self, target: &mut AssStyle) {
        target.set_name(self.name.clone());
        target.set_fontname(self.fontname.clone());
        target.set_fontsize(self.fontsize);
        target.set_primary_colour(self.primary_color.to_ass_string());
        target.set_secondary_colour(self.secondary_color.to_ass_string());
        target.set_outline_colour(self.outline_color.to_ass_string());
        target.set_back_colour(self.back_color.to_ass_string());
        target.set_bold(if self.bold { -1 } else { 0 });
        target.set_italic(if self.italic { -1 } else { 0 });
        target.set_underline(if self.underline { -1 } else { 0 });
        target.set_strike_out(if self.strike_out { -1 } else { 0 });
        target.set_scale_x(self.scale_x);
        target.set_scale_y(self.scale_y);
        target.set_spacing(self.spacing);
        target.set_angle(self.angle);
        target.set_border_style(self.border_style as i32);
        target.set_outline(self.outline);
        target.set_shadow(self.shadow);
        target.set_alignment(i32::from(self.alignment));
        target.set_margin_l(self.margin_l);
        target.set_margin_r(self.margin_r);
        target.set_margin_v(self.margin_v);
        target.set_encoding(self.encoding);
    }
}

/// Applies a [`StyleDraft`] to `script` (replacing `original_name`), propagating any rename to
/// event `Style` fields and `\rOldName` tags via [`styles_catalog::rename_style`] as ONE atomic
/// [`EditCommand::Batch`].
///
/// # Errors
/// Returns [`StyleEditorError`] on validation failure, missing style, or duplicate target name.
pub fn apply_style_edit(
    script: &ScriptFile,
    original_name: &str,
    draft: &mut StyleDraft,
    propagate_rename: bool,
) -> Result<EditCommand, StyleEditorError> {
    draft.validate_and_clamp()?;
    let styles_sec = script
        .styles_section()
        .ok_or(StyleEditorError::MissingStylesSection)?;
    let styles: Vec<&AssStyle> = styles_sec.styles().collect();

    let style_idx = styles
        .iter()
        .position(|s| s.name() == original_name)
        .ok_or_else(|| StyleEditorError::StyleNotFound(original_name.to_owned()))?;

    let renamed = draft.name != original_name;
    if renamed && styles.iter().any(|s| s.name() == draft.name) {
        return Err(StyleEditorError::DuplicateName(draft.name.clone()));
    }

    let before = styles[style_idx].clone();
    let mut after = before.clone();
    draft.write_into(&mut after);

    if !renamed {
        return Ok(EditCommand::ModifyStyle {
            index: style_idx,
            before: Box::new(before),
            after: Box::new(after),
        });
    }

    // Build rename propagation commands first via `styles-catalog`, then swap the style edit in
    // so both field edits AND rename propagation happen in one atomic Batch.
    let rename_cmd =
        styles_catalog::rename_style(script, original_name, &draft.name, propagate_rename)
            .map_err(|e| StyleEditorError::Rename(e.to_string()))?;

    let mut cmds = Vec::new();
    cmds.push(EditCommand::ModifyStyle {
        index: style_idx,
        before: Box::new(before),
        after: Box::new(after),
    });
    if let EditCommand::Batch(rename_items) = rename_cmd {
        for item in rename_items {
            // Skip the redundant ModifyStyle (already pushed with all updated draft fields).
            if !matches!(item, EditCommand::ModifyStyle { .. }) {
                cmds.push(item);
            }
        }
    }
    Ok(EditCommand::Batch(cmds))
}

/// Duplicates `style_name` with a unique suffix (`Copy of ...`) as an [`EditCommand::InsertStyle`].
///
/// # Errors
/// Returns [`StyleEditorError`] if `[V4+ Styles]` or `style_name` is missing.
pub fn duplicate_style(
    script: &ScriptFile,
    style_name: &str,
) -> Result<EditCommand, StyleEditorError> {
    let sec = script
        .styles_section()
        .ok_or(StyleEditorError::MissingStylesSection)?;
    let styles: Vec<&AssStyle> = sec.styles().collect();
    let src = styles
        .iter()
        .find(|s| s.name() == style_name)
        .ok_or_else(|| StyleEditorError::StyleNotFound(style_name.to_owned()))?;

    let existing: HashSet<&str> = styles.iter().map(|s| s.name()).collect();
    let mut candidate = format!("Copy of {style_name}");
    let mut n = 2usize;
    while existing.contains(candidate.as_str()) {
        candidate = format!("Copy of {style_name} ({n})");
        n += 1;
    }

    let mut cloned = (*src).clone();
    cloned.set_name(candidate);
    Ok(EditCommand::InsertStyle {
        index: styles.len(),
        style: Box::new(cloned),
    })
}

/// Outcome of preparing to delete a style, including the count of events currently using it.
#[derive(Debug, Clone)]
pub struct DeleteStylePlan {
    pub in_use_event_count: usize,
    pub command: EditCommand,
}

/// Prepares an atomic [`EditCommand::DeleteStyle`] along with an in-use event count warning.
///
/// # Errors
/// Returns [`StyleEditorError`] if `[V4+ Styles]` or `style_name` is missing.
pub fn prepare_delete_style(
    script: &ScriptFile,
    style_name: &str,
) -> Result<DeleteStylePlan, StyleEditorError> {
    let sec = script
        .styles_section()
        .ok_or(StyleEditorError::MissingStylesSection)?;
    let styles: Vec<&AssStyle> = sec.styles().collect();
    let idx = styles
        .iter()
        .position(|s| s.name() == style_name)
        .ok_or_else(|| StyleEditorError::StyleNotFound(style_name.to_owned()))?;
    let in_use = script.events().filter(|e| e.style() == style_name).count();
    Ok(DeleteStylePlan {
        in_use_event_count: in_use,
        command: EditCommand::DeleteStyle {
            index: idx,
            style: Box::new(styles[idx].clone()),
        },
    })
}

/// Sorts `[V4+ Styles]` alphabetically (case-insensitive) as one atomic [`EditCommand::Batch`].
///
/// # Errors
/// Returns [`StyleEditorError::MissingStylesSection`] if `[V4+ Styles]` is absent.
pub fn sort_styles(script: &ScriptFile) -> Result<EditCommand, StyleEditorError> {
    let sec = script
        .styles_section()
        .ok_or(StyleEditorError::MissingStylesSection)?;
    let styles: Vec<&AssStyle> = sec.styles().collect();
    let mut order: Vec<usize> = (0..styles.len()).collect();
    order.sort_by(|&a, &b| {
        styles[a]
            .name()
            .to_lowercase()
            .cmp(&styles[b].name().to_lowercase())
    });
    let mut cmds = Vec::new();
    for (dst, &src) in order.iter().enumerate() {
        if dst != src {
            cmds.push(EditCommand::ModifyStyle {
                index: dst,
                before: Box::new(styles[dst].clone()),
                after: Box::new(styles[src].clone()),
            });
        }
    }
    Ok(EditCommand::Batch(cmds))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn color_roundtrip_apply_rename_duplicate_delete_and_sort() {
        let col = AssColor::parse_ass("&H80332211").expect("col");
        assert_eq!(
            col,
            AssColor {
                r: 0x11,
                g: 0x22,
                b: 0x33,
                alpha: 0x80
            }
        );
        assert_eq!(col.to_ass_string(), "&H80332211");

        let mut script = ass_core::parse_ass(
            "[V4+ Styles]\nFormat: Name, Fontname, Fontsize, PrimaryColour, SecondaryColour, OutlineColour, BackColour, Bold, Italic, Underline, StrikeOut, ScaleX, ScaleY, Spacing, Angle, BorderStyle, Outline, Shadow, Alignment, MarginL, MarginR, MarginV, Encoding\nStyle: Zebra,Arial,48,&H00FFFFFF,&H000000FF,&H00000000,&H00000000,0,0,0,0,100,100,0,0,1,2,2,2,10,10,10,-1\nStyle: Alpha,Arial,36,&H00FFFFFF,&H000000FF,&H00000000,&H00000000,0,0,0,0,100,100,0,0,3,2,2,8,10,10,10,1\n\n[Events]\nFormat: Layer, Start, End, Style, Name, MarginL, MarginR, MarginV, Effect, Text\nDialogue: 0,0:00:01.00,0:00:03.00,Zebra,,0,0,0,,{\\rZebra}Hi\n",
        )
        .value;

        let z = script.styles().next().expect("z");
        assert_eq!(z.encoding(), -1);
        let mut draft = StyleDraft::from_style(z);
        draft.name = "SignMain".to_owned();
        draft.border_style = BorderStyle::ShadowBox;
        draft.fontsize = 54.0;

        let cmd = apply_style_edit(&script, "Zebra", &mut draft, true).expect("apply");
        cmd.apply(&mut script).expect("exec");
        assert_eq!(script.events().next().expect("ev").style(), "SignMain");
        assert_eq!(
            script.events().next().expect("ev").text(),
            r"{\rSignMain}Hi"
        );

        let dup = duplicate_style(&script, "SignMain").expect("dup");
        dup.apply(&mut script).expect("apply dup");
        assert_eq!(script.styles().count(), 3);

        let del_plan = prepare_delete_style(&script, "SignMain").expect("del");
        assert_eq!(del_plan.in_use_event_count, 1);

        let sort_cmd = sort_styles(&script).expect("sort");
        sort_cmd.apply(&mut script).expect("apply sort");
        let names: Vec<&str> = script.styles().map(AssStyle::name).collect();
        assert_eq!(names, vec!["Alpha", "Copy of SignMain", "SignMain"]);
    }
}
