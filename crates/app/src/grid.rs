//! Virtualized 50,000-line (up to 200k+) subtitle grid state and multi-select logic.

use ass_core::ScriptFile;
use std::fmt;
use std::ops::{Deref, RangeInclusive};
use std::sync::Arc;

/// Stack-resident valid ASCII string storing up to `CAP` bytes without heap allocation.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct InlineAscii<const CAP: usize> {
    buf: [u8; CAP],
    len: u8,
}

impl<const CAP: usize> Default for InlineAscii<CAP> {
    fn default() -> Self {
        Self::new()
    }
}

impl<const CAP: usize> InlineAscii<CAP> {
    #[must_use]
    pub const fn new() -> Self {
        Self {
            buf: [0; CAP],
            len: 0,
        }
    }

    #[inline]
    pub fn clear(&mut self) {
        self.len = 0;
    }

    #[inline]
    pub fn push_ascii(&mut self, byte: u8) {
        let idx = usize::from(self.len);
        if idx < CAP && byte.is_ascii() {
            self.buf[idx] = byte;
            #[allow(clippy::cast_possible_truncation)]
            {
                self.len = (idx + 1) as u8;
            }
        }
    }

    #[inline]
    pub fn push_ascii_slice(&mut self, bytes: &[u8]) {
        let idx = usize::from(self.len);
        let avail = CAP.saturating_sub(idx);
        let n = bytes.len().min(avail);
        for &b in &bytes[..n] {
            if !b.is_ascii() {
                return;
            }
        }
        self.buf[idx..idx + n].copy_from_slice(&bytes[..n]);
        #[allow(clippy::cast_possible_truncation)]
        {
            self.len = (idx + n) as u8;
        }
    }

    #[must_use]
    #[inline]
    pub fn as_str(&self) -> &str {
        let n = usize::from(self.len).min(CAP);
        // Invariant: constructors and push helpers only admit ASCII bytes into `buf[..len]`.
        std::str::from_utf8(&self.buf[..n]).unwrap_or("")
    }
}

impl<const CAP: usize> Deref for InlineAscii<CAP> {
    type Target = str;

    #[inline]
    fn deref(&self) -> &Self::Target {
        self.as_str()
    }
}

impl<const CAP: usize> AsRef<str> for InlineAscii<CAP> {
    #[inline]
    fn as_ref(&self) -> &str {
        self.as_str()
    }
}

impl<const CAP: usize> fmt::Display for InlineAscii<CAP> {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(self.as_str())
    }
}

/// Compact bitset for selected subtitle row indices with O(1) membership/count and O(N/64) bulk ops.
#[derive(Debug, Clone, Default)]
pub struct SelectionSet {
    words: Vec<u64>,
    count: usize,
}

impl SelectionSet {
    /// Allocates a selection bitset sized for `capacity_rows` rows.
    #[must_use]
    pub fn with_row_capacity(capacity_rows: usize) -> Self {
        Self {
            words: vec![0; capacity_rows.div_ceil(64)],
            count: 0,
        }
    }

    #[must_use]
    #[inline]
    pub const fn len(&self) -> usize {
        self.count
    }

    #[must_use]
    #[inline]
    pub const fn is_empty(&self) -> bool {
        self.count == 0
    }

    #[must_use]
    #[inline]
    pub fn contains(&self, row: &usize) -> bool {
        let word = *row >> 6;
        let bit = 1u64 << (*row & 63);
        self.words.get(word).is_some_and(|&w| (w & bit) != 0)
    }

    #[inline]
    pub fn insert(&mut self, row: usize) -> bool {
        let word = row >> 6;
        if word >= self.words.len() {
            self.words.resize(word + 1, 0);
        }
        let bit = 1u64 << (row & 63);
        let prev = self.words[word];
        if (prev & bit) != 0 {
            return false;
        }
        self.words[word] = prev | bit;
        self.count += 1;
        true
    }

    #[inline]
    pub fn remove(&mut self, row: &usize) -> bool {
        let word = *row >> 6;
        let Some(slot) = self.words.get_mut(word) else {
            return false;
        };
        let bit = 1u64 << (*row & 63);
        if (*slot & bit) == 0 {
            return false;
        }
        *slot &= !bit;
        self.count -= 1;
        true
    }

    #[inline]
    pub fn clear(&mut self) {
        if self.count != 0 {
            self.words.fill(0);
            self.count = 0;
        }
    }

    /// Selects every row in `0..len` using word-level fills (zero realloc when sized).
    pub fn select_all_rows(&mut self, len: usize) {
        if len == 0 {
            self.clear();
            return;
        }
        let full_words = len >> 6;
        let rem_bits = len & 63;
        let needed = full_words + usize::from(rem_bits != 0);
        if self.words.len() < needed {
            self.words.resize(needed, 0);
        }
        self.words[..full_words].fill(u64::MAX);
        if rem_bits != 0 {
            self.words[full_words] = (1u64 << rem_bits) - 1;
        }
        if self.words.len() > needed {
            self.words[needed..].fill(0);
        }
        self.count = len;
    }

    /// Inserts every index in `range` using word-level bitmasks.
    pub fn extend(&mut self, range: RangeInclusive<usize>) {
        if range.is_empty() {
            return;
        }
        let lo = *range.start();
        let hi = *range.end();
        let first_word = lo >> 6;
        let last_word = hi >> 6;
        if last_word >= self.words.len() {
            self.words.resize(last_word + 1, 0);
        }
        for wi in first_word..=last_word {
            let start_bit = if wi == first_word { lo & 63 } else { 0 };
            let end_bit = if wi == last_word { hi & 63 } else { 63 };
            let width = end_bit - start_bit + 1;
            let mask = if width == 64 {
                u64::MAX
            } else {
                ((1u64 << width) - 1) << start_bit
            };
            let prev = self.words[wi];
            let next = prev | mask;
            if next != prev {
                self.count += (next ^ prev).count_ones() as usize;
                self.words[wi] = next;
            }
        }
    }

    /// Returns the lowest selected row index, if any.
    #[must_use]
    pub fn first(&self) -> Option<usize> {
        if self.count == 0 {
            return None;
        }
        for (wi, &w) in self.words.iter().enumerate() {
            if w != 0 {
                return Some((wi << 6) + w.trailing_zeros() as usize);
            }
        }
        None
    }
}

/// A single subtitle line cached for zero-allocation virtualized grid painting.
#[derive(Debug, Clone)]
pub struct SubtitleLine {
    pub number: usize,
    pub number_str: InlineAscii<16>,
    pub start_cs: u32,
    pub end_cs: u32,
    pub start_str: InlineAscii<16>,
    pub end_str: InlineAscii<16>,
    pub cps: f32,
    pub cps_str: InlineAscii<16>,
    pub style: Arc<str>,
    pub text: String,
    pub version: u64,
}

impl SubtitleLine {
    /// Recomputes `cps` and `cps_str` after `text` changes without heap allocations.
    pub fn recompute_cps(&mut self) {
        let dur_sec = (self.end_cs.saturating_sub(self.start_cs).max(1) as f32) / 100.0;
        let visible_chars = visible_char_count(&self.text);
        self.cps = (visible_chars as f32) / dur_sec;
        format_tenths_inline(&mut self.cps_str, self.cps);
        self.version = self.version.wrapping_add(1);
    }
}

/// Counts dialogue characters outside `{...}` override blocks and `\N`/`\n`/`\h` escapes.
#[must_use]
pub fn visible_char_count(text: &str) -> usize {
    let bytes = text.as_bytes();
    if text.is_ascii() {
        let mut count = 0usize;
        let mut in_brace = false;
        let mut i = 0usize;
        while i < bytes.len() {
            let b = bytes[i];
            if b == b'{' {
                in_brace = true;
            } else if b == b'}' {
                in_brace = false;
            } else if !in_brace {
                if b == b'\\' && i + 1 < bytes.len() && matches!(bytes[i + 1], b'N' | b'n' | b'h') {
                    i += 1;
                } else {
                    count += 1;
                }
            }
            i += 1;
        }
        return count;
    }

    let mut count = 0usize;
    let mut in_brace = false;
    let mut chars = text.chars().peekable();
    while let Some(ch) = chars.next() {
        if ch == '{' {
            in_brace = true;
        } else if ch == '}' {
            in_brace = false;
        } else if !in_brace {
            if ch == '\\' && matches!(chars.peek(), Some('N' | 'n' | 'h')) {
                let _ = chars.next();
            } else {
                count += 1;
            }
        }
    }
    count
}

/// Grid model holding up to 200,000+ lines and bitset multi-select state.
pub struct GridState {
    pub lines: Vec<SubtitleLine>,
    pub selected: SelectionSet,
    pub active_row: usize,
    pub anchor_row: usize,
    pub scroll_to_active: bool,
    pub(crate) galley_cache: crate::RowGalleyCache,
}

impl SubtitleLine {
    #[inline]
    fn new_with_cps(
        number: usize,
        start_cs: u32,
        end_cs: u32,
        style: Arc<str>,
        text: String,
    ) -> Self {
        let mut number_str = InlineAscii::new();
        push_usize_inline(&mut number_str, number);
        let mut line = Self {
            number,
            number_str,
            start_cs,
            end_cs,
            start_str: format_ass_time_inline(start_cs),
            end_str: format_ass_time_inline(end_cs),
            cps: 0.0,
            cps_str: InlineAscii::new(),
            style,
            text,
            version: 0,
        };
        line.recompute_cps();
        line
    }
}

impl GridState {
    fn from_lines(lines: Vec<SubtitleLine>) -> Self {
        let mut selected = SelectionSet::with_row_capacity(lines.len());
        if !lines.is_empty() {
            selected.insert(0);
        }
        Self {
            lines,
            selected,
            active_row: 0,
            anchor_row: 0,
            scroll_to_active: false,
            galley_cache: crate::RowGalleyCache::default(),
        }
    }

    /// Generates `count` varied subtitle rows (default 50,000) containing tags, karaoke,
    /// escapes, drawing snippets, Vietnamese, and Japanese text.
    #[must_use]
    pub fn generate(count: usize) -> Self {
        const SAMPLES: &[(&str, &str)] = &[
            (
                "Dialogue",
                r"{\an8\fs44\c&H00FFFF&}Xin chào Tiếng Việt\N{\k30}Ka{\kf45}raoke — 日本語テスト",
            ),
            (
                "Default",
                "Welcome to hlouis-sub-edit Phase 0 spike! Smooth 50,000-row virtualized grid.",
            ),
            (
                "Karaoke",
                r"{\k30}Ka{\kf45}ra{\ko25}o{\K35}ke {\c&H80FF80&}timing line with \Nline break",
            ),
            (
                "Japanese",
                r"{\pos(960,980)\fad(150,150)}日本語の字幕テスト：漢字・ひらがな・カタカナ",
            ),
            (
                "Typeset",
                r"{\an5\frz-12\t(0,400,\fscx115\fscy115)}Styled sign\hwith\hnon-breaking spaces",
            ),
            (
                "Drawing",
                r"{\p1\pos(100,100)}m 0 0 l 120 0 120 40 0 40{\p0}Vector box label",
            ),
        ];

        let style_arcs: [Arc<str>; 6] = std::array::from_fn(|i| Arc::from(SAMPLES[i].0));
        let mut lines = Vec::with_capacity(count);
        let mut num_buf = InlineAscii::<16>::new();
        for idx in 0..count {
            let start_cs = u32::try_from(idx * 250).unwrap_or(u32::MAX);
            // Vary durations so CPS spans normal, warning (>18), and critical (>25).
            let dur_cs = match idx % 7 {
                0 => 90,
                3 => 140,
                _ => 240,
            };
            let end_cs = start_cs.saturating_add(dur_cs);
            let sample_idx = idx % SAMPLES.len();
            let template = SAMPLES[sample_idx].1;
            let number = idx + 1;
            num_buf.clear();
            push_usize_inline(&mut num_buf, number);
            let mut text = String::with_capacity(template.len() + 12);
            text.push_str(template);
            text.push_str(" [#");
            text.push_str(num_buf.as_str());
            text.push(']');

            lines.push(SubtitleLine::new_with_cps(
                number,
                start_cs,
                end_cs,
                Arc::clone(&style_arcs[sample_idx]),
                text,
            ));
        }

        Self::from_lines(lines)
    }

    /// Builds a grid view from a parsed [`ScriptFile`].
    #[must_use]
    pub fn from_script(script: &ScriptFile) -> Self {
        let count = script
            .events_section()
            .map_or(0, ass_core::EventsSection::event_count);
        let mut lines = Vec::with_capacity(count);
        let mut interned_styles: Vec<Arc<str>> = Vec::with_capacity(8);
        let mut last_style: Option<Arc<str>> = None;
        for (idx, ev) in script.events().enumerate() {
            let raw_style = ev.style();
            let style = if let Some(last) = &last_style {
                if &**last == raw_style {
                    Arc::clone(last)
                } else {
                    intern_style(raw_style, &mut interned_styles, &mut last_style)
                }
            } else {
                intern_style(raw_style, &mut interned_styles, &mut last_style)
            };
            lines.push(SubtitleLine::new_with_cps(
                idx + 1,
                ev.start().as_cs(),
                ev.end().as_cs(),
                style,
                ev.text().to_owned(),
            ));
        }
        Self::from_lines(lines)
    }

    /// Handles a row click with Shift (range) and Ctrl (toggle) modifiers.
    pub fn click_row(&mut self, row: usize, shift: bool, ctrl: bool) {
        if row >= self.lines.len() {
            return;
        }
        if shift {
            if !ctrl {
                self.selected.clear();
            }
            let lo = self.anchor_row.min(row);
            let hi = self.anchor_row.max(row);
            self.selected.extend(lo..=hi);
            self.active_row = row;
        } else if ctrl {
            if self.selected.contains(&row) {
                if self.selected.len() > 1 {
                    self.selected.remove(&row);
                    if self.active_row == row {
                        if let Some(fallback) = self.selected.first() {
                            self.active_row = fallback;
                            self.anchor_row = fallback;
                        }
                    }
                }
            } else {
                self.selected.insert(row);
                self.active_row = row;
                self.anchor_row = row;
            }
        } else {
            self.selected.clear();
            self.selected.insert(row);
            self.active_row = row;
            self.anchor_row = row;
        }
    }

    /// Selects every row in `self.lines`.
    pub fn select_all(&mut self) {
        self.selected.select_all_rows(self.lines.len());
    }

    /// Moves the active row by `delta` (`-1` or `+1`), optionally extending selection with `Shift`.
    pub fn move_active(&mut self, delta: isize, shift: bool) {
        if self.lines.is_empty() {
            return;
        }
        let max_idx = self.lines.len() - 1;
        let next = self.active_row.saturating_add_signed(delta).min(max_idx);
        if shift {
            self.selected.clear();
            let lo = self.anchor_row.min(next);
            let hi = self.anchor_row.max(next);
            self.selected.extend(lo..=hi);
        } else {
            self.selected.clear();
            self.selected.insert(next);
            self.anchor_row = next;
        }
        self.active_row = next;
        self.scroll_to_active = true;
    }

    /// Mutable access to the currently active line (for binding to the Edit Box).
    #[must_use]
    pub fn active_line_mut(&mut self) -> Option<&mut SubtitleLine> {
        self.lines.get_mut(self.active_row)
    }

    /// Immutable access to the currently active line.
    #[must_use]
    pub fn active_line(&self) -> Option<&SubtitleLine> {
        self.lines.get(self.active_row)
    }
}

#[inline]
fn intern_style(raw: &str, interned: &mut Vec<Arc<str>>, last: &mut Option<Arc<str>>) -> Arc<str> {
    if let Some(found) = interned.iter().find(|s| &***s == raw) {
        let arc = Arc::clone(found);
        *last = Some(Arc::clone(&arc));
        return arc;
    }
    let arc: Arc<str> = Arc::from(raw);
    if interned.len() < 64 {
        interned.push(Arc::clone(&arc));
    }
    *last = Some(Arc::clone(&arc));
    arc
}

/// Formats centiseconds as `H:MM:SS.CC` into an inline ASCII buffer (zero heap allocations).
#[must_use]
#[inline]
pub fn format_ass_time_inline(cs: u32) -> InlineAscii<16> {
    let total_sec = cs / 100;
    let cc = cs % 100;
    let ss = total_sec % 60;
    let total_min = total_sec / 60;
    let mm = total_min % 60;
    let hh = total_min / 60;
    let mut out = InlineAscii::new();
    push_usize_inline(&mut out, hh as usize);
    #[allow(clippy::cast_possible_truncation)]
    let tail = [
        b':',
        b'0' + (mm / 10) as u8,
        b'0' + (mm % 10) as u8,
        b':',
        b'0' + (ss / 10) as u8,
        b'0' + (ss % 10) as u8,
        b'.',
        b'0' + (cc / 10) as u8,
        b'0' + (cc % 10) as u8,
    ];
    out.push_ascii_slice(&tail);
    out
}

/// Formats centiseconds as `H:MM:SS.CC` using fixed-buffer ASCII (no `core::fmt`).
#[must_use]
pub fn format_ass_time(cs: u32) -> String {
    format_ass_time_inline(cs).as_str().to_owned()
}

#[inline]
fn push_usize_inline<const CAP: usize>(out: &mut InlineAscii<CAP>, mut val: usize) {
    if val == 0 {
        out.push_ascii(b'0');
        return;
    }
    let mut buf = [0u8; 20];
    let mut pos = buf.len();
    while val > 0 {
        pos -= 1;
        #[allow(clippy::cast_possible_truncation)]
        {
            buf[pos] = b'0' + (val % 10) as u8;
        }
        val /= 10;
    }
    out.push_ascii_slice(&buf[pos..]);
}

#[inline]
fn format_tenths_inline<const CAP: usize>(out: &mut InlineAscii<CAP>, val: f32) {
    out.clear();
    if !val.is_finite() || val.abs() >= 1e9 {
        out.push_ascii_slice(b"0.0");
        return;
    }
    #[allow(clippy::cast_possible_truncation)]
    let tenths = (val * 10.0).round() as i64;
    if tenths < 0 {
        out.push_ascii(b'-');
    }
    let abs = tenths.unsigned_abs();
    let whole = abs / 10;
    #[allow(clippy::cast_possible_truncation)]
    let frac = (abs % 10) as u8;
    push_usize_inline(out, whole as usize);
    out.push_ascii(b'.');
    out.push_ascii(b'0' + frac);
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn generates_50k_lines_and_supports_multiselect() {
        let mut grid = GridState::generate(50_000);
        assert_eq!(grid.lines.len(), 50_000);
        assert_eq!(grid.selected.len(), 1);

        grid.click_row(10, true, false);
        assert_eq!(grid.selected.len(), 11);
        assert_eq!(grid.active_row, 10);

        grid.click_row(5, false, true);
        assert!(!grid.selected.contains(&5));
        assert_eq!(grid.selected.len(), 10);

        assert_eq!(grid.active_row, 10);
        grid.move_active(1, false);
        assert_eq!(grid.active_row, 11);
        assert_eq!(grid.selected.len(), 1);
    }

    #[test]
    fn ctrl_click_never_leaves_empty_selection_or_unbound_active_row() {
        let mut grid = GridState::generate(20);
        // Row 0 is the only selected row; Ctrl+click must keep it selected.
        grid.click_row(0, false, true);
        assert_eq!(grid.selected.len(), 1);
        assert!(grid.selected.contains(&0));
        assert_eq!(grid.active_row, 0);

        // Add row 4 via Ctrl+click, then Ctrl+click row 4 (active) to deselect it.
        grid.click_row(4, false, true);
        assert_eq!(grid.selected.len(), 2);
        assert_eq!(grid.active_row, 4);

        grid.click_row(4, false, true);
        assert_eq!(grid.selected.len(), 1);
        assert!(!grid.selected.contains(&4));
        assert!(grid.selected.contains(&grid.active_row));
        assert_eq!(grid.active_row, 0);
    }
}
