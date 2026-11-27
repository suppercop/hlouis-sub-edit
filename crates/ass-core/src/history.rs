//! Command-based undo/redo history over [`ScriptFile`] with text-edit coalescing
//! and a bounded memory budget.

use crate::model::{AssEvent, AssStyle, ScriptFile};
use crate::time::AssTime;
use crate::AssError;
use std::collections::VecDeque;

/// Default memory cap for undo + redo stacks (64 MiB).
pub const DEFAULT_MAX_HISTORY_BYTES: usize = 64 * 1024 * 1024;

/// Compact timestamp change record for bulk timing operations (`Shift Times`, `TPP`).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct EventTimeChange {
    pub index: usize,
    pub before_start: AssTime,
    pub before_end: AssTime,
    pub after_start: AssTime,
    pub after_end: AssTime,
}

/// Atomic, reversible edit operation on a [`ScriptFile`].
#[derive(Debug, Clone, PartialEq)]
pub enum EditCommand {
    /// Insert an event at logical event index `index`.
    InsertEvent { index: usize, event: AssEvent },
    /// Delete an event at logical event index `index`.
    DeleteEvent { index: usize, event: AssEvent },
    /// Delete multiple events at sorted strictly-ascending logical indices `(index, event)`.
    DeleteEvents(Vec<(usize, AssEvent)>),
    /// Reorder events according to permutation `order` (`after[dst] = before[order[dst]]`).
    PermuteEvents(Vec<usize>),
    /// Bulk start/end timestamp changes without cloning event strings or heap-boxing events.
    ModifyEventTimes(Vec<EventTimeChange>),
    /// Replace an entire event at logical event index `index`.
    ModifyEvent {
        index: usize,
        before: Box<AssEvent>,
        after: Box<AssEvent>,
    },
    /// Lightweight text-only modification on event `index` (eligible for coalescing).
    ModifyEventText {
        index: usize,
        before: String,
        after: String,
    },
    /// Insert a style at logical style index `index`.
    InsertStyle { index: usize, style: Box<AssStyle> },
    /// Delete a style at logical style index `index`.
    DeleteStyle { index: usize, style: Box<AssStyle> },
    /// Replace a style at logical style index `index`.
    ModifyStyle {
        index: usize,
        before: Box<AssStyle>,
        after: Box<AssStyle>,
    },
    /// Set, update, or remove a `[Script Info]` key.
    SetScriptInfo {
        key: String,
        before: Option<String>,
        after: Option<String>,
    },
    /// Group multiple commands into a single undo/redo step.
    Batch(Vec<EditCommand>),
}

impl EditCommand {
    /// Applies this command forward onto `script`.
    pub fn apply(&self, script: &mut ScriptFile) -> Result<(), AssError> {
        match self {
            Self::InsertEvent { index, event } => {
                script
                    .ensure_events_section_mut()
                    .insert_event(*index, event.clone());
                Ok(())
            }
            Self::DeleteEvent { index, .. } => {
                let removed = script
                    .events_section_mut()
                    .and_then(|sec| sec.remove_event(*index));
                if removed.is_none() {
                    return Err(AssError::EventIndexOutOfRange(*index));
                }
                Ok(())
            }
            Self::DeleteEvents(items) => {
                if items.is_empty() {
                    return Ok(());
                }
                let indices: Vec<usize> = items.iter().map(|(i, _)| *i).collect();
                let sec = script
                    .events_section_mut()
                    .ok_or(AssError::EventIndexOutOfRange(indices[0]))?;
                sec.remove_events_by_sorted_indices(&indices)
            }
            Self::PermuteEvents(order) => {
                if order.is_empty() {
                    return Ok(());
                }
                let sec = script
                    .events_section_mut()
                    .ok_or(AssError::InvalidPermutation(order.len()))?;
                sec.permute_events(order)
            }
            Self::ModifyEventTimes(changes) => {
                if changes.is_empty() {
                    return Ok(());
                }
                let sec = script
                    .events_section_mut()
                    .ok_or(AssError::EventIndexOutOfRange(changes[0].index))?;
                apply_time_changes(sec, changes, true)
            }
            Self::ModifyEvent { index, after, .. } => {
                let ev = script
                    .events_section_mut()
                    .and_then(|sec| sec.get_event_mut(*index))
                    .ok_or(AssError::EventIndexOutOfRange(*index))?;
                ev.clone_from(after);
                Ok(())
            }
            Self::ModifyEventText { index, after, .. } => {
                let ev = script
                    .events_section_mut()
                    .and_then(|sec| sec.get_event_mut(*index))
                    .ok_or(AssError::EventIndexOutOfRange(*index))?;
                after.clone_into(&mut ev.text);
                ev.raw_line = None;
                Ok(())
            }
            Self::InsertStyle { index, style } => {
                script
                    .ensure_styles_section_mut()
                    .insert_style(*index, (**style).clone());
                Ok(())
            }
            Self::DeleteStyle { index, .. } => {
                let removed = script
                    .styles_section_mut()
                    .and_then(|sec| sec.remove_style(*index));
                if removed.is_none() {
                    return Err(AssError::StyleIndexOutOfRange(*index));
                }
                Ok(())
            }
            Self::ModifyStyle { index, after, .. } => {
                let st = script
                    .styles_section_mut()
                    .and_then(|sec| sec.get_style_mut(*index))
                    .ok_or(AssError::StyleIndexOutOfRange(*index))?;
                st.clone_from(after);
                Ok(())
            }
            Self::SetScriptInfo { key, after, .. } => {
                apply_script_info_value(script, key, after.as_deref());
                Ok(())
            }
            Self::Batch(cmds) => {
                for cmd in cmds {
                    cmd.apply(script)?;
                }
                Ok(())
            }
        }
    }

    /// Reverses this command onto `script`.
    pub fn revert(&self, script: &mut ScriptFile) -> Result<(), AssError> {
        match self {
            Self::InsertEvent { index, .. } => {
                let removed = script
                    .events_section_mut()
                    .and_then(|sec| sec.remove_event(*index));
                if removed.is_none() {
                    return Err(AssError::EventIndexOutOfRange(*index));
                }
                Ok(())
            }
            Self::DeleteEvent { index, event } => {
                script
                    .ensure_events_section_mut()
                    .insert_event(*index, event.clone());
                Ok(())
            }
            Self::DeleteEvents(items) => {
                if items.is_empty() {
                    return Ok(());
                }
                script
                    .ensure_events_section_mut()
                    .insert_events_sorted(items)
            }
            Self::PermuteEvents(order) => {
                if order.is_empty() {
                    return Ok(());
                }
                let inv = invert_permutation(order)?;
                let sec = script
                    .events_section_mut()
                    .ok_or(AssError::InvalidPermutation(order.len()))?;
                sec.permute_events(&inv)
            }
            Self::ModifyEventTimes(changes) => {
                if changes.is_empty() {
                    return Ok(());
                }
                let sec = script
                    .events_section_mut()
                    .ok_or(AssError::EventIndexOutOfRange(changes[0].index))?;
                apply_time_changes(sec, changes, false)
            }
            Self::ModifyEvent { index, before, .. } => {
                let ev = script
                    .events_section_mut()
                    .and_then(|sec| sec.get_event_mut(*index))
                    .ok_or(AssError::EventIndexOutOfRange(*index))?;
                ev.clone_from(before);
                Ok(())
            }
            Self::ModifyEventText { index, before, .. } => {
                let ev = script
                    .events_section_mut()
                    .and_then(|sec| sec.get_event_mut(*index))
                    .ok_or(AssError::EventIndexOutOfRange(*index))?;
                before.clone_into(&mut ev.text);
                ev.raw_line = None;
                Ok(())
            }
            Self::InsertStyle { index, .. } => {
                let removed = script
                    .styles_section_mut()
                    .and_then(|sec| sec.remove_style(*index));
                if removed.is_none() {
                    return Err(AssError::StyleIndexOutOfRange(*index));
                }
                Ok(())
            }
            Self::DeleteStyle { index, style } => {
                script
                    .ensure_styles_section_mut()
                    .insert_style(*index, (**style).clone());
                Ok(())
            }
            Self::ModifyStyle { index, before, .. } => {
                let st = script
                    .styles_section_mut()
                    .and_then(|sec| sec.get_style_mut(*index))
                    .ok_or(AssError::StyleIndexOutOfRange(*index))?;
                st.clone_from(before);
                Ok(())
            }
            Self::SetScriptInfo { key, before, .. } => {
                apply_script_info_value(script, key, before.as_deref());
                Ok(())
            }
            Self::Batch(cmds) => {
                for cmd in cmds.iter().rev() {
                    cmd.revert(script)?;
                }
                Ok(())
            }
        }
    }

    /// Approximate heap + stack memory footprint in bytes for bounded-history eviction.
    #[must_use]
    pub fn approx_bytes(&self) -> usize {
        let base = std::mem::size_of::<Self>();
        let extra = match self {
            Self::InsertEvent { event, .. } | Self::DeleteEvent { event, .. } => {
                event_heap_bytes(event)
            }
            Self::DeleteEvents(items) => {
                items.capacity() * std::mem::size_of::<(usize, AssEvent)>()
                    + items
                        .iter()
                        .map(|(_, ev)| event_heap_bytes(ev))
                        .sum::<usize>()
            }
            Self::PermuteEvents(order) => order.capacity() * std::mem::size_of::<usize>(),
            Self::ModifyEventTimes(changes) => {
                changes.capacity() * std::mem::size_of::<EventTimeChange>()
            }
            Self::ModifyEvent { before, after, .. } => {
                event_heap_bytes(before) + event_heap_bytes(after)
            }
            Self::ModifyEventText { before, after, .. } => before.capacity() + after.capacity(),
            Self::InsertStyle { style, .. } | Self::DeleteStyle { style, .. } => {
                style_heap_bytes(style)
            }
            Self::ModifyStyle { before, after, .. } => {
                style_heap_bytes(before) + style_heap_bytes(after)
            }
            Self::SetScriptInfo { key, before, after } => {
                key.capacity()
                    + before.as_ref().map_or(0, String::capacity)
                    + after.as_ref().map_or(0, String::capacity)
            }
            Self::Batch(cmds) => cmds.iter().map(Self::approx_bytes).sum(),
        };
        base.saturating_add(extra)
    }
}

fn apply_time_changes(
    sec: &mut crate::model::EventsSection,
    changes: &[EventTimeChange],
    forward: bool,
) -> Result<(), AssError> {
    if sec.refresh_all_events_cache() {
        for ch in changes {
            let Some(crate::model::EventEntry::Event(ev)) = sec.entries.get_mut(ch.index) else {
                return Err(AssError::EventIndexOutOfRange(ch.index));
            };
            let (s, e) = if forward {
                (ch.after_start, ch.after_end)
            } else {
                (ch.before_start, ch.before_end)
            };
            ev.set_start(s);
            ev.set_end(e);
        }
        return Ok(());
    }

    // Fallback when non-event entries (comments/blanks) exist: single O(N + M) pass if sorted,
    // or indexed lookup otherwise.
    let mut entry_positions: Vec<usize> = Vec::with_capacity(sec.entries.len());
    for (pos, entry) in sec.entries.iter().enumerate() {
        if matches!(entry, crate::model::EventEntry::Event(_)) {
            entry_positions.push(pos);
        }
    }
    for ch in changes {
        let Some(&pos) = entry_positions.get(ch.index) else {
            return Err(AssError::EventIndexOutOfRange(ch.index));
        };
        if let crate::model::EventEntry::Event(ref mut ev) = sec.entries[pos] {
            let (s, e) = if forward {
                (ch.after_start, ch.after_end)
            } else {
                (ch.before_start, ch.before_end)
            };
            ev.set_start(s);
            ev.set_end(e);
        }
    }
    Ok(())
}

fn apply_script_info_value(script: &mut ScriptFile, key: &str, value: Option<&str>) {
    match value {
        Some(v) => script.ensure_script_info_mut().set(key, v),
        None => {
            if let Some(info) = script.script_info_mut() {
                let _ = info.remove(key);
            }
        }
    }
}

fn invert_permutation(order: &[usize]) -> Result<Vec<usize>, AssError> {
    let n = order.len();
    let mut inv = vec![usize::MAX; n];
    for (dst, &src) in order.iter().enumerate() {
        if src >= n || inv[src] != usize::MAX {
            return Err(AssError::InvalidPermutation(n));
        }
        inv[src] = dst;
    }
    Ok(inv)
}

fn event_heap_bytes(ev: &AssEvent) -> usize {
    ev.style.capacity()
        + ev.name.capacity()
        + ev.effect.capacity()
        + ev.text.capacity()
        + ev.raw_line.as_ref().map_or(0, String::capacity)
}

fn style_heap_bytes(st: &AssStyle) -> usize {
    st.name.capacity()
        + st.fontname.capacity()
        + st.primary_colour.capacity()
        + st.secondary_colour.capacity()
        + st.outline_colour.capacity()
        + st.back_colour.capacity()
        + st.raw_line.as_ref().map_or(0, String::capacity)
}

struct HistoryEntry {
    cmd: EditCommand,
    bytes: usize,
}

/// Memory-bounded command undo/redo stack with consecutive same-line text coalescing.
pub struct UndoHistory {
    undo_stack: VecDeque<HistoryEntry>,
    redo_stack: Vec<HistoryEntry>,
    used_bytes: usize,
    max_bytes: usize,
    coalescing_open: bool,
}

impl Default for UndoHistory {
    fn default() -> Self {
        Self::new(DEFAULT_MAX_HISTORY_BYTES)
    }
}

impl UndoHistory {
    /// Creates a new history capped at `max_bytes` approximate memory.
    #[must_use]
    pub fn new(max_bytes: usize) -> Self {
        Self {
            undo_stack: VecDeque::new(),
            redo_stack: Vec::new(),
            used_bytes: 0,
            max_bytes: max_bytes.max(1024),
            coalescing_open: false,
        }
    }

    /// Prevents the next text edit from merging with the previous undo entry (e.g. on focus loss).
    pub fn break_coalescing(&mut self) {
        self.coalescing_open = false;
    }

    /// Number of steps currently on the undo stack.
    #[must_use]
    pub fn undo_len(&self) -> usize {
        self.undo_stack.len()
    }

    /// Number of steps currently on the redo stack.
    #[must_use]
    pub fn redo_len(&self) -> usize {
        self.redo_stack.len()
    }

    /// Approximate tracked memory usage in bytes.
    #[must_use]
    pub const fn used_bytes(&self) -> usize {
        self.used_bytes
    }

    /// Applies `cmd` to `script` and pushes it onto the undo stack, clearing the redo stack.
    /// Consecutive `ModifyEventText` commands targeting the same line coalesce in-place.
    pub fn execute(&mut self, script: &mut ScriptFile, cmd: EditCommand) -> Result<(), AssError> {
        cmd.apply(script)?;
        self.clear_redo();

        if self.coalescing_open {
            if let (
                Some(last),
                EditCommand::ModifyEventText {
                    index,
                    after: new_after,
                    ..
                },
            ) = (self.undo_stack.back_mut(), &cmd)
            {
                if let EditCommand::ModifyEventText {
                    index: prev_index,
                    after: prev_after,
                    ..
                } = &mut last.cmd
                {
                    if *prev_index == *index {
                        let old_bytes = last.bytes;
                        prev_after.clone_from(new_after);
                        let new_bytes = last.cmd.approx_bytes();
                        last.bytes = new_bytes;
                        self.used_bytes = self
                            .used_bytes
                            .saturating_sub(old_bytes)
                            .saturating_add(new_bytes);
                        self.evict_oldest_if_over_budget();
                        return Ok(());
                    }
                }
            }
        }

        self.coalescing_open = matches!(cmd, EditCommand::ModifyEventText { .. });
        let bytes = cmd.approx_bytes();
        self.undo_stack.push_back(HistoryEntry { cmd, bytes });
        self.used_bytes = self.used_bytes.saturating_add(bytes);
        self.evict_oldest_if_over_budget();
        Ok(())
    }

    /// Undoes the most recent command, returning `true` if a step was undone.
    pub fn undo(&mut self, script: &mut ScriptFile) -> Result<bool, AssError> {
        self.coalescing_open = false;
        let Some(entry) = self.undo_stack.pop_back() else {
            return Ok(false);
        };
        entry.cmd.revert(script)?;
        self.redo_stack.push(entry);
        Ok(true)
    }

    /// Redoes the most recently undone command, returning `true` if a step was redone.
    pub fn redo(&mut self, script: &mut ScriptFile) -> Result<bool, AssError> {
        self.coalescing_open = false;
        let Some(entry) = self.redo_stack.pop() else {
            return Ok(false);
        };
        entry.cmd.apply(script)?;
        self.undo_stack.push_back(entry);
        Ok(true)
    }

    fn clear_redo(&mut self) {
        for entry in self.redo_stack.drain(..) {
            self.used_bytes = self.used_bytes.saturating_sub(entry.bytes);
        }
    }

    fn evict_oldest_if_over_budget(&mut self) {
        while self.used_bytes > self.max_bytes && self.undo_stack.len() > 1 {
            if let Some(oldest) = self.undo_stack.pop_front() {
                self.used_bytes = self.used_bytes.saturating_sub(oldest.bytes);
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::model::{AssEvent, AssStyle, EventEntry};
    use crate::time::AssTime;
    use std::time::Instant;

    #[test]
    fn coalesces_consecutive_text_edits_and_supports_undo_redo() {
        let mut script = ScriptFile::new_default();
        let mut history = UndoHistory::default();

        history
            .execute(
                &mut script,
                EditCommand::InsertEvent {
                    index: 0,
                    event: AssEvent {
                        text: "H".to_owned(),
                        ..AssEvent::default()
                    },
                },
            )
            .expect("insert");
        assert_eq!(history.undo_len(), 1);

        // Consecutive text edits on line 0 coalesce into a single undo entry.
        history
            .execute(
                &mut script,
                EditCommand::ModifyEventText {
                    index: 0,
                    before: "H".to_owned(),
                    after: "He".to_owned(),
                },
            )
            .expect("edit 1");
        history
            .execute(
                &mut script,
                EditCommand::ModifyEventText {
                    index: 0,
                    before: "He".to_owned(),
                    after: "Hello".to_owned(),
                },
            )
            .expect("edit 2");
        assert_eq!(history.undo_len(), 2);
        assert_eq!(
            script
                .events_section()
                .and_then(|s| s.get_event(0))
                .map(|e| e.text.as_str()),
            Some("Hello")
        );

        // Single undo reverts both coalesced edits back to "H".
        assert!(history.undo(&mut script).expect("undo"));
        assert_eq!(
            script
                .events_section()
                .and_then(|s| s.get_event(0))
                .map(|e| e.text.as_str()),
            Some("H")
        );

        // Redo restores "Hello".
        assert!(history.redo(&mut script).expect("redo"));
        assert_eq!(
            script
                .events_section()
                .and_then(|s| s.get_event(0))
                .map(|e| e.text.as_str()),
            Some("Hello")
        );
    }

    #[test]
    fn style_edits_and_memory_bound_eviction_work() {
        let mut script = ScriptFile::new_default();
        let mut history = UndoHistory::new(2048);

        let custom = AssStyle {
            name: "Sign".to_owned(),
            fontsize: 64.0,
            ..AssStyle::default()
        };
        history
            .execute(
                &mut script,
                EditCommand::InsertStyle {
                    index: 1,
                    style: Box::new(custom),
                },
            )
            .expect("insert style");
        assert_eq!(script.styles().count(), 2);

        history.undo(&mut script).expect("undo style");
        assert_eq!(script.styles().count(), 1);
        history.redo(&mut script).expect("redo style");
        assert_eq!(script.styles().count(), 2);

        // Push many large text edits with coalescing broken to trigger bounded eviction.
        history
            .execute(
                &mut script,
                EditCommand::InsertEvent {
                    index: 0,
                    event: AssEvent::default(),
                },
            )
            .expect("insert event");
        for i in 0..30 {
            history.break_coalescing();
            let payload = format!("{i}:{}", "x".repeat(200));
            history
                .execute(
                    &mut script,
                    EditCommand::ModifyEventText {
                        index: 0,
                        before: String::new(),
                        after: payload,
                    },
                )
                .expect("large edit");
        }
        assert!(history.used_bytes() <= 2048);
        assert!(history.undo_len() < 30);
    }

    #[test]
    fn undo_redo_step_is_fast_on_20k_lines() {
        let mut script = ScriptFile::new_default();
        let entries: Vec<EventEntry> = (0..20_000)
            .map(|i| {
                EventEntry::Event(AssEvent {
                    start: AssTime::from_cs(i * 100),
                    end: AssTime::from_cs(i * 100 + 80),
                    text: format!("Line {i}"),
                    ..AssEvent::default()
                })
            })
            .collect();
        script.ensure_events_section_mut().entries = entries;

        let mut history = UndoHistory::default();
        history
            .execute(
                &mut script,
                EditCommand::ModifyEventText {
                    index: 19_999,
                    before: "Line 19999".to_owned(),
                    after: "Edited 19999".to_owned(),
                },
            )
            .expect("edit");

        let t0 = Instant::now();
        assert!(history.undo(&mut script).expect("undo"));
        assert!(history.redo(&mut script).expect("redo"));
        let elapsed_ms = t0.elapsed().as_secs_f64() * 1000.0;
        assert!(
            elapsed_ms < 5.0,
            "undo+redo on 20k lines took {elapsed_ms:.3} ms (budget < 5 ms)"
        );
    }

    #[test]
    fn set_script_info_supports_insert_update_remove_and_undo_redo() {
        let mut script = ScriptFile::new_default();
        let mut history = UndoHistory::default();

        history
            .execute(
                &mut script,
                EditCommand::SetScriptInfo {
                    key: "YCbCr Matrix".to_owned(),
                    before: None,
                    after: Some("TV.709".to_owned()),
                },
            )
            .expect("insert");
        assert_eq!(
            script.script_info().and_then(|s| s.get("YCbCr Matrix")),
            Some("TV.709")
        );

        history
            .execute(
                &mut script,
                EditCommand::SetScriptInfo {
                    key: "PlayResX".to_owned(),
                    before: Some("1920".to_owned()),
                    after: Some("1280".to_owned()),
                },
            )
            .expect("update");
        assert_eq!(
            script.script_info().and_then(|s| s.get("PlayResX")),
            Some("1280")
        );

        assert!(history.undo(&mut script).expect("undo update"));
        assert_eq!(
            script.script_info().and_then(|s| s.get("PlayResX")),
            Some("1920")
        );

        assert!(history.undo(&mut script).expect("undo insert"));
        assert_eq!(
            script.script_info().and_then(|s| s.get("YCbCr Matrix")),
            None
        );

        assert!(history.redo(&mut script).expect("redo insert"));
        assert_eq!(
            script.script_info().and_then(|s| s.get("YCbCr Matrix")),
            Some("TV.709")
        );
    }

    #[test]
    fn permute_and_batch_delete_preserve_comments_and_round_trip_undo_redo() {
        let raw = "\
[Script Info]
ScriptType: v4.00+

[Events]
Format: Layer, Start, End, Style, Name, MarginL, MarginR, MarginV, Effect, Text
; leading comment
Dialogue: 0,0:00:01.00,0:00:02.00,Default,,0,0,0,,Zero
Dialogue: 0,0:00:02.00,0:00:03.00,Default,,0,0,0,,One
; middle comment
Dialogue: 0,0:00:03.00,0:00:04.00,Default,,0,0,0,,Two
Dialogue: 0,0:00:04.00,0:00:05.00,Default,,0,0,0,,Three
";
        let mut script = crate::parse_ass(raw).value;
        let mut history = UndoHistory::default();

        // Permute [Zero, One, Two, Three] -> [Two, Three, Zero, One].
        history
            .execute(&mut script, EditCommand::PermuteEvents(vec![2, 3, 0, 1]))
            .expect("permute");
        let texts: Vec<&str> = script.events().map(AssEvent::text).collect();
        assert_eq!(texts, vec!["Two", "Three", "Zero", "One"]);
        // Non-event entries stay in place.
        assert!(matches!(
            script.events_section().unwrap().entries[0],
            EventEntry::Comment(_)
        ));
        assert!(matches!(
            script.events_section().unwrap().entries[3],
            EventEntry::Comment(_)
        ));

        history.undo(&mut script).expect("undo permute");
        let reverted: Vec<&str> = script.events().map(AssEvent::text).collect();
        assert_eq!(reverted, vec!["Zero", "One", "Two", "Three"]);

        // Batch delete rows 0 and 2 ([Zero, Two]) -> leaves [One, Three], then undo restores all.
        let ev0 = script.events().next().unwrap().clone();
        let ev2 = script.events().nth(2).unwrap().clone();
        history
            .execute(
                &mut script,
                EditCommand::DeleteEvents(vec![(0, ev0), (2, ev2)]),
            )
            .expect("batch delete");
        let after_del: Vec<&str> = script.events().map(AssEvent::text).collect();
        assert_eq!(after_del, vec!["One", "Three"]);

        history.undo(&mut script).expect("undo delete");
        let restored: Vec<&str> = script.events().map(AssEvent::text).collect();
        assert_eq!(restored, vec!["Zero", "One", "Two", "Three"]);
        assert_eq!(crate::serialize_ass(&script), raw);

        // Invalid permutation rejected cleanly.
        assert!(matches!(
            EditCommand::PermuteEvents(vec![0, 0, 1, 2]).apply(&mut script),
            Err(AssError::InvalidPermutation(4))
        ));
    }
}
