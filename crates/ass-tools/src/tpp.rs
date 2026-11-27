//! Timing Post-Processor (TPP) matching Aegisub 3.5.1 `Timing -> Timing Post-Processor` dialog.
//!
//! Reference behaviour (Aegisub 3.5.1 manual — Timing Post-Processor):
//! Pipeline runs in three ordered passes over eligible lines (sorted by start time for adjacency):
//! 1. **Add Lead-in / Lead-out**: subtracts `lead_in_ms` from `start` (clamped at 0) and adds
//!    `lead_out_ms` to `end`.
//! 2. **Make adjacent subtitles continuous**: for consecutive lines whose gap `next.start - prev.end`
//!    is within `0 < gap <= threshold_ms`, closes the gap at `prev.end + gap * (1.0 - bias)`
//!    (bias `0.0` = extend first line end to second start; bias `1.0` = pull second start back
//!    to first end; bias `0.5` = meet at midpoint). Optionally also resolves small overlaps
//!    when `prevent_overlap` is enabled.
//! 3. **Keyframe snapping**: snaps `start` to the nearest keyframe within `[kf - start_before_ms,
//!    kf + start_after_ms]` and `end` within `[kf - end_before_ms, kf + end_after_ms]` using
//!    Aegisub's four independent thresholds.

use ass_core::{AssTime, EditCommand, EventKind, EventTimeChange, ScriptFile};

/// Configuration for adjacent-line linking.
#[derive(Debug, Clone, Copy)]
pub struct AdjacentLinkConfig {
    /// Maximum gap (in milliseconds) between adjacent lines to close.
    pub threshold_ms: u32,
    /// Bias in `0.0..=1.0`: `0.0` extends previous end, `1.0` pulls next start back, `0.5` splits evenly.
    pub bias: f64,
    /// When `true`, also resolves overlaps up to `threshold_ms` by meeting at the biased split point.
    pub prevent_overlap: bool,
}

/// Configuration for keyframe snapping using Aegisub's four independent thresholds.
#[derive(Debug, Clone, Copy)]
pub struct KeyframeSnapConfig<'a> {
    /// Sorted list of keyframe timestamps in centiseconds.
    pub keyframes_cs: &'a [u32],
    /// Maximum ms *before* a keyframe that a line's start time may snap forward to the keyframe.
    pub start_before_ms: u32,
    /// Maximum ms *after* a keyframe that a line's start time may snap backward to the keyframe.
    pub start_after_ms: u32,
    /// Maximum ms *before* a keyframe that a line's end time may snap forward to the keyframe.
    pub end_before_ms: u32,
    /// Maximum ms *after* a keyframe that a line's end time may snap backward to the keyframe.
    pub end_after_ms: u32,
}

/// Full Timing Post-Processor options matching Aegisub's dialog.
#[derive(Debug, Clone, Copy, Default)]
pub struct TppOptions<'a> {
    pub lead_in_ms: Option<u32>,
    pub lead_out_ms: Option<u32>,
    pub adjacent: Option<AdjacentLinkConfig>,
    pub keyframes: Option<KeyframeSnapConfig<'a>>,
    pub skip_comments: bool,
    pub styles_filter: Option<&'a [String]>,
}

/// Runs the Timing Post-Processor over `script` and returns one atomic [`EditCommand::Batch`].
#[must_use]
pub fn timing_post_process(script: &ScriptFile, opts: TppOptions<'_>) -> EditCommand {
    let Some(events_sec) = script.events_section() else {
        return EditCommand::Batch(Vec::new());
    };

    // Snapshot event references once in O(N) to avoid O(N^2) indexed lookups.
    let all_events: Vec<&ass_core::AssEvent> = events_sec.events().collect();

    // Collect eligible lines: (event_index, start_cs, end_cs).
    let mut items: Vec<(usize, u32, u32)> = Vec::with_capacity(all_events.len());
    for (idx, &ev) in all_events.iter().enumerate() {
        if opts.skip_comments && ev.kind() == EventKind::Comment {
            continue;
        }
        if let Some(styles) = opts.styles_filter {
            if !styles.iter().any(|s| s == ev.style()) {
                continue;
            }
        }
        items.push((idx, ev.start().as_cs(), ev.end().as_cs()));
    }

    if items.is_empty() {
        return EditCommand::Batch(Vec::new());
    }

    // Pass 1: Lead-in / Lead-out.
    let lead_in_cs = opts.lead_in_ms.map_or(0, ms_to_cs);
    let lead_out_cs = opts.lead_out_ms.map_or(0, ms_to_cs);
    if lead_in_cs > 0 || lead_out_cs > 0 {
        for item in &mut items {
            item.1 = item.1.saturating_sub(lead_in_cs);
            item.2 = item.2.saturating_add(lead_out_cs);
        }
    }

    // Pass 2: Adjacent-line linking (sorted by start time, preserving original index order on ties).
    if let Some(adj) = opts.adjacent {
        let thresh_cs = ms_to_cs(adj.threshold_ms);
        let bias = adj.bias.clamp(0.0, 1.0);
        let mut order: Vec<usize> = (0..items.len()).collect();
        order.sort_by_key(|&i| (items[i].1, items[i].2, items[i].0));

        for pair in order.windows(2) {
            let a = pair[0];
            let b = pair[1];
            let prev_end = items[a].2;
            let next_start = items[b].1;

            if next_start > prev_end {
                let gap = next_start - prev_end;
                if gap <= thresh_cs {
                    // bias 0.0 -> meet at next_start (extend prev); bias 1.0 -> meet at prev_end (pull next).
                    #[allow(clippy::cast_possible_truncation, clippy::cast_sign_loss)]
                    let shift = (f64::from(gap) * (1.0 - bias)).round() as u32;
                    let meet = prev_end.saturating_add(shift).min(next_start);
                    items[a].2 = meet;
                    items[b].1 = meet;
                }
            } else if adj.prevent_overlap && prev_end > next_start {
                let overlap = prev_end - next_start;
                if overlap <= thresh_cs {
                    #[allow(clippy::cast_possible_truncation, clippy::cast_sign_loss)]
                    let shift = (f64::from(overlap) * (1.0 - bias)).round() as u32;
                    let meet = next_start.saturating_add(shift).min(prev_end);
                    // Never invert a line (keep end >= start).
                    if meet >= items[a].1 && meet <= items[b].2 {
                        items[a].2 = meet;
                        items[b].1 = meet;
                    }
                }
            }
        }
    }

    // Pass 3: Keyframe snapping with Aegisub's 4 independent thresholds.
    if let Some(kf) = opts.keyframes {
        if !kf.keyframes_cs.is_empty() {
            let start_before = ms_to_cs(kf.start_before_ms);
            let start_after = ms_to_cs(kf.start_after_ms);
            let end_before = ms_to_cs(kf.end_before_ms);
            let end_after = ms_to_cs(kf.end_after_ms);

            for item in &mut items {
                let snapped_start =
                    snap_to_keyframe(item.1, kf.keyframes_cs, start_before, start_after);
                let snapped_end = snap_to_keyframe(item.2, kf.keyframes_cs, end_before, end_after);
                if snapped_start <= snapped_end {
                    item.1 = snapped_start;
                    item.2 = snapped_end;
                }
            }
        }
    }

    // Emit compact timestamp changes only for lines whose timestamps actually changed.
    let mut changes = Vec::with_capacity(items.len());
    for (idx, new_start_cs, new_end_cs) in items {
        if let Some(&ev) = all_events.get(idx) {
            let new_start = AssTime::from_cs(new_start_cs);
            let new_end = AssTime::from_cs(new_end_cs);
            if new_start != ev.start() || new_end != ev.end() {
                changes.push(EventTimeChange {
                    index: idx,
                    before_start: ev.start(),
                    before_end: ev.end(),
                    after_start: new_start,
                    after_end: new_end,
                });
            }
        }
    }

    EditCommand::ModifyEventTimes(changes)
}

#[inline]
const fn ms_to_cs(ms: u32) -> u32 {
    ms.saturating_add(5) / 10
}

/// Snaps `time_cs` to the closest eligible keyframe in sorted `keyframes_cs`:
/// - a keyframe `kf >= time_cs` is eligible if `kf - time_cs <= before_cs` (line is *before* kf)
/// - a keyframe `kf < time_cs` is eligible if `time_cs - kf <= after_cs` (line is *after* kf)
fn snap_to_keyframe(time_cs: u32, keyframes_cs: &[u32], before_cs: u32, after_cs: u32) -> u32 {
    let pos = keyframes_cs.partition_point(|&kf| kf < time_cs);
    let mut best: Option<(u32, u32)> = None; // (distance, kf_cs)

    if pos < keyframes_cs.len() {
        let kf = keyframes_cs[pos];
        let dist = kf - time_cs;
        if dist <= before_cs {
            best = Some((dist, kf));
        }
    }
    if pos > 0 {
        let kf = keyframes_cs[pos - 1];
        let dist = time_cs - kf;
        if dist <= after_cs && best.is_none_or(|(best_d, _)| dist < best_d) {
            best = Some((dist, kf));
        }
    }

    best.map_or(time_cs, |(_, kf)| kf)
}

#[cfg(test)]
mod tests {
    use super::*;
    use ass_core::{AssEvent, EventEntry};

    #[test]
    fn lead_in_out_adjacent_linking_and_keyframe_snapping_handle_edge_cases() {
        let mut script = ScriptFile::new_default();
        let mut comment = AssEvent::dialogue(AssTime::from_cs(100), AssTime::from_cs(200), "Cmt");
        comment.set_kind(EventKind::Comment);
        script.ensure_events_section_mut().entries = vec![
            EventEntry::Event(comment),
            // Line 1: 100..280 cs; Line 2: 300..490 cs (20 cs = 200 ms gap).
            EventEntry::Event(AssEvent::dialogue(
                AssTime::from_cs(100),
                AssTime::from_cs(280),
                "First",
            )),
            EventEntry::Event(AssEvent::dialogue(
                AssTime::from_cs(300),
                AssTime::from_cs(490),
                "Second",
            )),
        ];

        // Keyframes at 95 cs and 500 cs.
        let kfs = [95u32, 500u32];
        let cmd = timing_post_process(
            &script,
            TppOptions {
                lead_in_ms: Some(0),
                lead_out_ms: Some(0),
                adjacent: Some(AdjacentLinkConfig {
                    threshold_ms: 250,
                    bias: 0.5,
                    prevent_overlap: false,
                }),
                keyframes: Some(KeyframeSnapConfig {
                    keyframes_cs: &kfs,
                    start_before_ms: 100,
                    start_after_ms: 100,
                    end_before_ms: 150,
                    end_after_ms: 100,
                }),
                skip_comments: true,
                styles_filter: None,
            },
        );
        cmd.apply(&mut script).expect("apply tpp");

        let evs: Vec<&AssEvent> = script.events().collect();
        // Comment untouched.
        assert_eq!(evs[0].start(), AssTime::from_cs(100));
        // Line 1 start snapped back 5 cs to keyframe 95; end met Line 2 at midpoint 290.
        assert_eq!(evs[1].start(), AssTime::from_cs(95));
        assert_eq!(evs[1].end(), AssTime::from_cs(290));
        // Line 2 start met at 290; end snapped forward 10 cs to keyframe 500.
        assert_eq!(evs[2].start(), AssTime::from_cs(290));
        assert_eq!(evs[2].end(), AssTime::from_cs(500));
    }

    #[test]
    fn adjacent_overlap_resolution_and_line_crossing_keyframe() {
        let mut script = ScriptFile::new_default();
        script.ensure_events_section_mut().entries = vec![
            EventEntry::Event(AssEvent::dialogue(
                AssTime::from_cs(100),
                AssTime::from_cs(220),
                "A",
            )),
            EventEntry::Event(AssEvent::dialogue(
                AssTime::from_cs(200),
                AssTime::from_cs(350),
                "B",
            )),
        ];

        // Keyframe at 250 cs is inside Line B (200..350), far from both endpoints -> no snap.
        let kfs = [250u32];
        let cmd = timing_post_process(
            &script,
            TppOptions {
                lead_in_ms: None,
                lead_out_ms: None,
                adjacent: Some(AdjacentLinkConfig {
                    threshold_ms: 300,
                    bias: 0.5,
                    prevent_overlap: true,
                }),
                keyframes: Some(KeyframeSnapConfig {
                    keyframes_cs: &kfs,
                    start_before_ms: 100,
                    start_after_ms: 100,
                    end_before_ms: 100,
                    end_after_ms: 100,
                }),
                skip_comments: true,
                styles_filter: None,
            },
        );
        cmd.apply(&mut script).expect("apply");
        let evs: Vec<&AssEvent> = script.events().collect();
        assert_eq!(evs[0].end(), AssTime::from_cs(210));
        assert_eq!(evs[1].start(), AssTime::from_cs(210));
        assert_eq!(evs[1].end(), AssTime::from_cs(350));
    }
}
