//! Virtualized 50,000-line subtitle grid state and multi-select logic.

use std::collections::BTreeSet;

/// A single generated subtitle line for the Phase 0 grid spike.
#[derive(Debug, Clone)]
pub struct SubtitleLine {
    pub number: usize,
    pub start_cs: u32,
    pub end_cs: u32,
    pub start_str: String,
    pub end_str: String,
    pub cps: f32,
    pub cps_str: String,
    pub style: String,
    pub text: String,
}

impl SubtitleLine {
    /// Recomputes `cps` and `cps_str` after `text` changes.
    pub fn recompute_cps(&mut self) {
        let dur_sec = (self.end_cs.saturating_sub(self.start_cs).max(1) as f32) / 100.0;
        let visible_chars = visible_char_count(&self.text);
        self.cps = (visible_chars as f32) / dur_sec;
        self.cps_str = format!("{:.1}", self.cps);
    }
}

/// Counts dialogue characters outside `{...}` override blocks and `\N`/`\n`/`\h` escapes.
#[must_use]
pub fn visible_char_count(text: &str) -> usize {
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

/// Grid model holding 50,000 lines and multi-select state.
pub struct GridState {
    pub lines: Vec<SubtitleLine>,
    pub selected: BTreeSet<usize>,
    pub active_row: usize,
    pub anchor_row: usize,
    pub scroll_to_active: bool,
}

impl GridState {
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

        let mut lines = Vec::with_capacity(count);
        for idx in 0..count {
            let start_cs = u32::try_from(idx * 250).unwrap_or(u32::MAX);
            // Vary durations so CPS spans normal, warning (>18), and critical (>25).
            let dur_cs = match idx % 7 {
                0 => 90,
                3 => 140,
                _ => 240,
            };
            let end_cs = start_cs.saturating_add(dur_cs);
            let (style, template) = SAMPLES[idx % SAMPLES.len()];
            let text = format!("{template} [#{n}]", n = idx + 1);
            let mut line = SubtitleLine {
                number: idx + 1,
                start_cs,
                end_cs,
                start_str: format_ass_time(start_cs),
                end_str: format_ass_time(end_cs),
                cps: 0.0,
                cps_str: String::new(),
                style: (*style).to_owned(),
                text,
            };
            line.recompute_cps();
            lines.push(line);
        }

        let mut selected = BTreeSet::new();
        if !lines.is_empty() {
            selected.insert(0);
        }

        Self {
            lines,
            selected,
            active_row: 0,
            anchor_row: 0,
            scroll_to_active: false,
        }
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
                        if let Some(&fallback) = self.selected.iter().next() {
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
        self.selected = (0..self.lines.len()).collect();
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

/// Formats centiseconds as `H:MM:SS.CC`.
#[must_use]
pub fn format_ass_time(cs: u32) -> String {
    let total_sec = cs / 100;
    let cc = cs % 100;
    let ss = total_sec % 60;
    let total_min = total_sec / 60;
    let mm = total_min % 60;
    let hh = total_min / 60;
    format!("{hh}:{mm:02}:{ss:02}.{cc:02}")
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
