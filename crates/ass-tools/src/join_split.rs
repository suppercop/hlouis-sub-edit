//! Join and Split Lines tools matching Aegisub 3.5.1 semantics.
//!
//! Reference behaviour (Aegisub 3.5.1 manual — Join / Split Lines):
//! - **Join**:
//!   - `Concatenate`: combines texts with a separator (typically `" "` or `r"\N"`), span = `min(start)..max(end)`.
//!   - `KeepFirst`: keeps only the first line's text, span = `min(start)..max(end)`.
//!   - `AsKaraoke`: prefixes each line's text with `{\k<dur_cs>}` (and inserts a silent `{\k<gap>}`
//!     if there is a positive gap between consecutive lines), span = `min(start)..max(end)`.
//! - **Split**:
//!   - `AtCursor(byte_pos)`: splits at a UTF-8 char boundary in `Text`, dividing duration proportionally
//!     by visible character count (or 50/50 if both sides have 0 visible characters).
//!   - `ByHardNewline`: splits on `\N` segments, distributing duration proportionally by visible chars.
//!   - `ByKaraoke`: splits at each `\k`/`\kf`/`\K`/`\ko` syllable using exact syllable centisecond timings.

use crate::ToolsError;
use ass_core::{
    parse_text_blocks, serialize_text_blocks, AssEvent, AssTime, EditCommand, EscapeKind,
    OverrideItem, ScriptFile, TagArgs, TextBlock,
};
use std::collections::BTreeSet;
use std::fmt::Write as _;

/// Strategy for [`join_lines`].
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum JoinMode {
    /// Concatenate line texts separated by `separator` (e.g. `" "` or `r"\N"`).
    Concatenate { separator: String },
    /// Keep only the first selected line's text while expanding its timing to cover all joined lines.
    KeepFirst,
    /// Join as karaoke syllables using each line's duration in centiseconds (`{\k<dur>}`).
    AsKaraoke,
}

/// Strategy for [`split_line`].
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum SplitMode {
    /// Split at the given byte offset inside `Text` (snapped down to a UTF-8 char boundary).
    AtCursor(usize),
    /// Split on `\N` hard line breaks.
    ByHardNewline,
    /// Split into one line per karaoke syllable (`\k`, `\kf`, `\K`, `\ko`).
    ByKaraoke,
}

/// Joins the selected logical event indices into a single event at the lowest selected index,
/// deleting the remaining selected events as one atomic [`EditCommand::Batch`].
pub fn join_lines(
    script: &ScriptFile,
    selected: &BTreeSet<usize>,
    mode: &JoinMode,
) -> Result<EditCommand, ToolsError> {
    if selected.len() < 2 {
        return Err(ToolsError::InsufficientLines);
    }
    let Some(events_sec) = script.events_section() else {
        return Err(ToolsError::InsufficientLines);
    };

    let indices: Vec<usize> = selected.iter().copied().collect();
    let mut events = Vec::with_capacity(indices.len());
    for &idx in &indices {
        let ev = events_sec
            .get_event(idx)
            .ok_or(ToolsError::InvalidEventIndex(idx))?;
        events.push(ev);
    }

    let first = events[0];
    let min_start = events
        .iter()
        .map(|e| e.start())
        .min()
        .unwrap_or(first.start());
    let max_end = events.iter().map(|e| e.end()).max().unwrap_or(first.end());

    let joined_text = match mode {
        JoinMode::KeepFirst => first.text().to_owned(),
        JoinMode::Concatenate { separator } => {
            let mut out = String::new();
            for (i, ev) in events.iter().enumerate() {
                if i > 0 {
                    out.push_str(separator);
                }
                out.push_str(ev.text());
            }
            out
        }
        JoinMode::AsKaraoke => {
            let mut out = String::new();
            let mut prev_end = min_start;
            for (i, ev) in events.iter().enumerate() {
                if i > 0 && ev.start() > prev_end {
                    let gap = ev.start().as_cs().saturating_sub(prev_end.as_cs());
                    if gap > 0 {
                        let _ = write!(out, "{{\\k{gap}}}");
                    }
                }
                let dur = ev.end().as_cs().saturating_sub(ev.start().as_cs());
                let _ = write!(out, "{{\\k{dur}}}{}", ev.text());
                prev_end = ev.end();
            }
            out
        }
    };

    let mut merged = first.clone();
    merged.set_start(min_start);
    merged.set_end(max_end);
    merged.set_text(joined_text);

    let mut cmds = Vec::with_capacity(indices.len());
    cmds.push(EditCommand::ModifyEvent {
        index: indices[0],
        before: Box::new(first.clone()),
        after: Box::new(merged),
    });
    // Delete extra lines in reverse index order so earlier indices stay valid during apply & revert.
    for (&idx, &ev) in indices.iter().skip(1).zip(events.iter().skip(1)).rev() {
        cmds.push(EditCommand::DeleteEvent {
            index: idx,
            event: ev.clone(),
        });
    }

    Ok(EditCommand::Batch(cmds))
}

/// Splits the event at `index` according to `mode`, returning one atomic [`EditCommand::Batch`].
/// If the line cannot be split into 2+ pieces (e.g. no `\N` or no karaoke tags), returns an empty batch.
pub fn split_line(
    script: &ScriptFile,
    index: usize,
    mode: SplitMode,
) -> Result<EditCommand, ToolsError> {
    let ev = script
        .events_section()
        .and_then(|sec| sec.get_event(index))
        .ok_or(ToolsError::InvalidEventIndex(index))?;

    let pieces: Vec<(AssTime, AssTime, String)> = match mode {
        SplitMode::AtCursor(byte_pos) => split_at_cursor(ev, byte_pos),
        SplitMode::ByHardNewline => split_by_hard_newline(ev),
        SplitMode::ByKaraoke => split_by_karaoke(ev),
    };

    if pieces.len() < 2 {
        return Ok(EditCommand::Batch(Vec::new()));
    }

    let mut cmds = Vec::with_capacity(pieces.len());
    let mut first_after = ev.clone();
    first_after.set_start(pieces[0].0);
    first_after.set_end(pieces[0].1);
    first_after.set_text(pieces[0].2.clone());

    cmds.push(EditCommand::ModifyEvent {
        index,
        before: Box::new(ev.clone()),
        after: Box::new(first_after),
    });

    for (offset, (start, end, text)) in pieces.into_iter().enumerate().skip(1) {
        let mut next_ev = ev.clone();
        next_ev.set_start(start);
        next_ev.set_end(end);
        next_ev.set_text(text);
        cmds.push(EditCommand::InsertEvent {
            index: index + offset,
            event: next_ev,
        });
    }

    Ok(EditCommand::Batch(cmds))
}

fn split_at_cursor(ev: &AssEvent, byte_pos: usize) -> Vec<(AssTime, AssTime, String)> {
    let text = ev.text();
    let mut split_idx = byte_pos.min(text.len());
    while split_idx > 0 && !text.is_char_boundary(split_idx) {
        split_idx -= 1;
    }
    if split_idx == 0 || split_idx == text.len() {
        return Vec::new();
    }
    let left = text[..split_idx].to_owned();
    let right = text[split_idx..].to_owned();
    distribute_times_proportionally(ev.start(), ev.end(), vec![left, right])
}

fn split_by_hard_newline(ev: &AssEvent) -> Vec<(AssTime, AssTime, String)> {
    let blocks = parse_text_blocks(ev.text());
    let mut segments: Vec<Vec<TextBlock>> = vec![Vec::new()];

    for block in blocks {
        if matches!(block, TextBlock::Escape(EscapeKind::HardNewline)) {
            segments.push(Vec::new());
        } else if let Some(last) = segments.last_mut() {
            last.push(block);
        }
    }

    if segments.len() < 2 {
        return Vec::new();
    }

    let texts: Vec<String> = segments
        .iter()
        .map(|seg| serialize_text_blocks(seg))
        .collect();
    distribute_times_proportionally(ev.start(), ev.end(), texts)
}

fn split_by_karaoke(ev: &AssEvent) -> Vec<(AssTime, AssTime, String)> {
    let blocks = parse_text_blocks(ev.text());
    let mut syllables: Vec<(u32, String)> = Vec::new();
    let mut current_dur: Option<u32> = None;
    let mut current_text = String::new();
    let mut saw_karaoke = false;

    for block in blocks {
        match block {
            TextBlock::Override { items, closed } => {
                let mut non_k_items = Vec::new();
                for item in items {
                    if let OverrideItem::Tag(ref tag) = item {
                        if let TagArgs::Karaoke { duration_cs, .. } = tag.args {
                            saw_karaoke = true;
                            if let Some(prev_dur) = current_dur.take() {
                                syllables.push((prev_dur, std::mem::take(&mut current_text)));
                            } else if !current_text.is_empty() {
                                syllables.push((0, std::mem::take(&mut current_text)));
                            }
                            if !non_k_items.is_empty() {
                                let prefix = serialize_text_blocks(&[TextBlock::Override {
                                    items: std::mem::take(&mut non_k_items),
                                    closed: true,
                                }]);
                                current_text.push_str(&prefix);
                            }
                            let dur_u32 = u32::try_from(duration_cs.max(0)).unwrap_or(u32::MAX);
                            current_dur = Some(dur_u32);
                            continue;
                        }
                    }
                    non_k_items.push(item);
                }
                if !non_k_items.is_empty() {
                    let s = serialize_text_blocks(&[TextBlock::Override {
                        items: non_k_items,
                        closed,
                    }]);
                    current_text.push_str(&s);
                }
            }
            other => {
                current_text.push_str(&serialize_text_blocks(&[other]));
            }
        }
    }

    if !saw_karaoke {
        return Vec::new();
    }
    if let Some(dur) = current_dur {
        syllables.push((dur, current_text));
    } else if !current_text.is_empty() {
        syllables.push((0, current_text));
    }

    if syllables.len() < 2 {
        return Vec::new();
    }

    let mut out = Vec::with_capacity(syllables.len());
    let mut cursor = ev.start();
    for (dur_cs, text) in syllables {
        let next = cursor.saturating_add_cs(dur_cs).min(ev.end());
        out.push((cursor, next, text));
        cursor = next;
    }
    out
}

fn distribute_times_proportionally(
    start: AssTime,
    end: AssTime,
    texts: Vec<String>,
) -> Vec<(AssTime, AssTime, String)> {
    let total_cs = end.as_cs().saturating_sub(start.as_cs());
    let weights: Vec<u64> = texts.iter().map(|t| visible_chars_len(t) as u64).collect();
    let total_weight: u64 = weights.iter().sum();

    let count = texts.len();
    let mut out = Vec::with_capacity(count);
    let mut cursor = start;
    let mut acc_weight = 0u64;

    for (i, text) in texts.into_iter().enumerate() {
        let piece_end = if i + 1 == count {
            end
        } else if let Some(offset_u64) = {
            acc_weight += weights[i];
            (u64::from(total_cs) * acc_weight).checked_div(total_weight)
        } {
            let offset = u32::try_from(offset_u64).unwrap_or(total_cs);
            start.saturating_add_cs(offset)
        } else {
            let offset = total_cs.saturating_mul(u32::try_from(i + 1).unwrap_or(1))
                / u32::try_from(count).unwrap_or(1);
            start.saturating_add_cs(offset)
        };
        out.push((cursor, piece_end, text));
        cursor = piece_end;
    }
    out
}

fn visible_chars_len(text: &str) -> usize {
    parse_text_blocks(text)
        .into_iter()
        .map(|b| match b {
            TextBlock::Plain(s) => s.chars().count(),
            _ => 0,
        })
        .sum()
}

#[cfg(test)]
mod tests {
    use super::*;
    use ass_core::{EventEntry, UndoHistory};

    #[test]
    fn join_concatenate_keep_first_and_karaoke_round_trip_with_undo() {
        let mut script = ScriptFile::new_default();
        script.ensure_events_section_mut().entries = vec![
            EventEntry::Event(AssEvent::dialogue(
                AssTime::from_cs(100),
                AssTime::from_cs(200),
                "Hello",
            )),
            EventEntry::Event(AssEvent::dialogue(
                AssTime::from_cs(250),
                AssTime::from_cs(400),
                "World",
            )),
        ];
        let sel = BTreeSet::from([0, 1]);
        let mut history = UndoHistory::default();

        // Join as karaoke (includes 50 cs gap between 200 and 250).
        let cmd = join_lines(&script, &sel, &JoinMode::AsKaraoke).expect("join karaoke");
        history.execute(&mut script, cmd).expect("exec");
        assert_eq!(script.events().count(), 1);
        let ev = script.events().next().unwrap();
        assert_eq!(ev.start(), AssTime::from_cs(100));
        assert_eq!(ev.end(), AssTime::from_cs(400));
        assert_eq!(ev.text(), r"{\k100}Hello{\k50}{\k150}World");

        // Undo restores both original lines.
        history.undo(&mut script).expect("undo");
        assert_eq!(script.events().count(), 2);

        // Join concatenate with \N.
        let cmd2 = join_lines(
            &script,
            &sel,
            &JoinMode::Concatenate {
                separator: r"\N".to_owned(),
            },
        )
        .expect("concat");
        history.execute(&mut script, cmd2).expect("exec2");
        assert_eq!(script.events().next().unwrap().text(), r"Hello\NWorld");
    }

    #[test]
    fn split_by_cursor_hard_newline_and_karaoke() {
        let mut script = ScriptFile::new_default();
        script.ensure_events_section_mut().entries = vec![
            EventEntry::Event(AssEvent::dialogue(
                AssTime::from_cs(0),
                AssTime::from_cs(400),
                r"AA\NBBBBBB",
            )),
            EventEntry::Event(AssEvent::dialogue(
                AssTime::from_cs(500),
                AssTime::from_cs(620),
                r"{\k30}Ka{\kf40}ra{\ko50}oke",
            )),
        ];

        // Split line 1 by karaoke -> 3 syllables (30, 40, 50 cs).
        let cmd_k = split_line(&script, 1, SplitMode::ByKaraoke).expect("split k");
        cmd_k.apply(&mut script).expect("apply k");
        let evs: Vec<&AssEvent> = script.events().collect();
        assert_eq!(evs.len(), 4);
        assert_eq!(evs[1].start(), AssTime::from_cs(500));
        assert_eq!(evs[1].end(), AssTime::from_cs(530));
        assert_eq!(evs[1].text(), "Ka");
        assert_eq!(evs[2].start(), AssTime::from_cs(530));
        assert_eq!(evs[2].end(), AssTime::from_cs(570));
        assert_eq!(evs[2].text(), "ra");
        assert_eq!(evs[3].start(), AssTime::from_cs(570));
        assert_eq!(evs[3].end(), AssTime::from_cs(620));
        assert_eq!(evs[3].text(), "oke");

        // Split line 0 by \N -> 2:6 char ratio => 100 cs and 300 cs.
        let cmd_n = split_line(&script, 0, SplitMode::ByHardNewline).expect("split \\N");
        cmd_n.apply(&mut script).expect("apply \\N");
        let evs2: Vec<&AssEvent> = script.events().collect();
        assert_eq!(evs2[0].end(), AssTime::from_cs(100));
        assert_eq!(evs2[0].text(), "AA");
        assert_eq!(evs2[1].start(), AssTime::from_cs(100));
        assert_eq!(evs2[1].end(), AssTime::from_cs(400));
        assert_eq!(evs2[1].text(), "BBBBBB");

        // Split zero-length line at cursor works without panicking.
        let mut zero_script = ScriptFile::new_default();
        zero_script.ensure_events_section_mut().entries = vec![EventEntry::Event(
            AssEvent::dialogue(AssTime::from_cs(100), AssTime::from_cs(100), "abcd"),
        )];
        let cmd_c = split_line(&zero_script, 0, SplitMode::AtCursor(2)).expect("split cursor");
        cmd_c.apply(&mut zero_script).expect("apply cursor");
        assert_eq!(zero_script.events().count(), 2);
    }
}
