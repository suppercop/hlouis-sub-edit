//! Aegisub 3.5.1 character-per-second (CPS) and visual line-length metrics.
//!
//! Mirrors Aegisub `src/utils.cpp` (`CharacterCount`) and `src/subs_controller.cpp`:
//! - Strips `{...}` override blocks.
//! - Tracks `\pN` vector drawing mode (`\p1`..`\pN` suppresses visible text until `\p0`).
//! - Splits visual lines on `\N` (hard break) and optionally `\n` (soft break).
//! - Treats `\h` (hard space) as whitespace.
//! - Zero heap allocation per line (single-pass byte/UTF-8 scanner).

use ass_core::AssEvent;

/// Options controlling CPS and character counting (matches Aegisub 3.5.1 preferences).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct CpsOptions {
    /// Ignore whitespace characters (`' '`, `'\t'`, `\h`, etc.). Aegisub default: `true`.
    pub ignore_whitespace: bool,
    /// Ignore ASCII & Unicode punctuation characters. Aegisub default: `false`.
    pub ignore_punctuation: bool,
    /// Treat soft line break `\n` as a visual line break (when `false`, treated as whitespace).
    pub treat_soft_break_as_newline: bool,
    /// Minimum duration in centiseconds used as CPS denominator (prevents division by zero;
    /// Aegisub clamps very short/zero durations). Default: `1` (0.01 s).
    pub min_duration_cs: i64,
}

impl Default for CpsOptions {
    fn default() -> Self {
        Self {
            ignore_whitespace: true,
            ignore_punctuation: false,
            treat_soft_break_as_newline: false,
            min_duration_cs: 1,
        }
    }
}

/// Computed metrics for a single subtitle line.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct LineMetrics {
    /// Total visible character count (excluding override tags, drawings, and ignored chars).
    pub char_count: usize,
    /// Longest single visual line (in counted characters) across `\N` splits.
    pub max_line_len: usize,
    /// Number of visual lines (1 + number of `\N` breaks).
    pub line_count: usize,
    /// Characters per second (`char_count / duration_seconds`), or `0.0` if `char_count == 0`.
    pub cps: f64,
}

/// Computes visible character count, max visual line length, line count, and CPS for an
/// [`AssEvent`]. Zero heap allocations.
#[must_use]
pub fn event_metrics(event: &AssEvent, options: CpsOptions) -> LineMetrics {
    let (char_count, max_line_len, line_count) = count_text_metrics(event.text(), options);
    let dur_cs = (i64::from(event.end().as_cs()) - i64::from(event.start().as_cs()))
        .max(options.min_duration_cs.max(1));
    #[allow(clippy::cast_precision_loss)]
    let cps = if char_count == 0 {
        0.0
    } else {
        (char_count as f64) * 100.0 / (dur_cs as f64)
    };
    LineMetrics {
        char_count,
        max_line_len,
        line_count,
        cps,
    }
}

/// Computes `(char_count, max_line_len, line_count)` for raw ASS event text in a single pass
/// with zero heap allocations.
#[must_use]
pub fn count_text_metrics(text: &str, options: CpsOptions) -> (usize, usize, usize) {
    let bytes = text.as_bytes();
    let len = bytes.len();
    let mut pos = 0usize;
    let mut in_drawing = false;

    let mut total_chars = 0usize;
    let mut cur_line_len = 0usize;
    let mut max_line_len = 0usize;
    let mut line_count = 1usize;

    while pos < len {
        let b = bytes[pos];

        // Override tag block `{...}`
        if b == b'{' {
            if let Some(rel_end) = bytes[pos + 1..].iter().position(|&c| c == b'}') {
                let tag_block = &bytes[pos + 1..pos + 1 + rel_end];
                update_drawing_mode(tag_block, &mut in_drawing);
                pos += rel_end + 2;
                continue;
            }
            // Unclosed `{` is treated as plain text by libass/Aegisub.
        }

        // Escape sequences `\N`, `\n`, `\h`
        if b == b'\\' && pos + 1 < len {
            let next = bytes[pos + 1];
            if next == b'N' {
                if !in_drawing {
                    if cur_line_len > max_line_len {
                        max_line_len = cur_line_len;
                    }
                    cur_line_len = 0;
                    line_count += 1;
                }
                pos += 2;
                continue;
            }
            if next == b'n' {
                if !in_drawing {
                    if options.treat_soft_break_as_newline {
                        if cur_line_len > max_line_len {
                            max_line_len = cur_line_len;
                        }
                        cur_line_len = 0;
                        line_count += 1;
                    } else if !options.ignore_whitespace {
                        total_chars += 1;
                        cur_line_len += 1;
                    }
                }
                pos += 2;
                continue;
            }
            if next == b'h' {
                if !in_drawing && !options.ignore_whitespace {
                    total_chars += 1;
                    cur_line_len += 1;
                }
                pos += 2;
                continue;
            }
        }

        // Fast ASCII path (avoids UTF-8 decoding overhead for >90% of characters).
        if b < 0x80 {
            if !in_drawing && should_count_ascii(b, options) {
                total_chars += 1;
                cur_line_len += 1;
            }
            pos += 1;
        } else {
            // Decode one UTF-8 scalar from `text[pos..]`.
            let ch = text[pos..].chars().next().unwrap_or('\0');
            if !in_drawing && should_count_unicode(ch, options) {
                total_chars += 1;
                cur_line_len += 1;
            }
            pos += ch.len_utf8();
        }
    }

    if cur_line_len > max_line_len {
        max_line_len = cur_line_len;
    }

    (total_chars, max_line_len, line_count)
}

/// Scans an override tag block (without outer braces) for `\p<digits>` drawing toggles.
#[inline]
fn update_drawing_mode(tag_block: &[u8], in_drawing: &mut bool) {
    let mut i = 0usize;
    let len = tag_block.len();
    while i + 1 < len {
        if tag_block[i] == b'\\' && tag_block[i + 1] == b'p' {
            // Ensure it's `\p<digit>` and NOT `\pos`, `\pbo`, etc.
            let next_idx = i + 2;
            if next_idx < len && tag_block[next_idx].is_ascii_digit() {
                let mut val = 0u32;
                let mut j = next_idx;
                while j < len && tag_block[j].is_ascii_digit() {
                    val = val
                        .saturating_mul(10)
                        .saturating_add(u32::from(tag_block[j] - b'0'));
                    j += 1;
                }
                *in_drawing = val > 0;
                i = j;
                continue;
            }
        }
        i += 1;
    }
}

#[inline]
const fn should_count_ascii(b: u8, options: CpsOptions) -> bool {
    if options.ignore_whitespace && b.is_ascii_whitespace() {
        return false;
    }
    if options.ignore_punctuation && b.is_ascii_punctuation() {
        return false;
    }
    true
}

#[inline]
fn should_count_unicode(ch: char, options: CpsOptions) -> bool {
    if options.ignore_whitespace && ch.is_whitespace() {
        return false;
    }
    if options.ignore_punctuation && is_unicode_punctuation(ch) {
        return false;
    }
    true
}

#[inline]
fn is_unicode_punctuation(ch: char) -> bool {
    ch.is_ascii_punctuation()
        || matches!(
            ch,
            '\u{2000}'..='\u{206F}'
                | '\u{3000}'..='\u{303F}'
                | '\u{FF01}'..='\u{FF0F}'
                | '\u{FF1A}'..='\u{FF20}'
                | '\u{FF3B}'..='\u{FF40}'
                | '\u{FF5B}'..='\u{FF65}'
        )
}
