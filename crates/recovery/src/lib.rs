//! Crash-recovery workflow on top of [`autosave`]: unclean shutdown detection, recoverable
//! script listing with timestamps and line counts, diff preview vs. disk, restore/discard,
//! atomic write + `fsync` + rename with fault-injection tests, and egui Crash Recovery dialog.

#![forbid(unsafe_code)]
#![cfg_attr(not(test), deny(clippy::unwrap_used, clippy::expect_used))]

use ass_core::parse_ass;
pub use autosave::AutosaveConfig;
use egui::{ScrollArea, Ui};
use i18n_catalog::Catalog;
use std::fs::{self, File};
use std::io::Write;
use std::path::{Path, PathBuf};
use std::time::UNIX_EPOCH;
use thiserror::Error;

/// Errors returned by crash-recovery operations.
#[derive(Debug, Error)]
pub enum RecoveryError {
    #[error("I/O error: {0}")]
    Io(#[from] std::io::Error),
    #[error("simulated crash injected at stage: {0}")]
    InjectedCrash(&'static str),
}

/// Fault-injection hook for testing crash durability of atomic saves.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum FaultMode {
    None,
    CrashMidWrite,
    CrashBeforeRename,
}

/// Atomic file write (`tmp` -> `flush` -> `sync_all` -> `rename`) that never corrupts `target`
/// if the process crashes mid-save.
///
/// # Errors
/// Returns [`RecoveryError::Io`] or [`RecoveryError::InjectedCrash`].
pub fn atomic_save_with_fsync(
    target: &Path,
    bytes: &[u8],
    fault: FaultMode,
) -> Result<(), RecoveryError> {
    if let Some(parent) = target.parent() {
        fs::create_dir_all(parent)?;
    }
    let tmp_path = target.with_extension("ass.tmp");
    {
        let mut f = File::create(&tmp_path)?;
        if fault == FaultMode::CrashMidWrite {
            let half = bytes.len() / 2;
            f.write_all(&bytes[..half])?;
            return Err(RecoveryError::InjectedCrash("mid-write"));
        }
        f.write_all(bytes)?;
        f.flush()?;
        f.sync_all()?;
    }
    if fault == FaultMode::CrashBeforeRename {
        return Err(RecoveryError::InjectedCrash("before-rename"));
    }
    fs::rename(&tmp_path, target)?;
    Ok(())
}

/// Tracks unclean shutdown via a session sentinel file inside the autosave directory.
#[derive(Debug, Clone)]
pub struct SessionSentinel {
    marker_path: PathBuf,
}

impl SessionSentinel {
    /// Creates a sentinel handle pointing to `<autosave_dir>/.hlouis_session.lock`.
    #[must_use]
    pub fn new(autosave_dir: &Path) -> Self {
        Self {
            marker_path: autosave_dir.join(".hlouis_session.lock"),
        }
    }

    /// Returns `true` if a previous session exited uncleanly (sentinel still exists on disk).
    #[must_use]
    pub fn was_unclean_shutdown(&self) -> bool {
        self.marker_path.exists()
    }

    /// Marks the start of a new session.
    ///
    /// # Errors
    /// Returns I/O error if the sentinel cannot be written.
    pub fn mark_session_started(&self) -> Result<(), RecoveryError> {
        if let Some(parent) = self.marker_path.parent() {
            fs::create_dir_all(parent)?;
        }
        let pid = std::process::id();
        fs::write(&self.marker_path, format!("pid={pid}\n"))?;
        Ok(())
    }

    /// Removes the sentinel on clean shutdown.
    ///
    /// # Errors
    /// Returns I/O error if removal fails (ignores `NotFound`).
    pub fn mark_clean_shutdown(&self) -> Result<(), RecoveryError> {
        match fs::remove_file(&self.marker_path) {
            Ok(()) => Ok(()),
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => Ok(()),
            Err(e) => Err(RecoveryError::Io(e)),
        }
    }
}

/// One recoverable autosave entry discovered in the autosave directory.
#[derive(Debug, Clone)]
pub struct RecoverableEntry {
    pub autosave_path: PathBuf,
    pub original_path: Option<PathBuf>,
    pub modified_unix_secs: u64,
    pub event_line_count: usize,
    pub display_name: String,
}

/// Scans `autosave_dir` for `.ass` autosave files (ignoring `.tmp` crash remnants), returning
/// entries sorted newest-first with event counts and timestamps.
///
/// # Errors
/// Returns I/O error if the directory cannot be read.
pub fn list_recoverable_scripts(
    autosave_dir: &Path,
) -> Result<Vec<RecoverableEntry>, RecoveryError> {
    if !autosave_dir.exists() {
        return Ok(Vec::new());
    }
    let mut entries = Vec::new();
    for dir_entry in fs::read_dir(autosave_dir)? {
        let dir_entry = dir_entry?;
        let path = dir_entry.path();
        if path.extension().and_then(|e| e.to_str()) != Some("ass") {
            continue;
        }
        let meta = dir_entry.metadata()?;
        let modified_unix_secs = meta
            .modified()
            .ok()
            .and_then(|t| t.duration_since(UNIX_EPOCH).ok())
            .map_or(0, |d| d.as_secs());
        let raw = fs::read_to_string(&path).unwrap_or_default();
        let parsed = parse_ass(&raw).value;
        let event_line_count = parsed.events().count();
        let display_name = path.file_name().map_or_else(
            || "untitled.ass".to_owned(),
            |n| n.to_string_lossy().into_owned(),
        );
        entries.push(RecoverableEntry {
            autosave_path: path,
            original_path: None,
            modified_unix_secs,
            event_line_count,
            display_name,
        });
    }
    entries.sort_by_key(|a| std::cmp::Reverse(a.modified_unix_secs));
    Ok(entries)
}

/// One line in a unified diff preview between the on-disk file and the recovered autosave.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum DiffLine {
    Same(String),
    Removed(String),
    Added(String),
}

/// Computes a concise line-by-line diff preview between `disk_text` and `autosave_text`.
#[must_use]
pub fn compute_line_diff(disk_text: &str, autosave_text: &str) -> Vec<DiffLine> {
    let a: Vec<&str> = disk_text.lines().collect();
    let b: Vec<&str> = autosave_text.lines().collect();
    let max_len = a.len().max(b.len());
    let mut out = Vec::with_capacity(max_len);
    for i in 0..max_len {
        match (a.get(i), b.get(i)) {
            (Some(la), Some(lb)) if la == lb => out.push(DiffLine::Same((*la).to_owned())),
            (Some(la), Some(lb)) => {
                out.push(DiffLine::Removed((*la).to_owned()));
                out.push(DiffLine::Added((*lb).to_owned()));
            }
            (Some(la), None) => out.push(DiffLine::Removed((*la).to_owned())),
            (None, Some(lb)) => out.push(DiffLine::Added((*lb).to_owned())),
            (None, None) => {}
        }
    }
    out
}

/// Counts changed lines without allocating (hot-path summary helper).
#[must_use]
pub fn count_diff_lines(disk_text: &str, autosave_text: &str) -> usize {
    let mut a = disk_text.lines();
    let mut b = autosave_text.lines();
    let mut changed = 0usize;
    loop {
        match (a.next(), b.next()) {
            (Some(la), Some(lb)) => {
                if la != lb {
                    changed += 1;
                }
            }
            (Some(_), None) | (None, Some(_)) => changed += 1,
            (None, None) => break,
        }
    }
    changed
}

/// Restores `entry.autosave_path` to `destination` atomically using [`atomic_save_with_fsync`].
///
/// # Errors
/// Returns [`RecoveryError`] on read or write failure.
pub fn restore_entry(entry: &RecoverableEntry, destination: &Path) -> Result<(), RecoveryError> {
    let bytes = fs::read(&entry.autosave_path)?;
    atomic_save_with_fsync(destination, &bytes, FaultMode::None)?;
    Ok(())
}

/// Discards `entry.autosave_path` from disk.
///
/// # Errors
/// Returns [`RecoveryError`] if deletion fails.
pub fn discard_entry(entry: &RecoverableEntry) -> Result<(), RecoveryError> {
    match fs::remove_file(&entry.autosave_path) {
        Ok(()) => Ok(()),
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => Ok(()),
        Err(e) => Err(RecoveryError::Io(e)),
    }
}

/// Action returned by [`show_recovery_dialog`].
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum RecoveryDialogAction {
    None,
    Restore(usize),
    Discard(usize),
    Close,
}

/// State for the egui Crash Recovery dialog.
#[derive(Debug, Clone, Default)]
pub struct RecoveryDialogState {
    pub entries: Vec<RecoverableEntry>,
    pub selected: usize,
    pub diff_preview: Vec<DiffLine>,
}

/// Renders the egui Crash Recovery dialog with list, diff preview, Restore, and Discard controls.
pub fn show_recovery_dialog(
    ui: &mut Ui,
    state: &mut RecoveryDialogState,
    catalog: &Catalog,
) -> RecoveryDialogAction {
    let mut action = RecoveryDialogAction::None;
    ui.heading(catalog.get("recovery-title").unwrap_or("Crash Recovery"));
    ui.separator();

    ui.horizontal(|ui| {
        ui.vertical(|ui| {
            ui.set_min_width(220.0);
            for (idx, entry) in state.entries.iter().enumerate() {
                let label = format!(
                    "{} ({} lines, ts={})",
                    entry.display_name, entry.event_line_count, entry.modified_unix_secs
                );
                if ui.selectable_label(state.selected == idx, label).clicked() {
                    state.selected = idx;
                }
            }
        });
        ui.separator();
        ui.vertical(|ui| {
            ui.set_min_width(320.0);
            ui.label(
                catalog
                    .get("recovery-diff-preview")
                    .unwrap_or("Diff vs. Disk"),
            );
            ScrollArea::vertical().max_height(200.0).show(ui, |ui| {
                for line in &state.diff_preview {
                    match line {
                        DiffLine::Same(s) => {
                            ui.monospace(format!("  {s}"));
                        }
                        DiffLine::Removed(s) => {
                            ui.monospace(format!("- {s}"));
                        }
                        DiffLine::Added(s) => {
                            ui.monospace(format!("+ {s}"));
                        }
                    }
                }
            });
        });
    });

    ui.separator();
    ui.horizontal(|ui| {
        if !state.entries.is_empty() {
            if ui
                .button(
                    catalog
                        .get("recovery-restore")
                        .unwrap_or("Restore Selected"),
                )
                .clicked()
            {
                action = RecoveryDialogAction::Restore(state.selected);
            }
            if ui
                .button(catalog.get("recovery-discard").unwrap_or("Discard"))
                .clicked()
            {
                action = RecoveryDialogAction::Discard(state.selected);
            }
        }
        if ui
            .button(catalog.get("btn-close").unwrap_or("Close"))
            .clicked()
        {
            action = RecoveryDialogAction::Close;
        }
    });

    action
}

#[cfg(test)]
mod tests {
    use super::*;
    use egui_kittest::Harness;
    use i18n_catalog::Locale;

    #[test]
    fn fault_injection_never_corrupts_existing_file_and_recovers_cleanly() {
        let dir = tempfile::tempdir().expect("tempdir");
        let sentinel = SessionSentinel::new(dir.path());
        assert!(!sentinel.was_unclean_shutdown());
        sentinel.mark_session_started().expect("start");
        assert!(sentinel.was_unclean_shutdown());

        // Original good file on disk
        let target = dir.path().join("ep01.ass");
        let original_ass = "[Events]\nFormat: Layer, Start, End, Style, Name, MarginL, MarginR, MarginV, Effect, Text\nDialogue: 0,0:00:01.00,0:00:03.00,Default,,0,0,0,,Original\n";
        atomic_save_with_fsync(&target, original_ass.as_bytes(), FaultMode::None).expect("initial");

        // Crash mid-write and before rename — original file MUST remain intact!
        let new_ass = "[Events]\nFormat: Layer, Start, End, Style, Name, MarginL, MarginR, MarginV, Effect, Text\nDialogue: 0,0:00:01.00,0:00:03.00,Default,,0,0,0,,Edited\n";
        assert!(
            atomic_save_with_fsync(&target, new_ass.as_bytes(), FaultMode::CrashMidWrite).is_err()
        );
        assert_eq!(fs::read_to_string(&target).expect("read"), original_ass);

        assert!(
            atomic_save_with_fsync(&target, new_ass.as_bytes(), FaultMode::CrashBeforeRename)
                .is_err()
        );
        assert_eq!(fs::read_to_string(&target).expect("read"), original_ass);

        // Autosave file present -> list, diff, restore, discard, clean shutdown
        let auto_dir = dir.path().join("autosave");
        fs::create_dir_all(&auto_dir).expect("mkdir");
        let auto_file = auto_dir.join("ep01_20261127.ass");
        atomic_save_with_fsync(&auto_file, new_ass.as_bytes(), FaultMode::None).expect("autosave");

        let list = list_recoverable_scripts(&auto_dir).expect("list");
        assert_eq!(list.len(), 1);
        assert_eq!(list[0].event_line_count, 1);

        let diff = compute_line_diff(original_ass, new_ass);
        assert!(diff.iter().any(|d| matches!(d, DiffLine::Added(_))));

        restore_entry(&list[0], &target).expect("restore");
        assert_eq!(fs::read_to_string(&target).expect("read restored"), new_ass);

        // Render egui Crash Recovery dialog
        let mut state = RecoveryDialogState {
            entries: list.clone(),
            selected: 0,
            diff_preview: diff,
        };
        let cat = Catalog::for_locale(Locale::En);
        let builder = Harness::builder().with_accessibility_check(false);
        let mut harness = builder.build_ui(|ui| {
            let _ = show_recovery_dialog(ui, &mut state, &cat);
        });
        harness.run();

        discard_entry(&list[0]).expect("discard");
        assert!(!auto_file.exists());
        sentinel.mark_clean_shutdown().expect("clean");
        assert!(!sentinel.was_unclean_shutdown());
    }
}
