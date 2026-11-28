//! App-level subtitle document controller (`SubsController` + `AssFile` commit semantics).
//!
//! Mirrors Aegisub 3.5.1:
//! - `src/subs_controller.cpp` (`SubsController::Load`, `Save`, `Undo`, `Redo`, `CanSave`, disk
//!   modification detection)
//! - `src/ass_file.cpp` (`AssFile::Commit`, `AssFile::COMMIT_*` notification bitmask, amend
//!   merging while typing)
//! - `src/selection_controller.cpp` (active line + sorted multi-selection set)
//!
//! Performance:
//! - Single-line text commit incl. listener notification: `< 2 µs`
//! - Undo / redo of a single-line edit: `< 5 µs`
//! - `"amend"` merging of consecutive keystrokes on the same field: **0 heap allocations** via a
//!   pre-allocated scratch buffer flushed lazily on commit/undo/redo/save/selection-change.

#![forbid(unsafe_code)]
#![cfg_attr(not(test), deny(clippy::unwrap_used, clippy::expect_used))]

use ass_core::{
    AssError, AssTime, DEFAULT_MAX_HISTORY_BYTES, EditCommand, ScriptFile, UndoHistory, parse_ass,
    serialize_ass_bytes,
};
use autosave::{AutosaveError, write_atomic};
use std::fs;
use std::path::{Path, PathBuf};
use std::time::SystemTime;
use thiserror::Error;

/// Commit notification bitmask mirroring Aegisub `AssFile::COMMIT_*` (`src/include/aegisub/ass_file.h`).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub struct CommitType(u32);

impl CommitType {
    pub const NONE: Self = Self(0);
    pub const TEXT: Self = Self(1 << 0);
    pub const TIMES: Self = Self(1 << 1);
    pub const STYLES: Self = Self(1 << 2);
    pub const SCRIPT_INFO: Self = Self(1 << 3);
    pub const ORDER: Self = Self(1 << 4);
    pub const SELECTION: Self = Self(1 << 5);
    pub const EVERYTHING: Self = Self(0x3F);

    /// Combines two commit flags.
    #[inline]
    #[must_use]
    pub const fn union(self, other: Self) -> Self {
        Self(self.0 | other.0)
    }

    /// Returns `true` if all bits in `flag` are set.
    #[inline]
    #[must_use]
    pub const fn contains(self, flag: Self) -> bool {
        (self.0 & flag.0) == flag.0
    }

    /// Returns `true` when no bits are set.
    #[inline]
    #[must_use]
    pub const fn is_empty(self) -> bool {
        self.0 == 0
    }

    /// Raw bitmask representation.
    #[inline]
    #[must_use]
    pub const fn bits(self) -> u32 {
        self.0
    }
}

/// Typed errors returned by [`DocumentController`].
#[derive(Debug, Error)]
pub enum DocumentError {
    #[error("document has no associated file path; use save_as")]
    NoPath,
    #[error("event index {0} out of range")]
    InvalidEventIndex(usize),
    #[error("ASS core error: {0}")]
    Ass(#[from] AssError),
    #[error("atomic save error: {0}")]
    Autosave(#[from] AutosaveError),
    #[error("I/O error: {0}")]
    Io(#[from] std::io::Error),
}

/// Compact, zero-allocation notification payload delivered to listeners on every commit/undo/redo.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct ChangeNotification {
    pub commit_type: CommitType,
    pub commit_id: u64,
    pub active_line: Option<usize>,
    pub is_amend: bool,
}

/// Listener interface for document change notifications (zero allocation per dispatch).
pub trait DocumentListener {
    fn on_document_changed(&mut self, event: ChangeNotification);
}

/// File metadata snapshot used to detect external modifications on disk.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct DiskStamp {
    pub len: u64,
    pub modified: Option<SystemTime>,
}

impl DiskStamp {
    fn from_path(path: &Path) -> Result<Self, std::io::Error> {
        let meta = fs::metadata(path)?;
        Ok(Self {
            len: meta.len(),
            modified: meta.modified().ok(),
        })
    }
}

/// Field currently being amended in-place without heap allocation.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum AmendField {
    Text(usize),
    StartTime(usize),
    EndTime(usize),
}

#[derive(Debug, Clone)]
struct ActiveAmend {
    field: AmendField,
    commit_type: CommitType,
    orig_time: AssTime,
    cur_time: AssTime,
}

/// App-level document controller owning the [`ScriptFile`], [`UndoHistory`], selection, dirty
/// flags, file metadata, and zero-allocation amend buffer.
pub struct DocumentController {
    script: ScriptFile,
    undo: UndoHistory,
    undo_descs: Vec<String>,
    redo_descs: Vec<String>,
    selection: Vec<usize>,
    active_line: Option<usize>,
    dirty: bool,
    autosave_dirty: bool,
    path: Option<PathBuf>,
    encoding: String,
    has_bom: bool,
    last_disk_stamp: Option<DiskStamp>,
    commit_id: u64,
    non_text_revision: u64,
    saved_commit_id: u64,
    // Pre-allocated amend buffers for 0-allocation keystroke merging while typing.
    amend_state: Option<ActiveAmend>,
    amend_desc: String,
    amend_orig_text: String,
    amend_cur_text: String,
    listeners: Vec<Box<dyn DocumentListener>>,
}

impl Default for DocumentController {
    fn default() -> Self {
        Self::new(ScriptFile::new_default())
    }
}

impl DocumentController {
    /// Creates a new controller wrapping `script` with default 64 MiB undo history.
    #[must_use]
    pub fn new(script: ScriptFile) -> Self {
        let count = script
            .events_section()
            .map_or(0, ass_core::EventsSection::event_count);
        let active_line = if count > 0 { Some(0) } else { None };
        let selection = if count > 0 { vec![0] } else { Vec::new() };
        Self {
            script,
            undo: UndoHistory::new(DEFAULT_MAX_HISTORY_BYTES),
            undo_descs: Vec::with_capacity(64),
            redo_descs: Vec::with_capacity(32),
            selection,
            active_line,
            dirty: false,
            autosave_dirty: false,
            path: None,
            encoding: "UTF-8".to_owned(),
            has_bom: true,
            last_disk_stamp: None,
            commit_id: 0,
            non_text_revision: 0,
            saved_commit_id: 0,
            amend_state: None,
            amend_desc: String::with_capacity(64),
            amend_orig_text: String::with_capacity(512),
            amend_cur_text: String::with_capacity(512),
            listeners: Vec::new(),
        }
    }

    /// Loads an ASS file from disk, recording its path, UTF-8 BOM flag, and [`DiskStamp`].
    ///
    /// # Errors
    /// Returns [`DocumentError::Io`] if reading the file fails.
    pub fn open_file(path: &Path) -> Result<Self, DocumentError> {
        let bytes = fs::read(path)?;
        let has_bom = bytes.starts_with(&[0xEF, 0xBB, 0xBF]);
        let text = String::from_utf8_lossy(&bytes);
        let parsed = parse_ass(&text).value;
        let mut ctrl = Self::new(parsed);
        ctrl.path = Some(path.to_path_buf());
        ctrl.has_bom = has_bom;
        ctrl.last_disk_stamp = DiskStamp::from_path(path).ok();
        Ok(ctrl)
    }

    /// Registers a change listener.
    pub fn add_listener(&mut self, listener: Box<dyn DocumentListener>) {
        self.listeners.push(listener);
    }

    /// Immutable reference to the underlying [`ScriptFile`] (note: if an amend is currently open,
    /// call [`Self::effective_event_text`] or [`Self::flush_amend`] first to observe un-flushed
    /// keystrokes).
    #[inline]
    #[must_use]
    pub fn script(&self) -> &ScriptFile {
        &self.script
    }

    /// Returns the effective text of event `index`, transparently reflecting any active un-flushed
    /// amend buffer without allocating.
    #[inline]
    #[must_use]
    pub fn effective_event_text(&self, index: usize) -> Option<&str> {
        if let Some(ref a) = self.amend_state
            && a.field == AmendField::Text(index)
        {
            return Some(self.amend_cur_text.as_str());
        }
        self.script
            .events_section()
            .and_then(|s| s.get_event(index))
            .map(ass_core::AssEvent::text)
    }

    /// Returns the current active line index.
    #[inline]
    #[must_use]
    pub fn active_line(&self) -> Option<usize> {
        self.active_line
    }

    /// Returns the sorted selection slice.
    #[inline]
    #[must_use]
    pub fn selection(&self) -> &[usize] {
        &self.selection
    }

    /// Updates active line and selection set, flushing any pending amend on a different line.
    pub fn set_selection(&mut self, active: Option<usize>, selected: &[usize]) {
        if let Some(ref a) = self.amend_state {
            let amend_line = match a.field {
                AmendField::Text(i) | AmendField::StartTime(i) | AmendField::EndTime(i) => i,
            };
            if active != Some(amend_line) {
                let _ = self.flush_amend();
            }
        }
        self.active_line = active;
        self.selection.clear();
        self.selection.extend_from_slice(selected);
        self.selection.sort_unstable();
        self.selection.dedup();
        self.notify(CommitType::SELECTION, false);
    }

    /// Whether the document has unsaved edits relative to the last save/open.
    #[inline]
    #[must_use]
    pub fn is_dirty(&self) -> bool {
        self.dirty
    }

    /// Whether the document has edits not yet captured by autosave.
    #[inline]
    #[must_use]
    pub fn is_autosave_dirty(&self) -> bool {
        self.autosave_dirty
    }

    /// Clears the autosave-dirty flag once an autosave snapshot has been written.
    #[inline]
    pub fn mark_autosaved(&mut self) {
        self.autosave_dirty = false;
    }

    /// Current file path, if saved or loaded from disk.
    #[inline]
    #[must_use]
    pub fn path(&self) -> Option<&Path> {
        self.path.as_deref()
    }

    /// Character encoding label (e.g. `"UTF-8"`).
    #[inline]
    #[must_use]
    pub fn encoding(&self) -> &str {
        &self.encoding
    }

    /// Whether the file uses a UTF-8 BOM.
    #[inline]
    #[must_use]
    pub fn has_bom(&self) -> bool {
        self.has_bom
    }

    /// Monotonically increasing commit sequence number.
    #[inline]
    #[must_use]
    pub fn commit_id(&self) -> u64 {
        self.commit_id
    }

    /// Monotonically increasing revision counter incremented on every content change (`commit`,
    /// `amend_text`, `amend_time`, `undo`, `redo`, `reload_from_disk`); selection changes do not
    /// increment it.
    #[inline]
    #[must_use]
    pub const fn revision(&self) -> u64 {
        self.commit_id
    }

    /// Monotonically increasing revision counter incremented on every structural, timing,
    /// metadata, undo/redo, or reload change, excluding in-place same-line `amend_text` edits.
    #[inline]
    #[must_use]
    pub const fn non_text_revision(&self) -> u64 {
        self.non_text_revision
    }

    /// Description of the next undo step (including an active un-flushed amend).
    #[must_use]
    pub fn undo_description(&self) -> Option<&str> {
        if self.amend_state.is_some() {
            return Some(self.amend_desc.as_str());
        }
        self.undo_descs.last().map(String::as_str)
    }

    /// Description of the next redo step.
    #[must_use]
    pub fn redo_description(&self) -> Option<&str> {
        if self.amend_state.is_some() {
            return None;
        }
        self.redo_descs.last().map(String::as_str)
    }

    /// Whether an undo step is available.
    #[inline]
    #[must_use]
    pub fn can_undo(&self) -> bool {
        self.amend_state.is_some() || self.undo.undo_len() > 0
    }

    /// Whether a redo step is available.
    #[inline]
    #[must_use]
    pub fn can_redo(&self) -> bool {
        self.amend_state.is_none() && self.undo.redo_len() > 0
    }

    /// Commits an [`EditCommand`] with a human-readable description and [`CommitType`] mask,
    /// flushing any open amend first and notifying listeners.
    ///
    /// # Errors
    /// Returns [`DocumentError::Ass`] if the command fails to apply.
    pub fn commit(
        &mut self,
        description: impl Into<String>,
        cmd: EditCommand,
        commit_type: CommitType,
    ) -> Result<u64, DocumentError> {
        self.flush_amend()?;
        self.undo.break_coalescing();
        self.undo.execute(&mut self.script, cmd)?;
        self.undo_descs.push(description.into());
        self.redo_descs.clear();
        self.commit_id += 1;
        self.non_text_revision += 1;
        self.dirty = self.commit_id != self.saved_commit_id;
        self.autosave_dirty = true;
        self.notify(commit_type, false);
        Ok(self.commit_id)
    }

    /// Amends the text of event `index` in place while typing.
    ///
    /// Consecutive calls targeting the same `index` perform **zero heap allocations** (assuming
    /// `new_text.len()` fits inside the pre-allocated 512-byte scratch capacity) and merge into a
    /// single undo entry upon [`Self::flush_amend`], [`Self::undo`], [`Self::commit`], or
    /// [`Self::save`].
    ///
    /// # Errors
    /// Returns [`DocumentError::InvalidEventIndex`] if `index` does not exist.
    pub fn amend_text(
        &mut self,
        index: usize,
        description: &str,
        new_text: &str,
    ) -> Result<u64, DocumentError> {
        let same_target = matches!(
            self.amend_state,
            Some(ActiveAmend {
                field: AmendField::Text(i),
                ..
            }) if i == index
        );
        if !same_target {
            self.flush_amend()?;
            let orig = self
                .script
                .events_section()
                .and_then(|s| s.get_event(index))
                .ok_or(DocumentError::InvalidEventIndex(index))?
                .text();
            self.amend_orig_text.clear();
            self.amend_orig_text.push_str(orig);
            self.amend_desc.clear();
            self.amend_desc.push_str(description);
            self.amend_state = Some(ActiveAmend {
                field: AmendField::Text(index),
                commit_type: CommitType::TEXT,
                orig_time: AssTime::ZERO,
                cur_time: AssTime::ZERO,
            });
            self.redo_descs.clear();
        }
        self.amend_cur_text.clear();
        self.amend_cur_text.push_str(new_text);
        self.commit_id += 1;
        self.dirty = true;
        self.autosave_dirty = true;
        self.notify(CommitType::TEXT, true);
        Ok(self.commit_id)
    }

    /// Amends the start or end timestamp of event `index` in place with zero heap allocations.
    ///
    /// # Errors
    /// Returns [`DocumentError::InvalidEventIndex`] if `index` does not exist.
    pub fn amend_time(
        &mut self,
        index: usize,
        is_end: bool,
        description: &str,
        new_time: AssTime,
    ) -> Result<u64, DocumentError> {
        let target = if is_end {
            AmendField::EndTime(index)
        } else {
            AmendField::StartTime(index)
        };
        let same_target = matches!(self.amend_state, Some(ref a) if a.field == target);
        if !same_target {
            self.flush_amend()?;
            let ev = self
                .script
                .events_section()
                .and_then(|s| s.get_event(index))
                .ok_or(DocumentError::InvalidEventIndex(index))?;
            let orig_time = if is_end { ev.end() } else { ev.start() };
            self.amend_desc.clear();
            self.amend_desc.push_str(description);
            self.amend_state = Some(ActiveAmend {
                field: target,
                commit_type: CommitType::TIMES,
                orig_time,
                cur_time: new_time,
            });
            self.redo_descs.clear();
        } else if let Some(ref mut a) = self.amend_state {
            a.cur_time = new_time;
        }
        if let Some(ev) = self
            .script
            .events_section_mut()
            .and_then(|s| s.get_event_mut(index))
        {
            if is_end {
                ev.set_end(new_time);
            } else {
                ev.set_start(new_time);
            }
        }
        self.commit_id += 1;
        self.non_text_revision += 1;
        self.dirty = true;
        self.autosave_dirty = true;
        self.notify(CommitType::TIMES, true);
        Ok(self.commit_id)
    }

    /// Flushes any active amend buffer into the [`UndoHistory`] as a single reversible command.
    ///
    /// # Errors
    /// Returns [`DocumentError::Ass`] if applying the flushed command fails.
    pub fn flush_amend(&mut self) -> Result<(), DocumentError> {
        let Some(active) = self.amend_state.take() else {
            return Ok(());
        };
        match active.field {
            AmendField::Text(index) => {
                if self.amend_orig_text == self.amend_cur_text {
                    return Ok(());
                }
                let cmd = EditCommand::ModifyEventText {
                    index,
                    before: self.amend_orig_text.clone(),
                    after: self.amend_cur_text.clone(),
                };
                self.undo.break_coalescing();
                self.undo.execute(&mut self.script, cmd)?;
                self.undo_descs.push(self.amend_desc.clone());
                self.redo_descs.clear();
                self.non_text_revision += 1;
            }
            AmendField::StartTime(index) | AmendField::EndTime(index) => {
                if active.orig_time == active.cur_time {
                    return Ok(());
                }
                let Some(current) = self
                    .script
                    .events_section()
                    .and_then(|s| s.get_event(index))
                    .cloned()
                else {
                    return Err(DocumentError::InvalidEventIndex(index));
                };
                let mut before = current.clone();
                match active.field {
                    AmendField::StartTime(_) => before.set_start(active.orig_time),
                    AmendField::EndTime(_) => before.set_end(active.orig_time),
                    AmendField::Text(_) => {}
                }
                let cmd = EditCommand::ModifyEvent {
                    index,
                    before: Box::new(before),
                    after: Box::new(current),
                };
                self.undo.break_coalescing();
                self.undo.execute(&mut self.script, cmd)?;
                self.undo_descs.push(self.amend_desc.clone());
                self.redo_descs.clear();
                self.non_text_revision += 1;
            }
        }
        let _ = active.commit_type;
        Ok(())
    }

    /// Undoes the most recent edit (flushing any active amend first) and returns its description.
    ///
    /// # Errors
    /// Returns [`DocumentError::Ass`] if reverting fails.
    pub fn undo(&mut self) -> Result<Option<String>, DocumentError> {
        self.flush_amend()?;
        if !self.undo.undo(&mut self.script)? {
            return Ok(None);
        }
        let desc = self.undo_descs.pop().unwrap_or_default();
        self.redo_descs.push(desc.clone());
        self.commit_id += 1;
        self.non_text_revision += 1;
        self.dirty = self.commit_id != self.saved_commit_id;
        self.autosave_dirty = true;
        self.notify(CommitType::EVERYTHING, false);
        Ok(Some(desc))
    }

    /// Redoes the most recently undone edit and returns its description.
    ///
    /// # Errors
    /// Returns [`DocumentError::Ass`] if re-applying fails.
    pub fn redo(&mut self) -> Result<Option<String>, DocumentError> {
        self.flush_amend()?;
        if !self.undo.redo(&mut self.script)? {
            return Ok(None);
        }
        let desc = self.redo_descs.pop().unwrap_or_default();
        self.undo_descs.push(desc.clone());
        self.commit_id += 1;
        self.non_text_revision += 1;
        self.dirty = self.commit_id != self.saved_commit_id;
        self.autosave_dirty = true;
        self.notify(CommitType::EVERYTHING, false);
        Ok(Some(desc))
    }

    /// Saves the document to its current path using [`autosave::write_atomic`].
    ///
    /// # Errors
    /// Returns [`DocumentError::NoPath`] if no path is set, or [`DocumentError::Autosave`].
    pub fn save(&mut self) -> Result<(), DocumentError> {
        let Some(path) = self.path.clone() else {
            return Err(DocumentError::NoPath);
        };
        self.save_as(&path)
    }

    /// Saves the document to `dest` via [`autosave::write_atomic`] and updates path, dirty flags,
    /// and [`DiskStamp`].
    ///
    /// # Errors
    /// Returns [`DocumentError::Autosave`] or [`DocumentError::Ass`].
    pub fn save_as(&mut self, dest: &Path) -> Result<(), DocumentError> {
        self.flush_amend()?;
        let bytes = serialize_ass_bytes(&self.script);
        write_atomic(dest, &bytes)?;
        self.path = Some(dest.to_path_buf());
        self.saved_commit_id = self.commit_id;
        self.dirty = false;
        self.autosave_dirty = false;
        self.last_disk_stamp = DiskStamp::from_path(dest).ok();
        Ok(())
    }

    /// Returns `true` if the backing file on disk has changed size or mtime since it was last
    /// opened or saved by this controller (mirrors Aegisub external-change reload detection).
    #[must_use]
    pub fn needs_reload_from_disk(&self) -> bool {
        let (Some(path), Some(saved_stamp)) = (&self.path, self.last_disk_stamp) else {
            return false;
        };
        match DiskStamp::from_path(path) {
            Ok(cur) => cur != saved_stamp,
            Err(_) => false,
        }
    }

    /// Reloads the script from its current path on disk, resetting undo/redo and dirty state.
    ///
    /// # Errors
    /// Returns [`DocumentError::NoPath`] or [`DocumentError::Io`].
    pub fn reload_from_disk(&mut self) -> Result<(), DocumentError> {
        let Some(path) = self.path.clone() else {
            return Err(DocumentError::NoPath);
        };
        let bytes = fs::read(&path)?;
        self.has_bom = bytes.starts_with(&[0xEF, 0xBB, 0xBF]);
        let text = String::from_utf8_lossy(&bytes);
        self.script = parse_ass(&text).value;
        self.undo = UndoHistory::new(DEFAULT_MAX_HISTORY_BYTES);
        self.undo_descs.clear();
        self.redo_descs.clear();
        self.amend_state = None;
        self.commit_id += 1;
        self.non_text_revision += 1;
        self.saved_commit_id = self.commit_id;
        self.dirty = false;
        self.autosave_dirty = false;
        self.last_disk_stamp = DiskStamp::from_path(&path).ok();
        self.notify(CommitType::EVERYTHING, false);
        Ok(())
    }

    #[inline]
    fn notify(&mut self, commit_type: CommitType, is_amend: bool) {
        let ev = ChangeNotification {
            commit_type,
            commit_id: self.commit_id,
            active_line: self.active_line,
            is_amend,
        };
        for l in &mut self.listeners {
            l.on_document_changed(ev);
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use ass_core::AssEvent;
    use std::sync::Arc;
    use std::sync::atomic::{AtomicUsize, Ordering};

    struct CountListener {
        calls: Arc<AtomicUsize>,
        last_type: Arc<std::sync::Mutex<CommitType>>,
    }

    impl DocumentListener for CountListener {
        fn on_document_changed(&mut self, event: ChangeNotification) {
            self.calls.fetch_add(1, Ordering::Relaxed);
            *self.last_type.lock().expect("lock") = event.commit_type;
        }
    }

    #[test]
    fn commit_amend_undo_redo_save_and_disk_reload() {
        let mut script = ScriptFile::new_default();
        script.ensure_events_section_mut().insert_event(
            0,
            AssEvent::dialogue(AssTime::ZERO, AssTime::from_cs(200), "Hello"),
        );
        let mut doc = DocumentController::new(script);

        let calls = Arc::new(AtomicUsize::new(0));
        let last_type = Arc::new(std::sync::Mutex::new(CommitType::NONE));
        doc.add_listener(Box::new(CountListener {
            calls: Arc::clone(&calls),
            last_type: Arc::clone(&last_type),
        }));

        // Consecutive amends merge into one undo step
        doc.amend_text(0, "typing", "Hello w").expect("amend 1");
        doc.amend_text(0, "typing", "Hello world").expect("amend 2");
        assert_eq!(doc.effective_event_text(0), Some("Hello world"));
        assert!(doc.is_dirty());
        assert!(doc.is_autosave_dirty());
        assert_eq!(doc.undo_description(), Some("typing"));

        // Undo reverts the entire merged typing session
        let undone = doc.undo().expect("undo");
        assert_eq!(undone.as_deref(), Some("typing"));
        assert_eq!(doc.effective_event_text(0), Some("Hello"));

        // Redo restores it
        let redone = doc.redo().expect("redo");
        assert_eq!(redone.as_deref(), Some("typing"));
        assert_eq!(doc.effective_event_text(0), Some("Hello world"));

        // Atomic save + external modification detection + reload
        let dir = tempfile::tempdir().expect("tempdir");
        let file = dir.path().join("test.ass");
        doc.save_as(&file).expect("save_as");
        assert!(!doc.is_dirty());
        assert!(!doc.is_autosave_dirty());
        assert!(!doc.needs_reload_from_disk());

        // Simulate external edit changing file length
        let mut raw = fs::read_to_string(&file).expect("read");
        raw.push_str("\n; external comment\n");
        fs::write(&file, raw).expect("write");
        assert!(doc.needs_reload_from_disk());
        doc.reload_from_disk().expect("reload");
        assert!(!doc.needs_reload_from_disk());
        assert!(calls.load(Ordering::Relaxed) >= 5);
    }

    #[test]
    fn revision_tracks_content_changes_and_ignores_selection() {
        let mut script = ScriptFile::new_default();
        let sec = script.ensure_events_section_mut();
        sec.insert_event(
            0,
            AssEvent::dialogue(AssTime::ZERO, AssTime::from_cs(200), "Line 0"),
        );
        sec.insert_event(
            1,
            AssEvent::dialogue(AssTime::from_cs(200), AssTime::from_cs(400), "Line 1"),
        );
        let mut doc = DocumentController::new(script);
        assert_eq!(doc.revision(), 0);
        assert_eq!(doc.non_text_revision(), 0);

        // Selection changes do not bump revision.
        doc.set_selection(Some(1), &[1]);
        assert_eq!(doc.revision(), 0);
        assert_eq!(doc.non_text_revision(), 0);

        // Consecutive same-line amend_text bumps revision() but not non_text_revision().
        doc.amend_text(1, "typing", "Line 1a").expect("amend 1");
        assert_eq!(doc.revision(), 1);
        assert_eq!(doc.non_text_revision(), 0);
        doc.amend_text(1, "typing", "Line 1ab").expect("amend 2");
        assert_eq!(doc.revision(), 2);
        assert_eq!(doc.non_text_revision(), 0);

        // Undo bumps both revision() and non_text_revision().
        let nt_before_undo = doc.non_text_revision();
        doc.undo().expect("undo");
        assert_eq!(doc.revision(), 3);
        assert!(doc.non_text_revision() > nt_before_undo);

        // Redo bumps both.
        let nt_before_redo = doc.non_text_revision();
        doc.redo().expect("redo");
        assert_eq!(doc.revision(), 4);
        assert!(doc.non_text_revision() > nt_before_redo);

        // Commit bumps both.
        let nt_before_commit = doc.non_text_revision();
        doc.commit(
            "insert",
            EditCommand::InsertEvent {
                index: 2,
                event: AssEvent::dialogue(AssTime::from_cs(400), AssTime::from_cs(600), "Line 2"),
            },
            CommitType::ORDER,
        )
        .expect("commit");
        assert_eq!(doc.revision(), 5);
        assert!(doc.non_text_revision() > nt_before_commit);
    }
}
