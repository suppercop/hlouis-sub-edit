//! Atomic grid editing operations, Aegisub clipboard / paste-over, and CPS metrics.
//!
//! Every mutating operation returns an [`ass_core::EditCommand`] (`EditCommand::Batch` or single
//! command) without cloning the entire event list, so operations plug directly into
//! [`ass_core::UndoStack`] and execute in linear / O(N log N) time on 20k-line scripts.

#![forbid(unsafe_code)]
#![cfg_attr(not(test), deny(clippy::unwrap_used, clippy::expect_used))]

pub mod cps;

use ass_core::{AssEvent, AssTime, EditCommand, EventKind, ScriptFile};
use std::fmt::Write as _;
use thiserror::Error;

pub use cps::{count_text_metrics, event_metrics, CpsOptions, LineMetrics};

/// Errors returned by `edit-ops`.
#[derive(Debug, Error, PartialEq, Eq)]
pub enum EditOpsError {
    /// Script has no `[Events]` section.
    #[error("script has no [Events] section")]
    MissingEventsSection,
    /// Event index was out of range.
    #[error("event index {index} out of bounds (count = {count})")]
    IndexOutOfBounds { index: usize, count: usize },
}

/// Sort criteria for [`stable_sort`].
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum SortKey {
    Start,
    End,
    Style,
    Actor,
    Effect,
    Layer,
}

/// Field mask for [`paste_over`] (mirrors Aegisub `src/dialog_paste_over.cpp`).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
#[allow(clippy::struct_excessive_bools)]
pub struct PasteOverMask {
    pub kind: bool,
    pub layer: bool,
    pub start: bool,
    pub end: bool,
    pub style: bool,
    pub name: bool,
    pub margin_l: bool,
    pub margin_r: bool,
    pub margin_v: bool,
    pub effect: bool,
    pub text: bool,
}

impl PasteOverMask {
    /// Mask selecting all fields.
    #[must_use]
    pub const fn all() -> Self {
        Self {
            kind: true,
            layer: true,
            start: true,
            end: true,
            style: true,
            name: true,
            margin_l: true,
            margin_r: true,
            margin_v: true,
            effect: true,
            text: true,
        }
    }

    /// Mask selecting only `start` and `end` times.
    #[must_use]
    pub const fn times_only() -> Self {
        Self {
            start: true,
            end: true,
            ..Self::none()
        }
    }

    /// Mask selecting only `text`.
    #[must_use]
    pub const fn text_only() -> Self {
        Self {
            text: true,
            ..Self::none()
        }
    }

    /// Empty mask (no fields selected).
    #[must_use]
    pub const fn none() -> Self {
        Self {
            kind: false,
            layer: false,
            start: false,
            end: false,
            style: false,
            name: false,
            margin_l: false,
            margin_r: false,
            margin_v: false,
            effect: false,
            text: false,
        }
    }
}

fn collect_events(script: &ScriptFile) -> Result<Vec<&AssEvent>, EditOpsError> {
    let sec = script
        .events_section()
        .ok_or(EditOpsError::MissingEventsSection)?;
    Ok(sec.events().collect())
}

fn normalize_selection(selected: &[usize], count: usize) -> Vec<usize> {
    let mut idxs: Vec<usize> = selected.iter().copied().filter(|&i| i < count).collect();
    idxs.sort_unstable();
    idxs.dedup();
    idxs
}

/// Inserts `event` immediately before `index`.
///
/// # Errors
/// Returns [`EditOpsError`] if `[Events]` is missing or `index > event_count`.
pub fn insert_before(
    script: &ScriptFile,
    index: usize,
    event: AssEvent,
) -> Result<EditCommand, EditOpsError> {
    let count = script
        .events_section()
        .ok_or(EditOpsError::MissingEventsSection)?
        .event_count();
    if index > count {
        return Err(EditOpsError::IndexOutOfBounds { index, count });
    }
    Ok(EditCommand::InsertEvent { index, event })
}

/// Inserts `event` immediately after `index`.
///
/// # Errors
/// Returns [`EditOpsError`] if `[Events]` is missing or `index >= event_count`.
pub fn insert_after(
    script: &ScriptFile,
    index: usize,
    event: AssEvent,
) -> Result<EditCommand, EditOpsError> {
    let count = script
        .events_section()
        .ok_or(EditOpsError::MissingEventsSection)?
        .event_count();
    if index >= count {
        return Err(EditOpsError::IndexOutOfBounds { index, count });
    }
    Ok(EditCommand::InsertEvent {
        index: index + 1,
        event,
    })
}

/// Duplicates each selected event directly after itself (matches Aegisub `edit/line/duplicate`).
///
/// # Errors
/// Returns [`EditOpsError::MissingEventsSection`] if `[Events]` is absent.
pub fn duplicate_selected(
    script: &ScriptFile,
    selected: &[usize],
) -> Result<EditCommand, EditOpsError> {
    let evs = collect_events(script)?;
    let idxs = normalize_selection(selected, evs.len());
    let mut cmds = Vec::with_capacity(idxs.len());
    // Insert from bottom to top so earlier indices remain valid.
    for &idx in idxs.iter().rev() {
        cmds.push(EditCommand::InsertEvent {
            index: idx + 1,
            event: evs[idx].clone(),
        });
    }
    Ok(EditCommand::Batch(cmds))
}

/// Deletes all selected events atomically (descending index order so indices stay valid).
///
/// # Errors
/// Returns [`EditOpsError::MissingEventsSection`] if `[Events]` is absent.
pub fn delete_selected(
    script: &ScriptFile,
    selected: &[usize],
) -> Result<EditCommand, EditOpsError> {
    let evs = collect_events(script)?;
    let idxs = normalize_selection(selected, evs.len());
    let mut cmds = Vec::with_capacity(idxs.len());
    for &idx in idxs.iter().rev() {
        cmds.push(EditCommand::DeleteEvent {
            index: idx,
            event: evs[idx].clone(),
        });
    }
    Ok(EditCommand::Batch(cmds))
}

/// Swaps two events by index.
///
/// # Errors
/// Returns [`EditOpsError`] if `[Events]` is missing or either index is out of bounds.
pub fn swap_lines(
    script: &ScriptFile,
    index_a: usize,
    index_b: usize,
) -> Result<EditCommand, EditOpsError> {
    let evs = collect_events(script)?;
    let count = evs.len();
    if index_a >= count {
        return Err(EditOpsError::IndexOutOfBounds {
            index: index_a,
            count,
        });
    }
    if index_b >= count {
        return Err(EditOpsError::IndexOutOfBounds {
            index: index_b,
            count,
        });
    }
    if index_a == index_b {
        return Ok(EditCommand::Batch(Vec::new()));
    }
    Ok(EditCommand::Batch(vec![
        EditCommand::ModifyEvent {
            index: index_a,
            before: Box::new(evs[index_a].clone()),
            after: Box::new(evs[index_b].clone()),
        },
        EditCommand::ModifyEvent {
            index: index_b,
            before: Box::new(evs[index_b].clone()),
            after: Box::new(evs[index_a].clone()),
        },
    ]))
}

/// Moves the selected lines up by one row (preserving relative order; no-op if row 0 is selected).
///
/// # Errors
/// Returns [`EditOpsError::MissingEventsSection`] if `[Events]` is absent.
pub fn move_selection_up(
    script: &ScriptFile,
    selected: &[usize],
) -> Result<EditCommand, EditOpsError> {
    let evs = collect_events(script)?;
    let idxs = normalize_selection(selected, evs.len());
    if idxs.is_empty() || idxs[0] == 0 {
        return Ok(EditCommand::Batch(Vec::new()));
    }

    // Compute final permutation only for the touched range `[first - 1 ..= last]`.
    let min_i = idxs[0] - 1;
    let max_i = idxs[idxs.len() - 1];
    let span = max_i - min_i + 1;
    let mut perm: Vec<usize> = (min_i..=max_i).collect();
    let mut is_sel = vec![false; span];
    for &i in &idxs {
        is_sel[i - min_i] = true;
    }
    for pos in 1..span {
        if is_sel[pos] && !is_sel[pos - 1] {
            perm.swap(pos, pos - 1);
            is_sel.swap(pos, pos - 1);
        }
    }

    let mut cmds = Vec::new();
    for (offset, &src_idx) in perm.iter().enumerate() {
        let dst_idx = min_i + offset;
        if src_idx != dst_idx {
            cmds.push(EditCommand::ModifyEvent {
                index: dst_idx,
                before: Box::new(evs[dst_idx].clone()),
                after: Box::new(evs[src_idx].clone()),
            });
        }
    }
    Ok(EditCommand::Batch(cmds))
}

/// Moves the selected lines down by one row (preserving relative order; no-op if last row is
/// selected).
///
/// # Errors
/// Returns [`EditOpsError::MissingEventsSection`] if `[Events]` is absent.
pub fn move_selection_down(
    script: &ScriptFile,
    selected: &[usize],
) -> Result<EditCommand, EditOpsError> {
    let evs = collect_events(script)?;
    let idxs = normalize_selection(selected, evs.len());
    if idxs.is_empty() || idxs[idxs.len() - 1] + 1 >= evs.len() {
        return Ok(EditCommand::Batch(Vec::new()));
    }

    let min_i = idxs[0];
    let max_i = idxs[idxs.len() - 1] + 1;
    let span = max_i - min_i + 1;
    let mut perm: Vec<usize> = (min_i..=max_i).collect();
    let mut is_sel = vec![false; span];
    for &i in &idxs {
        is_sel[i - min_i] = true;
    }
    for pos in (0..span - 1).rev() {
        if is_sel[pos] && !is_sel[pos + 1] {
            perm.swap(pos, pos + 1);
            is_sel.swap(pos, pos + 1);
        }
    }

    let mut cmds = Vec::new();
    for (offset, &src_idx) in perm.iter().enumerate() {
        let dst_idx = min_i + offset;
        if src_idx != dst_idx {
            cmds.push(EditCommand::ModifyEvent {
                index: dst_idx,
                before: Box::new(evs[dst_idx].clone()),
                after: Box::new(evs[src_idx].clone()),
            });
        }
    }
    Ok(EditCommand::Batch(cmds))
}

/// Sets `EventKind::Comment` on all selected lines that are not already comments.
///
/// # Errors
/// Returns [`EditOpsError::MissingEventsSection`] if `[Events]` is absent.
pub fn comment_selected(
    script: &ScriptFile,
    selected: &[usize],
) -> Result<EditCommand, EditOpsError> {
    set_kind_selected(script, selected, EventKind::Comment)
}

/// Sets `EventKind::Dialogue` on all selected lines that are currently comments.
///
/// # Errors
/// Returns [`EditOpsError::MissingEventsSection`] if `[Events]` is absent.
pub fn uncomment_selected(
    script: &ScriptFile,
    selected: &[usize],
) -> Result<EditCommand, EditOpsError> {
    set_kind_selected(script, selected, EventKind::Dialogue)
}

/// Toggles `Dialogue` <-> `Comment` on all selected lines.
///
/// # Errors
/// Returns [`EditOpsError::MissingEventsSection`] if `[Events]` is absent.
pub fn toggle_comment_selected(
    script: &ScriptFile,
    selected: &[usize],
) -> Result<EditCommand, EditOpsError> {
    let evs = collect_events(script)?;
    let idxs = normalize_selection(selected, evs.len());
    let mut cmds = Vec::with_capacity(idxs.len());
    for idx in idxs {
        let before = evs[idx].clone();
        let mut after = before.clone();
        let next_kind = match before.kind() {
            EventKind::Dialogue => EventKind::Comment,
            EventKind::Comment => EventKind::Dialogue,
        };
        after.set_kind(next_kind);
        cmds.push(EditCommand::ModifyEvent {
            index: idx,
            before: Box::new(before),
            after: Box::new(after),
        });
    }
    Ok(EditCommand::Batch(cmds))
}

fn set_kind_selected(
    script: &ScriptFile,
    selected: &[usize],
    target: EventKind,
) -> Result<EditCommand, EditOpsError> {
    let evs = collect_events(script)?;
    let idxs = normalize_selection(selected, evs.len());
    let mut cmds = Vec::new();
    for idx in idxs {
        if evs[idx].kind() != target {
            let before = evs[idx].clone();
            let mut after = before.clone();
            after.set_kind(target);
            cmds.push(EditCommand::ModifyEvent {
                index: idx,
                before: Box::new(before),
                after: Box::new(after),
            });
        }
    }
    Ok(EditCommand::Batch(cmds))
}

/// Stable-sorts either the selected rows (in place within their slots) or the entire `[Events]`
/// section when `selection` is `None`.
///
/// Never clones unchanged rows; emits `ModifyEvent` only for positions whose event moved.
///
/// # Errors
/// Returns [`EditOpsError::MissingEventsSection`] if `[Events]` is absent.
pub fn stable_sort(
    script: &ScriptFile,
    key: SortKey,
    descending: bool,
    selection: Option<&[usize]>,
) -> Result<EditCommand, EditOpsError> {
    let evs = collect_events(script)?;
    let slots: Vec<usize> = match selection {
        Some(sel) => normalize_selection(sel, evs.len()),
        None => (0..evs.len()).collect(),
    };
    if slots.len() <= 1 {
        return Ok(EditCommand::Batch(Vec::new()));
    }

    let sorted = compute_sorted_order(&evs, &slots, key, descending);

    let moved = slots
        .iter()
        .zip(sorted.iter())
        .filter(|(&d, &s)| d != s)
        .count();
    let mut cmds = Vec::with_capacity(moved);
    for (&dst_idx, &src_idx) in slots.iter().zip(sorted.iter()) {
        if dst_idx != src_idx {
            cmds.push(EditCommand::ModifyEvent {
                index: dst_idx,
                before: Box::new(evs[dst_idx].clone()),
                after: Box::new(evs[src_idx].clone()),
            });
        }
    }
    Ok(EditCommand::Batch(cmds))
}

/// Zero-clone in-place stable sort directly permuting `EventsSection::entries`.
///
/// Returns the permutation applied to the sorted slots so callers can invert in O(N) without
/// cloning 20k `AssEvent` rows.
///
/// # Errors
/// Returns [`EditOpsError::MissingEventsSection`] if `[Events]` is absent.
pub fn stable_sort_in_place(
    script: &mut ScriptFile,
    key: SortKey,
    descending: bool,
    selection: Option<&[usize]>,
) -> Result<Vec<usize>, EditOpsError> {
    let sec = script
        .events_section_mut()
        .ok_or(EditOpsError::MissingEventsSection)?;
    let event_entry_positions: Vec<usize> = sec
        .entries
        .iter()
        .enumerate()
        .filter_map(|(pos, e)| matches!(e, ass_core::EventEntry::Event(_)).then_some(pos))
        .collect();
    let count = event_entry_positions.len();
    let slots: Vec<usize> = match selection {
        Some(sel) => normalize_selection(sel, count),
        None => (0..count).collect(),
    };
    if slots.len() <= 1 {
        return Ok(Vec::new());
    }

    let sorted = {
        let evs: Vec<&AssEvent> = event_entry_positions
            .iter()
            .filter_map(|&pos| match &sec.entries[pos] {
                ass_core::EventEntry::Event(ev) => Some(ev),
                _ => None,
            })
            .collect();
        compute_sorted_order(&evs, &slots, key, descending)
    };

    // Apply permutation in-place via cycle decomposition (zero AssEvent clones).
    let mut visited = vec![false; slots.len()];
    let mut slot_pos_map = vec![usize::MAX; count];
    for (slot_i, &ev_idx) in slots.iter().enumerate() {
        slot_pos_map[ev_idx] = slot_i;
    }

    for start_i in 0..slots.len() {
        if visited[start_i] || sorted[start_i] == slots[start_i] {
            visited[start_i] = true;
            continue;
        }
        let mut cur = start_i;
        visited[cur] = true;
        let saved = std::mem::replace(
            &mut sec.entries[event_entry_positions[slots[cur]]],
            ass_core::EventEntry::Blank,
        );
        loop {
            let src_ev = sorted[cur];
            let next = slot_pos_map[src_ev];
            if next == start_i {
                sec.entries[event_entry_positions[slots[cur]]] = saved;
                break;
            }
            let moved_entry = std::mem::replace(
                &mut sec.entries[event_entry_positions[slots[next]]],
                ass_core::EventEntry::Blank,
            );
            sec.entries[event_entry_positions[slots[cur]]] = moved_entry;
            visited[next] = true;
            cur = next;
        }
    }

    Ok(sorted)
}

fn compute_sorted_order(
    evs: &[&AssEvent],
    slots: &[usize],
    key: SortKey,
    descending: bool,
) -> Vec<usize> {
    match key {
        SortKey::Start | SortKey::End | SortKey::Layer => {
            let mut keyed: Vec<(i64, usize)> = slots
                .iter()
                .map(|&idx| {
                    let ev = evs[idx];
                    let k = match key {
                        SortKey::Start => i64::from(ev.start().as_cs()),
                        SortKey::End => i64::from(ev.end().as_cs()),
                        SortKey::Layer => i64::from(ev.layer()),
                        _ => 0,
                    };
                    (k, idx)
                })
                .collect();
            if descending {
                keyed.sort_by_key(|a| std::cmp::Reverse(a.0));
            } else {
                keyed.sort_by_key(|&(k, _)| k);
            }
            keyed.into_iter().map(|(_, idx)| idx).collect()
        }
        SortKey::Style | SortKey::Actor | SortKey::Effect => {
            let mut keyed: Vec<(&str, usize)> = slots
                .iter()
                .map(|&idx| {
                    let ev = evs[idx];
                    let k = match key {
                        SortKey::Style => ev.style(),
                        SortKey::Actor => ev.name(),
                        SortKey::Effect => ev.effect(),
                        _ => "",
                    };
                    (k, idx)
                })
                .collect();
            if descending {
                keyed.sort_by(|a, b| b.0.cmp(a.0));
            } else {
                keyed.sort_by_key(|&(k, _)| k);
            }
            keyed.into_iter().map(|(_, idx)| idx).collect()
        }
    }
}

/// Formats selected events in Aegisub clipboard format (`Dialogue: ...` / `Comment: ...`,
/// newline-separated).
///
/// # Errors
/// Returns [`EditOpsError::MissingEventsSection`] if `[Events]` is absent.
pub fn copy_lines(script: &ScriptFile, selected: &[usize]) -> Result<String, EditOpsError> {
    let evs = collect_events(script)?;
    let idxs = normalize_selection(selected, evs.len());
    let mut out = String::with_capacity(idxs.len() * 64);
    for (i, &idx) in idxs.iter().enumerate() {
        if i > 0 {
            out.push('\n');
        }
        format_event_clipboard(evs[idx], &mut out);
    }
    Ok(out)
}

/// Copies selected lines to Aegisub clipboard text and returns the atomic delete command.
///
/// # Errors
/// Returns [`EditOpsError::MissingEventsSection`] if `[Events]` is absent.
pub fn cut_lines(
    script: &ScriptFile,
    selected: &[usize],
) -> Result<(String, EditCommand), EditOpsError> {
    let text = copy_lines(script, selected)?;
    let cmd = delete_selected(script, selected)?;
    Ok((text, cmd))
}

/// Parses clipboard text into [`AssEvent`] rows.
///
/// Lines starting with `Dialogue:` or `Comment:` are parsed as standard 10-field ASS events;
/// any other non-empty line is imported as a plain-text `Dialogue` (`0:00:00.00 -> 0:00:05.00`),
/// matching Aegisub's `command/edit.cpp` clipboard paste behaviour.
#[must_use]
pub fn parse_clipboard_lines(clipboard: &str) -> Vec<AssEvent> {
    let mut out = Vec::new();
    for raw in clipboard.lines() {
        let line = raw.trim_end_matches('\r');
        if line.is_empty() {
            continue;
        }
        if let Some(ev) = parse_ass_event_line(line) {
            out.push(ev);
        } else {
            out.push(AssEvent::dialogue(
                AssTime::from_cs(0),
                AssTime::from_cs(500),
                line,
            ));
        }
    }
    out
}

/// Builds an atomic [`EditCommand`] inserting parsed clipboard lines at `insert_at`.
///
/// # Errors
/// Returns [`EditOpsError`] if `[Events]` is missing or `insert_at > event_count`.
pub fn paste_lines(
    script: &ScriptFile,
    insert_at: usize,
    clipboard: &str,
) -> Result<EditCommand, EditOpsError> {
    let count = script
        .events_section()
        .ok_or(EditOpsError::MissingEventsSection)?
        .event_count();
    if insert_at > count {
        return Err(EditOpsError::IndexOutOfBounds {
            index: insert_at,
            count,
        });
    }
    let parsed = parse_clipboard_lines(clipboard);
    let mut cmds = Vec::with_capacity(parsed.len());
    for (offset, ev) in parsed.into_iter().enumerate() {
        cmds.push(EditCommand::InsertEvent {
            index: insert_at + offset,
            event: ev,
        });
    }
    Ok(EditCommand::Batch(cmds))
}

/// Pastes selected fields (`mask`) from clipboard lines over `target_indices` in order
/// (mirrors Aegisub `src/dialog_paste_over.cpp`).
///
/// # Errors
/// Returns [`EditOpsError::MissingEventsSection`] if `[Events]` is absent.
pub fn paste_over(
    script: &ScriptFile,
    target_indices: &[usize],
    clipboard: &str,
    mask: PasteOverMask,
) -> Result<EditCommand, EditOpsError> {
    let evs = collect_events(script)?;
    let targets = normalize_selection(target_indices, evs.len());
    let sources = parse_clipboard_lines(clipboard);
    let pair_count = targets.len().min(sources.len());
    let mut cmds = Vec::with_capacity(pair_count);

    for i in 0..pair_count {
        let idx = targets[i];
        let src = &sources[i];
        let before = evs[idx].clone();
        let mut after = before.clone();

        if mask.kind {
            after.set_kind(src.kind());
        }
        if mask.layer {
            after.set_layer(src.layer());
        }
        if mask.start {
            after.set_start(src.start());
        }
        if mask.end {
            after.set_end(src.end());
        }
        if mask.style {
            after.set_style(src.style());
        }
        if mask.name {
            after.set_name(src.name());
        }
        if mask.margin_l {
            after.set_margin_l(src.margin_l());
        }
        if mask.margin_r {
            after.set_margin_r(src.margin_r());
        }
        if mask.margin_v {
            after.set_margin_v(src.margin_v());
        }
        if mask.effect {
            after.set_effect(src.effect());
        }
        if mask.text {
            after.set_text(src.text());
        }

        cmds.push(EditCommand::ModifyEvent {
            index: idx,
            before: Box::new(before),
            after: Box::new(after),
        });
    }

    Ok(EditCommand::Batch(cmds))
}

fn format_event_clipboard(ev: &AssEvent, out: &mut String) {
    let prefix = match ev.kind() {
        EventKind::Dialogue => "Dialogue",
        EventKind::Comment => "Comment",
    };
    let _ = write!(
        out,
        "{prefix}: {},{},{},{},{},{},{},{},{},{}",
        ev.layer(),
        ev.start(),
        ev.end(),
        ev.style(),
        ev.name(),
        ev.margin_l(),
        ev.margin_r(),
        ev.margin_v(),
        ev.effect(),
        ev.text()
    );
}

fn parse_ass_event_line(line: &str) -> Option<AssEvent> {
    let (kind, rest) = if let Some(r) = line.strip_prefix("Dialogue:") {
        (EventKind::Dialogue, r.trim_start())
    } else {
        let r = line.strip_prefix("Comment:")?;
        (EventKind::Comment, r.trim_start())
    };

    let mut parts = rest.splitn(10, ',');
    let layer = parts.next()?.trim().parse::<i32>().ok()?;
    let start = AssTime::parse_ass(parts.next()?.trim())?;
    let end = AssTime::parse_ass(parts.next()?.trim())?;
    let style = parts.next()?;
    let name = parts.next()?;
    let margin_l = parts.next()?.trim().parse::<i32>().unwrap_or(0);
    let margin_r = parts.next()?.trim().parse::<i32>().unwrap_or(0);
    let margin_v = parts.next()?.trim().parse::<i32>().unwrap_or(0);
    let effect = parts.next()?;
    let text = parts.next()?;

    let mut ev = AssEvent::dialogue(start, end, text);
    ev.set_kind(kind);
    ev.set_layer(layer);
    ev.set_style(style);
    ev.set_name(name);
    ev.set_margin_l(margin_l);
    ev.set_margin_r(margin_r);
    ev.set_margin_v(margin_v);
    ev.set_effect(effect);
    Some(ev)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn sample_script() -> ScriptFile {
        let raw = "\
[Script Info]
ScriptType: v4.00+

[Events]
Format: Layer, Start, End, Style, Name, MarginL, MarginR, MarginV, Effect, Text
Dialogue: 0,0:00:05.00,0:00:07.00,Default,Bob,0,0,0,,Second line
Dialogue: 0,0:00:01.00,0:00:03.00,Default,Alice,0,0,0,,First line
Dialogue: 0,0:00:10.00,0:00:12.00,Sign,Cara,0,0,0,,Third line
";
        ass_core::parse_ass(raw).value
    }

    #[test]
    fn grid_ops_and_undo_roundtrip() {
        let mut script = sample_script();

        // Stable sort by start
        let sort_cmd = stable_sort(&script, SortKey::Start, false, None).expect("sort");
        sort_cmd.apply(&mut script).expect("apply sort");
        let texts: Vec<&str> = script.events().map(AssEvent::text).collect();
        assert_eq!(texts, vec!["First line", "Second line", "Third line"]);

        // Move selection up
        let mv_cmd = move_selection_up(&script, &[1, 2]).expect("move up");
        mv_cmd.apply(&mut script).expect("apply move up");
        let texts2: Vec<&str> = script.events().map(AssEvent::text).collect();
        assert_eq!(texts2, vec!["Second line", "Third line", "First line"]);

        // Comment & uncomment
        let c_cmd = comment_selected(&script, &[0, 2]).expect("comment");
        c_cmd.apply(&mut script).expect("apply comment");
        assert_eq!(
            script.events().next().expect("0").kind(),
            EventKind::Comment
        );
        assert_eq!(
            script.events().nth(1).expect("1").kind(),
            EventKind::Dialogue
        );

        // Duplicate & delete
        let dup_cmd = duplicate_selected(&script, &[1]).expect("dup");
        dup_cmd.apply(&mut script).expect("apply dup");
        assert_eq!(script.events().count(), 4);

        let del_cmd = delete_selected(&script, &[1, 2]).expect("del");
        del_cmd.apply(&mut script).expect("apply del");
        assert_eq!(script.events().count(), 2);
    }

    #[test]
    fn clipboard_copy_paste_and_paste_over() {
        let mut script = sample_script();
        let clip = copy_lines(&script, &[0, 1]).expect("copy");
        assert!(clip.starts_with("Dialogue: 0,0:00:05.00,0:00:07.00,Default,Bob"));

        // Plain-text paste fallback
        let paste_cmd = paste_lines(&script, 1, "Plain line A\nPlain line B").expect("paste");
        paste_cmd.apply(&mut script).expect("apply paste");
        assert_eq!(script.events().count(), 5);
        assert_eq!(script.events().nth(1).expect("1").text(), "Plain line A");

        // Paste-over times only
        let po_cmd =
            paste_over(&script, &[1], &clip, PasteOverMask::times_only()).expect("paste_over");
        po_cmd.apply(&mut script).expect("apply po");
        let row1 = script.events().nth(1).expect("row1");
        assert_eq!(row1.start(), AssTime::from_cs(500));
        assert_eq!(row1.text(), "Plain line A");
    }

    #[test]
    fn aegisub_cps_ignores_tags_drawings_and_handles_breaks() {
        let ev = AssEvent::dialogue(
            AssTime::from_cs(0),
            AssTime::from_cs(200),
            r"{\pos(100,200)\an8}Hello,\hworld!\N{\p1}m 0 0 l 10 10{\p0}Next line.",
        );
        let m = event_metrics(&ev, CpsOptions::default());
        // "Hello,world!" (12 chars, \h ignored) + "Nextline." (9 chars, space ignored) = 21 chars
        assert_eq!(m.char_count, 21);
        assert_eq!(m.max_line_len, 12);
        assert_eq!(m.line_count, 2);
        assert!((m.cps - 10.5).abs() < 1e-6);

        let m_no_punct = event_metrics(
            &ev,
            CpsOptions {
                ignore_punctuation: true,
                ..CpsOptions::default()
            },
        );
        // Strips ',' '!' '.' -> 18 chars
        assert_eq!(m_no_punct.char_count, 18);
    }
}
