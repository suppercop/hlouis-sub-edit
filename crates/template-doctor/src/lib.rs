//! Karaoke Template Doctor: linter and safe auto-fixer for Aegisub 3.5.1 `kara-templater.lua`
//! (`template`, `code`, `mixin`) lines and karaoke `\k` lines.
//!
//! Reference: Aegisub 3.5.1 `automation/autoload/kara-templater.lua` & `automation/include/karaskel.lua`.
//!
//! Performs:
//! - ASS override tag & brace/paren validation (missing/doubled `\`, numeric comma typos,
//!   argument counts, empty `{}`, empty `\t()`, unknown tag suggestions).
//! - Template language validation (`$var` suggestions, unbalanced `!expr!`, Effect modifier typos
//!   and conflicting modifiers).
//! - Real Lua 5.1 / `LuaJIT` syntax checking via `full_moon` (detects `!=`, `&&`, `||`, missing
//!   `end`/`then`/`do`, top-level `local` misuse in `code` lines, unknown global calls).
//! - Safe, unambiguous, idempotent auto-fixes (`fix_all_safe`) returned as ONE undoable
//!   [`EditCommand::Batch`], never touching non-template lines.

#![forbid(unsafe_code)]
#![cfg_attr(not(test), deny(clippy::unwrap_used, clippy::expect_used))]

use ass_core::{AssEvent, EditCommand, EventKind, ScriptFile};
use karaskel::classify_template_line;
use std::collections::HashSet;

/// Diagnostic severity level.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Severity {
    Error,
    Warning,
    Info,
}

/// Which field of the ASS line a diagnostic/fix targets.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum DiagnosticTarget {
    Effect,
    Text,
}

/// Optional replacement fix for a diagnostic.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SuggestedFix {
    pub target: DiagnosticTarget,
    /// Byte span in the target field (`Effect` or `Text`).
    pub start: usize,
    pub end: usize,
    pub replacement: String,
    /// `true` only when the fix is completely unambiguous and safe for [`fix_all_safe`].
    pub is_safe: bool,
}

/// One lint diagnostic produced by Template Doctor.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Diagnostic {
    pub line_index: usize,
    /// 1-based column inside the target field.
    pub column: usize,
    pub severity: Severity,
    pub code: &'static str,
    pub target: DiagnosticTarget,
    pub message_en: String,
    pub message_vi: String,
    pub fix: Option<SuggestedFix>,
}

/// Before/after diff for one fixed line.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct LineDiff {
    pub line_index: usize,
    pub before_effect: String,
    pub after_effect: String,
    pub before_text: String,
    pub after_text: String,
}

/// Output of [`fix_all_safe`].
#[derive(Debug, Clone, PartialEq)]
pub struct FixResult {
    pub command: EditCommand,
    pub fixed_lines: usize,
    pub applied_fixes: usize,
    pub diffs: Vec<LineDiff>,
}

// ===========================================================================
// Canonical Aegisub / kara-templater tables
// ===========================================================================

const KNOWN_ASS_TAGS: &[&str] = &[
    "an", "a", "alpha", "1a", "2a", "3a", "4a", "c", "1c", "2c", "3c", "4c", "b", "i", "u", "s",
    "bord", "xbord", "ybord", "shad", "xshad", "yshad", "be", "blur", "fn", "fs", "fscx", "fscy",
    "fsp", "fr", "frx", "fry", "frz", "fax", "fay", "fe", "k", "K", "kf", "ko", "kt", "q", "r",
    "pos", "move", "org", "fad", "fade", "t", "clip", "iclip", "p", "pbo",
];

const KNOWN_TEMPLATE_MODS: &[&str] = &[
    "once", "line", "syl", "char", "furi", "pre-line", "all", "keeptags", "multi", "noblank",
    "notext", "loop", "repeat", "fx", "fxgroup", "nobase", "notime",
];

const KNOWN_INLINE_VARS: &[&str] = &[
    "lstart",
    "lend",
    "ldur",
    "lmid",
    "lwidth",
    "lheight",
    "lleft",
    "lcenter",
    "lright",
    "ltop",
    "lmiddle",
    "lbottom",
    "lx",
    "ly",
    "li",
    "ln",
    "layer",
    "lstyle",
    "style",
    "actor",
    "effect",
    "start",
    "end",
    "dur",
    "mid",
    "width",
    "height",
    "left",
    "center",
    "right",
    "top",
    "middle",
    "bottom",
    "x",
    "y",
    "sstart",
    "send",
    "sdur",
    "smid",
    "swidth",
    "sheight",
    "sleft",
    "scenter",
    "sright",
    "stop",
    "smiddle",
    "sbottom",
    "sx",
    "sy",
    "si",
    "sn",
    "kdur",
    "skdur",
    "syl_start",
    "syl_end",
    "syl_dur",
    "syl_mid",
    "syl_i",
    "syl_n",
    "ci",
    "cn",
    "cstart",
    "cend",
    "cdur",
    "cmid",
    "cx",
    "cy",
    "cleft",
    "ccenter",
    "cright",
    "ctop",
    "cmiddle",
    "cbottom",
    "cwidth",
    "cheight",
    "fi",
    "fn",
    "fstart",
    "fend",
    "fdur",
    "fmid",
    "fx",
    "fy",
    "fleft",
    "fcenter",
    "fright",
    "ftop",
    "fmiddle",
    "fbottom",
    "fwidth",
    "fheight",
    "ascent",
    "descent",
    "extlead",
    "xres",
    "yres",
    "meta_res_x",
    "meta_res_y",
    "loop",
    "maxloop",
    "j",
    "maxj",
    "text",
    "syl_text",
    "char_text",
    "furi_text",
];

const KNOWN_LUA_GLOBALS: &[&str] = &[
    // Lua 5.1 standard library
    "assert",
    "collectgarbage",
    "dofile",
    "error",
    "getfenv",
    "getmetatable",
    "ipairs",
    "load",
    "loadfile",
    "loadstring",
    "next",
    "pairs",
    "pcall",
    "print",
    "rawequal",
    "rawget",
    "rawset",
    "select",
    "setfenv",
    "setmetatable",
    "tonumber",
    "tostring",
    "type",
    "unpack",
    "xpcall",
    "math",
    "string",
    "table",
    "os",
    "io",
    "coroutine",
    "bit",
    "bit32",
    "_G",
    // kara-templater / karaskel / Aegisub utils
    "retime",
    "relayer",
    "restyle",
    "maxloop",
    "remember",
    "recall",
    "set_temp",
    "aegisub",
    "karaskel",
    "utils",
    "unicode",
    "ass_color",
    "ass_alpha",
    "ass_style_color",
    "interpolate",
    "interpolate_color",
    "interpolate_alpha",
    "clamp",
    "color_from_style",
    "alpha_from_style",
    "extract_color",
    "head_tail_split",
    "line",
    "orgline",
    "syl",
    "basesyl",
    "char",
    "furi",
    "meta",
    "styles",
    "tenv",
    "fxgroup",
    "j",
    "maxj",
];

/// Returns `true` if `event` is a `template`, `code`, or `mixin` comment line.
#[inline]
#[must_use]
pub fn is_template_or_code_line(event: &AssEvent) -> bool {
    if event.kind() != EventKind::Comment {
        return false;
    }
    let Some(first) = event.effect().split_whitespace().next() else {
        return false;
    };
    first.eq_ignore_ascii_case("template")
        || first.eq_ignore_ascii_case("code")
        || first.eq_ignore_ascii_case("mixin")
}

/// Lints an entire [`ScriptFile`] (all template/code/mixin lines and karaoke `\k` lines).
#[must_use]
pub fn lint_script(script: &ScriptFile) -> Vec<Diagnostic> {
    let mut diags = Vec::new();
    lint_script_into(script, &mut diags);
    diags
}

/// Lints `script` into a reusable `out` vector (0 allocations when no diagnostics are produced).
pub fn lint_script_into(script: &ScriptFile, out: &mut Vec<Diagnostic>) {
    out.clear();
    let Some(ev_sec) = script.events_section() else {
        return;
    };
    // Collect user-defined globals from `code` lines so calls to them are not flagged.
    let mut user_defined: HashSet<&str> = HashSet::new();
    for ev in ev_sec.events() {
        if ev.kind() == EventKind::Comment {
            let eff = ev.effect().trim_start();
            if eff
                .as_bytes()
                .get(..4)
                .is_some_and(|b| b.eq_ignore_ascii_case(b"code"))
            {
                collect_defined_functions(ev.text(), &mut user_defined);
            }
        }
    }

    for (idx, ev) in ev_sec.events().enumerate() {
        if is_template_or_code_line(ev) {
            lint_template_event(idx, ev, &user_defined, out);
        } else if ev.kind() == EventKind::Dialogue && has_karaoke_tag(ev.text()) {
            lint_karaoke_line(idx, ev.text(), out);
        }
    }
}

/// Applies all safe, unambiguous fixes across template/code/mixin lines (never touches
/// non-template lines) and returns ONE undoable [`EditCommand::Batch`] plus before/after diffs.
#[must_use]
#[allow(clippy::too_many_lines)]
pub fn fix_all_safe(script: &ScriptFile) -> FixResult {
    let Some(ev_sec) = script.events_section() else {
        return FixResult {
            command: EditCommand::Batch(Vec::new()),
            fixed_lines: 0,
            applied_fixes: 0,
            diffs: Vec::new(),
        };
    };

    let mut working = script.clone();
    let mut touched: std::collections::BTreeSet<usize> = std::collections::BTreeSet::new();
    let mut applied_fixes = 0usize;

    for _pass in 0..3 {
        let diags = lint_script(&working);
        let mut by_line: std::collections::BTreeMap<usize, Vec<&SuggestedFix>> =
            std::collections::BTreeMap::new();
        for d in &diags {
            if let Some(ref fix) = d.fix
                && fix.is_safe
            {
                by_line.entry(d.line_index).or_default().push(fix);
            }
        }
        if by_line.is_empty() {
            break;
        }
        let mut updates: Vec<(usize, String, String)> = Vec::new();
        if let Some(cur_sec) = working.events_section() {
            for (line_idx, fixes) in by_line {
                let Some(ev) = cur_sec.get_event(line_idx) else {
                    continue;
                };
                if !is_template_or_code_line(ev) {
                    continue;
                }
                let (new_eff, eff_applied) =
                    apply_non_overlapping(ev.effect(), &fixes, DiagnosticTarget::Effect);
                let (new_text, text_applied) =
                    apply_non_overlapping(ev.text(), &fixes, DiagnosticTarget::Text);
                if new_eff != ev.effect() || new_text != ev.text() {
                    applied_fixes += eff_applied + text_applied;
                    touched.insert(line_idx);
                    updates.push((line_idx, new_eff, new_text));
                }
            }
        }
        if updates.is_empty() {
            break;
        }
        if let Some(cur_sec_mut) = working.events_section_mut() {
            for (line_idx, new_eff, new_text) in updates {
                if let Some(ev) = cur_sec_mut.get_event_mut(line_idx) {
                    ev.set_effect(new_eff);
                    ev.set_text(new_text);
                }
            }
        }
    }

    let mut cmds = Vec::with_capacity(touched.len());
    let mut diffs = Vec::with_capacity(touched.len());
    for line_idx in touched {
        let (Some(before_ev), Some(after_ev)) = (
            ev_sec.get_event(line_idx),
            working.events_section().and_then(|s| s.get_event(line_idx)),
        ) else {
            continue;
        };
        if before_ev.effect() == after_ev.effect() && before_ev.text() == after_ev.text() {
            continue;
        }
        cmds.push(EditCommand::ModifyEvent {
            index: line_idx,
            before: Box::new(before_ev.clone()),
            after: Box::new(after_ev.clone()),
        });
        diffs.push(LineDiff {
            line_index: line_idx,
            before_effect: before_ev.effect().to_owned(),
            after_effect: after_ev.effect().to_owned(),
            before_text: before_ev.text().to_owned(),
            after_text: after_ev.text().to_owned(),
        });
    }

    let fixed_lines = cmds.len();
    FixResult {
        command: EditCommand::Batch(cmds),
        fixed_lines,
        applied_fixes,
        diffs,
    }
}

fn apply_non_overlapping(
    src: &str,
    fixes: &[&SuggestedFix],
    target: DiagnosticTarget,
) -> (String, usize) {
    let mut relevant: Vec<&SuggestedFix> = fixes
        .iter()
        .copied()
        .filter(|f| f.target == target && f.start <= f.end && f.end <= src.len())
        .collect();
    if relevant.is_empty() {
        return (src.to_owned(), 0);
    }
    relevant.sort_by_key(|f| (f.start, f.end));
    let mut out = String::with_capacity(src.len() + 16);
    let mut cursor = 0usize;
    let mut count = 0usize;
    for f in relevant {
        if f.start < cursor {
            continue; // skip overlapping fix
        }
        out.push_str(&src[cursor..f.start]);
        out.push_str(&f.replacement);
        cursor = f.end;
        count += 1;
    }
    out.push_str(&src[cursor..]);
    (out, count)
}

// ===========================================================================
// Per-line linters
// ===========================================================================

fn lint_template_event(
    line_index: usize,
    event: &AssEvent,
    user_defined: &HashSet<&str>,
    out: &mut Vec<Diagnostic>,
) {
    let _classified = classify_template_line(line_index, event);
    let effect = event.effect();
    let is_code = effect
        .split_whitespace()
        .next()
        .is_some_and(|w| w.eq_ignore_ascii_case("code"));

    lint_effect_modifiers(line_index, effect, is_code, out);

    if is_code {
        lint_lua_code(line_index, event.text(), 0, false, user_defined, out);
    } else {
        lint_template_text(line_index, event.text(), user_defined, out);
    }
}

#[allow(clippy::too_many_lines)]
fn lint_effect_modifiers(
    line_index: usize,
    effect: &str,
    is_code: bool,
    out: &mut Vec<Diagnostic>,
) {
    let bytes = effect.as_bytes();
    let len = bytes.len();
    let mut i = 0usize;
    let mut token_idx = 0usize;
    let mut skip_next_arg = false;
    let mut seen_once = false;
    let mut seen_syl = false;
    let mut seen_notext = false;
    let mut seen_char = false;

    while i < len {
        while i < len && bytes[i].is_ascii_whitespace() {
            i += 1;
        }
        if i >= len {
            break;
        }
        let start = i;
        while i < len && !bytes[i].is_ascii_whitespace() {
            i += 1;
        }
        let tok = &effect[start..i];
        token_idx += 1;
        if token_idx == 1 {
            continue; // "template" | "code" | "mixin"
        }
        if skip_next_arg {
            skip_next_arg = false;
            continue;
        }
        let lower = tok.to_ascii_lowercase();
        if matches!(lower.as_str(), "loop" | "repeat" | "fx" | "fxgroup") {
            skip_next_arg = true;
            continue;
        }
        if lower == "once" {
            seen_once = true;
        } else if lower == "syl" {
            seen_syl = true;
        } else if lower == "notext" {
            seen_notext = true;
        } else if lower == "char" {
            seen_char = true;
        }

        if !KNOWN_TEMPLATE_MODS.contains(&lower.as_str()) {
            let suggestion = closest_match(&lower, KNOWN_TEMPLATE_MODS, 2);
            let fix = suggestion.map(|best| SuggestedFix {
                target: DiagnosticTarget::Effect,
                start,
                end: i,
                replacement: best.to_owned(),
                is_safe: true,
            });
            let sug_txt =
                suggestion.map_or_else(String::new, |s| format!(" (did you mean `{s}`?)"));
            let sug_vi = suggestion.map_or_else(String::new, |s| format!(" (ý bạn là `{s}`?)"));
            out.push(Diagnostic {
                line_index,
                column: start + 1,
                severity: Severity::Error,
                code: "E_UNKNOWN_MODIFIER",
                target: DiagnosticTarget::Effect,
                message_en: format!("Unknown template modifier `{tok}`{sug_txt}"),
                message_vi: format!("Tham số template không hợp lệ `{tok}`{sug_vi}"),
                fix,
            });
        }
    }

    if seen_once && seen_syl {
        out.push(Diagnostic {
            line_index,
            column: 1,
            severity: Severity::Warning,
            code: "W_CONFLICT_MODS",
            target: DiagnosticTarget::Effect,
            message_en: "Conflicting modifiers: `once` cannot be combined with `syl`.".to_owned(),
            message_vi: "Xung đột tham số: `once` không thể dùng chung với `syl`.".to_owned(),
            fix: None,
        });
    }
    if is_code && seen_notext {
        out.push(Diagnostic {
            line_index,
            column: 1,
            severity: Severity::Warning,
            code: "W_CONFLICT_MODS",
            target: DiagnosticTarget::Effect,
            message_en: "Modifier `notext` has no effect on `code` lines.".to_owned(),
            message_vi: "Tham số `notext` không có tác dụng trên dòng `code`.".to_owned(),
            fix: None,
        });
    }
    if seen_notext && seen_char {
        out.push(Diagnostic {
            line_index,
            column: 1,
            severity: Severity::Warning,
            code: "W_CONFLICT_MODS",
            target: DiagnosticTarget::Effect,
            message_en: "Conflicting modifiers: `notext` with `char` discards character text."
                .to_owned(),
            message_vi: "Xung đột tham số: `notext` kết hợp `char` sẽ bỏ nội dung ký tự."
                .to_owned(),
            fix: None,
        });
    }
}

#[allow(clippy::too_many_lines)]
fn lint_template_text(
    line_index: usize,
    text: &str,
    user_defined: &HashSet<&str>,
    out: &mut Vec<Diagnostic>,
) {
    let bytes = text.as_bytes();
    let len = bytes.len();
    let mut i = 0usize;

    // 1. Check balanced `{` `}` and lint each override block.
    let mut open_brace: Option<usize> = None;
    let mut in_bang = false;
    while i < len {
        match bytes[i] {
            b'!' => {
                in_bang = !in_bang;
                i += 1;
            }
            b'{' if !in_bang => {
                if let Some(prev) = open_brace {
                    out.push(Diagnostic {
                        line_index,
                        column: prev + 1,
                        severity: Severity::Error,
                        code: "E_UNCLOSED_BRACE",
                        target: DiagnosticTarget::Text,
                        message_en: "Unclosed `{` before next `{`.".to_owned(),
                        message_vi: "Thiếu dấu `}` trước dấu `{` tiếp theo.".to_owned(),
                        fix: Some(SuggestedFix {
                            target: DiagnosticTarget::Text,
                            start: i,
                            end: i,
                            replacement: "}".to_owned(),
                            is_safe: true,
                        }),
                    });
                }
                open_brace = Some(i);
                i += 1;
            }
            b'}' if !in_bang => {
                if let Some(start) = open_brace.take() {
                    let inner = &text[start + 1..i];
                    if inner.trim().is_empty() {
                        out.push(Diagnostic {
                            line_index,
                            column: start + 1,
                            severity: Severity::Warning,
                            code: "W_EMPTY_BLOCK",
                            target: DiagnosticTarget::Text,
                            message_en: "Empty `{}` override block.".to_owned(),
                            message_vi: "Khối thẻ `{}` rỗng.".to_owned(),
                            fix: None,
                        });
                    } else {
                        lint_override_block(line_index, inner, start + 1, out);
                    }
                } else {
                    out.push(Diagnostic {
                        line_index,
                        column: i + 1,
                        severity: Severity::Error,
                        code: "E_STRAY_BRACE",
                        target: DiagnosticTarget::Text,
                        message_en: "Stray closing `}` with no matching `{`.".to_owned(),
                        message_vi: "Dư dấu đóng `}` không có `{` tương ứng.".to_owned(),
                        fix: None,
                    });
                }
                i += 1;
            }
            _ => i += 1,
        }
    }
    if let Some(start) = open_brace {
        out.push(Diagnostic {
            line_index,
            column: start + 1,
            severity: Severity::Error,
            code: "E_UNCLOSED_BRACE",
            target: DiagnosticTarget::Text,
            message_en: "Unclosed `{` override block at end of line.".to_owned(),
            message_vi: "Thiếu dấu đóng `}` ở cuối dòng.".to_owned(),
            fix: (!in_bang).then(|| SuggestedFix {
                target: DiagnosticTarget::Text,
                start: len,
                end: len,
                replacement: "}".to_owned(),
                is_safe: true,
            }),
        });
    }

    // 2. Check `!expr!` inline Lua expressions (balance + real Lua 5.1 parse).
    let mut bang_start: Option<usize> = None;
    for (pos, &b) in bytes.iter().enumerate() {
        if b == b'!' {
            if let Some(start) = bang_start.take() {
                let expr = &text[start + 1..pos];
                if expr.trim().is_empty() {
                    out.push(Diagnostic {
                        line_index,
                        column: start + 1,
                        severity: Severity::Warning,
                        code: "W_EMPTY_INLINE_LUA",
                        target: DiagnosticTarget::Text,
                        message_en: "Empty `!!` inline Lua expression.".to_owned(),
                        message_vi: "Biểu thức Lua nội tuyến `!!` rỗng.".to_owned(),
                        fix: None,
                    });
                } else {
                    lint_lua_code(line_index, expr, start + 1, true, user_defined, out);
                }
            } else {
                bang_start = Some(pos);
            }
        }
    }
    if let Some(start) = bang_start {
        out.push(Diagnostic {
            line_index,
            column: start + 1,
            severity: Severity::Error,
            code: "E_UNBALANCED_BANG",
            target: DiagnosticTarget::Text,
            message_en: "Unclosed `!` inline Lua expression.".to_owned(),
            message_vi: "Thiếu dấu `!` đóng biểu thức Lua nội tuyến.".to_owned(),
            fix: None,
        });
    }

    // 3. Check `$var` inline variables outside `!expr!`.
    let mut in_expr = false;
    let mut p = 0usize;
    while p < len {
        if bytes[p] == b'!' {
            in_expr = !in_expr;
            p += 1;
            continue;
        }
        if !in_expr && bytes[p] == b'$' {
            let var_start = p + 1;
            let mut var_end = var_start;
            while var_end < len
                && (bytes[var_end].is_ascii_alphanumeric() || bytes[var_end] == b'_')
            {
                var_end += 1;
            }
            if var_end > var_start {
                let name = &text[var_start..var_end];
                let lower = name.to_ascii_lowercase();
                if !KNOWN_INLINE_VARS.contains(&lower.as_str()) {
                    let suggestion = closest_match(&lower, KNOWN_INLINE_VARS, 2);
                    let sug_en =
                        suggestion.map_or_else(String::new, |s| format!(" (did you mean `${s}`?)"));
                    let sug_vi =
                        suggestion.map_or_else(String::new, |s| format!(" (ý bạn là `${s}`?)"));
                    out.push(Diagnostic {
                        line_index,
                        column: p + 1,
                        severity: Severity::Warning,
                        code: "W_UNKNOWN_VAR",
                        target: DiagnosticTarget::Text,
                        message_en: format!("Unknown template inline variable `${name}`{sug_en}"),
                        message_vi: format!("Biến template không xác định `${name}`{sug_vi}"),
                        fix: suggestion.map(|s| SuggestedFix {
                            target: DiagnosticTarget::Text,
                            start: var_start,
                            end: var_end,
                            replacement: s.to_owned(),
                            is_safe: false,
                        }),
                    });
                }
                p = var_end;
                continue;
            }
        }
        p += 1;
    }
}

#[allow(clippy::too_many_lines)]
fn lint_override_block(
    line_index: usize,
    inner: &str,
    base_offset: usize,
    out: &mut Vec<Diagnostic>,
) {
    let bytes = inner.as_bytes();
    let len = bytes.len();

    // Check missing backslash directly after `{` (e.g. `{fs20}` or `{blur2}`).
    let trimmed_start = bytes
        .iter()
        .position(|b| !b.is_ascii_whitespace())
        .unwrap_or(len);
    if trimmed_start < len && bytes[trimmed_start] != b'\\' && bytes[trimmed_start] != b'!' {
        let rest = &inner[trimmed_start..];
        if let Some(tag_name) = match_known_tag_prefix(rest) {
            let after_tag = &rest[tag_name.len()..];
            if after_tag.starts_with(|c: char| {
                c.is_ascii_digit() || c == '(' || c == '-' || c == '&' || c == '$' || c == '!'
            }) {
                out.push(Diagnostic {
                    line_index,
                    column: base_offset + trimmed_start + 1,
                    severity: Severity::Error,
                    code: "E_MISSING_BACKSLASH",
                    target: DiagnosticTarget::Text,
                    message_en: format!("Missing `\\` before tag `{tag_name}`."),
                    message_vi: format!("Thiếu dấu `\\` trước thẻ `{tag_name}`."),
                    fix: Some(SuggestedFix {
                        target: DiagnosticTarget::Text,
                        start: base_offset + trimmed_start,
                        end: base_offset + trimmed_start,
                        replacement: "\\".to_owned(),
                        is_safe: true,
                    }),
                });
            }
        }
    }

    // Check paren balance inside the override block (ignoring `!expr!`).
    let mut paren_depth = 0i32;
    let mut last_open_paren = 0usize;
    let mut in_bang = false;
    for (idx, &b) in bytes.iter().enumerate() {
        if b == b'!' {
            in_bang = !in_bang;
            continue;
        }
        if in_bang {
            continue;
        }
        if b == b'(' {
            if paren_depth == 0 {
                last_open_paren = idx;
            }
            paren_depth += 1;
        } else if b == b')' {
            if paren_depth == 0 {
                out.push(Diagnostic {
                    line_index,
                    column: base_offset + idx + 1,
                    severity: Severity::Error,
                    code: "E_UNBALANCED_PAREN",
                    target: DiagnosticTarget::Text,
                    message_en: "Stray closing `)` inside override block.".to_owned(),
                    message_vi: "Dư dấu đóng `)` trong khối thẻ override.".to_owned(),
                    fix: None,
                });
            } else {
                paren_depth -= 1;
            }
        }
    }
    if paren_depth > 0 {
        let safe_fix = (paren_depth == 1).then(|| SuggestedFix {
            target: DiagnosticTarget::Text,
            start: base_offset + len,
            end: base_offset + len,
            replacement: ")".to_owned(),
            is_safe: true,
        });
        out.push(Diagnostic {
            line_index,
            column: base_offset + last_open_paren + 1,
            severity: Severity::Error,
            code: "E_UNBALANCED_PAREN",
            target: DiagnosticTarget::Text,
            message_en: "Unclosed `(` inside override block.".to_owned(),
            message_vi: "Thiếu dấu đóng `)` trong khối thẻ override.".to_owned(),
            fix: safe_fix,
        });
    }

    // Walk backslash-prefixed tags outside `!expr!`.
    let mut i = 0usize;
    let mut in_expr = false;
    while i < len {
        if bytes[i] == b'!' {
            in_expr = !in_expr;
            i += 1;
            continue;
        }
        if in_expr || bytes[i] != b'\\' {
            i += 1;
            continue;
        }
        // Doubled backslash `\\`
        if i + 1 < len && bytes[i + 1] == b'\\' {
            out.push(Diagnostic {
                line_index,
                column: base_offset + i + 1,
                severity: Severity::Error,
                code: "E_DOUBLE_BACKSLASH",
                target: DiagnosticTarget::Text,
                message_en: "Doubled backslash `\\\\` before override tag.".to_owned(),
                message_vi: "Dư dấu gạch chéo ngược kép `\\\\` trước thẻ.".to_owned(),
                fix: Some(SuggestedFix {
                    target: DiagnosticTarget::Text,
                    start: base_offset + i,
                    end: base_offset + i + 2,
                    replacement: "\\".to_owned(),
                    is_safe: true,
                }),
            });
            i += 2;
            continue;
        }
        let tag_start = i + 1;
        let rest = &inner[tag_start..];
        if let Some(tag) = match_known_tag_prefix(rest) {
            let arg_start = tag_start + tag.len();
            let arg_end = find_tag_arg_end(inner, arg_start);
            let arg = &inner[arg_start..arg_end];
            lint_known_tag_args(line_index, tag, arg, base_offset + arg_start, out);
            i = arg_end;
        } else {
            // Read identifier letters for unknown tag diagnostic.
            let mut end = tag_start;
            while end < len && bytes[end].is_ascii_alphabetic() {
                end += 1;
            }
            if end > tag_start {
                let unknown = &inner[tag_start..end];
                let suggestion = closest_match(unknown, KNOWN_ASS_TAGS, 2);
                let sug_en =
                    suggestion.map_or_else(String::new, |s| format!(" (did you mean `\\{s}`?)"));
                let sug_vi =
                    suggestion.map_or_else(String::new, |s| format!(" (ý bạn là `\\{s}`?)"));
                out.push(Diagnostic {
                    line_index,
                    column: base_offset + i + 1,
                    severity: Severity::Warning,
                    code: "W_UNKNOWN_TAG",
                    target: DiagnosticTarget::Text,
                    message_en: format!("Unknown ASS tag `\\{unknown}`{sug_en}"),
                    message_vi: format!("Thẻ ASS không xác định `\\{unknown}`{sug_vi}"),
                    fix: suggestion.map(|s| SuggestedFix {
                        target: DiagnosticTarget::Text,
                        start: base_offset + tag_start,
                        end: base_offset + end,
                        replacement: s.to_owned(),
                        is_safe: false,
                    }),
                });
                i = end;
            } else {
                i += 1;
            }
        }
    }
}

fn find_tag_arg_end(inner: &str, start: usize) -> usize {
    let bytes = inner.as_bytes();
    let len = bytes.len();
    let mut i = start;
    let mut paren = 0i32;
    let mut in_bang = false;
    while i < len {
        match bytes[i] {
            b'!' => in_bang = !in_bang,
            b'(' if !in_bang => paren += 1,
            b')' if !in_bang => paren = (paren - 1).max(0),
            b'\\' if !in_bang && paren == 0 => break,
            _ => {}
        }
        i += 1;
    }
    i
}

#[allow(clippy::too_many_lines)]
fn lint_known_tag_args(
    line_index: usize,
    tag: &str,
    arg: &str,
    arg_offset: usize,
    out: &mut Vec<Diagnostic>,
) {
    // 1. Comma instead of dot in single numeric tag args (e.g. `\fs20,5`, `\blur1,5`).
    if matches!(
        tag,
        "fs" | "fscx"
            | "fscy"
            | "fsp"
            | "fr"
            | "frx"
            | "fry"
            | "frz"
            | "fax"
            | "fay"
            | "bord"
            | "xbord"
            | "ybord"
            | "shad"
            | "xshad"
            | "yshad"
            | "be"
            | "blur"
    ) && !arg.contains('$')
        && !arg.contains('!')
    {
        if let Some(comma_pos) = arg.find(',') {
            let before = &arg[..comma_pos];
            let after = &arg[comma_pos + 1..];
            if !before.is_empty()
                && !after.is_empty()
                && before.bytes().all(|b| b.is_ascii_digit() || b == b'-')
                && after.bytes().all(|b| b.is_ascii_digit())
            {
                out.push(Diagnostic {
                    line_index,
                    column: arg_offset + comma_pos + 1,
                    severity: Severity::Error,
                    code: "E_COMMA_DECIMAL",
                    target: DiagnosticTarget::Text,
                    message_en: format!("Decimal comma in `\\{tag}{arg}`; ASS requires a dot `.`."),
                    message_vi: format!(
                        "Dùng dấu phẩy thập phân trong `\\{tag}{arg}`; ASS yêu cầu dấu chấm `.`."
                    ),
                    fix: Some(SuggestedFix {
                        target: DiagnosticTarget::Text,
                        start: arg_offset + comma_pos,
                        end: arg_offset + comma_pos + 1,
                        replacement: ".".to_owned(),
                        is_safe: true,
                    }),
                });
                return;
            }
        }
        if !arg.is_empty() && arg.trim().parse::<f64>().is_err() {
            out.push(Diagnostic {
                line_index,
                column: arg_offset + 1,
                severity: Severity::Error,
                code: "E_NON_NUMERIC_ARG",
                target: DiagnosticTarget::Text,
                message_en: format!("Tag `\\{tag}` expects a numeric argument, found `{arg}`."),
                message_vi: format!("Thẻ `\\{tag}` yêu cầu tham số số, nhận được `{arg}`."),
                fix: None,
            });
        }
        return;
    }

    // 2. Parenthesized multi-arg tags: \pos, \org, \move, \fad, \fade, \t, \clip, \iclip.
    if !matches!(
        tag,
        "pos" | "org" | "move" | "fad" | "fade" | "t" | "clip" | "iclip"
    ) {
        return;
    }
    let trimmed = arg.trim();
    if !(trimmed.starts_with('(') && trimmed.ends_with(')')) {
        return; // Unbalanced parens already reported above
    }
    let inner = &trimmed[1..trimmed.len() - 1];
    let parts = split_top_level_commas(inner);

    match tag {
        "pos" | "org" | "fad" => {
            if parts.len() == 2 {
                check_numeric_tuple(line_index, tag, &parts, arg_offset, out);
            } else {
                out.push(Diagnostic {
                    line_index,
                    column: arg_offset + 1,
                    severity: Severity::Error,
                    code: "E_TAG_ARG_COUNT",
                    target: DiagnosticTarget::Text,
                    message_en: format!(
                        "`\\{tag}` requires exactly 2 arguments, found {}.",
                        parts.len()
                    ),
                    message_vi: format!("`\\{tag}` cần đúng 2 tham số, hiện có {}.", parts.len()),
                    fix: None,
                });
            }
        }
        "move" => {
            if parts.len() != 4 && parts.len() != 6 {
                out.push(Diagnostic {
                    line_index,
                    column: arg_offset + 1,
                    severity: Severity::Error,
                    code: "E_TAG_ARG_COUNT",
                    target: DiagnosticTarget::Text,
                    message_en: format!(
                        "`\\move` requires 4 or 6 arguments, found {}.",
                        parts.len()
                    ),
                    message_vi: format!("`\\move` cần 4 hoặc 6 tham số, hiện có {}.", parts.len()),
                    fix: None,
                });
            } else {
                check_numeric_tuple(line_index, tag, &parts, arg_offset, out);
            }
        }
        "fade" => {
            if parts.len() == 7 {
                check_numeric_tuple(line_index, tag, &parts, arg_offset, out);
            } else {
                out.push(Diagnostic {
                    line_index,
                    column: arg_offset + 1,
                    severity: Severity::Error,
                    code: "E_TAG_ARG_COUNT",
                    target: DiagnosticTarget::Text,
                    message_en: format!("`\\fade` requires 7 arguments, found {}.", parts.len()),
                    message_vi: format!("`\\fade` cần 7 tham số, hiện có {}.", parts.len()),
                    fix: None,
                });
            }
        }
        "t" => {
            if parts.is_empty() || parts.len() > 4 {
                out.push(Diagnostic {
                    line_index,
                    column: arg_offset + 1,
                    severity: Severity::Error,
                    code: "E_TAG_ARG_COUNT",
                    target: DiagnosticTarget::Text,
                    message_en: format!("`\\t` expects 1 to 4 arguments, found {}.", parts.len()),
                    message_vi: format!("`\\t` cần từ 1 đến 4 tham số, hiện có {}.", parts.len()),
                    fix: None,
                });
            } else if let Some(last) = parts.last()
                && !last.contains('\\')
                && !last.contains('!')
                && !last.contains('$')
            {
                out.push(Diagnostic {
                    line_index,
                    column: arg_offset + 1,
                    severity: Severity::Warning,
                    code: "W_EMPTY_T",
                    target: DiagnosticTarget::Text,
                    message_en: "`\\t(...)` contains no override tags to animate.".to_owned(),
                    message_vi: "`\\t(...)` không chứa thẻ override nào để tạo chuyển động."
                        .to_owned(),
                    fix: None,
                });
            }
        }
        "clip" | "iclip" => {
            // Valid: 4 numbers (rect clip), 1 vector drawing, or (scale, vector drawing) = 2 args.
            if parts.len() == 4 {
                check_numeric_tuple(line_index, tag, &parts, arg_offset, out);
            } else if parts.len() != 1 && parts.len() != 2 {
                out.push(Diagnostic {
                    line_index,
                    column: arg_offset + 1,
                    severity: Severity::Error,
                    code: "E_TAG_ARG_COUNT",
                    target: DiagnosticTarget::Text,
                    message_en: format!(
                        "`\\{tag}` expects 4 coordinates or a vector drawing, found {} arguments.",
                        parts.len()
                    ),
                    message_vi: format!(
                        "`\\{tag}` cần 4 tọa độ hoặc đường vẽ vector, hiện có {} tham số.",
                        parts.len()
                    ),
                    fix: None,
                });
            }
        }
        _ => {}
    }
}

fn check_numeric_tuple(
    line_index: usize,
    tag: &str,
    parts: &[&str],
    arg_offset: usize,
    out: &mut Vec<Diagnostic>,
) {
    for p in parts {
        let t = p.trim();
        if t.contains('$') || t.contains('!') {
            continue; // dynamic template expression
        }
        if t.parse::<f64>().is_err() {
            out.push(Diagnostic {
                line_index,
                column: arg_offset + 1,
                severity: Severity::Error,
                code: "E_NON_NUMERIC_ARG",
                target: DiagnosticTarget::Text,
                message_en: format!("Non-numeric argument `{t}` in `\\{tag}`."),
                message_vi: format!("Tham số không phải số `{t}` trong `\\{tag}`."),
                fix: None,
            });
        }
    }
}

fn split_top_level_commas(s: &str) -> Vec<&str> {
    let mut parts = Vec::new();
    let bytes = s.as_bytes();
    let mut start = 0usize;
    let mut paren = 0i32;
    let mut in_bang = false;
    for (i, &b) in bytes.iter().enumerate() {
        match b {
            b'!' => in_bang = !in_bang,
            b'(' if !in_bang => paren += 1,
            b')' if !in_bang => paren = (paren - 1).max(0),
            b',' if !in_bang && paren == 0 => {
                parts.push(&s[start..i]);
                start = i + 1;
            }
            _ => {}
        }
    }
    if !s.is_empty() || !parts.is_empty() {
        parts.push(&s[start..]);
    }
    parts
}

// ===========================================================================
// Lua 5.1 / `LuaJIT` syntax & semantic checker (via `full_moon`)
// ===========================================================================

#[allow(clippy::too_many_lines)]
fn lint_lua_code(
    line_index: usize,
    code: &str,
    base_offset: usize,
    is_inline_expr: bool,
    user_defined: &HashSet<&str>,
    out: &mut Vec<Diagnostic>,
) {
    // 1. Detect C-style operators (`!=`, `&&`, `||`) outside string literals -> safe auto-fix!
    let mut found_c_op = false;
    let bytes = code.as_bytes();
    let len = bytes.len();
    let mut i = 0usize;
    let mut in_quote: u8 = 0;
    while i < len {
        let b = bytes[i];
        if in_quote != 0 {
            if b == b'\\' && i + 1 < len {
                i += 2;
                continue;
            }
            if b == in_quote {
                in_quote = 0;
            }
            i += 1;
            continue;
        }
        if b == b'"' || b == b'\'' {
            in_quote = b;
            i += 1;
            continue;
        }
        if i + 1 < len {
            let pair = &bytes[i..i + 2];
            let (code_id, repl, en, vi) = match pair {
                b"!=" => (
                    "E_LUA_C_NEQ",
                    "~=",
                    "Lua uses `~=` instead of `!=`.",
                    "Lua dùng `~=` thay cho `!=`.",
                ),
                b"&&" => (
                    "E_LUA_C_AND",
                    " and ",
                    "Lua uses `and` instead of `&&`.",
                    "Lua dùng `and` thay cho `&&`.",
                ),
                b"||" => (
                    "E_LUA_C_OR",
                    " or ",
                    "Lua uses `or` instead of `||`.",
                    "Lua dùng `or` thay cho `||`.",
                ),
                _ => ("", "", "", ""),
            };
            if !code_id.is_empty() {
                found_c_op = true;
                out.push(Diagnostic {
                    line_index,
                    column: base_offset + i + 1,
                    severity: Severity::Error,
                    code: code_id,
                    target: DiagnosticTarget::Text,
                    message_en: en.to_owned(),
                    message_vi: vi.to_owned(),
                    fix: Some(SuggestedFix {
                        target: DiagnosticTarget::Text,
                        start: base_offset + i,
                        end: base_offset + i + 2,
                        replacement: repl.to_owned(),
                        is_safe: true,
                    }),
                });
                i += 2;
                continue;
            }
        }
        i += 1;
    }

    // 2. Detect top-level `local` in `code` lines (not visible to `template` lines).
    if !is_inline_expr {
        let trimmed = code.trim_start();
        if trimmed.starts_with("local ") || trimmed.starts_with("local\t") {
            let col = code.len() - trimmed.len() + 1;
            out.push(Diagnostic {
                line_index,
                column: base_offset + col,
                severity: Severity::Warning,
                code: "W_LUA_TOP_LOCAL",
                target: DiagnosticTarget::Text,
                message_en: "`local` at top level of a `code` line is not visible to `template` lines.".to_owned(),
                message_vi: "Khai báo `local` ở đầu dòng `code` sẽ không dùng được trong các dòng `template`.".to_owned(),
                fix: None,
            });
        }
    }

    // 3. Real Lua 5.1 syntax parse with `full_moon`.
    if !found_c_op {
        let wrapped = if is_inline_expr {
            format!("return ({code})")
        } else {
            code.to_owned()
        };
        if let Err(errors) = full_moon::parse(&wrapped) {
            for err in errors.iter().take(2) {
                let err_msg = err.to_string();
                out.push(Diagnostic {
                    line_index,
                    column: base_offset + 1,
                    severity: Severity::Error,
                    code: "E_LUA_SYNTAX",
                    target: DiagnosticTarget::Text,
                    message_en: format!("Lua syntax error: {err_msg}"),
                    message_vi: format!("Lỗi cú pháp Lua: {err_msg}"),
                    fix: None,
                });
            }
        }
    }

    // 4. Flag calls to unknown global functions not in Lua 5.1 / kara-templater / user code.
    check_unknown_lua_calls(line_index, code, base_offset, user_defined, out);
}

fn collect_defined_functions<'a>(code: &'a str, out: &mut HashSet<&'a str>) {
    let mut rest = code;
    while let Some(pos) = rest.find("function ") {
        let after = rest[pos + 9..].trim_start();
        let end = after
            .bytes()
            .position(|b| !(b.is_ascii_alphanumeric() || b == b'_'))
            .unwrap_or(after.len());
        if end > 0 {
            out.insert(&after[..end]);
        }
        rest = &after[end..];
    }
    // Also detect simple global assignments `name = function(` or `name = ...`.
    for stmt in code.split(';') {
        let t = stmt.trim();
        if let Some((lhs, _)) = t.split_once('=') {
            let name = lhs.trim();
            if !name.is_empty()
                && !name.starts_with("local ")
                && name.bytes().all(|b| b.is_ascii_alphanumeric() || b == b'_')
            {
                out.insert(name);
            }
        }
    }
}

fn check_unknown_lua_calls(
    line_index: usize,
    code: &str,
    base_offset: usize,
    user_defined: &HashSet<&str>,
    out: &mut Vec<Diagnostic>,
) {
    let bytes = code.as_bytes();
    let len = bytes.len();
    let mut i = 0usize;
    let mut in_quote: u8 = 0;

    while i < len {
        let b = bytes[i];
        if in_quote != 0 {
            if b == b'\\' && i + 1 < len {
                i += 2;
                continue;
            }
            if b == in_quote {
                in_quote = 0;
            }
            i += 1;
            continue;
        }
        if b == b'"' || b == b'\'' {
            in_quote = b;
            i += 1;
            continue;
        }
        if b.is_ascii_alphabetic() || b == b'_' {
            let start = i;
            while i < len && (bytes[i].is_ascii_alphanumeric() || bytes[i] == b'_') {
                i += 1;
            }
            let ident = &code[start..i];
            // Skip member accesses `.foo(` or `:foo(` and `function foo(`.
            let prev_non_ws = bytes[..start]
                .iter()
                .rposition(|c| !c.is_ascii_whitespace())
                .map(|p| bytes[p]);
            if matches!(prev_non_ws, Some(b'.' | b':')) {
                continue;
            }
            if code[..start].trim_end().ends_with("function") {
                continue;
            }
            let next_non_ws = bytes[i..]
                .iter()
                .position(|c| !c.is_ascii_whitespace())
                .map(|p| bytes[i + p]);
            if next_non_ws == Some(b'(')
                && !matches!(
                    ident,
                    "if" | "elseif" | "while" | "return" | "not" | "and" | "or" | "function"
                )
                && !KNOWN_LUA_GLOBALS.contains(&ident)
                && !user_defined.contains(ident)
            {
                let suggestion = closest_match(ident, KNOWN_LUA_GLOBALS, 2);
                let sug_en =
                    suggestion.map_or_else(String::new, |s| format!(" (did you mean `{s}`?)"));
                let sug_vi = suggestion.map_or_else(String::new, |s| format!(" (ý bạn là `{s}`?)"));
                out.push(Diagnostic {
                    line_index,
                    column: base_offset + start + 1,
                    severity: Severity::Warning,
                    code: "W_LUA_UNKNOWN_FN",
                    target: DiagnosticTarget::Text,
                    message_en: format!("Call to unknown Lua function `{ident}`{sug_en}"),
                    message_vi: format!("Gọi hàm Lua không xác định `{ident}`{sug_vi}"),
                    fix: None,
                });
            }
            continue;
        }
        i += 1;
    }
}

// ===========================================================================
// Karaoke `\k` line checker
// ===========================================================================

fn has_karaoke_tag(text: &str) -> bool {
    let bytes = text.as_bytes();
    let mut i = 0usize;
    while i + 1 < bytes.len() {
        if bytes[i] == b'\\' && matches!(bytes[i + 1], b'k' | b'K') {
            return true;
        }
        i += 1;
    }
    false
}

fn lint_karaoke_line(line_index: usize, text: &str, out: &mut Vec<Diagnostic>) {
    let bytes = text.as_bytes();
    let len = bytes.len();
    let mut i = 0usize;
    while i < len {
        if bytes[i] == b'{' {
            let Some(rel) = bytes[i + 1..].iter().position(|&b| b == b'}') else {
                out.push(Diagnostic {
                    line_index,
                    column: i + 1,
                    severity: Severity::Error,
                    code: "E_KARA_UNCLOSED_BRACE",
                    target: DiagnosticTarget::Text,
                    message_en: "Unclosed `{` in karaoke line.".to_owned(),
                    message_vi: "Thiếu dấu `}` trong dòng karaoke.".to_owned(),
                    fix: None,
                });
                return;
            };
            let block = &text[i + 1..i + 1 + rel];
            let blk = block.as_bytes();
            let mut j = 0usize;
            while j + 1 < blk.len() {
                if blk[j] == b'\\' && matches!(blk[j + 1], b'k' | b'K') {
                    let mut arg_start = j + 2;
                    if arg_start < blk.len() && matches!(blk[arg_start], b'f' | b'o' | b't') {
                        arg_start += 1;
                    }
                    let arg_end = block[arg_start..]
                        .find('\\')
                        .map_or(block.len(), |p| arg_start + p);
                    let arg = block[arg_start..arg_end].trim();
                    if arg.is_empty() || arg.parse::<i64>().map_or(true, |v| v < 0) {
                        out.push(Diagnostic {
                            line_index,
                            column: i + 1 + j + 1,
                            severity: Severity::Error,
                            code: "E_KARA_BAD_K",
                            target: DiagnosticTarget::Text,
                            message_en: format!("Invalid karaoke duration `\\k` argument `{arg}`."),
                            message_vi: format!("Thời lượng karaoke `\\k` không hợp lệ: `{arg}`."),
                            fix: None,
                        });
                    }
                    j = arg_end;
                    continue;
                }
                j += 1;
            }
            i = i + 1 + rel + 1;
            continue;
        }
        i += 1;
    }
}

// ===========================================================================
// Helpers: tag prefix matcher & bounded Levenshtein closest match
// ===========================================================================

fn match_known_tag_prefix(s: &str) -> Option<&'static str> {
    let mut best: Option<&'static str> = None;
    for &tag in KNOWN_ASS_TAGS {
        if s.starts_with(tag) {
            let next = s.as_bytes().get(tag.len()).copied();
            // Ensure we don't match `\b` as prefix of `\blur` or `\f` as prefix of `\fad`.
            let boundary_ok = match next {
                None => true,
                Some(b) => !b.is_ascii_alphabetic(),
            };
            if boundary_ok && best.is_none_or(|b| tag.len() > b.len()) {
                best = Some(tag);
            }
        }
    }
    best
}

/// Returns the unique closest match in `candidates` within Levenshtein distance `<= max_dist`.
fn closest_match(
    query: &str,
    candidates: &[&'static str],
    max_dist: usize,
) -> Option<&'static str> {
    let mut best: Option<&'static str> = None;
    let mut best_d = max_dist + 1;
    let mut ties = 0usize;
    for &cand in candidates {
        let d = levenshtein(query, cand);
        if d < best_d {
            best_d = d;
            best = Some(cand);
            ties = 1;
        } else if d == best_d {
            ties += 1;
        }
    }
    if best_d <= max_dist && ties == 1 {
        best
    } else {
        None
    }
}

fn levenshtein(a: &str, b: &str) -> usize {
    let a = a.as_bytes();
    let b = b.as_bytes();
    if a.len().abs_diff(b.len()) > 3 {
        return 4;
    }
    let mut prev: [usize; 32] = [0; 32];
    let mut cur: [usize; 32] = [0; 32];
    let n = b.len().min(31);
    for (j, slot) in prev.iter_mut().enumerate().take(n + 1) {
        *slot = j;
    }
    for (i, &ca) in a.iter().take(31).enumerate() {
        cur[0] = i + 1;
        for (j, &cb) in b.iter().take(n).enumerate() {
            let cost = usize::from(ca != cb);
            cur[j + 1] = (prev[j + 1] + 1).min(cur[j] + 1).min(prev[j] + cost);
        }
        prev[..=n].copy_from_slice(&cur[..=n]);
    }
    prev[n]
}

#[cfg(test)]
mod tests {
    use super::*;
    use ass_core::parse_ass;
    use std::fmt::Write as _;

    fn script_from_events(events: &[(&str, &str, &str)]) -> ScriptFile {
        let mut s = String::from(
            "[Script Info]\nPlayResX: 1920\nPlayResY: 1080\n\n\
             [V4+ Styles]\n\
             Format: Name, Fontname, Fontsize, PrimaryColour, SecondaryColour, OutlineColour, BackColour, Bold, Italic, Underline, StrikeOut, ScaleX, ScaleY, Spacing, Angle, BorderStyle, Outline, Shadow, Alignment, MarginL, MarginR, MarginV, Encoding\n\
             Style: Default,Arial,48,&H00FFFFFF,&H000000FF,&H00000000,&H00000000,0,0,0,0,100,100,0,0,1,2,2,2,10,10,10,1\n\n\
             [Events]\n\
             Format: Layer, Start, End, Style, Name, MarginL, MarginR, MarginV, Effect, Text\n",
        );
        for (kind, effect, text) in events {
            let _ = writeln!(
                s,
                "{kind}: 0,0:00:00.00,0:00:05.00,Default,,0,0,0,{effect},{text}"
            );
        }
        parse_ass(&s).value
    }

    #[test]
    fn valid_templates_produce_zero_diagnostics() {
        let script = script_from_events(&[
            ("Comment", "code once", "function ease(t) return t * t end"),
            (
                "Comment",
                "template syl noblank",
                r"{\an5\pos($scenter,$smiddle)\fad(100,150)\t($start,$end,\fscx120\fscy120\blur2)}!ease(0.5)!",
            ),
            (
                "Comment",
                "template line all",
                r"{\pos($center,$middle)\clip(0,0,1920,1080)\fade(255,0,255,0,200,800,1000)}",
            ),
            ("Comment", "mixin syl", r"{\be1\bord2\shad1\c&H00FFFF&}"),
            ("Dialogue", "karaoke", r"{\k25}Ka{\kf30}ra{\ko20}o{\K15}ke"),
        ]);
        let diags = lint_script(&script);
        assert!(diags.is_empty(), "expected 0 diagnostics, got: {diags:#?}");
    }

    #[test]
    #[allow(clippy::too_many_lines)]
    fn corpus_of_44_broken_templates_diagnostics_safe_fixes_and_idempotence() {
        let broken: &[(&str, &str, &str, &str)] = &[
            // (kind, effect, text, expected_diag_code)
            (
                "Comment",
                "template syl",
                r"{\pos(100,200)",
                "E_UNCLOSED_BRACE",
            ),
            (
                "Comment",
                "template syl",
                r"{\pos(100,200)}text}",
                "E_STRAY_BRACE",
            ),
            (
                "Comment",
                "template syl",
                r"{\pos(100,200{\blur2}",
                "E_UNCLOSED_BRACE",
            ),
            (
                "Comment",
                "template syl",
                r"{\pos(100,200}",
                "E_UNBALANCED_PAREN",
            ),
            (
                "Comment",
                "template syl",
                r"{\pos(100,200))}",
                "E_UNBALANCED_PAREN",
            ),
            ("Comment", "template syl", r"{\\fs30}", "E_DOUBLE_BACKSLASH"),
            (
                "Comment",
                "template syl",
                r"{\\blur3}",
                "E_DOUBLE_BACKSLASH",
            ),
            ("Comment", "template syl", r"{fs30}", "E_MISSING_BACKSLASH"),
            ("Comment", "template syl", r"{blur2}", "E_MISSING_BACKSLASH"),
            ("Comment", "template syl", r"{\fs20,5}", "E_COMMA_DECIMAL"),
            ("Comment", "template syl", r"{\blur1,25}", "E_COMMA_DECIMAL"),
            (
                "Comment",
                "template syl",
                r"{\fscx120,5}",
                "E_COMMA_DECIMAL",
            ),
            ("Comment", "template syl", r"{\bord2,5}", "E_COMMA_DECIMAL"),
            ("Comment", "template syl", r"{\fs20px}", "E_NON_NUMERIC_ARG"),
            ("Comment", "template syl", r"{\pos(100)}", "E_TAG_ARG_COUNT"),
            (
                "Comment",
                "template syl",
                r"{\pos(100,200,300)}",
                "E_TAG_ARG_COUNT",
            ),
            ("Comment", "template syl", r"{\org(100)}", "E_TAG_ARG_COUNT"),
            ("Comment", "template syl", r"{\fad(100)}", "E_TAG_ARG_COUNT"),
            (
                "Comment",
                "template syl",
                r"{\move(0,0,100)}",
                "E_TAG_ARG_COUNT",
            ),
            (
                "Comment",
                "template syl",
                r"{\move(0,0,100,100,50)}",
                "E_TAG_ARG_COUNT",
            ),
            (
                "Comment",
                "template syl",
                r"{\fade(255,0,255,0,100)}",
                "E_TAG_ARG_COUNT",
            ),
            (
                "Comment",
                "template syl",
                r"{\clip(0,0,100)}",
                "E_TAG_ARG_COUNT",
            ),
            (
                "Comment",
                "template syl",
                r"{\pos(100,foo)}",
                "E_NON_NUMERIC_ARG",
            ),
            ("Comment", "template syl", r"{\t(0,500)}", "W_EMPTY_T"),
            (
                "Comment",
                "template syl",
                r"{\t(0,1,2,3,4)}",
                "E_TAG_ARG_COUNT",
            ),
            ("Comment", "template syl", r"{}", "W_EMPTY_BLOCK"),
            ("Comment", "template syl", r"{\blru2}", "W_UNKNOWN_TAG"),
            ("Comment", "template syl", r"{\fscz100}", "W_UNKNOWN_TAG"),
            (
                "Comment",
                "template syl",
                r"{\pos($scentr,$smiddle)}",
                "W_UNKNOWN_VAR",
            ),
            (
                "Comment",
                "template syl",
                r"{\t($strt,$end,\blur2)}",
                "W_UNKNOWN_VAR",
            ),
            (
                "Comment",
                "template syl",
                r"{\pos(!100 + 20,200)}",
                "E_UNBALANCED_BANG",
            ),
            (
                "Comment",
                "template syl",
                r"{\pos(!!,200)}",
                "W_EMPTY_INLINE_LUA",
            ),
            ("Comment", "template sly", r"{\blur2}", "E_UNKNOWN_MODIFIER"),
            (
                "Comment",
                "template syl notex",
                r"{\blur2}",
                "E_UNKNOWN_MODIFIER",
            ),
            (
                "Comment",
                "template syl keeptag",
                r"{\blur2}",
                "E_UNKNOWN_MODIFIER",
            ),
            (
                "Comment",
                "template once syl",
                r"{\blur2}",
                "W_CONFLICT_MODS",
            ),
            ("Comment", "code once notext", "x = 1", "W_CONFLICT_MODS"),
            (
                "Comment",
                "template char notext",
                r"{\blur2}",
                "W_CONFLICT_MODS",
            ),
            (
                "Comment",
                "code syl",
                "if x != 0 then y = 1 end",
                "E_LUA_C_NEQ",
            ),
            (
                "Comment",
                "code syl",
                "if a && b then y = 1 end",
                "E_LUA_C_AND",
            ),
            (
                "Comment",
                "code syl",
                "if a || b then y = 1 end",
                "E_LUA_C_OR",
            ),
            (
                "Comment",
                "code once",
                "local helper = 10",
                "W_LUA_TOP_LOCAL",
            ),
            (
                "Comment",
                "code syl",
                "if x == 1 then y = 2",
                "E_LUA_SYNTAX",
            ),
            (
                "Comment",
                "code syl",
                "x = nonExistentFunc(1)",
                "W_LUA_UNKNOWN_FN",
            ),
            (
                "Dialogue",
                "karaoke",
                r"{\k-10}Bad{\k}Empty",
                "E_KARA_BAD_K",
            ),
        ];
        assert!(broken.len() >= 44);

        let ev_tuples: Vec<(&str, &str, &str)> =
            broken.iter().map(|&(k, e, t, _)| (k, e, t)).collect();
        let mut script = script_from_events(&ev_tuples);
        let diags = lint_script(&script);

        for (idx, &(_, _, _, expected_code)) in broken.iter().enumerate() {
            assert!(
                diags
                    .iter()
                    .any(|d| d.line_index == idx && d.code == expected_code),
                "line {idx} missing expected diagnostic {expected_code}; got: {:?}",
                diags
                    .iter()
                    .filter(|d| d.line_index == idx)
                    .collect::<Vec<_>>()
            );
        }

        // Apply safe fixes once, then verify idempotence (second fix_all_safe produces 0 fixes)
        let first = fix_all_safe(&script);
        assert!(
            first.fixed_lines >= 14,
            "expected >= 14 safely fixed lines, got {}",
            first.fixed_lines
        );
        first.command.apply(&mut script).expect("apply safe fixes");

        // Non-template Dialogue karaoke line must NOT be touched by fix_all_safe
        let last_idx = broken.len() - 1;
        assert_eq!(
            script
                .events_section()
                .unwrap()
                .get_event(last_idx)
                .unwrap()
                .text(),
            r"{\k-10}Bad{\k}Empty"
        );

        let second = fix_all_safe(&script);
        assert_eq!(
            second.fixed_lines, 0,
            "fix_all_safe must be idempotent, second pass changed: {:?}",
            second.diffs
        );
    }
}
