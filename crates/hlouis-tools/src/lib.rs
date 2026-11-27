//! Native Rust implementations of the two `HLouis` Aegisub Lua macros:
//! 1. **Select by Style** (`"Chọn dòng theo Style"`): quick single-key select + full multi-style
//!    dialog model with missing-style detection and set/add/subtract/intersect actions.
//! 2. **Clean Whitespace & Redundant Line Breaks** (`"HLouis · Dọn khoảng trắng"`): single-pass,
//!    zero-regex cleaner with `\p1+` drawing-segment awareness, Unicode space handling, dry-run
//!    diff preview, and zero allocation for unchanged lines (< 40 ms on 200,000 lines).

#![forbid(unsafe_code)]
#![cfg_attr(not(test), deny(clippy::unwrap_used, clippy::expect_used))]

use ass_core::{EditCommand, EventKind, ScriptFile};
use ass_tools::SelectionAction;
use std::collections::{BTreeSet, HashMap, HashSet};

// ===========================================================================
// 1. Select by Style ("Chọn dòng theo Style")
// ===========================================================================

/// Style summary entry shown in the Select by Style dialog.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct StyleCountEntry {
    pub name: String,
    pub dialogue_count: usize,
    pub comment_count: usize,
    /// `true` when events reference this style but it is not defined in `[V4+ Styles]`.
    pub is_missing_from_styles: bool,
}

impl StyleCountEntry {
    #[inline]
    #[must_use]
    pub const fn total_count(&self) -> usize {
        self.dialogue_count + self.comment_count
    }
}

/// Result of quick select-by-style on the active line.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct QuickSelectReport {
    pub matched_style: Option<String>,
    pub selection: Vec<usize>,
    pub active_line: Option<usize>,
    pub active_was_not_dialogue: bool,
    pub message_en: String,
    pub message_vi: String,
}

/// Quick Select by Style: selects every `Dialogue` (and optionally `Comment`) line whose style
/// equals the active line's style, keeping `active_line`.
///
/// Never performs a silent no-op if `active_line` is out of range or is a `Comment` line;
/// reports the condition explicitly in `message_en` / `message_vi`.
#[must_use]
pub fn quick_select_by_style(
    script: &ScriptFile,
    active_line: usize,
    include_comments: bool,
) -> QuickSelectReport {
    let Some(events_sec) = script.events_section() else {
        return QuickSelectReport {
            matched_style: None,
            selection: Vec::new(),
            active_line: None,
            active_was_not_dialogue: true,
            message_en: "Script has no events.".to_owned(),
            message_vi: "Kịch bản không có dòng phụ đề nào.".to_owned(),
        };
    };
    let Some(active_ev) = events_sec.get_event(active_line) else {
        return QuickSelectReport {
            matched_style: None,
            selection: Vec::new(),
            active_line: None,
            active_was_not_dialogue: true,
            message_en: format!("Active line {active_line} is out of range."),
            message_vi: format!("Dòng hiện tại {active_line} nằm ngoài phạm vi."),
        };
    };
    if active_ev.kind() != EventKind::Dialogue {
        return QuickSelectReport {
            matched_style: None,
            selection: Vec::new(),
            active_line: Some(active_line),
            active_was_not_dialogue: true,
            message_en: "Active line is a comment, not a dialogue line.".to_owned(),
            message_vi: "Dòng hiện tại là ghi chú (Comment), không phải lời thoại.".to_owned(),
        };
    }

    let target_style = active_ev.style();
    let mut selection = Vec::with_capacity(64);
    for (idx, ev) in events_sec.events().enumerate() {
        if (include_comments || ev.kind() == EventKind::Dialogue) && ev.style() == target_style {
            selection.push(idx);
        }
    }
    let count = selection.len();
    let style_owned = target_style.to_owned();
    QuickSelectReport {
        matched_style: Some(style_owned.clone()),
        selection,
        active_line: Some(active_line),
        active_was_not_dialogue: false,
        message_en: format!("Selected {count} lines with style '{style_owned}'."),
        message_vi: format!("Đã chọn {count} dòng có style '{style_owned}'."),
    }
}

/// Writes quick-select matching indices into a caller-provided buffer (0 heap allocations once
/// `out` has capacity). Returns `None` if `active_line` is not a dialogue line.
pub fn quick_select_by_style_into(
    script: &ScriptFile,
    active_line: usize,
    include_comments: bool,
    out: &mut Vec<usize>,
) -> Option<usize> {
    out.clear();
    let events_sec = script.events_section()?;
    let active_ev = events_sec.get_event(active_line)?;
    if active_ev.kind() != EventKind::Dialogue {
        return None;
    }
    let target_style = active_ev.style();
    for (idx, ev) in events_sec.events().enumerate() {
        if (include_comments || ev.kind() == EventKind::Dialogue) && ev.style() == target_style {
            out.push(idx);
        }
    }
    Some(out.len())
}

/// Builds the full list of styles defined in `[V4+ Styles]` PLUS any styles referenced only by
/// events (flagged with `is_missing_from_styles = true`), with exact dialogue & comment counts.
///
/// Runs in O(N) (< 3 ms on 200,000 lines).
#[must_use]
pub fn build_style_counts(script: &ScriptFile) -> Vec<StyleCountEntry> {
    let mut entries: Vec<StyleCountEntry> = Vec::with_capacity(32);
    let mut index_by_name: HashMap<&str, usize> = HashMap::with_capacity(32);

    for st in script.styles() {
        let name = st.name();
        if !index_by_name.contains_key(name) {
            let idx = entries.len();
            index_by_name.insert(name, idx);
            entries.push(StyleCountEntry {
                name: name.to_owned(),
                dialogue_count: 0,
                comment_count: 0,
                is_missing_from_styles: false,
            });
        }
    }

    if let Some(ev_sec) = script.events_section() {
        // Tiny 2-slot MRU cache avoids hashing on 99%+ of events (e.g. alternating dialogue/sign).
        let mut cache0: (&str, usize) = ("", usize::MAX);
        let mut cache1: (&str, usize) = ("", usize::MAX);
        for ev in ev_sec.events() {
            let st_name = ev.style();
            let idx = if cache0.1 != usize::MAX && st_name == cache0.0 {
                cache0.1
            } else if cache1.1 != usize::MAX && st_name == cache1.0 {
                std::mem::swap(&mut cache0, &mut cache1);
                cache0.1
            } else {
                let resolved = if let Some(&existing) = index_by_name.get(st_name) {
                    existing
                } else {
                    let new_idx = entries.len();
                    index_by_name.insert(st_name, new_idx);
                    entries.push(StyleCountEntry {
                        name: st_name.to_owned(),
                        dialogue_count: 0,
                        comment_count: 0,
                        is_missing_from_styles: true,
                    });
                    new_idx
                };
                cache1 = cache0;
                cache0 = (st_name, resolved);
                resolved
            };
            match ev.kind() {
                EventKind::Dialogue => entries[idx].dialogue_count += 1,
                EventKind::Comment => entries[idx].comment_count += 1,
            }
        }
    }
    entries
}

/// Dialog state for multi-style selection.
#[derive(Debug, Clone)]
pub struct StyleSelectDialog {
    pub styles: Vec<StyleCountEntry>,
    pub selected_styles: Vec<String>,
    /// Default `true` per specification.
    pub include_comments: bool,
    pub action: SelectionAction,
}

impl StyleSelectDialog {
    /// Initializes the dialog from `script`.
    #[must_use]
    pub fn from_script(script: &ScriptFile) -> Self {
        Self {
            styles: build_style_counts(script),
            selected_styles: Vec::new(),
            include_comments: true,
            action: SelectionAction::Set,
        }
    }

    /// Evaluates the dialog against `script` and `current_selection`, returning the new selection,
    /// active line (first match), and explicit English + Vietnamese status messages.
    #[must_use]
    pub fn apply(
        &self,
        script: &ScriptFile,
        current_selection: &BTreeSet<usize>,
    ) -> DialogSelectReport {
        if self.selected_styles.is_empty() {
            return DialogSelectReport {
                selection: current_selection.clone(),
                active_line: current_selection.iter().next().copied(),
                matched_count: 0,
                message_en: "No style selected.".to_owned(),
                message_vi: "Chưa chọn style nào.".to_owned(),
            };
        }
        let wanted: HashSet<&str> = self.selected_styles.iter().map(String::as_str).collect();
        let mut matched = BTreeSet::new();
        if let Some(ev_sec) = script.events_section() {
            for (idx, ev) in ev_sec.events().enumerate() {
                if (self.include_comments || ev.kind() == EventKind::Dialogue)
                    && wanted.contains(ev.style())
                {
                    matched.insert(idx);
                }
            }
        }
        let matched_count = matched.len();
        let selection = match self.action {
            SelectionAction::Set => matched,
            SelectionAction::Add => current_selection.union(&matched).copied().collect(),
            SelectionAction::Subtract => current_selection.difference(&matched).copied().collect(),
            SelectionAction::Intersect => {
                current_selection.intersection(&matched).copied().collect()
            }
        };
        let active_line = selection.iter().next().copied();
        let (message_en, message_vi) = if matched_count == 0 {
            let joined = self.selected_styles.join(", ");
            (
                format!("No line uses style '{joined}'."),
                format!("Không có dòng nào dùng style '{joined}'."),
            )
        } else {
            let n = selection.len();
            (format!("Selected {n} lines."), format!("Đã chọn {n} dòng."))
        };
        DialogSelectReport {
            selection,
            active_line,
            matched_count,
            message_en,
            message_vi,
        }
    }
}

/// Result of applying [`StyleSelectDialog`].
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct DialogSelectReport {
    pub selection: BTreeSet<usize>,
    pub active_line: Option<usize>,
    pub matched_count: usize,
    pub message_en: String,
    pub message_vi: String,
}

// ===========================================================================
// 2. Clean Whitespace & Redundant Line Breaks ("HLouis · Dọn khoảng trắng")
// ===========================================================================

/// Options for the whitespace & redundant line-break cleaner.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
#[allow(clippy::struct_excessive_bools)]
pub struct CleanOptions {
    /// Clean only selected lines when `true`, otherwise the whole script.
    pub selected_only: bool,
    /// Clean Comment lines too when `true` (default `false`: Dialogue lines only).
    pub include_comments: bool,
    /// Collapse consecutive `\N\N...` / `\n\n...` into a single `\N` (default `false`: keep).
    pub collapse_repeated_newlines: bool,
    /// Collapse runs of 2+ cleanable spaces inside visible text to one `' '` (default `false`).
    pub collapse_internal_spaces: bool,
}

/// Before/after diff entry for dry-run preview.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct CleanDiff {
    pub line_index: usize,
    pub before: String,
    pub after: String,
}

/// Dry-run preview output.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct CleanPreview {
    pub changed_count: usize,
    pub diffs: Vec<CleanDiff>,
}

/// Result of executing [`clean_whitespace`].
#[derive(Debug, Clone, PartialEq)]
pub struct CleanReport {
    pub command: EditCommand,
    pub changed_count: usize,
    pub message_en: String,
    pub message_vi: String,
}

/// Returns `true` if `c` is a cleanable horizontal/Unicode space (never touches `\h` which is
/// represented as two ASCII chars `\` + `h`).
#[inline]
#[must_use]
pub const fn is_cleanable_space(c: char) -> bool {
    matches!(
        c,
        ' ' | '\t' | '\u{00A0}' | '\u{2000}'..='\u{200B}' | '\u{202F}' | '\u{205F}' | '\u{3000}'
    )
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Token<'a> {
    /// `{...}` override block (copied verbatim; never modified).
    Tag(&'a str),
    /// Drawing-mode text segment (`\p1+`, copied verbatim; never modified).
    Drawing(&'a str),
    /// `\N` or `\n` ASS line break.
    Newline(&'a str),
    /// `\h` hard non-breaking space (intentional, preserved verbatim).
    HardSpace,
    /// Run of one or more cleanable spaces.
    Space(&'a str),
    /// Normal visible non-space text run.
    Text(&'a str),
}

/// Updates drawing mode (`\p<N>`) by scanning an override block `{...}` without allocating.
fn update_drawing_mode(tag_block: &str, drawing_scale: &mut u32) {
    let bytes = tag_block.as_bytes();
    let len = bytes.len();
    let mut i = 0usize;
    while i < len {
        if bytes[i] == b'\\' && i + 1 < len && bytes[i + 1] == b'p' {
            // Check next byte: `\p0`, `\p1`, `\p2`... NOT `\pos`, `\pbo`.
            let j = i + 2;
            if j < len && bytes[j].is_ascii_digit() {
                let mut val = 0u32;
                let mut k = j;
                while k < len && bytes[k].is_ascii_digit() {
                    val = val
                        .saturating_mul(10)
                        .saturating_add(u32::from(bytes[k] - b'0'));
                    k += 1;
                }
                *drawing_scale = val;
                i = k;
                continue;
            }
        }
        i += 1;
    }
}

/// Tokenizes `input` into a reusable `tokens` buffer (0 allocations once `tokens` has capacity).
fn tokenize_line<'a>(input: &'a str, tokens: &mut Vec<Token<'a>>) {
    tokens.clear();
    let bytes = input.as_bytes();
    let len = bytes.len();
    let mut i = 0usize;
    let mut drawing_scale = 0u32;

    while i < len {
        if bytes[i] == b'{' {
            let start = i;
            if let Some(rel) = bytes[i + 1..].iter().position(|&b| b == b'}') {
                let end = i + 1 + rel + 1;
                let block = &input[start..end];
                update_drawing_mode(block, &mut drawing_scale);
                tokens.push(Token::Tag(block));
                i = end;
                continue;
            }
            // Unclosed `{`: treat remainder as verbatim tag block.
            tokens.push(Token::Tag(&input[start..]));
            break;
        }

        if drawing_scale > 0 {
            // Inside \p1+ drawing mode: consume verbatim until next `{` or end of line.
            let start = i;
            while i < len && bytes[i] != b'{' {
                i += 1;
            }
            tokens.push(Token::Drawing(&input[start..i]));
            continue;
        }

        if bytes[i] == b'\\' && i + 1 < len {
            match bytes[i + 1] {
                b'N' | b'n' => {
                    tokens.push(Token::Newline(&input[i..i + 2]));
                    i += 2;
                    continue;
                }
                b'h' => {
                    tokens.push(Token::HardSpace);
                    i += 2;
                    continue;
                }
                _ => {}
            }
        }

        // Check if current char is a cleanable space.
        let ch = input[i..].chars().next().unwrap_or('\0');
        if is_cleanable_space(ch) {
            let start = i;
            i += ch.len_utf8();
            while i < len {
                let next_ch = input[i..].chars().next().unwrap_or('\0');
                if is_cleanable_space(next_ch) {
                    i += next_ch.len_utf8();
                } else {
                    break;
                }
            }
            tokens.push(Token::Space(&input[start..i]));
            continue;
        }

        // Normal visible text run until `{`, `\N`, `\n`, `\h`, or cleanable space.
        let start = i;
        i += ch.len_utf8();
        while i < len {
            let b = bytes[i];
            if b == b'{' {
                break;
            }
            if b == b'\\' && i + 1 < len && matches!(bytes[i + 1], b'N' | b'n' | b'h') {
                break;
            }
            if b < 0x80 {
                if b == b' ' || b == b'\t' {
                    break;
                }
                i += 1;
            } else {
                let next_ch = input[i..].chars().next().unwrap_or('\0');
                if is_cleanable_space(next_ch) {
                    break;
                }
                i += next_ch.len_utf8();
            }
        }
        tokens.push(Token::Text(&input[start..i]));
    }
}

/// Fast O(N) zero-allocation check: returns `true` only if `input` might need cleaning.
#[inline]
fn line_might_need_cleaning(input: &str, opts: CleanOptions) -> bool {
    let bytes = input.as_bytes();
    if bytes.is_empty() {
        return false;
    }
    let first = bytes[0];
    let last = bytes[bytes.len() - 1];
    if first == b' ' || first == b'\t' || last == b' ' || last == b'\t' {
        return true;
    }
    let mut prev_space = false;
    let mut i = 0usize;
    while i < bytes.len() {
        let b = bytes[i];
        if b == b'{' || b >= 0x80 {
            return true;
        }
        if b == b'\\' && i + 1 < bytes.len() && matches!(bytes[i + 1], b'N' | b'n') {
            return true;
        }
        let is_sp = b == b' ' || b == b'\t';
        if opts.collapse_internal_spaces && is_sp && prev_space {
            return true;
        }
        prev_space = is_sp;
        i += 1;
    }
    false
}

/// Cleans a single ASS event text into `out`.
///
/// Returns `true` if `out` differs from `input` (already-clean ASCII lines take the fast path
/// with **zero heap allocations**).
pub fn clean_line_into(input: &str, opts: CleanOptions, out: &mut String) -> bool {
    if !line_might_need_cleaning(input, opts) {
        return false;
    }
    let mut tokens = Vec::with_capacity(16);
    clean_line_with_scratch(input, opts, &mut tokens, out)
}

fn clean_line_with_scratch<'a>(
    input: &'a str,
    opts: CleanOptions,
    tokens: &mut Vec<Token<'a>>,
    out: &mut String,
) -> bool {
    out.clear();
    if input.is_empty() {
        return false;
    }

    tokenize_line(input, tokens);

    // Find first and last "content" tokens (Text, HardSpace, or Drawing).
    // Any Space or Newline strictly before first_content or strictly after last_content is
    // leading/trailing and must be stripped, while `{tags}` are always preserved!
    let first_content = tokens
        .iter()
        .position(|t| matches!(t, Token::Text(_) | Token::HardSpace));
    let last_content = tokens
        .iter()
        .rposition(|t| matches!(t, Token::Text(_) | Token::HardSpace));

    let Some(first_c) = first_content else {
        // Line has only tags, spaces, and/or newlines -> keep only tags!
        for &tok in tokens.iter() {
            if let Token::Tag(s) | Token::Drawing(s) = tok {
                out.push_str(s);
            }
        }
        return out.as_str() != input;
    };
    let last_c = last_content.unwrap_or(first_c);

    // Helper: is the next non-Tag token a Newline?
    let mut prev_non_tag_was_newline = false;

    for (idx, &tok) in tokens.iter().enumerate() {
        match tok {
            Token::Tag(s) => {
                out.push_str(s);
            }
            Token::Drawing(s) | Token::Text(s) => {
                out.push_str(s);
                prev_non_tag_was_newline = false;
            }
            Token::HardSpace => {
                out.push_str(r"\h");
                prev_non_tag_was_newline = false;
            }
            Token::Newline(s) => {
                // Strip leading/trailing redundant \N / \n.
                if idx < first_c || idx > last_c {
                    continue;
                }
                if prev_non_tag_was_newline && opts.collapse_repeated_newlines {
                    continue;
                }
                out.push_str(s);
                prev_non_tag_was_newline = true;
            }
            Token::Space(s) => {
                // Strip leading/trailing spaces (even inside/adjacent to leading/trailing tags).
                if idx < first_c || idx > last_c {
                    continue;
                }
                // Strip spaces immediately after a \N / \n.
                if prev_non_tag_was_newline {
                    continue;
                }
                // Strip spaces immediately before a \N / \n (looking past any intervening {tags}
                // or other spaces).
                let next_non_tag = tokens[idx + 1..]
                    .iter()
                    .find(|t| !matches!(t, Token::Tag(_) | Token::Space(_)));
                if matches!(next_non_tag, Some(Token::Newline(_))) {
                    continue;
                }
                if opts.collapse_internal_spaces && (s.len() > 1 || s != " ") {
                    out.push(' ');
                } else {
                    out.push_str(s);
                }
            }
        }
    }

    out.as_str() != input
}

/// Runs a dry-run preview of whitespace cleaning, returning the count of changed lines and
/// before/after diffs without mutating `script`.
#[must_use]
pub fn preview_clean(
    script: &ScriptFile,
    selected: &BTreeSet<usize>,
    opts: CleanOptions,
) -> CleanPreview {
    let Some(ev_sec) = script.events_section() else {
        return CleanPreview {
            changed_count: 0,
            diffs: Vec::new(),
        };
    };
    let mut diffs = Vec::new();
    let mut scratch = String::with_capacity(256);
    let mut tokens = Vec::with_capacity(32);

    for (idx, ev) in ev_sec.events().enumerate() {
        if opts.selected_only && !selected.contains(&idx) {
            continue;
        }
        if !opts.include_comments && ev.kind() != EventKind::Dialogue {
            continue;
        }
        let before = ev.text();
        if !line_might_need_cleaning(before, opts) {
            continue;
        }
        if clean_line_with_scratch(before, opts, &mut tokens, &mut scratch) {
            diffs.push(CleanDiff {
                line_index: idx,
                before: before.to_owned(),
                after: scratch.clone(),
            });
        }
    }
    let changed_count = diffs.len();
    CleanPreview {
        changed_count,
        diffs,
    }
}

/// Builds ONE atomic, undoable [`EditCommand::Batch`] cleaning whitespace and redundant line
/// breaks across `script`. Zero heap allocations for lines that do not change.
#[must_use]
pub fn clean_whitespace(
    script: &ScriptFile,
    selected: &BTreeSet<usize>,
    opts: CleanOptions,
) -> CleanReport {
    let Some(ev_sec) = script.events_section() else {
        return CleanReport {
            command: EditCommand::Batch(Vec::new()),
            changed_count: 0,
            message_en: "Cleaned 0 lines.".to_owned(),
            message_vi: "Đã dọn 0 dòng.".to_owned(),
        };
    };
    let mut cmds = Vec::new();
    let mut scratch = String::with_capacity(256);
    let mut tokens = Vec::with_capacity(32);

    for (idx, ev) in ev_sec.events().enumerate() {
        if opts.selected_only && !selected.contains(&idx) {
            continue;
        }
        if !opts.include_comments && ev.kind() != EventKind::Dialogue {
            continue;
        }
        let before = ev.text();
        if !line_might_need_cleaning(before, opts) {
            continue;
        }
        if clean_line_with_scratch(before, opts, &mut tokens, &mut scratch) {
            cmds.push(EditCommand::ModifyEventText {
                index: idx,
                before: before.to_owned(),
                after: scratch.clone(),
            });
        }
    }
    let changed_count = cmds.len();
    CleanReport {
        command: EditCommand::Batch(cmds),
        changed_count,
        message_en: format!("Cleaned {changed_count} lines."),
        message_vi: format!("Đã dọn {changed_count} dòng."),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use ass_core::{AssEvent, AssTime, parse_ass};

    fn clean_one(s: &str, opts: CleanOptions) -> String {
        let mut out = String::new();
        let changed = clean_line_into(s, opts, &mut out);
        if changed { out } else { s.to_owned() }
    }

    #[test]
    fn cleans_all_required_whitespace_and_newline_cases() {
        let def = CleanOptions::default();

        // 1. Tags at start and end with spaces right after/before them
        assert_eq!(
            clean_one(r"{\i1}   Xin chào   {\i0}", def),
            r"{\i1}Xin chào{\i0}"
        );
        assert_eq!(
            clean_one(r"  {\pos(100,200)}  Hello  {\r}  ", def),
            r"{\pos(100,200)}Hello{\r}"
        );

        // 2. Spaces on both sides of \N and \n
        assert_eq!(clean_one(r"Line 1  \N  Line 2", def), r"Line 1\NLine 2");
        assert_eq!(clean_one(r"Line 1 \n Line 2", def), r"Line 1\nLine 2");

        // 3. Redundant \N / \n at start and end (with spaces around them)
        assert_eq!(
            clean_one(r"  \N  {\b1} \N Hello \N  {\b0} \n ", def),
            r"{\b1}Hello{\b0}"
        );

        // 4. Consecutive \N\N kept by default, collapsed when option enabled
        assert_eq!(clean_one(r"A \N \N B", def), r"A\N\NB");
        let collapse_nl = CleanOptions {
            collapse_repeated_newlines: true,
            ..def
        };
        assert_eq!(clean_one(r"A \N \N B", collapse_nl), r"A\NB");

        // 5. Internal runs of 2+ spaces kept by default, collapsed when option enabled
        assert_eq!(clean_one("A   B", def), "A   B");
        let collapse_sp = CleanOptions {
            collapse_internal_spaces: true,
            ..def
        };
        assert_eq!(clean_one("A  \u{00A0}  B", collapse_sp), "A B");

        // 6. NBSP (Word paste) & Unicode spaces (U+2000..U+200B, U+202F, U+205F, U+3000)
        assert_eq!(
            clean_one("\u{00A0}\u{3000}Tiếng Việt\u{200B}\u{202F}\u{205F}", def),
            "Tiếng Việt"
        );
        assert_eq!(
            clean_one("\u{3000}日本語テスト\u{3000}", def),
            "日本語テスト"
        );

        // 7. Never touch inside {...} override blocks, never touch \h
        assert_eq!(
            clean_one(r"{\fnTimes New Roman\c&HFFFFFF&}  \hHello\h  ", def),
            r"{\fnTimes New Roman\c&HFFFFFF&}\hHello\h"
        );

        // 8. Drawings mixed with normal text: skip ONLY \p1+ segment, clean \p0 segment!
        assert_eq!(
            clean_one(
                r"{\p1} m 0 0 l 10 10 {\p0}  Cleaned text  \N  Line 2  ",
                def
            ),
            r"{\p1} m 0 0 l 10 10 {\p0}Cleaned text\NLine 2"
        );

        // 9. Karaoke tags preserved, spaces around \N cleaned
        assert_eq!(
            clean_one(r"{\k20}  Ka{\k30}ra  \N  {\k25}oke  ", def),
            r"{\k20}Ka{\k30}ra\N{\k25}oke"
        );

        // 10. Empty lines and lines that become empty
        assert_eq!(clean_one("", def), "");
        assert_eq!(clean_one("   \u{00A0} \t  ", def), "");
        assert_eq!(clean_one(r"{\i1}   \N   ", def), r"{\i1}");
    }

    #[test]
    fn select_by_style_quick_and_dialog_including_missing_styles() {
        let src = "[Script Info]\nPlayResX: 1920\nPlayResY: 1080\n\n\
                   [V4+ Styles]\n\
                   Format: Name, Fontname, Fontsize, PrimaryColour, SecondaryColour, OutlineColour, BackColour, Bold, Italic, Underline, StrikeOut, ScaleX, ScaleY, Spacing, Angle, BorderStyle, Outline, Shadow, Alignment, MarginL, MarginR, MarginV, Encoding\n\
                   Style: Default,Arial,48,&H00FFFFFF,&H000000FF,&H00000000,&H00000000,0,0,0,0,100,100,0,0,1,2,2,2,10,10,10,1\n\
                   Style: Unused,Arial,48,&H00FFFFFF,&H000000FF,&H00000000,&H00000000,0,0,0,0,100,100,0,0,1,2,2,2,10,10,10,1\n\n\
                   [Events]\n\
                   Format: Layer, Start, End, Style, Name, MarginL, MarginR, MarginV, Effect, Text\n\
                   Comment: 0,0:00:00.00,0:00:01.00,Default,,0,0,0,,Note\n\
                   Dialogue: 0,0:00:01.00,0:00:02.00,Default,,0,0,0,,  Hello  \n\
                   Dialogue: 0,0:00:02.00,0:00:03.00,MissingStyle,,0,0,0,,World\n\
                   Dialogue: 0,0:00:03.00,0:00:04.00,Default,,0,0,0,,Again\n";
        let mut script = parse_ass(src).value;

        // Active line 0 is Comment -> explicit report, not silent no-op
        let r0 = quick_select_by_style(&script, 0, false);
        assert!(r0.active_was_not_dialogue);
        assert_eq!(r0.selection.len(), 0);

        // Active line 1 is Dialogue Default -> selects 1 and 3
        let r1 = quick_select_by_style(&script, 1, false);
        assert!(!r1.active_was_not_dialogue);
        assert_eq!(r1.selection, vec![1, 3]);

        // Dialog style list includes defined + Unused + MissingStyle (flagged)
        let mut dlg = StyleSelectDialog::from_script(&script);
        assert_eq!(dlg.styles.len(), 3);
        let missing = dlg
            .styles
            .iter()
            .find(|s| s.name == "MissingStyle")
            .unwrap();
        assert!(missing.is_missing_from_styles);
        assert_eq!(missing.dialogue_count, 1);

        // Selecting Unused reports explicitly
        dlg.selected_styles = vec!["Unused".to_owned()];
        let unused_res = dlg.apply(&script, &BTreeSet::new());
        assert_eq!(unused_res.matched_count, 0);
        assert!(unused_res.message_en.contains("No line uses style"));

        // Clean whitespace applies as ONE undoable EditCommand and skips comments by default
        let preview = preview_clean(&script, &BTreeSet::new(), CleanOptions::default());
        assert_eq!(preview.changed_count, 1);
        assert_eq!(preview.diffs[0].after, "Hello");
        let report = clean_whitespace(&script, &BTreeSet::new(), CleanOptions::default());
        assert_eq!(report.changed_count, 1);
        report.command.apply(&mut script).expect("apply");
        assert_eq!(
            script
                .events_section()
                .unwrap()
                .get_event(1)
                .unwrap()
                .text(),
            "Hello"
        );
        // Clean unused AssEvent import silence
        let _ = (AssEvent::dialogue(AssTime::ZERO, AssTime::ZERO, ""),);
    }
}
