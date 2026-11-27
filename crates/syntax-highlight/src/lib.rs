//! Incremental ASS edit-box syntax tokenizer and Aegisub 3.5.1 grid tag-display formatter.
//!
//! Mirrors Aegisub:
//! - `libaegisub/ass/dialogue_parser.cpp` (`TokenizeDialogueBody` / `MarkDrawings`)
//! - `src/subs_edit_ctrl.cpp` (syntax highlighting token categories)
//! - `src/base_grid.cpp` (`Subtitle/Grid/Hide Overrides`: show / simplify / hide + truncation)
//!
//! Hot paths are zero-allocation when callers reuse the output `Vec<TokenSpan>` / `String`.

#![forbid(unsafe_code)]
#![cfg_attr(not(test), deny(clippy::unwrap_used, clippy::expect_used))]

/// Syntax token category matching Aegisub's edit-box highlighter.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum TokenKind {
    /// Normal visible subtitle text.
    PlainText,
    /// `{` or `}` delimiter of an override block.
    OverrideBrace,
    /// Known ASS override tag name (e.g. `\pos`, `\fs`, `\1c`, `\an`).
    TagName,
    /// Parameter / value following a tag name (e.g. `(100,200)`, `48`, `&H00FFFFFF&`).
    TagValue,
    /// Karaoke timing tag (`\k`, `\K`, `\kf`, `\ko`, `\kt`).
    KaraokeTag,
    /// Special escape sequence in dialogue text (`\N`, `\n`, `\h`).
    Escape,
    /// Comment text inside `{...}` (text before any `\` tag or inside a pure comment block).
    Comment,
    /// Vector drawing commands/coordinates inside `\p1..` mode or vector `\clip`/`\iclip`.
    DrawingCommand,
    /// Syntax error (unclosed `{`, stray `}`, nested `{`, or unrecognized tag name).
    Error,
}

/// Byte-range token span produced by the tokenizer.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct TokenSpan {
    pub start: usize,
    pub end: usize,
    pub kind: TokenKind,
    /// Whether `\pN` (N > 0) drawing mode is active immediately *after* this token.
    /// Used by [`IncrementalTokenizer`] to re-lex only the edited region safely.
    pub drawing_after: bool,
}

/// Tokenizes a full ASS dialogue text string into `out` (clearing `out` first, reusing capacity).
pub fn tokenize_into(text: &str, out: &mut Vec<TokenSpan>) {
    out.clear();
    tokenize_slice_with_state(text, 0, false, out);
}

/// Convenience wrapper allocating a fresh `Vec<TokenSpan>`.
#[must_use]
pub fn tokenize(text: &str) -> Vec<TokenSpan> {
    let mut out = Vec::with_capacity(text.len() / 8 + 4);
    tokenize_into(text, &mut out);
    out
}

/// Stateful incremental tokenizer for the active edit-box line.
///
/// After a small edit (typing/deleting characters), [`Self::re_tokenize_edit`] re-lexes only the
/// enclosing override block / local text run and shifts trailing token byte offsets in place,
/// falling back to a full tail re-lex only if the edit toggled `\pN` drawing mode or broke a
/// `{...}` brace balance.
#[derive(Debug, Default, Clone)]
pub struct IncrementalTokenizer {
    tokens: Vec<TokenSpan>,
    scratch: Vec<TokenSpan>,
}

impl IncrementalTokenizer {
    /// Creates an empty incremental tokenizer.
    #[must_use]
    pub fn new() -> Self {
        Self::default()
    }

    /// Returns the current token slice.
    #[must_use]
    pub fn tokens(&self) -> &[TokenSpan] {
        &self.tokens
    }

    /// Full tokenization of `text` (resets state, reuses buffer).
    pub fn set_text(&mut self, text: &str) {
        tokenize_into(text, &mut self.tokens);
    }

    /// Incrementally updates tokens after `text[edit_start..old_end]` was replaced with
    /// `new_text[edit_start..new_end]`.
    pub fn re_tokenize_edit(
        &mut self,
        new_text: &str,
        edit_start: usize,
        old_end: usize,
        new_end: usize,
    ) {
        if self.tokens.is_empty() {
            self.set_text(new_text);
            return;
        }

        let delta: isize = new_end.cast_signed() - old_end.cast_signed();

        // 1. Find the first token whose `end > edit_start` (binary search: O(log T)).
        let mut first_tok = self.tokens.partition_point(|t| t.end <= edit_start);
        // Step back one token if the edit is adjacent to a PlainText/Escape (e.g. `\` + `N`),
        // but NEVER start re-lexing on a closing `}` without its matching `{`.
        if first_tok > 0 && self.tokens[first_tok - 1].kind != TokenKind::OverrideBrace {
            first_tok = first_tok.saturating_sub(1);
        }
        while first_tok > 0 && is_inside_override_token(self.tokens[first_tok].kind) {
            first_tok = first_tok.saturating_sub(1);
        }
        if first_tok < self.tokens.len()
            && self.tokens[first_tok].kind == TokenKind::OverrideBrace
            && new_text.as_bytes().get(self.tokens[first_tok].start) == Some(&b'}')
        {
            while first_tok > 0 {
                first_tok = first_tok.saturating_sub(1);
                if self.tokens[first_tok].kind == TokenKind::OverrideBrace
                    && new_text.as_bytes().get(self.tokens[first_tok].start) == Some(&b'{')
                {
                    break;
                }
            }
        }

        let relex_start = self.tokens.get(first_tok).map_or(0, |t| t.start);
        let drawing_before = if first_tok > 0 {
            self.tokens[first_tok - 1].drawing_after
        } else {
            false
        };

        // 2. Find the first untouched token strictly after `old_end` at a safe top-level boundary
        // (PlainText / Escape / DrawingCommand / opening `{`).
        let mut after_tok = self.tokens.partition_point(|t| t.start <= old_end);
        while after_tok < self.tokens.len()
            && !is_safe_sync_start(new_text, self.tokens[after_tok], delta)
        {
            after_tok += 1;
        }

        let relex_end_new = if after_tok < self.tokens.len() {
            self.tokens[after_tok]
                .start
                .saturating_add_signed(delta)
                .min(new_text.len())
        } else {
            new_text.len()
        };

        if relex_start > relex_end_new
            || !new_text.is_char_boundary(relex_start)
            || !new_text.is_char_boundary(relex_end_new)
        {
            self.set_text(new_text);
            return;
        }

        // 3. Re-lex only `new_text[relex_start..relex_end_new]`.
        self.scratch.clear();
        let end_drawing = tokenize_slice_with_state(
            &new_text[relex_start..relex_end_new],
            relex_start,
            drawing_before,
            &mut self.scratch,
        );

        // 4. Verify whether drawing state at sync point matches what `after_tok` expected.
        let expected_drawing = if after_tok > 0 {
            self.tokens[after_tok - 1].drawing_after
        } else {
            false
        };
        let sync_broken = after_tok < self.tokens.len() && end_drawing != expected_drawing;
        if sync_broken {
            // Re-lex the entire tail from `relex_start` to `new_text.len()`.
            self.scratch.clear();
            tokenize_slice_with_state(
                &new_text[relex_start..],
                relex_start,
                drawing_before,
                &mut self.scratch,
            );
            self.tokens.truncate(first_tok);
            self.tokens.extend_from_slice(&self.scratch);
            return;
        }

        // 5. Shift offsets of untouched trailing tokens and splice `scratch` in place.
        if delta != 0 {
            for tok in &mut self.tokens[after_tok..] {
                tok.start = tok.start.saturating_add_signed(delta);
                tok.end = tok.end.saturating_add_signed(delta);
            }
        }
        self.tokens
            .splice(first_tok..after_tok, self.scratch.iter().copied());
    }
}

#[inline]
const fn is_inside_override_token(kind: TokenKind) -> bool {
    matches!(
        kind,
        TokenKind::TagName
            | TokenKind::TagValue
            | TokenKind::KaraokeTag
            | TokenKind::Comment
            | TokenKind::Error
    )
}

#[inline]
fn is_safe_sync_start(new_text: &str, tok: TokenSpan, delta: isize) -> bool {
    let new_start = tok.start.saturating_add_signed(delta);
    if new_start >= new_text.len() {
        return false;
    }
    match tok.kind {
        TokenKind::OverrideBrace => new_text.as_bytes()[new_start] == b'{',
        TokenKind::PlainText | TokenKind::Escape | TokenKind::DrawingCommand => true,
        _ => false,
    }
}

/// Core single-pass zero-alloc ASS dialogue tokenizer. Returns final `in_drawing` state.
fn tokenize_slice_with_state(
    slice: &str,
    base_offset: usize,
    mut in_drawing: bool,
    out: &mut Vec<TokenSpan>,
) -> bool {
    let bytes = slice.as_bytes();
    let len = bytes.len();
    let mut pos = 0usize;

    while pos < len {
        let b = bytes[pos];

        if b == b'{' {
            // Find matching `}` (flagging nested `{` or unclosed `{` as Error).
            if let Some(rel_close) = bytes[pos + 1..].iter().position(|&c| c == b'}') {
                let close_pos = pos + 1 + rel_close;
                out.push(TokenSpan {
                    start: base_offset + pos,
                    end: base_offset + pos + 1,
                    kind: TokenKind::OverrideBrace,
                    drawing_after: in_drawing,
                });
                tokenize_override_body(
                    &bytes[pos + 1..close_pos],
                    base_offset + pos + 1,
                    &mut in_drawing,
                    out,
                );
                out.push(TokenSpan {
                    start: base_offset + close_pos,
                    end: base_offset + close_pos + 1,
                    kind: TokenKind::OverrideBrace,
                    drawing_after: in_drawing,
                });
                pos = close_pos + 1;
            } else {
                // Unclosed `{` -> Error through end of slice.
                out.push(TokenSpan {
                    start: base_offset + pos,
                    end: base_offset + len,
                    kind: TokenKind::Error,
                    drawing_after: in_drawing,
                });
                break;
            }
            continue;
        }

        if b == b'}' {
            // Stray closing brace outside any override block.
            out.push(TokenSpan {
                start: base_offset + pos,
                end: base_offset + pos + 1,
                kind: TokenKind::Error,
                drawing_after: in_drawing,
            });
            pos += 1;
            continue;
        }

        if !in_drawing && b == b'\\' && pos + 1 < len {
            let next = bytes[pos + 1];
            if next == b'N' || next == b'n' || next == b'h' {
                out.push(TokenSpan {
                    start: base_offset + pos,
                    end: base_offset + pos + 2,
                    kind: TokenKind::Escape,
                    drawing_after: in_drawing,
                });
                pos += 2;
                continue;
            }
        }

        // Consume a maximal run of plain text (or drawing commands if `in_drawing`).
        let run_start = pos;
        pos += 1;
        while pos < len {
            let c = bytes[pos];
            if c == b'{' || c == b'}' {
                break;
            }
            if !in_drawing && c == b'\\' && pos + 1 < len {
                let next = bytes[pos + 1];
                if next == b'N' || next == b'n' || next == b'h' {
                    break;
                }
            }
            pos += 1;
        }
        out.push(TokenSpan {
            start: base_offset + run_start,
            end: base_offset + pos,
            kind: if in_drawing {
                TokenKind::DrawingCommand
            } else {
                TokenKind::PlainText
            },
            drawing_after: in_drawing,
        });
    }

    in_drawing
}

fn tokenize_override_body(
    body: &[u8],
    base: usize,
    in_drawing: &mut bool,
    out: &mut Vec<TokenSpan>,
) {
    let len = body.len();
    let mut pos = 0usize;

    while pos < len {
        if body[pos] == b'{' {
            out.push(TokenSpan {
                start: base + pos,
                end: base + pos + 1,
                kind: TokenKind::Error,
                drawing_after: *in_drawing,
            });
            pos += 1;
            continue;
        }

        if body[pos] != b'\\' {
            // Comment text before next `\` or `{`.
            let start = pos;
            while pos < len && body[pos] != b'\\' && body[pos] != b'{' {
                pos += 1;
            }
            out.push(TokenSpan {
                start: base + start,
                end: base + pos,
                kind: TokenKind::Comment,
                drawing_after: *in_drawing,
            });
            continue;
        }

        // Starts with `\`: find end of this tag (next `\` or `{`, respecting balanced `(...)`).
        let tag_start = pos;
        let mut scan = pos + 1;
        let mut paren_depth = 0usize;
        while scan < len {
            let c = body[scan];
            if c == b'(' {
                paren_depth += 1;
            } else if c == b')' && paren_depth > 0 {
                paren_depth -= 1;
            } else if paren_depth == 0 && (c == b'\\' || c == b'{') {
                break;
            }
            scan += 1;
        }
        let tag_end = scan;
        let raw_tag = &body[tag_start + 1..tag_end];

        if let Some((name_len, kind, is_p_tag, is_clip_tag)) = match_ass_tag(raw_tag) {
            let name_end = tag_start + 1 + name_len;
            if is_p_tag {
                // Parse drawing level digits.
                let mut val = 0u32;
                for &d in &body[name_end..tag_end] {
                    if d.is_ascii_digit() {
                        val = val.saturating_mul(10).saturating_add(u32::from(d - b'0'));
                    } else {
                        break;
                    }
                }
                *in_drawing = val > 0;
            }

            out.push(TokenSpan {
                start: base + tag_start,
                end: base + name_end,
                kind,
                drawing_after: *in_drawing,
            });

            if name_end < tag_end {
                let arg_slice = &body[name_end..tag_end];
                // Vector `\clip(m ...)` / `\iclip(scale, m ...)` highlights drawing payload as DrawingCommand.
                let val_kind = if is_clip_tag && is_vector_clip_arg(arg_slice) {
                    TokenKind::DrawingCommand
                } else {
                    TokenKind::TagValue
                };
                out.push(TokenSpan {
                    start: base + name_end,
                    end: base + tag_end,
                    kind: val_kind,
                    drawing_after: *in_drawing,
                });
            }
        } else {
            out.push(TokenSpan {
                start: base + tag_start,
                end: base + tag_end,
                kind: TokenKind::Error,
                drawing_after: *in_drawing,
            });
        }

        pos = tag_end;
    }
}

/// Matches a known ASS tag prefix (longest match first, matching libass / Aegisub).
/// Returns `(name_len_without_backslash, TokenKind, is_drawing_p_tag, is_clip_tag)`.
#[inline]
fn match_ass_tag(raw: &[u8]) -> Option<(usize, TokenKind, bool, bool)> {
    // Ordered longest-first within each starting letter.
    const KNOWN_TAGS: &[&[u8]] = &[
        b"alpha", b"an", b"a", b"blur", b"bord", b"be", b"b", b"clip", b"c", b"fade", b"fad",
        b"fscx", b"fscy", b"fsp", b"frx", b"fry", b"frz", b"fr", b"fax", b"fay", b"fe", b"fn",
        b"fs", b"iclip", b"i", b"kf", b"ko", b"kt", b"k", b"K", b"move", b"org", b"pbo", b"pos",
        b"p", b"q", b"r", b"shad", b"s", b"t", b"u", b"xbord", b"xshad", b"ybord", b"yshad", b"1a",
        b"2a", b"3a", b"4a", b"1c", b"2c", b"3c", b"4c",
    ];

    for &tag in KNOWN_TAGS {
        if raw.starts_with(tag) {
            // Disambiguate single-letter tags like `\b`, `\i`, `\s`, `\u`, `\p`, `\q`, `\k`, `\c`:
            // e.g. `\bold` should NOT match `\b` + `old`; next byte must be non-alphabetic
            // (except `\fn` and `\r` which take arbitrary text names).
            let rest = &raw[tag.len()..];
            let takes_text = tag == b"fn" || tag == b"r";
            if !takes_text && !rest.is_empty() && rest[0].is_ascii_alphabetic() {
                // Allow hex after `\c` / `\1c`? Those start with `&H`, not ascii alpha.
                continue;
            }
            let is_karaoke =
                tag == b"k" || tag == b"K" || tag == b"kf" || tag == b"ko" || tag == b"kt";
            let is_p = tag == b"p";
            let is_clip = tag == b"clip" || tag == b"iclip";
            let kind = if is_karaoke {
                TokenKind::KaraokeTag
            } else {
                TokenKind::TagName
            };
            return Some((tag.len(), kind, is_p, is_clip));
        }
    }
    None
}

#[inline]
fn is_vector_clip_arg(arg: &[u8]) -> bool {
    // Vector clip contains drawing command letters like `m`, `l`, `b`, `s` inside `(...)`.
    arg.iter().any(|&c| matches!(c, b'm' | b'M' | b'b' | b'B'))
}

/// Grid override-tag display mode (mirrors Aegisub `Subtitle/Grid/Hide Overrides`).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum TagDisplayMode {
    /// Show raw ASS override blocks verbatim (`0` in Aegisub).
    Show,
    /// Replace each `{...}` override block with `simplify_marker` (`1` in Aegisub).
    Simplify,
    /// Hide `{...}` override blocks completely (`2` in Aegisub).
    Hide,
}

/// Configuration for formatting a dialogue line for the subtitle grid.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct GridDisplayConfig {
    pub mode: TagDisplayMode,
    /// Replacement character used in [`TagDisplayMode::Simplify`] (Aegisub default: `'★'` / `'◊'`).
    pub simplify_marker: char,
    /// Maximum visible characters before appending `ellipsis` (`None` = no truncation).
    pub max_chars: Option<usize>,
    /// Ellipsis suffix appended when truncated (default `"..."`).
    pub ellipsis: &'static str,
}

impl Default for GridDisplayConfig {
    fn default() -> Self {
        Self {
            mode: TagDisplayMode::Simplify,
            simplify_marker: '★',
            max_chars: Some(256),
            ellipsis: "...",
        }
    }
}

/// Formats `raw` ASS event text into `out` according to `cfg` (reusing `out`'s allocation).
pub fn format_grid_text_into(raw: &str, cfg: &GridDisplayConfig, out: &mut String) {
    out.clear();
    let max_chars = cfg.max_chars.unwrap_or(usize::MAX);
    if max_chars == 0 {
        return;
    }

    match cfg.mode {
        TagDisplayMode::Show => {
            append_truncated(raw, max_chars, cfg.ellipsis, out);
        }
        TagDisplayMode::Simplify | TagDisplayMode::Hide => {
            let bytes = raw.as_bytes();
            let len = bytes.len();
            let mut pos = 0usize;
            let mut emitted_chars = 0usize;
            let mut truncated = false;

            while pos < len {
                if bytes[pos] == b'{' {
                    if let Some(rel_close) = bytes[pos + 1..].iter().position(|&c| c == b'}') {
                        if cfg.mode == TagDisplayMode::Simplify {
                            if emitted_chars >= max_chars {
                                truncated = true;
                                break;
                            }
                            out.push(cfg.simplify_marker);
                            emitted_chars += 1;
                        }
                        pos += rel_close + 2;
                        continue;
                    }
                }

                // Find next `{` (or end of string) and copy plain segment with char-limit check.
                let seg_start = pos;
                let next_brace = bytes[pos + 1..]
                    .iter()
                    .position(|&c| c == b'{')
                    .map_or(len, |r| pos + 1 + r);
                let seg = &raw[seg_start..next_brace];

                // Fast ASCII path when `seg` is pure ASCII and fits completely.
                if seg.is_ascii() && emitted_chars + seg.len() <= max_chars {
                    out.push_str(seg);
                    emitted_chars += seg.len();
                    pos = next_brace;
                    continue;
                }

                for ch in seg.chars() {
                    if emitted_chars >= max_chars {
                        truncated = true;
                        break;
                    }
                    out.push(ch);
                    emitted_chars += 1;
                }
                if truncated {
                    break;
                }
                pos = next_brace;
            }

            if truncated {
                out.push_str(cfg.ellipsis);
            }
        }
    }
}

fn append_truncated(src: &str, max_chars: usize, ellipsis: &str, out: &mut String) {
    if src.is_ascii() {
        if src.len() <= max_chars {
            out.push_str(src);
        } else {
            out.push_str(&src[..max_chars]);
            out.push_str(ellipsis);
        }
        return;
    }
    for (count, (byte_idx, _)) in src.char_indices().enumerate() {
        if count == max_chars {
            out.push_str(&src[..byte_idx]);
            out.push_str(ellipsis);
            return;
        }
    }
    out.push_str(src);
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn tokenizes_tags_karaoke_escapes_comments_drawings_and_errors() {
        let line = r"{comment\an8\kf45\pos(10,20)\clip(m 0 0 l 10 10)\bogus}Hi\N{\p1}m 0 0 l 5 5{\p0}ok{unclosed";
        let toks = tokenize(line);

        assert!(toks
            .iter()
            .any(|t| t.kind == TokenKind::Comment && &line[t.start..t.end] == "comment"));
        assert!(toks
            .iter()
            .any(|t| t.kind == TokenKind::TagName && &line[t.start..t.end] == r"\an"));
        assert!(toks
            .iter()
            .any(|t| t.kind == TokenKind::KaraokeTag && &line[t.start..t.end] == r"\kf"));
        assert!(toks
            .iter()
            .any(|t| t.kind == TokenKind::Escape && &line[t.start..t.end] == r"\N"));
        assert!(
            toks.iter()
                .any(|t| t.kind == TokenKind::DrawingCommand
                    && &line[t.start..t.end] == "m 0 0 l 5 5")
        );
        assert!(toks
            .iter()
            .any(|t| t.kind == TokenKind::DrawingCommand
                && &line[t.start..t.end] == "(m 0 0 l 10 10)"));
        assert!(toks
            .iter()
            .any(|t| t.kind == TokenKind::Error && &line[t.start..t.end] == r"\bogus"));
        assert!(toks
            .iter()
            .any(|t| t.kind == TokenKind::Error && &line[t.start..t.end] == "{unclosed"));
    }

    #[test]
    fn incremental_retokenize_matches_full_tokenize_after_edits() {
        let base = r"{\an8\pos(100,200)\fs48}Hello\N{\c&H00FF00&}World!{\p1}m 0 0{\p0}Done";
        let mut inc = IncrementalTokenizer::new();
        inc.set_text(base);

        // 1-char edit inside plain text ("Hello" -> "HellXo")
        let edit_pos = base.find("Hello").expect("find") + 4;
        let mut edited = base.to_owned();
        edited.insert(edit_pos, 'X');
        inc.re_tokenize_edit(&edited, edit_pos, edit_pos, edit_pos + 1);
        assert_eq!(inc.tokens(), tokenize(&edited).as_slice());

        // Edit toggling \p1 -> \p0 (must re-lex tail accurately!)
        let p1_pos = edited.find(r"\p1").expect("p1") + 2;
        let mut edited2 = edited.clone();
        edited2.replace_range(p1_pos..=p1_pos, "0");
        inc.re_tokenize_edit(&edited2, p1_pos, p1_pos + 1, p1_pos + 1);
        assert_eq!(inc.tokens(), tokenize(&edited2).as_slice());
    }

    #[test]
    fn grid_display_modes_and_truncation() {
        let raw = r"{\an8\pos(10,20)}Hello {\i1}world{\i0}!";
        let mut out = String::new();

        format_grid_text_into(
            raw,
            &GridDisplayConfig {
                mode: TagDisplayMode::Show,
                max_chars: None,
                ..GridDisplayConfig::default()
            },
            &mut out,
        );
        assert_eq!(out, raw);

        format_grid_text_into(
            raw,
            &GridDisplayConfig {
                mode: TagDisplayMode::Simplify,
                simplify_marker: '★',
                max_chars: None,
                ellipsis: "...",
            },
            &mut out,
        );
        assert_eq!(out, "★Hello ★world★!");

        format_grid_text_into(
            raw,
            &GridDisplayConfig {
                mode: TagDisplayMode::Hide,
                max_chars: Some(8),
                ..GridDisplayConfig::default()
            },
            &mut out,
        );
        assert_eq!(out, "Hello wo...");
    }
}
