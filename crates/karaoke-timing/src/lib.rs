//! Aegisub 3.5.1 karaoke audio timing controller (`AudioTimingControllerKaraoke` in
//! `src/audio_timing_karaoke.cpp` and `src/audio_karaoke.cpp`).
//!
//! Parses syllables via [`ass_tools::parse_karaoke_syllables`], maintains a flat cumulative
//! millisecond boundary array for O(1) allocation-free drag updates with snapping, supports
//! syllable split/join and `\k`/`\kf`/`\ko` tag-type switching, and commits back via
//! [`ass_tools::rebuild_karaoke_line`] as an atomic [`ass_core::EditCommand::ModifyEventText`].

#![forbid(unsafe_code)]
#![cfg_attr(not(test), deny(clippy::unwrap_used, clippy::expect_used))]

use ass_core::{AssEvent, EditCommand, KaraokeKind};
use ass_tools::{
    merge_syllables, parse_karaoke_syllables, rebuild_karaoke_line, split_syllable, KaraokeSyllable,
};
use thiserror::Error;

/// Errors returned by `karaoke-timing`.
#[derive(Debug, Error, PartialEq, Eq)]
pub enum KaraokeTimingError {
    /// Syllable index out of range.
    #[error("syllable index {index} out of bounds (count = {count})")]
    SyllableOutOfBounds { index: usize, count: usize },
    /// Boundary index out of range (`1..syllable_count`).
    #[error("boundary index {index} out of bounds (valid 1..{count})")]
    BoundaryOutOfBounds { index: usize, count: usize },
    /// Underlying `ass-tools` karaoke split/merge error.
    #[error("karaoke edit error: {0}")]
    Tools(String),
}

/// A syllable marker exposed to the audio display.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct SyllableSpan<'a> {
    pub index: usize,
    pub start_ms: i64,
    pub end_ms: i64,
    pub duration_cs: u32,
    pub kind: Option<KaraokeKind>,
    pub text: &'a str,
}

/// Pure state machine for Aegisub karaoke syllable timing.
#[derive(Debug, Clone)]
pub struct KaraokeTimingController {
    line_index: usize,
    line_start_ms: i64,
    line_end_ms: i64,
    syllables: Vec<KaraokeSyllable>,
    /// Cumulative timestamps in ms: `len == syllables.len() + 1`, with `boundaries_ms[0] == line_start_ms`.
    boundaries_ms: Vec<i64>,
    active_syllable: usize,
    dragging_boundary: Option<usize>,
    dirty: bool,
}

impl KaraokeTimingController {
    /// Loads an [`AssEvent`] into karaoke timing mode.
    ///
    /// If the line has no `\k` tags yet, `parse_karaoke_syllables` returns a single syllable
    /// spanning the full line duration (matching Aegisub's auto-initialize behaviour).
    #[must_use]
    pub fn from_event(line_index: usize, event: &AssEvent) -> Self {
        let line_start_ms = event.start().as_ms().cast_signed();
        let line_end_ms = event.end().as_ms().cast_signed().max(line_start_ms);
        let mut syllables = parse_karaoke_syllables(event.text());
        if syllables.is_empty() {
            syllables = parse_karaoke_syllables(r"{\k0}");
            let dur_cs = u32::try_from((line_end_ms - line_start_ms + 5) / 10).unwrap_or(0);
            if let Some(first) = syllables.first_mut() {
                first.set_duration_cs(dur_cs);
            }
        } else if syllables.len() == 1 && syllables[0].kind.is_none() {
            let dur_cs = u32::try_from((line_end_ms - line_start_ms + 5) / 10).unwrap_or(0);
            syllables[0].kind = Some(KaraokeKind::K);
            syllables[0].set_duration_cs(dur_cs);
        }

        let mut ctrl = Self {
            line_index,
            line_start_ms,
            line_end_ms,
            syllables,
            boundaries_ms: Vec::new(),
            active_syllable: 0,
            dragging_boundary: None,
            dirty: false,
        };
        ctrl.rebuild_boundaries();
        ctrl
    }

    /// Number of syllables in the active line.
    #[must_use]
    pub fn syllable_count(&self) -> usize {
        self.syllables.len()
    }

    /// Currently focused syllable index.
    #[must_use]
    pub const fn active_syllable(&self) -> usize {
        self.active_syllable
    }

    /// Selects a syllable index (clamped to valid range).
    pub fn set_active_syllable(&mut self, index: usize) {
        if !self.syllables.is_empty() {
            self.active_syllable = index.min(self.syllables.len() - 1);
        }
    }

    /// Returns the cumulative boundary array (`len == syllable_count() + 1`) in ms.
    #[must_use]
    pub fn boundaries_ms(&self) -> &[i64] {
        &self.boundaries_ms
    }

    /// Returns metadata for syllable `index`.
    #[must_use]
    pub fn syllable(&self, index: usize) -> Option<SyllableSpan<'_>> {
        let s = self.syllables.get(index)?;
        Some(SyllableSpan {
            index,
            start_ms: self.boundaries_ms[index],
            end_ms: self.boundaries_ms[index + 1],
            duration_cs: s.duration_cs,
            kind: s.kind,
            text: &s.text,
        })
    }

    /// Begins dragging internal syllable boundary `boundary_index` (`1..syllable_count()`),
    /// which separates syllable `boundary_index - 1` and `boundary_index`.
    ///
    /// # Errors
    /// Returns [`KaraokeTimingError::BoundaryOutOfBounds`] if `boundary_index` is not in
    /// `1..syllable_count()`.
    pub fn begin_drag_boundary(&mut self, boundary_index: usize) -> Result<(), KaraokeTimingError> {
        if boundary_index == 0 || boundary_index >= self.syllables.len() {
            return Err(KaraokeTimingError::BoundaryOutOfBounds {
                index: boundary_index,
                count: self.syllables.len(),
            });
        }
        self.dragging_boundary = Some(boundary_index);
        Ok(())
    }

    /// Updates the dragged syllable boundary in O(1) with optional O(log K) snapping against sorted
    /// `snap_targets_ms` (e.g. keyframes). Zero heap allocations.
    ///
    /// Returns the new clamped boundary timestamp in ms.
    #[inline]
    pub fn update_drag_boundary(
        &mut self,
        pointer_ms: i64,
        snap_targets_ms: &[i64],
        snap_threshold_ms: i64,
        disable_snap: bool,
    ) -> Option<i64> {
        let b = self.dragging_boundary?;
        let min_ms = self.boundaries_ms[b - 1];
        let max_ms = self.boundaries_ms[b + 1];

        let mut candidate = pointer_ms.clamp(min_ms, max_ms);
        if !disable_snap && snap_threshold_ms > 0 && !snap_targets_ms.is_empty() {
            let pos = snap_targets_ms.partition_point(|&t| t < candidate);
            let mut best_dist = snap_threshold_ms + 1;
            let mut best_val = candidate;
            if let Some(&r) = snap_targets_ms.get(pos) {
                let d = (r - candidate).abs();
                if d <= snap_threshold_ms && d < best_dist && (min_ms..=max_ms).contains(&r) {
                    best_dist = d;
                    best_val = r;
                }
            }
            if pos > 0 {
                let l = snap_targets_ms[pos - 1];
                let d = (candidate - l).abs();
                if d <= snap_threshold_ms && d < best_dist && (min_ms..=max_ms).contains(&l) {
                    best_val = l;
                }
            }
            candidate = best_val;
        }

        // Quantize to centiseconds (10 ms) as Aegisub karaoke markers do on drag.
        let quant_ms = ((candidate + 5) / 10) * 10;
        let final_ms = quant_ms.clamp(min_ms, max_ms);

        self.boundaries_ms[b] = final_ms;
        let left_cs = u32::try_from((final_ms - min_ms).max(0) / 10).unwrap_or(0);
        let right_cs = u32::try_from((max_ms - final_ms).max(0) / 10).unwrap_or(0);
        self.syllables[b - 1].set_duration_cs(left_cs);
        self.syllables[b].set_duration_cs(right_cs);
        self.dirty = true;
        Some(final_ms)
    }

    /// Ends the active boundary drag.
    pub fn end_drag_boundary(&mut self) {
        self.dragging_boundary = None;
    }

    /// Splits syllable `index` at character offset `char_offset` (delegates to
    /// [`ass_tools::split_syllable`]) and updates boundaries.
    ///
    /// # Errors
    /// Returns [`KaraokeTimingError`] if index or split offset is invalid.
    pub fn split_at(&mut self, index: usize, char_offset: usize) -> Result<(), KaraokeTimingError> {
        if index >= self.syllables.len() {
            return Err(KaraokeTimingError::SyllableOutOfBounds {
                index,
                count: self.syllables.len(),
            });
        }
        if !split_syllable(&mut self.syllables, index, char_offset) {
            return Err(KaraokeTimingError::Tools(format!(
                "cannot split syllable {index} at char offset {char_offset}"
            )));
        }
        self.rebuild_boundaries();
        self.dirty = true;
        Ok(())
    }

    /// Joins syllables `start_index..=end_index` into one syllable (delegates to
    /// [`ass_tools::merge_syllables`]) and updates boundaries.
    ///
    /// # Errors
    /// Returns [`KaraokeTimingError`] if the range is invalid.
    pub fn join_range(
        &mut self,
        start_index: usize,
        end_index: usize,
    ) -> Result<(), KaraokeTimingError> {
        if end_index >= self.syllables.len() || start_index > end_index {
            return Err(KaraokeTimingError::SyllableOutOfBounds {
                index: end_index,
                count: self.syllables.len(),
            });
        }
        if !merge_syllables(&mut self.syllables, start_index, end_index) {
            return Err(KaraokeTimingError::Tools(format!(
                "cannot merge syllables {start_index}..={end_index}"
            )));
        }
        if self.active_syllable >= self.syllables.len() {
            self.active_syllable = self.syllables.len().saturating_sub(1);
        }
        self.rebuild_boundaries();
        self.dirty = true;
        Ok(())
    }

    /// Switches the karaoke tag type (`\k`, `\kf`, `\ko`, `\K`) for one syllable or all syllables.
    ///
    /// # Errors
    /// Returns [`KaraokeTimingError::SyllableOutOfBounds`] if `target_syllable` is out of range.
    pub fn set_tag_type(
        &mut self,
        kind: KaraokeKind,
        target_syllable: Option<usize>,
    ) -> Result<(), KaraokeTimingError> {
        if let Some(idx) = target_syllable {
            let Some(syl) = self.syllables.get_mut(idx) else {
                return Err(KaraokeTimingError::SyllableOutOfBounds {
                    index: idx,
                    count: self.syllables.len(),
                });
            };
            syl.kind = Some(kind);
            syl.set_duration_cs(syl.duration_cs);
        } else {
            for syl in &mut self.syllables {
                if syl.kind.is_some() {
                    syl.kind = Some(kind);
                    syl.set_duration_cs(syl.duration_cs);
                }
            }
        }
        self.dirty = true;
        Ok(())
    }

    /// Playback range `(start_ms, end_ms)` for a single syllable.
    #[must_use]
    pub fn play_syllable_range(&self, index: usize) -> Option<(i64, i64)> {
        if index >= self.syllables.len() {
            return None;
        }
        Some((self.boundaries_ms[index], self.boundaries_ms[index + 1]))
    }

    /// Rebuilds the ASS event text and returns an atomic [`EditCommand::ModifyEventText`] if
    /// changed.
    #[must_use]
    pub fn commit(&mut self, original_text: &str) -> Option<EditCommand> {
        let new_text = rebuild_karaoke_line(&self.syllables);
        self.dirty = false;
        if new_text == original_text {
            return None;
        }
        Some(EditCommand::ModifyEventText {
            index: self.line_index,
            before: original_text.to_owned(),
            after: new_text,
        })
    }

    /// Whether uncommitted karaoke edits exist.
    #[must_use]
    pub const fn is_dirty(&self) -> bool {
        self.dirty
    }

    /// Line end timestamp in ms.
    #[must_use]
    pub const fn line_end_ms(&self) -> i64 {
        self.line_end_ms
    }

    fn rebuild_boundaries(&mut self) {
        self.boundaries_ms.clear();
        self.boundaries_ms.reserve(self.syllables.len() + 1);
        let mut t = self.line_start_ms;
        self.boundaries_ms.push(t);
        for s in &self.syllables {
            t += i64::from(s.duration_cs) * 10;
            self.boundaries_ms.push(t);
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use ass_core::AssTime;

    #[test]
    fn drag_split_join_tag_switch_and_commit() {
        let ev = AssEvent::dialogue(
            AssTime::from_cs(100), // 1000 ms
            AssTime::from_cs(200), // 2000 ms
            r"{\k40}ka{\k60}raoke",
        );
        let mut ctrl = KaraokeTimingController::from_event(0, &ev);
        assert_eq!(ctrl.syllable_count(), 2);
        assert_eq!(ctrl.boundaries_ms(), &[1000, 1400, 2000]);

        // Drag boundary 1 near snap target 1500 ms
        ctrl.begin_drag_boundary(1).expect("begin");
        let snapped = ctrl
            .update_drag_boundary(1512, &[1500], 20, false)
            .expect("snapped");
        assert_eq!(snapped, 1500);
        ctrl.end_drag_boundary();
        assert_eq!(ctrl.play_syllable_range(0), Some((1000, 1500)));
        assert_eq!(ctrl.play_syllable_range(1), Some((1500, 2000)));

        // Split syllable 1 ("raoke") at char 2 -> "ra", "oke"
        ctrl.split_at(1, 2).expect("split");
        assert_eq!(ctrl.syllable_count(), 3);

        // Switch all to \kf
        ctrl.set_tag_type(KaraokeKind::Kf, None).expect("kf");
        let cmd = ctrl.commit(ev.text()).expect("commit");
        match cmd {
            EditCommand::ModifyEventText { after, .. } => {
                assert!(after.contains(r"\kf50"));
                assert!(after.contains("ra"));
                assert!(after.contains("oke"));
            }
            _ => panic!("expected ModifyEventText"),
        }

        // Join syllables 1..=2 back
        ctrl.join_range(1, 2).expect("join");
        assert_eq!(ctrl.syllable_count(), 2);
    }
}
