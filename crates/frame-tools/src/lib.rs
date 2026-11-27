//! Aegisub 3.5.1 frame-based timing tools:
//! - Framerate Transform (`src/export_framerate.cpp` `AssTransformFramerateFilter`), including
//!   CFR<->VFR and rewriting `\t`, `\move`, `\fad`, `\fade`, and karaoke `\k` times inside text.
//! - Snap to video frame (`src/command/time.cpp` `time/snap/scene` & frame-boundary snapping).
//! - Make times continuous (`src/command/time.cpp` `time/continuous/start` & `time/continuous/end`).
//! - Recombine lines (`src/command/edit.cpp` `edit_line_recombine`).

#![forbid(unsafe_code)]
#![cfg_attr(not(test), deny(clippy::unwrap_used, clippy::expect_used))]

pub mod tag_time;

use ass_core::{AssEvent, AssTime, EditCommand, EventEntry, ScriptFile};
use keyframes::{TimeType, Timecodes};
pub use tag_time::{convert_time_ms, transform_text_time_tags};
use thiserror::Error;

/// Errors returned by `frame-tools`.
#[derive(Debug, Error, PartialEq, Eq)]
pub enum FrameToolsError {
    /// Script has no `[Events]` section.
    #[error("script has no [Events] section")]
    MissingEventsSection,
}

/// Direction for [`make_times_continuous`] (mirrors Aegisub `time/continuous/start` & `end`).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ContinuousMode {
    /// Adjust each subsequent line's `Start` to match the previous line's `End`.
    ChangeStart,
    /// Adjust each preceding line's `End` to match the next line's `Start`.
    ChangeEnd,
}

/// Transforms all events in `script` from `input` timecodes to `output` timecodes (CFR<->VFR),
/// keeping timing locked to video frames and rewriting `\t`/`\move`/`\fad`/`\fade`/`\k` tag times
/// inside `Text`, mutating `script` in place with zero unnecessary event clones.
///
/// Returns the number of events modified.
///
/// # Errors
/// Returns [`FrameToolsError::MissingEventsSection`] if `[Events]` is absent.
pub fn transform_framerate_in_place(
    script: &mut ScriptFile,
    input: &Timecodes,
    output: &Timecodes,
) -> Result<usize, FrameToolsError> {
    let sec = script
        .events_section_mut()
        .ok_or(FrameToolsError::MissingEventsSection)?;
    let mut changed = 0usize;

    for entry in &mut sec.entries {
        let EventEntry::Event(ev) = entry else {
            continue;
        };
        let old_start = ev.start();
        let old_end = ev.end();
        let f_start = input.time_cs_to_frame(old_start.as_cs(), TimeType::Start);
        let f_end = input.time_cs_to_frame(old_end.as_cs(), TimeType::End);
        let new_start = AssTime::from_cs(output.frame_to_time_cs(f_start, TimeType::Start));
        let new_end =
            AssTime::from_cs(output.frame_to_time_cs(f_end, TimeType::End)).max(new_start);

        let new_text = transform_text_time_tags(
            ev.text(),
            old_start.as_ms().cast_signed(),
            old_end.as_ms().cast_signed(),
            new_start.as_ms().cast_signed(),
            new_end.as_ms().cast_signed(),
            input,
            output,
        );

        if new_start != old_start || new_end != old_end || new_text.is_some() {
            ev.set_start(new_start);
            ev.set_end(new_end);
            if let Some(txt) = new_text {
                ev.set_text(txt);
            }
            changed += 1;
        }
    }

    Ok(changed)
}

/// Atomic undoable version of Framerate Transform returning an [`EditCommand::Batch`].
///
/// # Errors
/// Returns [`FrameToolsError::MissingEventsSection`] if `[Events]` is absent.
pub fn transform_framerate(
    script: &ScriptFile,
    input: &Timecodes,
    output: &Timecodes,
) -> Result<EditCommand, FrameToolsError> {
    let sec = script
        .events_section()
        .ok_or(FrameToolsError::MissingEventsSection)?;
    let mut cmds = Vec::new();

    for (idx, ev) in sec.events().enumerate() {
        let old_start = ev.start();
        let old_end = ev.end();
        let f_start = input.time_cs_to_frame(old_start.as_cs(), TimeType::Start);
        let f_end = input.time_cs_to_frame(old_end.as_cs(), TimeType::End);
        let new_start = AssTime::from_cs(output.frame_to_time_cs(f_start, TimeType::Start));
        let new_end =
            AssTime::from_cs(output.frame_to_time_cs(f_end, TimeType::End)).max(new_start);

        let new_text = transform_text_time_tags(
            ev.text(),
            old_start.as_ms().cast_signed(),
            old_end.as_ms().cast_signed(),
            new_start.as_ms().cast_signed(),
            new_end.as_ms().cast_signed(),
            input,
            output,
        );

        if new_start != old_start || new_end != old_end || new_text.is_some() {
            let before = ev.clone();
            let mut after = before.clone();
            after.set_start(new_start);
            after.set_end(new_end);
            if let Some(txt) = new_text {
                after.set_text(txt);
            }
            cmds.push(EditCommand::ModifyEvent {
                index: idx,
                before: Box::new(before),
                after: Box::new(after),
            });
        }
    }

    Ok(EditCommand::Batch(cmds))
}

/// Snaps selected events (or all events when `selection` is `None`) to exact video frame
/// boundaries in-place using Aegisub's `TimeType::Start` / `TimeType::End` semantics.
///
/// Returns the number of events modified.
///
/// # Errors
/// Returns [`FrameToolsError::MissingEventsSection`] if `[Events]` is absent.
pub fn snap_to_video_frames_in_place(
    script: &mut ScriptFile,
    selection: Option<&[usize]>,
    timecodes: &Timecodes,
) -> Result<usize, FrameToolsError> {
    let sec = script
        .events_section_mut()
        .ok_or(FrameToolsError::MissingEventsSection)?;
    let mut changed = 0usize;

    if let Some(sel) = selection {
        let mut sorted: Vec<usize> = sel.to_vec();
        sorted.sort_unstable();
        sorted.dedup();
        let mut sel_cursor = 0usize;
        let mut logical_idx = 0usize;
        for entry in &mut sec.entries {
            let EventEntry::Event(ev) = entry else {
                continue;
            };
            if sel_cursor < sorted.len() && sorted[sel_cursor] == logical_idx {
                if snap_one_event(ev, timecodes) {
                    changed += 1;
                }
                sel_cursor += 1;
            }
            logical_idx += 1;
        }
    } else {
        for entry in &mut sec.entries {
            let EventEntry::Event(ev) = entry else {
                continue;
            };
            if snap_one_event(ev, timecodes) {
                changed += 1;
            }
        }
    }

    Ok(changed)
}

/// Atomic undoable version of Snap to Video Frame returning an [`EditCommand::Batch`].
///
/// # Errors
/// Returns [`FrameToolsError::MissingEventsSection`] if `[Events]` is absent.
pub fn snap_to_video_frames(
    script: &ScriptFile,
    selection: Option<&[usize]>,
    timecodes: &Timecodes,
) -> Result<EditCommand, FrameToolsError> {
    let sec = script
        .events_section()
        .ok_or(FrameToolsError::MissingEventsSection)?;
    let evs: Vec<&AssEvent> = sec.events().collect();
    let indices: Vec<usize> = match selection {
        Some(sel) => {
            let mut v: Vec<usize> = sel.iter().copied().filter(|&i| i < evs.len()).collect();
            v.sort_unstable();
            v.dedup();
            v
        }
        None => (0..evs.len()).collect(),
    };

    let mut cmds = Vec::new();
    for idx in indices {
        let ev = evs[idx];
        let mut after = ev.clone();
        if snap_one_event(&mut after, timecodes) {
            cmds.push(EditCommand::ModifyEvent {
                index: idx,
                before: Box::new(ev.clone()),
                after: Box::new(after),
            });
        }
    }
    Ok(EditCommand::Batch(cmds))
}

#[inline]
fn snap_one_event(ev: &mut AssEvent, timecodes: &Timecodes) -> bool {
    let old_s = ev.start();
    let old_e = ev.end();
    let fs = timecodes.time_cs_to_frame(old_s.as_cs(), TimeType::Start);
    let fe = timecodes.time_cs_to_frame(old_e.as_cs(), TimeType::End);
    let new_s = AssTime::from_cs(timecodes.frame_to_time_cs(fs, TimeType::Start));
    let new_e = AssTime::from_cs(timecodes.frame_to_time_cs(fe, TimeType::End)).max(new_s);
    if new_s != old_s || new_e != old_e {
        ev.set_start(new_s);
        ev.set_end(new_e);
        true
    } else {
        false
    }
}

/// Makes consecutive selected lines continuous (mirrors Aegisub `src/command/time.cpp`
/// `time/continuous/start` and `time/continuous/end`).
///
/// # Errors
/// Returns [`FrameToolsError::MissingEventsSection`] if `[Events]` is absent.
pub fn make_times_continuous(
    script: &ScriptFile,
    selected: &[usize],
    mode: ContinuousMode,
) -> Result<EditCommand, FrameToolsError> {
    let sec = script
        .events_section()
        .ok_or(FrameToolsError::MissingEventsSection)?;
    let evs: Vec<&AssEvent> = sec.events().collect();
    let mut idxs: Vec<usize> = selected
        .iter()
        .copied()
        .filter(|&i| i < evs.len())
        .collect();
    idxs.sort_unstable();
    idxs.dedup();
    if idxs.len() < 2 {
        return Ok(EditCommand::Batch(Vec::new()));
    }

    let mut cmds = Vec::new();
    match mode {
        ContinuousMode::ChangeStart => {
            for pair in idxs.windows(2) {
                let prev_end = evs[pair[0]].end();
                let cur = evs[pair[1]];
                if cur.start() != prev_end {
                    let before = cur.clone();
                    let mut after = before.clone();
                    after.set_start(prev_end);
                    cmds.push(EditCommand::ModifyEvent {
                        index: pair[1],
                        before: Box::new(before),
                        after: Box::new(after),
                    });
                }
            }
        }
        ContinuousMode::ChangeEnd => {
            for pair in idxs.windows(2) {
                let next_start = evs[pair[1]].start();
                let cur = evs[pair[0]];
                if cur.end() != next_start {
                    let before = cur.clone();
                    let mut after = before.clone();
                    after.set_end(next_start);
                    cmds.push(EditCommand::ModifyEvent {
                        index: pair[0],
                        before: Box::new(before),
                        after: Box::new(after),
                    });
                }
            }
        }
    }
    Ok(EditCommand::Batch(cmds))
}

/// Recombines split adjacent/overlapping subtitle lines (mirrors Aegisub `src/command/edit.cpp`
/// `edit_line_recombine`, "Recombine Lines"):
/// - Merges consecutive lines with identical text & compatible metadata (`Style`, `Name`, `Effect`,
///   `Layer`, `Kind`) whose times touch or overlap.
/// - Deduplicates two-line split overlaps (`A\NB` followed by `B\NC` -> `A` + `B` spanning both +
///   `C`) into clean single lines.
///
/// # Errors
/// Returns [`FrameToolsError::MissingEventsSection`] if `[Events]` is absent.
pub fn recombine_lines(
    script: &ScriptFile,
    selected: &[usize],
) -> Result<EditCommand, FrameToolsError> {
    let sec = script
        .events_section()
        .ok_or(FrameToolsError::MissingEventsSection)?;
    let evs: Vec<&AssEvent> = sec.events().collect();
    let mut idxs: Vec<usize> = selected
        .iter()
        .copied()
        .filter(|&i| i < evs.len())
        .collect();
    idxs.sort_unstable();
    idxs.dedup();
    if idxs.len() < 2 {
        return Ok(EditCommand::Batch(Vec::new()));
    }

    let mut mods = Vec::new();
    let mut deletes = Vec::new();

    let mut i = 0usize;
    while i + 1 < idxs.len() {
        let a_idx = idxs[i];
        let b_idx = idxs[i + 1];
        let a = evs[a_idx];
        let b = evs[b_idx];

        let same_meta = a.kind() == b.kind()
            && a.style() == b.style()
            && a.name() == b.name()
            && a.effect() == b.effect()
            && a.layer() == b.layer();

        if same_meta && b.start() <= a.end() {
            if a.text() == b.text() {
                let mut merged = a.clone();
                merged.set_end(a.end().max(b.end()));
                mods.push(EditCommand::ModifyEvent {
                    index: a_idx,
                    before: Box::new(a.clone()),
                    after: Box::new(merged),
                });
                deletes.push((b_idx, b.clone()));
                i += 2;
                continue;
            }
            // Check A\NB + B -> keep A on a_idx, extend B? Or A + A\NB -> extend A to B.end, trim B to second line.
            if let Some(rest) = b.text().strip_prefix(a.text()) {
                if let Some(b_only) = rest.strip_prefix(r"\N") {
                    let mut new_a = a.clone();
                    new_a.set_end(b.end());
                    let mut new_b = b.clone();
                    new_b.set_text(b_only);
                    mods.push(EditCommand::ModifyEvent {
                        index: a_idx,
                        before: Box::new(a.clone()),
                        after: Box::new(new_a),
                    });
                    mods.push(EditCommand::ModifyEvent {
                        index: b_idx,
                        before: Box::new(b.clone()),
                        after: Box::new(new_b),
                    });
                    i += 2;
                    continue;
                }
            }
        }
        i += 1;
    }

    // Apply deletes in descending order after modifications so indices remain valid.
    deletes.sort_by_key(|a| std::cmp::Reverse(a.0));
    for (del_idx, del_ev) in deletes {
        mods.push(EditCommand::DeleteEvent {
            index: del_idx,
            event: del_ev,
        });
    }

    Ok(EditCommand::Batch(mods))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn framerate_transform_rewrites_event_and_tag_times() {
        let mut script = ass_core::parse_ass(
            "[Events]\nFormat: Layer, Start, End, Style, Name, MarginL, MarginR, MarginV, Effect, Text\nDialogue: 0,0:00:01.00,0:00:02.00,Default,,0,0,0,,{\\fad(200,300)\\t(0,500,\\fs60)\\k50}Hi{\\k50}ya\n",
        )
        .value;

        let in_tc = Timecodes::from_cfr(24, 1).expect("24");
        let out_tc = Timecodes::from_cfr(48, 1).expect("48"); // 2x speed -> half timestamps

        let cmd = transform_framerate(&script, &in_tc, &out_tc).expect("tf");
        cmd.apply(&mut script).expect("apply");

        let ev = script.events().next().expect("ev");
        // 1.00 s at 24fps = frame 24 -> 0.50 s at 48fps
        assert!((i64::from(ev.start().as_cs()) - 50).abs() <= 2);
        assert!((i64::from(ev.end().as_cs()) - 100).abs() <= 2);
        assert!(ev.text().contains(r"\fad(100,150)"));
        assert!(ev.text().contains(r"\t(0,250,\fs60)"));
        assert!(ev.text().contains(r"\k25"));
    }

    #[test]
    fn snap_continuous_and_recombine_work() {
        let mut script = ass_core::parse_ass(
            "[Events]\nFormat: Layer, Start, End, Style, Name, MarginL, MarginR, MarginV, Effect, Text\nDialogue: 0,0:00:01.01,0:00:02.02,Default,,0,0,0,,Same\nDialogue: 0,0:00:02.00,0:00:03.00,Default,,0,0,0,,Same\nDialogue: 0,0:00:03.50,0:00:04.50,Default,,0,0,0,,Next\n",
        )
        .value;

        let tc = Timecodes::from_cfr(24_000, 1001).expect("cfr");
        let snap_cmd = snap_to_video_frames(&script, None, &tc).expect("snap");
        snap_cmd.apply(&mut script).expect("apply snap");

        let rec_cmd = recombine_lines(&script, &[0, 1]).expect("recombine");
        rec_cmd.apply(&mut script).expect("apply recombine");
        assert_eq!(script.events().count(), 2);

        let cont_cmd =
            make_times_continuous(&script, &[0, 1], ContinuousMode::ChangeEnd).expect("cont");
        cont_cmd.apply(&mut script).expect("apply cont");
        let evs: Vec<&AssEvent> = script.events().collect();
        assert_eq!(evs[0].end(), evs[1].start());
    }
}
