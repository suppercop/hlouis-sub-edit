//! Shift Times tool matching Aegisub 3.5.1 `Timing -> Shift Times` semantics.
//!
//! Reference behaviour (Aegisub 3.5.1 manual — Shift Times):
//! - Shifts by centisecond time delta or by video frame count using a sorted frame-time table.
//! - Direction: Forward (`+`) or Backward (`-`), saturating at `0:00:00.00` (never negative).
//! - Affects: All rows, Selected rows, or Selection onward (from the smallest selected index).
//! - Times: Start only, End only, or Both Start and End.

use crate::ToolsError;
use ass_core::{AssTime, EditCommand, ScriptFile};
use std::collections::BTreeSet;

/// Amount to shift timestamps by.
#[derive(Debug, Clone, Copy)]
pub enum ShiftAmount<'a> {
    /// Shift by an exact centisecond duration.
    TimeCs(u32),
    /// Shift by `frames` video frames using a sorted slice of frame start times (in centiseconds).
    Frames {
        frames: u32,
        frame_times_cs: &'a [u32],
    },
}

/// Shift direction.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ShiftDirection {
    Forward,
    Backward,
}

/// Which timestamp fields of each affected event to shift.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ShiftTarget {
    Start,
    End,
    Both,
}

/// Which lines in the script to shift.
#[derive(Debug, Clone, Copy)]
pub enum ShiftAffect<'a> {
    All,
    Selected(&'a BTreeSet<usize>),
    SelectionOnward(usize),
}

/// Configuration for [`shift_times`].
#[derive(Debug, Clone, Copy)]
pub struct ShiftOptions<'a> {
    pub amount: ShiftAmount<'a>,
    pub direction: ShiftDirection,
    pub target: ShiftTarget,
    pub affect: ShiftAffect<'a>,
}

/// Builds an atomic [`EditCommand::Batch`] shifting timestamps according to `opts`.
pub fn shift_times(script: &ScriptFile, opts: ShiftOptions<'_>) -> Result<EditCommand, ToolsError> {
    if let ShiftAmount::Frames { frame_times_cs, .. } = opts.amount {
        if frame_times_cs.is_empty() {
            return Err(ToolsError::EmptyFrameTable);
        }
    }

    let Some(events_sec) = script.events_section() else {
        return Ok(EditCommand::Batch(Vec::new()));
    };

    let mut cmds = Vec::new();
    for (idx, ev) in events_sec.events().enumerate() {
        let affected = match opts.affect {
            ShiftAffect::All => true,
            ShiftAffect::Selected(sel) => sel.contains(&idx),
            ShiftAffect::SelectionOnward(first) => idx >= first,
        };
        if !affected {
            continue;
        }

        let new_start = match opts.target {
            ShiftTarget::Start | ShiftTarget::Both => {
                shift_single_time(ev.start(), opts.amount, opts.direction)
            }
            ShiftTarget::End => ev.start(),
        };
        let new_end = match opts.target {
            ShiftTarget::End | ShiftTarget::Both => {
                shift_single_time(ev.end(), opts.amount, opts.direction)
            }
            ShiftTarget::Start => ev.end(),
        };

        // Ensure end never precedes start when shifting only one endpoint (matches Aegisub clamping).
        let (final_start, final_end) = match opts.target {
            ShiftTarget::Start => (new_start, new_end.max(new_start)),
            ShiftTarget::End => (new_start.min(new_end), new_end),
            ShiftTarget::Both => (new_start, new_end),
        };

        if final_start != ev.start() || final_end != ev.end() {
            let mut after = ev.clone();
            after.set_start(final_start);
            after.set_end(final_end);
            cmds.push(EditCommand::ModifyEvent {
                index: idx,
                before: Box::new(ev.clone()),
                after: Box::new(after),
            });
        }
    }

    Ok(EditCommand::Batch(cmds))
}

fn shift_single_time(time: AssTime, amount: ShiftAmount<'_>, dir: ShiftDirection) -> AssTime {
    match amount {
        ShiftAmount::TimeCs(cs) => match dir {
            ShiftDirection::Forward => time.saturating_add_cs(cs),
            ShiftDirection::Backward => time.saturating_sub_cs(cs),
        },
        ShiftAmount::Frames {
            frames,
            frame_times_cs,
        } => {
            let cur_frame = nearest_frame_index(frame_times_cs, time.as_cs());
            let delta = frames as usize;
            let last_idx = frame_times_cs.len() - 1;
            let new_frame = match dir {
                ShiftDirection::Forward => cur_frame.saturating_add(delta).min(last_idx),
                ShiftDirection::Backward => cur_frame.saturating_sub(delta),
            };
            AssTime::from_cs(frame_times_cs[new_frame])
        }
    }
}

/// Finds the frame index whose start time is closest to `time_cs` (ties prefer the later frame,
/// matching Aegisub's `FrameAtTime` midpoint boundary behaviour).
fn nearest_frame_index(frame_times_cs: &[u32], time_cs: u32) -> usize {
    let pos = frame_times_cs.partition_point(|&t| t < time_cs);
    if pos == 0 {
        return 0;
    }
    if pos >= frame_times_cs.len() {
        return frame_times_cs.len() - 1;
    }
    let prev = frame_times_cs[pos - 1];
    let next = frame_times_cs[pos];
    if time_cs - prev < next - time_cs {
        pos - 1
    } else {
        pos
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use ass_core::{AssEvent, EventEntry, UndoHistory};

    fn make_script(ranges: &[(u32, u32)]) -> ScriptFile {
        let mut script = ScriptFile::new_default();
        script.ensure_events_section_mut().entries = ranges
            .iter()
            .enumerate()
            .map(|(i, &(s, e))| {
                EventEntry::Event(AssEvent::dialogue(
                    AssTime::from_cs(s),
                    AssTime::from_cs(e),
                    format!("Line {i}"),
                ))
            })
            .collect();
        script
    }

    #[test]
    fn shifts_backward_without_negative_times_and_supports_undo() {
        let mut script = make_script(&[(50, 150), (200, 400)]);
        let mut history = UndoHistory::default();
        let cmd = shift_times(
            &script,
            ShiftOptions {
                amount: ShiftAmount::TimeCs(100),
                direction: ShiftDirection::Backward,
                target: ShiftTarget::Both,
                affect: ShiftAffect::All,
            },
        )
        .expect("shift");
        history.execute(&mut script, cmd).expect("exec");

        let evs: Vec<&AssEvent> = script.events().collect();
        assert_eq!(evs[0].start(), AssTime::ZERO);
        assert_eq!(evs[0].end(), AssTime::from_cs(50));
        assert_eq!(evs[1].start(), AssTime::from_cs(100));
        assert_eq!(evs[1].end(), AssTime::from_cs(300));

        history.undo(&mut script).expect("undo");
        assert_eq!(
            script.events().next().unwrap().start(),
            AssTime::from_cs(50)
        );
    }

    #[test]
    fn shifts_by_frames_and_selection_onward() {
        // 25 fps -> 4 cs per frame.
        let frames: Vec<u32> = (0..100).map(|f| f * 4).collect();
        let mut script = make_script(&[(0, 40), (40, 80), (80, 120)]);
        let cmd = shift_times(
            &script,
            ShiftOptions {
                amount: ShiftAmount::Frames {
                    frames: 5,
                    frame_times_cs: &frames,
                },
                direction: ShiftDirection::Forward,
                target: ShiftTarget::Both,
                affect: ShiftAffect::SelectionOnward(1),
            },
        )
        .expect("shift");
        cmd.apply(&mut script).expect("apply");

        let evs: Vec<&AssEvent> = script.events().collect();
        assert_eq!(evs[0].start(), AssTime::from_cs(0));
        assert_eq!(evs[1].start(), AssTime::from_cs(60));
        assert_eq!(evs[1].end(), AssTime::from_cs(100));
        assert_eq!(evs[2].start(), AssTime::from_cs(100));
        assert_eq!(evs[2].end(), AssTime::from_cs(140));
    }
}
