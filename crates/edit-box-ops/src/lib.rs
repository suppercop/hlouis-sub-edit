//! Aegisub 3.5.1 edit-box operations: override tag insertion/toggling, effective style state
//! resolution, `\h`/`\N` & Shift+Enter handling, autocomplete, time/frame field parsing, and CPS.
//!
//! Mirrors Aegisub:
//! - `src/subs_edit_box.cpp` (`SubsEditBox::SetSelectedRows`, `ToggleTag`, `SetOverrideTag`)
//! - `src/subs_edit_ctrl.cpp` (`SubsTextEditCtrl`)
//! - `src/timeedit_ctrl.cpp` (`TimeEdit`, `TimeType::START` / `TimeType::END`)

#![forbid(unsafe_code)]
#![cfg_attr(not(test), deny(clippy::unwrap_used, clippy::expect_used))]

use ass_core::{AssEvent, AssStyle, AssTime, ScriptFile};
use edit_ops::{event_metrics, CpsOptions, LineMetrics};
use keyframes::{TimeType, Timecodes};
use std::collections::HashMap;
use std::fmt::Write as _;
use thiserror::Error;

/// Errors returned by `edit-box-ops`.
#[derive(Debug, Error, PartialEq, Eq)]
pub enum EditBoxError {
    /// Invalid colour channel (expected 1..=4).
    #[error("invalid colour channel {0} (expected 1..=4)")]
    InvalidColorChannel(u8),
    /// Invalid alpha channel (expected 0 for `\\alpha` or 1..=4 for `\\1a`..`\\4a`).
    #[error("invalid alpha channel {0} (expected 0..=4)")]
    InvalidAlphaChannel(u8),
    /// Invalid `\an` alignment (expected 1..=9).
    #[error("invalid alignment {0} (expected 1..=9)")]
    InvalidAlignment(u8),
    /// Invalid time or frame string.
    #[error("invalid time/frame input: {0}")]
    InvalidTimeOrFrame(String),
}

/// Byte selection range in the edit box (`start <= end`, both UTF-8 char boundaries).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct TextSelection {
    pub start: usize,
    pub end: usize,
}

impl TextSelection {
    /// Collapsed caret at `pos`.
    #[must_use]
    pub const fn caret(pos: usize) -> Self {
        Self {
            start: pos,
            end: pos,
        }
    }

    /// Selection spanning `start..end` (clamped and normalized).
    #[must_use]
    pub fn clamped(start: usize, end: usize, text: &str) -> Self {
        let len = text.len();
        let a = snap_char_boundary(text, start.min(len));
        let b = snap_char_boundary(text, end.min(len));
        if a <= b {
            Self { start: a, end: b }
        } else {
            Self { start: b, end: a }
        }
    }

    /// Returns `true` when selection is collapsed (caret only).
    #[must_use]
    pub const fn is_empty(self) -> bool {
        self.start == self.end
    }
}

/// Result of an edit-box text transformation.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct TextEditResult {
    pub text: String,
    pub selection: TextSelection,
}

/// Effective formatting state at a cursor position, resolved from the line's base [`AssStyle`]
/// plus all preceding override blocks.
#[derive(Debug, Clone, PartialEq)]
#[allow(clippy::struct_excessive_bools)]
pub struct EffectiveStyleState {
    pub bold: bool,
    pub italic: bool,
    pub underline: bool,
    pub strikeout: bool,
    pub font_name: String,
    pub font_size: f64,
    pub alignment: u8,
    pub primary_color: String,
    pub secondary_color: String,
    pub outline_color: String,
    pub back_color: String,
}

impl EffectiveStyleState {
    /// Initializes state from an [`AssStyle`] (or Aegisub defaults if `None`).
    #[must_use]
    pub fn from_style(style: Option<&AssStyle>) -> Self {
        if let Some(st) = style {
            Self {
                bold: st.bold() != 0,
                italic: st.italic() != 0,
                underline: st.underline() != 0,
                strikeout: st.strike_out() != 0,
                font_name: st.fontname().to_owned(),
                font_size: st.fontsize(),
                alignment: u8::try_from(st.alignment().clamp(1, 9)).unwrap_or(2),
                primary_color: st.primary_colour().to_owned(),
                secondary_color: st.secondary_colour().to_owned(),
                outline_color: st.outline_colour().to_owned(),
                back_color: st.back_colour().to_owned(),
            }
        } else {
            Self {
                bold: false,
                italic: false,
                underline: false,
                strikeout: false,
                font_name: "Arial".to_owned(),
                font_size: 48.0,
                alignment: 2,
                primary_color: "&H00FFFFFF".to_owned(),
                secondary_color: "&H000000FF".to_owned(),
                outline_color: "&H00000000".to_owned(),
                back_color: "&H00000000".to_owned(),
            }
        }
    }
}

/// Computes the effective style state at `byte_pos`, resolving both bare `\r` (back to
/// `active_style_name`) and named `\rStyleName` against the script's full `styles` table
/// (mirrors Aegisub `SubsEditBox` / `AssDialogueBlockOverride`).
#[must_use]
pub fn effective_state_at(
    text: &str,
    active_style_name: &str,
    styles: &[AssStyle],
    byte_pos: usize,
) -> EffectiveStyleState {
    let base_style = styles.iter().find(|s| s.name() == active_style_name);
    let mut state = EffectiveStyleState::from_style(base_style);
    let limit = byte_pos.min(text.len());
    let bytes = text.as_bytes();
    let mut pos = 0usize;

    while pos < limit {
        let Some(rel_open) = bytes[pos..limit].iter().position(|&b| b == b'{') else {
            break;
        };
        let block_start = pos + rel_open + 1;
        let Some(rel_close) = bytes[block_start..].iter().position(|&b| b == b'}') else {
            break;
        };
        let block_end = block_start + rel_close;
        let scan_end = limit.min(block_end);
        apply_override_block_to_state(&text[block_start..scan_end], base_style, styles, &mut state);
        pos = block_end + 1;
    }

    state
}

fn apply_override_block_to_state(
    block: &str,
    base_style: Option<&AssStyle>,
    styles: &[AssStyle],
    state: &mut EffectiveStyleState,
) {
    let bytes = block.as_bytes();
    let len = bytes.len();
    let mut i = 0usize;
    while i < len {
        if bytes[i] != b'\\' {
            i += 1;
            continue;
        }
        let tag_start = i + 1;
        let mut j = tag_start;
        let mut parens = 0usize;
        while j < len {
            let c = bytes[j];
            if c == b'(' {
                parens += 1;
            } else if c == b')' && parens > 0 {
                parens -= 1;
            } else if parens == 0 && c == b'\\' {
                break;
            }
            j += 1;
        }
        let tag = &block[tag_start..j];
        if let Some(rest) = tag.strip_prefix("an") {
            if let Ok(v) = rest.trim().parse::<u8>() {
                if (1..=9).contains(&v) {
                    state.alignment = v;
                }
            }
        } else if let Some(rest) = tag.strip_prefix("fn") {
            if rest.is_empty() {
                if let Some(st) = base_style {
                    st.fontname().clone_into(&mut state.font_name);
                }
            } else {
                rest.clone_into(&mut state.font_name);
            }
        } else if let Some(rest) = tag.strip_prefix("fs") {
            if !rest.starts_with('c') && !rest.starts_with('p') {
                if let Ok(v) = rest.trim().parse::<f64>() {
                    state.font_size = v;
                }
            }
        } else if let Some(rest) = tag.strip_prefix("1c") {
            rest.clone_into(&mut state.primary_color);
        } else if let Some(rest) = tag.strip_prefix("2c") {
            rest.clone_into(&mut state.secondary_color);
        } else if let Some(rest) = tag.strip_prefix("3c") {
            rest.clone_into(&mut state.outline_color);
        } else if let Some(rest) = tag.strip_prefix("4c") {
            rest.clone_into(&mut state.back_color);
        } else if let Some(rest) = tag.strip_prefix('c') {
            if rest.starts_with('&') {
                rest.clone_into(&mut state.primary_color);
            }
        } else if let Some(rest) = tag.strip_prefix('b') {
            if rest.is_empty() || rest.as_bytes()[0].is_ascii_digit() {
                if let Ok(v) = rest.trim().parse::<i32>() {
                    state.bold = v != 0;
                }
            }
        } else if let Some(rest) = tag.strip_prefix('i') {
            if rest.is_empty() || rest.as_bytes()[0].is_ascii_digit() {
                if let Ok(v) = rest.trim().parse::<i32>() {
                    state.italic = v != 0;
                }
            }
        } else if let Some(rest) = tag.strip_prefix('u') {
            if rest.is_empty() || rest.as_bytes()[0].is_ascii_digit() {
                if let Ok(v) = rest.trim().parse::<i32>() {
                    state.underline = v != 0;
                }
            }
        } else if let Some(rest) = tag.strip_prefix('s') {
            if rest.is_empty() || rest.as_bytes()[0].is_ascii_digit() {
                if let Ok(v) = rest.trim().parse::<i32>() {
                    state.strikeout = v != 0;
                }
            }
        } else if let Some(rest) = tag.strip_prefix('r') {
            if rest.is_empty() {
                *state = EffectiveStyleState::from_style(base_style);
            } else {
                let target = styles.iter().find(|s| s.name() == rest).or(base_style);
                *state = EffectiveStyleState::from_style(target);
            }
        }
        i = j;
    }
}

/// Inserts or merges `\tag_name<value>` at `pos` following Aegisub `SubsEditBox::SetOverrideTag`:
/// - If `pos` lies inside an existing `{...}` block (or immediately after `}`), replaces any
///   existing `tag_name` in that block or appends right before `}`.
/// - Otherwise inserts a fresh `{\tag_name<value>}` at `pos`.
#[must_use]
pub fn set_tag(
    text: &str,
    selection: TextSelection,
    tag_name: &str,
    value: &str,
) -> TextEditResult {
    let sel = TextSelection::clamped(selection.start, selection.end, text);
    let clean_name = tag_name.strip_prefix('\\').unwrap_or(tag_name);
    let (new_text, delta_at_start) = insert_or_merge_tag_at(text, sel.start, clean_name, value);
    let new_start = sel.start.saturating_add_signed(delta_at_start);
    let new_end = sel.end.saturating_add_signed(delta_at_start);
    TextEditResult {
        text: new_text,
        selection: TextSelection {
            start: new_start,
            end: new_end,
        },
    }
}

/// Toggles bold (`\b1`/`\b0`) at cursor or around selection (restoring prior state after
/// selection, matching Aegisub `SubsEditBox::ToggleTag`).
#[must_use]
pub fn toggle_bold(
    text: &str,
    selection: TextSelection,
    active_style_name: &str,
    styles: &[AssStyle],
) -> TextEditResult {
    let sel = TextSelection::clamped(selection.start, selection.end, text);
    toggle_binary_tag(text, sel, "b", active_style_name, styles, |s| s.bold)
}

/// Toggles italic (`\i1`/`\i0`) at cursor or around selection.
#[must_use]
pub fn toggle_italic(
    text: &str,
    selection: TextSelection,
    active_style_name: &str,
    styles: &[AssStyle],
) -> TextEditResult {
    let sel = TextSelection::clamped(selection.start, selection.end, text);
    toggle_binary_tag(text, sel, "i", active_style_name, styles, |s| s.italic)
}

/// Toggles underline (`\u1`/`\u0`) at cursor or around selection.
#[must_use]
pub fn toggle_underline(
    text: &str,
    selection: TextSelection,
    active_style_name: &str,
    styles: &[AssStyle],
) -> TextEditResult {
    let sel = TextSelection::clamped(selection.start, selection.end, text);
    toggle_binary_tag(text, sel, "u", active_style_name, styles, |s| s.underline)
}

/// Toggles strikeout (`\s1`/`\s0`) at cursor or around selection.
#[must_use]
pub fn toggle_strikeout(
    text: &str,
    selection: TextSelection,
    active_style_name: &str,
    styles: &[AssStyle],
) -> TextEditResult {
    let sel = TextSelection::clamped(selection.start, selection.end, text);
    toggle_binary_tag(text, sel, "s", active_style_name, styles, |s| s.strikeout)
}

/// Sets font name (`\fn`) at cursor or around selection.
#[must_use]
pub fn set_font_name(text: &str, selection: TextSelection, font_name: &str) -> TextEditResult {
    set_tag(text, selection, "fn", font_name)
}

/// Sets font size (`\fs`) at cursor or around selection.
#[must_use]
pub fn set_font_size(text: &str, selection: TextSelection, size: f64) -> TextEditResult {
    let mut val = String::with_capacity(16);
    if (size - size.round()).abs() < 1e-6 {
        #[allow(clippy::cast_possible_truncation)]
        let _ = write!(val, "{}", size.round() as i64);
    } else {
        let _ = write!(val, "{size:.2}");
    }
    set_tag(text, selection, "fs", &val)
}

/// Sets colour channel 1..=4 (`\1c`, `\2c`, `\3c`, `\4c`) formatted as `&HBBGGRR&`.
///
/// # Errors
/// Returns [`EditBoxError::InvalidColorChannel`] if `channel` is not in `1..=4`.
pub fn set_color(
    text: &str,
    selection: TextSelection,
    channel: u8,
    r: u8,
    g: u8,
    b: u8,
) -> Result<TextEditResult, EditBoxError> {
    let tag = match channel {
        1 => "1c",
        2 => "2c",
        3 => "3c",
        4 => "4c",
        _ => return Err(EditBoxError::InvalidColorChannel(channel)),
    };
    let mut val = String::with_capacity(10);
    let _ = write!(val, "&H{b:02X}{g:02X}{r:02X}&");
    Ok(set_tag(text, selection, tag, &val))
}

/// Sets alpha (`0` -> `\alpha`, `1..=4` -> `\1a`..`\4a`) formatted as `&HAA&`.
///
/// # Errors
/// Returns [`EditBoxError::InvalidAlphaChannel`] if `channel > 4`.
pub fn set_alpha(
    text: &str,
    selection: TextSelection,
    channel: u8,
    alpha: u8,
) -> Result<TextEditResult, EditBoxError> {
    let tag = match channel {
        0 => "alpha",
        1 => "1a",
        2 => "2a",
        3 => "3a",
        4 => "4a",
        _ => return Err(EditBoxError::InvalidAlphaChannel(channel)),
    };
    let mut val = String::with_capacity(6);
    let _ = write!(val, "&H{alpha:02X}&");
    Ok(set_tag(text, selection, tag, &val))
}

/// Sets line alignment `\an1..=9` in the leading override block (matching libass/Aegisub, where
/// `\an` is line-global and always placed in the first block at byte 0).
///
/// # Errors
/// Returns [`EditBoxError::InvalidAlignment`] if `alignment` is not in `1..=9`.
pub fn set_alignment(
    text: &str,
    selection: TextSelection,
    alignment: u8,
) -> Result<TextEditResult, EditBoxError> {
    if !(1..=9).contains(&alignment) {
        return Err(EditBoxError::InvalidAlignment(alignment));
    }
    let sel = TextSelection::clamped(selection.start, selection.end, text);
    let mut val = String::with_capacity(2);
    let _ = write!(val, "{alignment}");
    let insert_pos = usize::from(text.starts_with('{'));
    let (new_text, delta) = insert_or_merge_tag_at(text, insert_pos, "an", &val);
    let new_start = if sel.start >= insert_pos {
        sel.start.saturating_add_signed(delta)
    } else {
        sel.start
    };
    let new_end = if sel.end >= insert_pos {
        sel.end.saturating_add_signed(delta)
    } else {
        sel.end
    };
    Ok(TextEditResult {
        text: new_text,
        selection: TextSelection {
            start: new_start,
            end: new_end,
        },
    })
}

/// Inserts `\N` (hard line break) replacing the current selection.
#[must_use]
pub fn insert_hard_newline(text: &str, selection: TextSelection) -> TextEditResult {
    replace_selection(text, selection, r"\N")
}

/// Inserts `\h` (hard space) replacing the current selection.
#[must_use]
pub fn insert_hard_space(text: &str, selection: TextSelection) -> TextEditResult {
    replace_selection(text, selection, r"\h")
}

/// Configured behaviour for `Shift+Enter` in the edit box (matches Aegisub
/// `Subtitle/Edit Box/Soft Line Break` preference).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ShiftEnterMode {
    InsertHardNewline,
    InsertSoftNewline,
    CommitAndNextLine,
}

/// Outcome of handling `Shift+Enter`.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ShiftEnterAction {
    TextEdited(TextEditResult),
    CommitAndAdvance,
}

/// Handles `Shift+Enter` according to `mode`.
#[must_use]
pub fn handle_shift_enter(
    text: &str,
    selection: TextSelection,
    mode: ShiftEnterMode,
) -> ShiftEnterAction {
    match mode {
        ShiftEnterMode::InsertHardNewline => {
            ShiftEnterAction::TextEdited(replace_selection(text, selection, r"\N"))
        }
        ShiftEnterMode::InsertSoftNewline => {
            ShiftEnterAction::TextEdited(replace_selection(text, selection, r"\n"))
        }
        ShiftEnterMode::CommitAndNextLine => ShiftEnterAction::CommitAndAdvance,
    }
}

/// Ranked, case-insensitive autocomplete candidates for style names in `script`.
#[must_use]
pub fn autocomplete_styles(script: &ScriptFile, query: &str, limit: usize) -> Vec<String> {
    let mut counts: HashMap<&str, usize> = HashMap::new();
    for st in script.styles() {
        counts.entry(st.name()).or_insert(1);
    }
    for ev in script.events() {
        if !ev.style().is_empty() {
            *counts.entry(ev.style()).or_insert(0) += 1;
        }
    }
    rank_candidates(counts, query, limit)
}

/// Ranked, case-insensitive autocomplete candidates for actor (`Name`) values in `script`.
#[must_use]
pub fn autocomplete_actors(script: &ScriptFile, query: &str, limit: usize) -> Vec<String> {
    let mut counts: HashMap<&str, usize> = HashMap::new();
    for ev in script.events() {
        if !ev.name().is_empty() {
            *counts.entry(ev.name()).or_insert(0) += 1;
        }
    }
    rank_candidates(counts, query, limit)
}

/// Ranked, case-insensitive autocomplete candidates for `Effect` values in `script`.
#[must_use]
pub fn autocomplete_effects(script: &ScriptFile, query: &str, limit: usize) -> Vec<String> {
    let mut counts: HashMap<&str, usize> = HashMap::new();
    for ev in script.events() {
        if !ev.effect().is_empty() {
            *counts.entry(ev.effect()).or_insert(0) += 1;
        }
    }
    rank_candidates(counts, query, limit)
}

/// Default fallback CFR timecodes (23.976 fps = `24000/1001`) used in frame mode when no video
/// is loaded, matching Aegisub's default dummy-video framerate.
#[must_use]
pub fn default_fallback_timecodes() -> Timecodes {
    Timecodes::from_cfr(24_000, 1_001)
        .unwrap_or_else(|_| Timecodes::from_vfr_ms(vec![0, 42]).unwrap_or_else(|_| unreachable!()))
}

/// Parses an edit-box time or frame field (mirrors Aegisub `src/timeedit_ctrl.cpp`):
/// - When `frame_mode` is `true`, parses an unsigned frame number and converts via `timecodes`
///   (or [`default_fallback_timecodes`] when no video is loaded) using `time_type`.
/// - Otherwise parses `H:MM:SS.cc` via [`AssTime::parse_ass`].
///
/// # Errors
/// Returns [`EditBoxError::InvalidTimeOrFrame`] if the input cannot be parsed.
pub fn parse_time_or_frame(
    input: &str,
    frame_mode: bool,
    time_type: TimeType,
    timecodes: Option<&Timecodes>,
) -> Result<AssTime, EditBoxError> {
    let trimmed = input.trim();
    if frame_mode {
        let fallback;
        let tc = if let Some(tc) = timecodes {
            tc
        } else {
            fallback = default_fallback_timecodes();
            &fallback
        };
        let frame: u32 = trimmed
            .parse()
            .map_err(|_| EditBoxError::InvalidTimeOrFrame(trimmed.to_owned()))?;
        let cs = tc.frame_to_time_cs(frame, time_type);
        Ok(AssTime::from_cs(cs))
    } else {
        AssTime::parse_ass(trimmed)
            .ok_or_else(|| EditBoxError::InvalidTimeOrFrame(trimmed.to_owned()))
    }
}

/// Formats an [`AssTime`] for the edit-box time or frame field (uses [`default_fallback_timecodes`]
/// in frame mode when no video is loaded).
#[must_use]
pub fn format_time_or_frame(
    time: AssTime,
    frame_mode: bool,
    time_type: TimeType,
    timecodes: Option<&Timecodes>,
) -> String {
    if frame_mode {
        let fallback;
        let tc = if let Some(tc) = timecodes {
            tc
        } else {
            fallback = default_fallback_timecodes();
            &fallback
        };
        return tc.time_cs_to_frame(time.as_cs(), time_type).to_string();
    }
    time.to_ass_string()
}

/// Computes live character-count / max-line-length / CPS metrics for the active line.
#[must_use]
pub fn active_line_metrics(event: &AssEvent, options: CpsOptions) -> LineMetrics {
    event_metrics(event, options)
}

fn toggle_binary_tag(
    text: &str,
    sel: TextSelection,
    tag: &str,
    active_style_name: &str,
    styles: &[AssStyle],
    flag_of: fn(&EffectiveStyleState) -> bool,
) -> TextEditResult {
    let eff_pos = find_enclosing_or_adjacent_block(text.as_bytes(), sel.start)
        .map_or(sel.start, |(_, close_idx)| close_idx + 1);
    let currently_on = flag_of(&effective_state_at(
        text,
        active_style_name,
        styles,
        eff_pos,
    ));
    let target_on = !currently_on;

    let (after_start, delta_start) = set_binary_tag_smart(
        text,
        sel.start,
        false,
        tag,
        target_on,
        active_style_name,
        styles,
        flag_of,
    );
    let new_start = sel.start.saturating_add_signed(delta_start);
    let new_end = sel.end.saturating_add_signed(delta_start);

    if sel.is_empty() {
        return TextEditResult {
            text: after_start,
            selection: TextSelection::caret(new_start),
        };
    }

    // Restore the state that was active right after the selection prior to toggling.
    let mut after_sel_pos = sel.end;
    if after_sel_pos < text.len() && text.as_bytes()[after_sel_pos] == b'{' {
        if let Some(rel_close) = text.as_bytes()[after_sel_pos + 1..]
            .iter()
            .position(|&b| b == b'}')
        {
            after_sel_pos += rel_close + 2;
        }
    }
    let restore_on = flag_of(&effective_state_at(
        text,
        active_style_name,
        styles,
        after_sel_pos,
    ));
    let (final_text, _delta_end) = set_binary_tag_smart(
        &after_start,
        new_end,
        true,
        tag,
        restore_on,
        active_style_name,
        styles,
        flag_of,
    );
    TextEditResult {
        text: final_text,
        selection: TextSelection {
            start: new_start,
            end: new_end,
        },
    }
}

/// Applies `tag` with boolean state `target_on` at `pos`, removing the tag (and the enclosing
/// `{}` block if it becomes empty) whenever removing it already produces `target_on` at `pos`.
#[allow(clippy::too_many_arguments)]
fn set_binary_tag_smart(
    text: &str,
    pos: usize,
    check_following_block: bool,
    tag: &str,
    target_on: bool,
    active_style_name: &str,
    styles: &[AssStyle],
    flag_of: fn(&EffectiveStyleState) -> bool,
) -> (String, isize) {
    let bytes = text.as_bytes();
    let pos = pos.min(bytes.len());
    let mut block_bounds = find_enclosing_or_adjacent_block(bytes, pos);
    if block_bounds.is_none() && check_following_block && pos < bytes.len() && bytes[pos] == b'{' {
        if let Some(rel_close) = bytes[pos + 1..].iter().position(|&b| b == b'}') {
            block_bounds = Some((pos, pos + 1 + rel_close));
        }
    }

    if let Some((open_idx, close_idx)) = block_bounds {
        let (stripped, strip_end) = remove_all_tag_in_block(text, open_idx, close_idx, tag);
        let state_without = flag_of(&effective_state_at(
            &stripped,
            active_style_name,
            styles,
            strip_end,
        ));
        if state_without == target_on {
            let delta = stripped.len().cast_signed() - text.len().cast_signed();
            return (stripped, delta);
        }
        let val = if target_on { "1" } else { "0" };
        let (merged, _) = insert_or_merge_tag_at(&stripped, strip_end, tag, val);
        let delta = merged.len().cast_signed() - text.len().cast_signed();
        return (merged, delta);
    }

    if flag_of(&effective_state_at(text, active_style_name, styles, pos)) == target_on {
        return (text.to_owned(), 0);
    }
    let val = if target_on { "1" } else { "0" };
    insert_or_merge_tag_at(text, pos, tag, val)
}

/// Removes every occurrence of `clean_name` from the `{...}` block at `open_idx..=close_idx`,
/// deleting the `{}` braces too if nothing remains inside. Returns `(new_text, block_end_in_new)`.
fn remove_all_tag_in_block(
    text: &str,
    open_idx: usize,
    close_idx: usize,
    clean_name: &str,
) -> (String, usize) {
    let mut body = text[open_idx + 1..close_idx].to_owned();
    while let Some((rel_start, rel_end)) = find_tag_in_block(&body, clean_name) {
        body.replace_range(rel_start..rel_end, "");
    }
    if body.is_empty() {
        let mut out = String::with_capacity(text.len().saturating_sub(close_idx + 1 - open_idx));
        out.push_str(&text[..open_idx]);
        out.push_str(&text[close_idx + 1..]);
        (out, open_idx)
    } else {
        let mut out = String::with_capacity(text.len());
        out.push_str(&text[..=open_idx]);
        out.push_str(&body);
        out.push_str(&text[close_idx..]);
        let new_after = open_idx + 1 + body.len() + 1;
        (out, new_after)
    }
}

fn replace_selection(text: &str, selection: TextSelection, insert: &str) -> TextEditResult {
    let sel = TextSelection::clamped(selection.start, selection.end, text);
    let mut out = String::with_capacity(text.len() - (sel.end - sel.start) + insert.len());
    out.push_str(&text[..sel.start]);
    out.push_str(insert);
    out.push_str(&text[sel.end..]);
    let caret = sel.start + insert.len();
    TextEditResult {
        text: out,
        selection: TextSelection::caret(caret),
    }
}

/// Core Aegisub tag insertion/merging helper. Returns `(new_text, byte_delta_at_pos)`.
fn insert_or_merge_tag_at(
    text: &str,
    pos: usize,
    clean_name: &str,
    value: &str,
) -> (String, isize) {
    let bytes = text.as_bytes();
    let len = bytes.len();
    let pos = pos.min(len);

    // Check if `pos` is inside an existing `{...}` block, or immediately after `}`.
    let block_bounds = find_enclosing_or_adjacent_block(bytes, pos);

    if let Some((open_idx, close_idx)) = block_bounds {
        // Strip any existing occurrences of `clean_name` in this block so we never write duplicate
        // or contradicting tags, replacing at the first occurrence or appending before `}`.
        let mut body = text[open_idx + 1..close_idx].to_owned();
        let mut insert_rel = None;
        while let Some((rel_start, rel_end)) = find_tag_in_block(&body, clean_name) {
            if insert_rel.is_none() {
                insert_rel = Some(rel_start);
            }
            body.replace_range(rel_start..rel_end, "");
        }
        let mut tag_str = String::with_capacity(1 + clean_name.len() + value.len());
        tag_str.push('\\');
        tag_str.push_str(clean_name);
        tag_str.push_str(value);
        let at = insert_rel.unwrap_or(body.len());
        body.insert_str(at, &tag_str);

        let mut out = String::with_capacity(text.len() + tag_str.len());
        out.push_str(&text[..=open_idx]);
        out.push_str(&body);
        out.push_str(&text[close_idx..]);
        let delta = out.len().cast_signed() - text.len().cast_signed();
        return (out, delta);
    }

    // Insert a brand-new `{\clean_name<value>}` at `pos`.
    let mut out = String::with_capacity(text.len() + 3 + clean_name.len() + value.len());
    out.push_str(&text[..pos]);
    out.push('{');
    out.push('\\');
    out.push_str(clean_name);
    out.push_str(value);
    out.push('}');
    out.push_str(&text[pos..]);
    let delta = (3 + clean_name.len() + value.len()).cast_signed();
    (out, delta)
}

fn find_enclosing_or_adjacent_block(bytes: &[u8], pos: usize) -> Option<(usize, usize)> {
    // Case 1: immediately after `}` (e.g. `{\an8}|Hello`)
    if pos > 0 && bytes[pos - 1] == b'}' {
        if let Some(open) = bytes[..pos - 1].iter().rposition(|&b| b == b'{') {
            return Some((open, pos - 1));
        }
    }
    // Case 2: strictly inside `{...}`
    let prev_open = bytes[..pos].iter().rposition(|&b| b == b'{')?;
    let prev_close = bytes[..pos].iter().rposition(|&b| b == b'}');
    if prev_close.is_none_or(|c| c < prev_open) {
        let next_close = bytes[pos..].iter().position(|&b| b == b'}')?;
        return Some((prev_open, pos + next_close));
    }
    None
}

fn find_tag_in_block(block: &str, clean_name: &str) -> Option<(usize, usize)> {
    let bytes = block.as_bytes();
    let target = clean_name.as_bytes();
    let len = bytes.len();
    let mut i = 0usize;
    while i < len {
        if bytes[i] != b'\\' {
            i += 1;
            continue;
        }
        let start = i;
        let mut j = i + 1;
        let mut parens = 0usize;
        while j < len {
            let c = bytes[j];
            if c == b'(' {
                parens += 1;
            } else if c == b')' && parens > 0 {
                parens -= 1;
            } else if parens == 0 && c == b'\\' {
                break;
            }
            j += 1;
        }
        let raw = &bytes[start + 1..j];
        if raw.starts_with(target) {
            let rest = &raw[target.len()..];
            let takes_text = target == b"fn" || target == b"r";
            if takes_text || rest.is_empty() || !rest[0].is_ascii_alphabetic() {
                return Some((start, j));
            }
        }
        i = j;
    }
    None
}

fn rank_candidates(counts: HashMap<&str, usize>, query: &str, limit: usize) -> Vec<String> {
    let q_lower = query.to_lowercase();
    let mut scored: Vec<(u8, usize, &str)> = Vec::new();
    for (name, freq) in counts {
        if q_lower.is_empty() {
            scored.push((0, freq, name));
            continue;
        }
        let n_lower = name.to_lowercase();
        if n_lower == q_lower {
            scored.push((0, freq, name));
        } else if n_lower.starts_with(&q_lower) {
            scored.push((1, freq, name));
        } else if n_lower.contains(&q_lower) {
            scored.push((2, freq, name));
        }
    }
    scored.sort_by(|a, b| {
        a.0.cmp(&b.0)
            .then_with(|| b.1.cmp(&a.1))
            .then_with(|| a.2.cmp(b.2))
    });
    scored
        .into_iter()
        .take(limit)
        .map(|(_, _, s)| s.to_owned())
        .collect()
}

fn snap_char_boundary(s: &str, mut idx: usize) -> usize {
    while idx > 0 && !s.is_char_boundary(idx) {
        idx -= 1;
    }
    idx
}

#[cfg(test)]
mod tests {
    use super::*;

    fn bold_style() -> AssStyle {
        let raw = "[V4+ Styles]\nFormat: Name, Fontname, Fontsize, PrimaryColour, SecondaryColour, OutlineColour, BackColour, Bold, Italic, Underline, StrikeOut, ScaleX, ScaleY, Spacing, Angle, BorderStyle, Outline, Shadow, Alignment, MarginL, MarginR, MarginV, Encoding\nStyle: BoldDef,Arial,48,&H00FFFFFF,&H000000FF,&H00000000,&H00000000,-1,0,0,0,100,100,0,0,1,2,2,2,10,10,10,1\n";
        ass_core::parse_ass(raw)
            .value
            .styles()
            .next()
            .expect("style")
            .clone()
    }

    #[test]
    fn toggle_bold_respects_style_and_merges_existing_override_block() {
        let st = bold_style();
        // Style is already bold -> toggling bold writes \b0!
        let r1 = toggle_bold(
            r"{\an8}Hello",
            TextSelection::caret(6),
            "BoldDef",
            std::slice::from_ref(&st),
        );
        assert_eq!(r1.text, r"{\an8\b0}Hello");

        // Selection wrap toggles bold off over "Hello" and restores \b1 after
        let r2 = toggle_bold(
            "Hello world",
            TextSelection { start: 0, end: 5 },
            "BoldDef",
            std::slice::from_ref(&st),
        );
        assert_eq!(r2.text, r"{\b0}Hello{\b1} world");

        // Color & alignment
        let r3 = set_color(&r2.text, TextSelection::caret(0), 1, 0x11, 0x22, 0x33).expect("c");
        assert!(r3.text.starts_with(r"{\1c&H332211&}"));
        let r4 = set_alignment(&r3.text, TextSelection::caret(0), 8).expect("an");
        assert!(r4.text.contains(r"\an8"));
    }

    #[test]
    fn resolves_named_r_style_reset_against_full_style_table() {
        let raw = "[V4+ Styles]\nFormat: Name, Fontname, Fontsize, PrimaryColour, SecondaryColour, OutlineColour, BackColour, Bold, Italic, Underline, StrikeOut, ScaleX, ScaleY, Spacing, Angle, BorderStyle, Outline, Shadow, Alignment, MarginL, MarginR, MarginV, Encoding\nStyle: Plain,Arial,48,&H00FFFFFF,&H000000FF,&H00000000,&H00000000,0,0,0,0,100,100,0,0,1,2,2,2,10,10,10,1\nStyle: Strong,Arial,48,&H00FFFFFF,&H000000FF,&H00000000,&H00000000,-1,0,0,0,100,100,0,0,1,2,2,2,10,10,10,1\n";
        let styles: Vec<AssStyle> = ass_core::parse_ass(raw).value.styles().cloned().collect();
        let line = r"Hello {\rStrong}BoldWord{\r}PlainAgain";
        let pos_in_strong = line.find("BoldWord").expect("pos");
        assert!(effective_state_at(line, "Plain", &styles, pos_in_strong).bold);
        let toggled = toggle_bold(line, TextSelection::caret(pos_in_strong), "Plain", &styles);
        assert!(toggled.text.contains(r"{\rStrong\b0}BoldWord"));
        let pos_after_reset = line.find("PlainAgain").expect("pos2");
        assert!(!effective_state_at(line, "Plain", &styles, pos_after_reset).bold);
    }

    #[test]
    fn escapes_autocomplete_and_time_frame_parsing() {
        let r = insert_hard_newline("Hello world", TextSelection { start: 5, end: 6 });
        assert_eq!(r.text, r"Hello\Nworld");
        let r2 = insert_hard_space(&r.text, r.selection);
        assert_eq!(r2.text, r"Hello\N\hworld");

        let tc = Timecodes::from_cfr(24_000, 1001).expect("cfr");
        let t = parse_time_or_frame("24", true, TimeType::Start, Some(&tc)).expect("frame 24");
        assert_eq!(t.as_cs(), 98);
        assert_eq!(
            format_time_or_frame(t, true, TimeType::Start, Some(&tc)),
            "24"
        );

        let script = ass_core::parse_ass(
            "[Events]\nFormat: Layer, Start, End, Style, Name, MarginL, MarginR, MarginV, Effect, Text\nDialogue: 0,0:00:01.00,0:00:02.00,Default,Sakura,0,0,0,karaoke,Hi\nDialogue: 0,0:00:02.00,0:00:03.00,Default,Satoshi,0,0,0,,Yo\n",
        )
        .value;
        let actors = autocomplete_actors(&script, "sa", 10);
        assert_eq!(actors, vec!["Sakura", "Satoshi"]);

        // Frame mode without video uses default 24000/1001 fallback timecodes.
        let t_fb = parse_time_or_frame("24", true, TimeType::Start, None).expect("fallback frame");
        assert_eq!(t_fb.as_cs(), 98);
        assert_eq!(
            format_time_or_frame(t_fb, true, TimeType::Start, None),
            "24"
        );
    }

    #[test]
    fn toggle_tags_undo_on_second_press_and_never_duplicate() {
        // B twice at caret -> text unchanged.
        let r1 = toggle_bold("Hello", TextSelection::caret(0), "Default", &[]);
        assert_eq!(r1.text, r"{\b1}Hello");
        let r2 = toggle_bold(&r1.text, r1.selection, "Default", &[]);
        assert_eq!(r2.text, "Hello");

        // B on a selection twice -> text unchanged.
        let s1 = toggle_bold(
            "Hello world",
            TextSelection { start: 0, end: 5 },
            "Default",
            &[],
        );
        assert_eq!(s1.text, r"{\b1}Hello{\b0} world");
        let s2 = toggle_bold(&s1.text, s1.selection, "Default", &[]);
        assert_eq!(s2.text, "Hello world");

        // B inside {\an8} -> {\an8\b1}, then B again -> {\an8}.
        let a1 = toggle_bold(r"{\an8}Hello", TextSelection::caret(3), "Default", &[]);
        assert_eq!(a1.text, r"{\an8\b1}Hello");
        let a2 = toggle_bold(&a1.text, a1.selection, "Default", &[]);
        assert_eq!(a2.text, r"{\an8}Hello");

        // fn / colour twice -> single tag (never duplicated).
        let f1 = set_font_name("Hello", TextSelection::caret(0), "Arial");
        assert_eq!(f1.text, r"{\fnArial}Hello");
        let f2 = set_font_name(&f1.text, f1.selection, "Arial");
        assert_eq!(f2.text, r"{\fnArial}Hello");

        let c1 = set_color("Hello", TextSelection::caret(0), 1, 0xFF, 0xFF, 0xFF).expect("c1");
        assert_eq!(c1.text, r"{\1c&HFFFFFF&}Hello");
        let c2 = set_color(&c1.text, c1.selection, 1, 0xFF, 0xFF, 0xFF).expect("c2");
        assert_eq!(c2.text, r"{\1c&HFFFFFF&}Hello");
    }
}
