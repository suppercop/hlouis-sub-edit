//! ASS line syntax highlighter producing `egui::text::LayoutJob` using theme colors.

use crate::theme::ThemePalette;
use egui::text::{LayoutJob, TextFormat};
use egui::{Color32, FontId};

/// Token categories highlighted in the ASS edit box.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum TokenKind {
    Brace,
    Tag,
    KaraokeTag,
    Value,
    Escape,
    DrawingCommand,
    DrawingCoord,
    Comment,
    Text,
}

/// A contiguous UTF-8 byte span with an assigned [`TokenKind`].
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct HighlightSpan {
    pub start: usize,
    pub end: usize,
    pub kind: TokenKind,
}

impl TokenKind {
    /// Maps a token kind to its palette color.
    #[must_use]
    pub const fn color(self, palette: &ThemePalette) -> Color32 {
        match self {
            Self::Brace => palette.syn_braces,
            Self::Tag => palette.syn_tag,
            Self::KaraokeTag => palette.syn_karaoke,
            Self::Value => palette.syn_value,
            Self::Escape => palette.syn_escape,
            Self::DrawingCommand => palette.syn_drawing_cmd,
            Self::DrawingCoord => palette.syn_drawing_coord,
            Self::Comment => palette.syn_comment,
            Self::Text => palette.syn_text,
        }
    }
}

/// Tokenizes an ASS dialogue line into contiguous spans covering `0..text.len()`.
#[must_use]
pub fn tokenize_ass_line(text: &str) -> Vec<HighlightSpan> {
    let mut spans = Vec::new();
    let bytes = text.as_bytes();
    let len = bytes.len();
    let mut i = 0usize;
    let mut drawing_level: u32 = 0;

    while i < len {
        if bytes[i] == b'{' {
            push_span(&mut spans, i, i + 1, TokenKind::Brace);
            i += 1;
            i = scan_override_block(text, i, &mut spans, &mut drawing_level);
            if i < len && bytes[i] == b'}' {
                push_span(&mut spans, i, i + 1, TokenKind::Brace);
                i += 1;
            }
        } else {
            let next_brace = find_byte(bytes, i, b'{').unwrap_or(len);
            if drawing_level > 0 {
                scan_drawing_segment(text, i, next_brace, &mut spans);
            } else {
                scan_dialogue_segment(text, i, next_brace, &mut spans);
            }
            i = next_brace;
        }
    }

    spans
}

/// Builds an `egui::text::LayoutJob` with syntax colors from `palette`.
#[must_use]
pub fn build_layout_job(
    text: &str,
    font_id: &FontId,
    wrap_width: f32,
    palette: &ThemePalette,
) -> LayoutJob {
    let mut job = LayoutJob::default();
    job.wrap.max_width = wrap_width;

    for span in tokenize_ass_line(text) {
        if let Some(slice) = text.get(span.start..span.end) {
            let format = TextFormat {
                font_id: font_id.clone(),
                color: span.kind.color(palette),
                ..Default::default()
            };
            job.append(slice, 0.0, format);
        }
    }

    if text.is_empty() {
        job.append(
            "",
            0.0,
            TextFormat {
                font_id: font_id.clone(),
                color: palette.syn_text,
                ..Default::default()
            },
        );
    }

    job
}

fn push_span(spans: &mut Vec<HighlightSpan>, start: usize, end: usize, kind: TokenKind) {
    if start >= end {
        return;
    }
    if let Some(last) = spans.last_mut() {
        if last.end == start && last.kind == kind {
            last.end = end;
            return;
        }
    }
    spans.push(HighlightSpan { start, end, kind });
}

fn find_byte(bytes: &[u8], start: usize, needle: u8) -> Option<usize> {
    bytes[start..]
        .iter()
        .position(|&b| b == needle)
        .map(|rel| start + rel)
}

fn scan_override_block(
    text: &str,
    mut i: usize,
    spans: &mut Vec<HighlightSpan>,
    drawing_level: &mut u32,
) -> usize {
    let bytes = text.as_bytes();
    let len = bytes.len();

    while i < len && bytes[i] != b'}' {
        if bytes[i] != b'\\' {
            let start = i;
            while i < len && bytes[i] != b'}' && bytes[i] != b'\\' {
                i = advance_char(text, i);
            }
            push_span(spans, start, i, TokenKind::Comment);
            continue;
        }

        let tag_start = i;
        i += 1; // consume '\'
        let (tag_name, after_name) = match_tag_name(bytes, i);
        let tag_kind = if is_karaoke_tag(tag_name) {
            TokenKind::KaraokeTag
        } else {
            TokenKind::Tag
        };
        push_span(spans, tag_start, after_name, tag_kind);
        i = after_name;

        let val_start = i;
        let mut paren_depth: u32 = 0;
        let mut pending_val_start = val_start;

        while i < len && bytes[i] != b'}' {
            let b = bytes[i];
            if b == b'(' {
                paren_depth += 1;
                i += 1;
            } else if b == b')' {
                paren_depth = paren_depth.saturating_sub(1);
                i += 1;
            } else if b == b'\\' {
                if paren_depth == 0 {
                    break;
                }
                // Nested override tag inside `\t(...)` or similar parenthesized argument.
                push_span(spans, pending_val_start, i, TokenKind::Value);
                let nested_start = i;
                i += 1;
                let (nested_name, nested_after) = match_tag_name(bytes, i);
                let nested_kind = if is_karaoke_tag(nested_name) {
                    TokenKind::KaraokeTag
                } else {
                    TokenKind::Tag
                };
                push_span(spans, nested_start, nested_after, nested_kind);
                i = nested_after;
                pending_val_start = i;
            } else {
                i = advance_char(text, i);
            }
        }
        push_span(spans, pending_val_start, i, TokenKind::Value);

        if tag_name == "p" {
            let val_slice = text.get(val_start..i).unwrap_or("").trim();
            *drawing_level = val_slice.parse::<u32>().unwrap_or(0);
        }
    }

    i
}

fn scan_dialogue_segment(text: &str, mut i: usize, end: usize, spans: &mut Vec<HighlightSpan>) {
    let bytes = text.as_bytes();
    let mut text_start = i;

    while i < end {
        if bytes[i] == b'\\' && i + 1 < end && matches!(bytes[i + 1], b'N' | b'n' | b'h') {
            push_span(spans, text_start, i, TokenKind::Text);
            push_span(spans, i, i + 2, TokenKind::Escape);
            i += 2;
            text_start = i;
        } else {
            i = advance_char(text, i);
        }
    }
    push_span(spans, text_start, end, TokenKind::Text);
}

fn scan_drawing_segment(text: &str, mut i: usize, end: usize, spans: &mut Vec<HighlightSpan>) {
    let bytes = text.as_bytes();
    while i < end {
        let b = bytes[i];
        if b.is_ascii_whitespace() {
            let start = i;
            while i < end && bytes[i].is_ascii_whitespace() {
                i += 1;
            }
            push_span(spans, start, i, TokenKind::Text);
        } else if matches!(b, b'm' | b'n' | b'l' | b'b' | b's' | b'p' | b'c')
            && (i + 1 == end || !bytes[i + 1].is_ascii_alphabetic())
        {
            push_span(spans, i, i + 1, TokenKind::DrawingCommand);
            i += 1;
        } else if b.is_ascii_digit() || b == b'-' || b == b'.' {
            let start = i;
            i += 1;
            while i < end && (bytes[i].is_ascii_digit() || bytes[i] == b'.') {
                i += 1;
            }
            push_span(spans, start, i, TokenKind::DrawingCoord);
        } else {
            let start = i;
            i = advance_char(text, i);
            push_span(spans, start, i, TokenKind::Text);
        }
    }
}

fn match_tag_name(bytes: &[u8], start: usize) -> (&str, usize) {
    const KNOWN_TAGS: &[&str] = &[
        "1img", "2img", "3img", "4img", "1vc", "2vc", "3vc", "4vc", "1va", "2va", "3va", "4va",
        "1c", "2c", "3c", "4c", "1a", "2a", "3a", "4a", "xbord", "ybord", "xshad", "yshad",
        "alpha", "fscx", "fscy", "iclip", "clip", "move", "fade", "blur", "bord", "shad", "fsvp",
        "fsc", "fsp", "frx", "fry", "frz", "fax", "fay", "pbo", "pos", "org", "fad", "frs", "rnd",
        "fn", "fs", "fe", "fr", "kf", "ko", "kt", "be", "an", "a", "b", "i", "u", "s", "c", "k",
        "K", "p", "q", "r", "t",
    ];

    let rest = &bytes[start..];
    for &tag in KNOWN_TAGS {
        let tb = tag.as_bytes();
        if rest.starts_with(tb) {
            return (tag, start + tb.len());
        }
    }

    // Fallback: take leading ASCII alphabetic characters for unknown tags.
    let mut end = start;
    while end < bytes.len() && bytes[end].is_ascii_alphabetic() {
        end += 1;
    }
    let slice = std::str::from_utf8(&bytes[start..end]).unwrap_or("");
    (slice, end)
}

fn is_karaoke_tag(tag_name: &str) -> bool {
    matches!(tag_name, "k" | "K" | "kf" | "ko" | "kt")
}

fn advance_char(text: &str, byte_idx: usize) -> usize {
    text.get(byte_idx..)
        .and_then(|s| s.chars().next())
        .map_or(text.len(), |ch| byte_idx + ch.len_utf8())
}

#[cfg(test)]
mod tests {
    use super::*;

    fn slice_tokens(line: &str) -> Vec<(TokenKind, &str)> {
        tokenize_ass_line(line)
            .into_iter()
            .map(|s| (s.kind, &line[s.start..s.end]))
            .collect()
    }

    #[test]
    fn highlights_tags_values_karaoke_and_escapes() {
        let line = r"{\an8\pos(960,120)\k25\kf40}Xin chào\N日本語\h!";
        let tokens = slice_tokens(line);
        assert_eq!(
            tokens,
            vec![
                (TokenKind::Brace, "{"),
                (TokenKind::Tag, r"\an"),
                (TokenKind::Value, "8"),
                (TokenKind::Tag, r"\pos"),
                (TokenKind::Value, "(960,120)"),
                (TokenKind::KaraokeTag, r"\k"),
                (TokenKind::Value, "25"),
                (TokenKind::KaraokeTag, r"\kf"),
                (TokenKind::Value, "40"),
                (TokenKind::Brace, "}"),
                (TokenKind::Text, "Xin chào"),
                (TokenKind::Escape, r"\N"),
                (TokenKind::Text, "日本語"),
                (TokenKind::Escape, r"\h"),
                (TokenKind::Text, "!"),
            ]
        );
    }

    #[test]
    fn highlights_nested_transform_and_drawing_commands() {
        let line = r"{\t(0,500,\frz360)\p1}m 0 0 l 100 0 100 50{\p0}Done";
        let tokens = slice_tokens(line);
        assert!(tokens.contains(&(TokenKind::Tag, r"\t")));
        assert!(tokens.contains(&(TokenKind::Tag, r"\frz")));
        assert!(tokens.contains(&(TokenKind::DrawingCommand, "m")));
        assert!(tokens.contains(&(TokenKind::DrawingCommand, "l")));
        assert!(tokens.contains(&(TokenKind::DrawingCoord, "100")));
        assert_eq!(tokens.last().copied(), Some((TokenKind::Text, "Done")));
    }

    #[test]
    fn handles_unclosed_brace_and_multibyte_utf8_safely() {
        let line = r"{\fnSegoe UI Tiếng Việt";
        let tokens = slice_tokens(line);
        let reconstructed: String = tokens.iter().map(|(_, s)| *s).collect();
        assert_eq!(reconstructed, line);
    }
}
