//! Pure state machine for Aegisub 3.5.1 dialogue audio timing mode
//! (`AudioTimingControllerDialogue` in `src/audio_timing_dialogue.cpp` and `src/audio_controller.cpp`).
//!
//! All internal times are in milliseconds (`i64`) and rounded to centiseconds on commit using
//! [`ass_core::AssTime::from_ms_rounded`]. Snap queries use O(log N) binary search over pre-sorted
//! keyframe and line-boundary indices so mouse-move drag updates stay sub-microsecond even on
//! 20,000-line scripts with 3,000 keyframes.

#![forbid(unsafe_code)]
#![cfg_attr(not(test), deny(clippy::unwrap_used, clippy::expect_used))]

use ass_core::{AssEvent, AssTime, EditCommand, ScriptFile};
use keyframes::{TimeType, Timecodes};

/// Which inactive lines' markers are displayed on the audio timeline (mirrors Aegisub
/// `Audio/Inactive Lines Display Mode`).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum InactiveLinesMode {
    None,
    Previous,
    PreviousAndNext,
    All,
}

/// Which marker is currently being dragged.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum DragTarget {
    Start,
    End,
    Both { anchor_offset_ms: i64 },
}

/// Source that a drag operation snapped to.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum SnapSource {
    Keyframe,
    VideoPosition,
    LineBoundary,
}

/// Result of a snap query.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct SnapResult {
    pub snapped_ms: i64,
    pub source: Option<SnapSource>,
}

/// Play-range presets matching Aegisub `src/command/audio.cpp`.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum PlayRangeKind {
    Selection,
    BeforeStart500ms,
    AfterStart500ms,
    BeforeEnd500ms,
    AfterEnd500ms,
    First500ms,
    Last500ms,
    ToEnd,
    CurrentLine,
    BetweenMarkers { start_ms: i64, end_ms: i64 },
}

/// Half-open playback range `[start_ms, end_ms]` in milliseconds.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct TimeRangeMs {
    pub start_ms: i64,
    pub end_ms: i64,
}

/// Video position marker + its frame-duration highlight area (`[frame_start_ms, frame_end_ms]`).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct VideoPositionMarker {
    pub position_ms: i64,
    pub frame_start_ms: i64,
    pub frame_end_ms: i64,
}

/// An inactive line's start/end span for display in the audio view.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct InactiveLineSpan {
    pub line_index: usize,
    pub start_ms: i64,
    pub end_ms: i64,
}

/// Configuration for [`DialogueTimingController`].
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
#[allow(clippy::struct_excessive_bools)]
pub struct TimingControllerConfig {
    pub auto_commit: bool,
    pub go_to_next_on_commit: bool,
    pub inactive_mode: InactiveLinesMode,
    pub lead_in_ms: i64,
    pub lead_out_ms: i64,
    pub snap_to_keyframes: bool,
    pub snap_to_video: bool,
    pub snap_to_lines: bool,
}

impl Default for TimingControllerConfig {
    fn default() -> Self {
        Self {
            auto_commit: true,
            go_to_next_on_commit: false,
            inactive_mode: InactiveLinesMode::PreviousAndNext,
            lead_in_ms: 200,
            lead_out_ms: 300,
            snap_to_keyframes: true,
            snap_to_video: true,
            snap_to_lines: true,
        }
    }
}

/// Pre-indexed sorted snap targets (rebuilt only when script/keyframes change, never per mouse move).
#[derive(Debug, Default, Clone)]
pub struct SnapIndex {
    keyframe_ms: Vec<i64>,
    line_boundary_ms: Vec<i64>,
}

impl SnapIndex {
    /// Builds a sorted snap index from script events and keyframe timestamps (ms).
    #[must_use]
    pub fn build(script: &ScriptFile, keyframes_ms: &[i64]) -> Self {
        let mut kf = keyframes_ms.to_vec();
        kf.sort_unstable();
        kf.dedup();

        let mut bounds = Vec::with_capacity(script.events().count() * 2);
        for ev in script.events() {
            bounds.push(ev.start().as_ms().cast_signed());
            bounds.push(ev.end().as_ms().cast_signed());
        }
        bounds.sort_unstable();
        bounds.dedup();

        Self {
            keyframe_ms: kf,
            line_boundary_ms: bounds,
        }
    }

    /// Builds a sorted snap index from raw sorted or unsorted slices (useful for tests/benches).
    #[must_use]
    pub fn from_raw(mut keyframe_ms: Vec<i64>, mut line_boundary_ms: Vec<i64>) -> Self {
        keyframe_ms.sort_unstable();
        keyframe_ms.dedup();
        line_boundary_ms.sort_unstable();
        line_boundary_ms.dedup();
        Self {
            keyframe_ms,
            line_boundary_ms,
        }
    }

    /// Finds the nearest value in a sorted slice within `threshold_ms`, in O(log N) with zero allocs.
    #[inline]
    fn nearest_in_sorted(sorted: &[i64], target_ms: i64, threshold_ms: i64) -> Option<(i64, i64)> {
        if sorted.is_empty() || threshold_ms < 0 {
            return None;
        }
        let pos = sorted.partition_point(|&t| t < target_ms);
        let mut best: Option<(i64, i64)> = None;
        if let Some(&right) = sorted.get(pos) {
            let d = (right - target_ms).abs();
            if d <= threshold_ms {
                best = Some((d, right));
            }
        }
        if pos > 0 {
            let left = sorted[pos - 1];
            let d = (target_ms - left).abs();
            if d <= threshold_ms && best.is_none_or(|(bd, _)| d < bd) {
                best = Some((d, left));
            }
        }
        best
    }

    /// Snaps `target_ms` against enabled snap sources within `threshold_ms`.
    #[inline]
    #[must_use]
    pub fn snap(
        &self,
        target_ms: i64,
        threshold_ms: i64,
        video_pos_ms: Option<i64>,
        cfg: &TimingControllerConfig,
        disable_snap: bool,
    ) -> SnapResult {
        if disable_snap || threshold_ms <= 0 {
            return SnapResult {
                snapped_ms: target_ms,
                source: None,
            };
        }

        let mut best_dist = threshold_ms + 1;
        let mut best_val = target_ms;
        let mut best_src: Option<SnapSource> = None;

        if cfg.snap_to_keyframes {
            if let Some((d, v)) =
                Self::nearest_in_sorted(&self.keyframe_ms, target_ms, threshold_ms)
            {
                if d < best_dist {
                    best_dist = d;
                    best_val = v;
                    best_src = Some(SnapSource::Keyframe);
                }
            }
        }
        if cfg.snap_to_video {
            if let Some(vp) = video_pos_ms {
                let d = (vp - target_ms).abs();
                if d <= threshold_ms && d < best_dist {
                    best_dist = d;
                    best_val = vp;
                    best_src = Some(SnapSource::VideoPosition);
                }
            }
        }
        if cfg.snap_to_lines {
            if let Some((d, v)) =
                Self::nearest_in_sorted(&self.line_boundary_ms, target_ms, threshold_ms)
            {
                if d < best_dist {
                    best_val = v;
                    best_src = Some(SnapSource::LineBoundary);
                }
            }
        }

        SnapResult {
            snapped_ms: best_val,
            source: best_src,
        }
    }
}

/// Output actions emitted by [`DialogueTimingController`].
#[derive(Debug, Clone, PartialEq)]
pub struct CommitOutcome {
    pub edit_command: Option<EditCommand>,
    pub advance_to_line: Option<usize>,
}

/// Pure dialogue audio timing state machine.
#[derive(Debug, Clone)]
pub struct DialogueTimingController {
    pub config: TimingControllerConfig,
    active_line_index: usize,
    /// Original line start/end in ms (as loaded from the script).
    committed_start_ms: i64,
    committed_end_ms: i64,
    /// Live editable selection markers in ms.
    start_ms: i64,
    end_ms: i64,
    audio_duration_ms: i64,
    video_marker: Option<VideoPositionMarker>,
    active_drag: Option<DragTarget>,
    dirty: bool,
}

impl DialogueTimingController {
    /// Creates a new controller initialized to a line's start/end times.
    #[must_use]
    pub fn new(config: TimingControllerConfig, audio_duration_ms: i64) -> Self {
        Self {
            config,
            active_line_index: 0,
            committed_start_ms: 0,
            committed_end_ms: 0,
            start_ms: 0,
            end_ms: 0,
            audio_duration_ms: audio_duration_ms.max(0),
            video_marker: None,
            active_drag: None,
            dirty: false,
        }
    }

    /// Loads an active line from the script into the controller.
    pub fn select_line(&mut self, line_index: usize, event: &AssEvent) {
        self.active_line_index = line_index;
        let s = event.start().as_ms().cast_signed();
        let e = event.end().as_ms().cast_signed();
        self.committed_start_ms = s;
        self.committed_end_ms = e;
        self.start_ms = s;
        self.end_ms = e;
        self.active_drag = None;
        self.dirty = false;
    }

    /// Current active selection range `(start_ms, end_ms)`.
    #[must_use]
    pub const fn selection_ms(&self) -> TimeRangeMs {
        TimeRangeMs {
            start_ms: self.start_ms,
            end_ms: self.end_ms,
        }
    }

    /// Whether the markers have uncommitted edits.
    #[must_use]
    pub const fn is_dirty(&self) -> bool {
        self.dirty
    }

    /// Updates the video position marker and its frame-duration highlight rectangle using `timecodes`.
    pub fn set_video_frame(&mut self, frame: u32, timecodes: &Timecodes) {
        let pos = timecodes.frame_to_time_ms(frame, TimeType::Exact);
        let f_start = timecodes.frame_to_time_ms(frame, TimeType::Start);
        let f_end = timecodes.frame_to_time_ms(frame, TimeType::End);
        self.video_marker = Some(VideoPositionMarker {
            position_ms: pos,
            frame_start_ms: f_start,
            frame_end_ms: f_end.max(f_start),
        });
    }

    /// Returns the current video position marker, if any.
    #[must_use]
    pub const fn video_marker(&self) -> Option<VideoPositionMarker> {
        self.video_marker
    }

    /// Begins dragging a marker (`Start`, `End`, or `Both`).
    pub fn begin_drag(&mut self, target: DragTarget) {
        self.active_drag = Some(target);
    }

    /// Updates the dragged marker position with O(log N) snapping. Zero heap allocations.
    #[inline]
    pub fn update_drag(
        &mut self,
        pointer_ms: i64,
        snap_threshold_ms: i64,
        disable_snap: bool,
        snap_index: &SnapIndex,
    ) -> SnapResult {
        let clamped_ptr = pointer_ms.clamp(0, self.audio_duration_ms.max(pointer_ms.max(0)));
        let video_ms = self.video_marker.map(|v| v.position_ms);

        let Some(target) = self.active_drag else {
            return SnapResult {
                snapped_ms: clamped_ptr,
                source: None,
            };
        };

        let snap = snap_index.snap(
            clamped_ptr,
            snap_threshold_ms,
            video_ms,
            &self.config,
            disable_snap,
        );

        match target {
            DragTarget::Start => {
                self.start_ms = snap.snapped_ms.clamp(0, self.end_ms);
                self.dirty = true;
            }
            DragTarget::End => {
                self.end_ms = snap.snapped_ms.max(self.start_ms);
                self.dirty = true;
            }
            DragTarget::Both { anchor_offset_ms } => {
                let dur = self.end_ms - self.start_ms;
                let desired_start = (clamped_ptr - anchor_offset_ms).max(0);
                let snapped = snap_index.snap(
                    desired_start,
                    snap_threshold_ms,
                    video_ms,
                    &self.config,
                    disable_snap,
                );
                self.start_ms = snapped.snapped_ms.max(0);
                self.end_ms = self.start_ms + dur;
                self.dirty = true;
                return snapped;
            }
        }
        snap
    }

    /// Ends the active drag; if `config.auto_commit` is enabled, commits immediately.
    pub fn end_drag(&mut self, script: &ScriptFile) -> Option<CommitOutcome> {
        self.active_drag = None;
        if self.config.auto_commit && self.dirty {
            Some(self.commit(script))
        } else {
            None
        }
    }

    /// Adds configured `lead_in_ms` to the start marker (clamped to `>= 0`).
    pub fn add_lead_in(&mut self) {
        self.start_ms = (self.start_ms - self.config.lead_in_ms).max(0);
        self.dirty = true;
    }

    /// Adds configured `lead_out_ms` to the end marker.
    pub fn add_lead_out(&mut self) {
        self.end_ms = self.end_ms.saturating_add(self.config.lead_out_ms);
        self.dirty = true;
    }

    /// Commits current ms markers to `AssTime` (centisecond-rounded) and emits an atomic
    /// [`EditCommand::ModifyEvent`] plus optional `advance_to_line`.
    pub fn commit(&mut self, script: &ScriptFile) -> CommitOutcome {
        let new_start = AssTime::from_ms_rounded(u64::try_from(self.start_ms.max(0)).unwrap_or(0));
        let new_end = AssTime::from_ms_rounded(u64::try_from(self.end_ms.max(0)).unwrap_or(0));

        let ev_count = script
            .events_section()
            .map_or(0, ass_core::EventsSection::event_count);
        let edit_command = script
            .events_section()
            .and_then(|sec| sec.get_event(self.active_line_index))
            .and_then(|before| {
                if before.start() == new_start && before.end() == new_end {
                    return None;
                }
                let mut after = before.clone();
                after.set_start(new_start);
                after.set_end(new_end);
                Some(EditCommand::ModifyEvent {
                    index: self.active_line_index,
                    before: Box::new(before.clone()),
                    after: Box::new(after),
                })
            });

        self.committed_start_ms = new_start.as_ms().cast_signed();
        self.committed_end_ms = new_end.as_ms().cast_signed();
        self.dirty = false;

        let advance_to_line =
            if self.config.go_to_next_on_commit && self.active_line_index + 1 < ev_count {
                Some(self.active_line_index + 1)
            } else {
                None
            };

        CommitOutcome {
            edit_command,
            advance_to_line,
        }
    }

    /// Resolves any Aegisub play-range command into a concrete `[start_ms, end_ms]` span.
    #[must_use]
    pub fn play_range(&self, kind: PlayRangeKind) -> TimeRangeMs {
        let (s, e) = (self.start_ms, self.end_ms);
        match kind {
            PlayRangeKind::Selection => TimeRangeMs {
                start_ms: s,
                end_ms: e,
            },
            PlayRangeKind::BeforeStart500ms => TimeRangeMs {
                start_ms: (s - 500).max(0),
                end_ms: s,
            },
            PlayRangeKind::AfterStart500ms => TimeRangeMs {
                start_ms: s,
                end_ms: s + 500,
            },
            PlayRangeKind::BeforeEnd500ms => TimeRangeMs {
                start_ms: (e - 500).max(0),
                end_ms: e,
            },
            PlayRangeKind::AfterEnd500ms => TimeRangeMs {
                start_ms: e,
                end_ms: e + 500,
            },
            PlayRangeKind::First500ms => TimeRangeMs {
                start_ms: s,
                end_ms: (s + 500).min(e),
            },
            PlayRangeKind::Last500ms => TimeRangeMs {
                start_ms: (e - 500).max(s),
                end_ms: e,
            },
            PlayRangeKind::ToEnd => TimeRangeMs {
                start_ms: s,
                end_ms: self.audio_duration_ms.max(e),
            },
            PlayRangeKind::CurrentLine => TimeRangeMs {
                start_ms: self.committed_start_ms,
                end_ms: self.committed_end_ms,
            },
            PlayRangeKind::BetweenMarkers { start_ms, end_ms } => TimeRangeMs {
                start_ms: start_ms.min(end_ms),
                end_ms: start_ms.max(end_ms),
            },
        }
    }

    /// Target timestamp (ms) to scroll the audio view to show the active line's start.
    #[must_use]
    pub const fn scroll_target_start(&self) -> i64 {
        self.start_ms
    }

    /// Target timestamp (ms) to scroll the audio view to show the active line's end.
    #[must_use]
    pub const fn scroll_target_end(&self) -> i64 {
        self.end_ms
    }

    /// Collects inactive line spans according to `config.inactive_mode` into `out` (reusing buffer).
    pub fn collect_inactive_lines(&self, script: &ScriptFile, out: &mut Vec<InactiveLineSpan>) {
        out.clear();
        let Some(sec) = script.events_section() else {
            return;
        };
        let active = self.active_line_index;
        match self.config.inactive_mode {
            InactiveLinesMode::None => {}
            InactiveLinesMode::Previous => {
                if active > 0 {
                    if let Some(ev) = sec.get_event(active - 1) {
                        out.push(InactiveLineSpan {
                            line_index: active - 1,
                            start_ms: ev.start().as_ms().cast_signed(),
                            end_ms: ev.end().as_ms().cast_signed(),
                        });
                    }
                }
            }
            InactiveLinesMode::PreviousAndNext => {
                if active > 0 {
                    if let Some(ev) = sec.get_event(active - 1) {
                        out.push(InactiveLineSpan {
                            line_index: active - 1,
                            start_ms: ev.start().as_ms().cast_signed(),
                            end_ms: ev.end().as_ms().cast_signed(),
                        });
                    }
                }
                if let Some(ev) = sec.get_event(active + 1) {
                    out.push(InactiveLineSpan {
                        line_index: active + 1,
                        start_ms: ev.start().as_ms().cast_signed(),
                        end_ms: ev.end().as_ms().cast_signed(),
                    });
                }
            }
            InactiveLinesMode::All => {
                for (i, ev) in sec.events().enumerate() {
                    if i != active {
                        out.push(InactiveLineSpan {
                            line_index: i,
                            start_ms: ev.start().as_ms().cast_signed(),
                            end_ms: ev.end().as_ms().cast_signed(),
                        });
                    }
                }
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn drag_snap_commit_and_play_ranges() {
        let mut script = ass_core::parse_ass(
            "[Events]\nFormat: Layer, Start, End, Style, Name, MarginL, MarginR, MarginV, Effect, Text\nDialogue: 0,0:00:01.00,0:00:03.00,Default,,0,0,0,,Line 0\nDialogue: 0,0:00:05.00,0:00:07.00,Default,,0,0,0,,Line 1\n",
        )
        .value;

        let snap_idx = SnapIndex::build(&script, &[1250, 3500]);
        let mut ctrl = DialogueTimingController::new(
            TimingControllerConfig {
                go_to_next_on_commit: true,
                ..TimingControllerConfig::default()
            },
            60_000,
        );
        let ev0 = script.events().next().expect("ev0").clone();
        ctrl.select_line(0, &ev0);

        // Drag start near keyframe 1250 ms (within 30 ms threshold) -> snaps to 1250 ms
        ctrl.begin_drag(DragTarget::Start);
        let snap = ctrl.update_drag(1265, 30, false, &snap_idx);
        assert_eq!(snap.snapped_ms, 1250);
        assert_eq!(snap.source, Some(SnapSource::Keyframe));

        // Modifier disables snap
        let nosnap = ctrl.update_drag(1265, 30, true, &snap_idx);
        assert_eq!(nosnap.snapped_ms, 1265);
        assert_eq!(nosnap.source, None);

        // Snap back to 1250 and auto-commit on drag end
        ctrl.update_drag(1255, 30, false, &snap_idx);
        let outcome = ctrl.end_drag(&script).expect("auto commit");
        assert_eq!(outcome.advance_to_line, Some(1));
        outcome
            .edit_command
            .expect("cmd")
            .apply(&mut script)
            .expect("apply");
        assert_eq!(script.events().next().expect("ev0").start().as_cs(), 125);

        // Play ranges
        assert_eq!(
            ctrl.play_range(PlayRangeKind::BeforeStart500ms),
            TimeRangeMs {
                start_ms: 750,
                end_ms: 1250
            }
        );
        assert_eq!(
            ctrl.play_range(PlayRangeKind::First500ms),
            TimeRangeMs {
                start_ms: 1250,
                end_ms: 1750
            }
        );
    }
}
