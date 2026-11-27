//! End-to-end application core connecting [`ui_shell::AppShell`] to all logic crates
//! (`document`, `autosave`, `edit-ops`, `ass-tools`, `hlouis-tools`, `template-doctor`,
//! `kara-templater`, `mkv-subs`, `mkv-import`, `settings`, `playback`, `timing-controller`)
//! with media providers behind pure-Rust traits in [`media`].

#![forbid(unsafe_code)]
#![allow(clippy::cast_precision_loss, clippy::missing_errors_doc)]
#![cfg_attr(not(test), deny(clippy::unwrap_used, clippy::expect_used))]

pub mod media;

use ass_core::{
    AssEvent, AssStyle, AssTime, EditCommand, EventKind, ScriptFile, parse_ass, serialize_ass,
};
use ass_tools::{
    FindReplaceOptions, JoinMode, ResampleOptions, SelectLinesOptions, ShiftAffect, ShiftAmount,
    ShiftDirection, ShiftOptions, ShiftTarget, SplitMode, TppOptions, find_and_replace, join_lines,
    resample_resolution, select_lines, shift_times, split_line, timing_post_process,
};
use audio_view::TimelineMarkerIndex;
use autosave::{
    AutosaveConfig, RecoveryCandidate, create_autobackup, discover_recoveries, write_atomic,
    write_autosave_now,
};
use document::{CommitType, DocumentController};
use edit_ops::{
    PasteOverMask, SortKey, copy_lines, cut_lines, delete_selected, duplicate_selected,
    insert_after, insert_before, paste_lines, paste_over, stable_sort, toggle_comment_selected,
};
use egui::{Order, Ui};
use hlouis_tools::{build_style_counts, clean_whitespace, quick_select_by_style};
use i18n_catalog::Locale;
use kara_templater::{EngineConfig, KaraTemplater, strip_fx_lines};
use karaskel::{TextExtents, TextMetric};
use media::{
    AudioColumnAdapter, AudioProvider, BasicFontProvider, DummyVideoProvider, FontProvider,
    PlainTextBoxRenderer, SubtitleRenderer, VideoProvider, VideoSupplierAdapter, WavAudioProvider,
};
use mkv_import::{ImportOutput, TrackPickerModel, import_tracks, srt_text_to_ass_into};
use mkv_subs::list_tracks;
use playback::Clock;
use settings::AppSettings;
use std::collections::BTreeSet;
use std::fs::{self, File};
use std::io::BufReader;
use std::path::{Path, PathBuf};
use template_doctor::{fix_all_safe, lint_script};
use thiserror::Error;
use ui_dialogs::ActiveDialog;
use ui_mkv_import::MkvImportDialogState;
use ui_shell::AppShell;

struct FallbackTextExtents;
impl TextExtents for FallbackTextExtents {
    fn extents(&self, style: &AssStyle, text: &str) -> TextMetric {
        let h = style.fontsize().max(1.0);
        let w = (text.chars().count() as f64) * h * 0.55;
        TextMetric {
            width: w,
            height: h,
            descent: h * 0.2,
            ext_lead: 0.0,
        }
    }
}

/// Errors returned by [`EditorApp`] file, media, and command operations (never silent).
#[derive(Debug, Error)]
pub enum AppError {
    #[error("I/O error: {0}")]
    Io(#[from] std::io::Error),
    #[error("document error: {0}")]
    Document(String),
    #[error("autosave error: {0}")]
    Autosave(String),
    #[error("edit error: {0}")]
    Edit(String),
    #[error("MKV error: {0}")]
    Mkv(String),
    #[error("audio error: {0}")]
    Audio(String),
    #[error("settings error: {0}")]
    Settings(String),
}

/// Complete wired desktop editor application controller.
pub struct EditorApp {
    pub shell: AppShell,
    pub current_path: Option<PathBuf>,
    pub mkv_source_path: Option<PathBuf>,
    pub autosave_cfg: AutosaveConfig,
    pub settings_path: Option<PathBuf>,
    pub clipboard: String,
    pub close_prompt_open: bool,
    pub recovery_candidates: Vec<RecoveryCandidate>,
    pub video: Box<dyn VideoProvider>,
    pub audio: Option<Box<dyn AudioProvider>>,
    pub renderer: Box<dyn SubtitleRenderer>,
    pub fonts: Box<dyn FontProvider>,
}

impl EditorApp {
    /// Creates a new editor session initialized with `script` and built-in dummy video + synthetic
    /// WAV audio providers.
    #[must_use]
    pub fn new(script: ScriptFile) -> Self {
        let mut shell = AppShell::new(script);
        let dummy_video = DummyVideoProvider::new(320, 180, 24_000, 120);
        shell.attach_video(
            dummy_video.timecodes().clone(),
            dummy_video.frame_count(),
            dummy_video.keyframes().to_vec(),
            dummy_video.width(),
            dummy_video.height(),
        );
        let synth_audio = WavAudioProvider::synthetic(30_000, 16_000);
        let markers = TimelineMarkerIndex::new(
            (0..30).map(|i| i * 1_000).collect(),
            (0..60).map(|i| i * 500).collect(),
        );
        shell.attach_audio(synth_audio.duration_ms(), markers);
        let base_dir = std::env::temp_dir().join("hlouis-sub-edit");
        Self {
            shell,
            current_path: None,
            mkv_source_path: None,
            autosave_cfg: AutosaveConfig::from_base_dir(&base_dir),
            settings_path: None,
            clipboard: String::new(),
            close_prompt_open: false,
            recovery_candidates: Vec::new(),
            video: Box::new(dummy_video),
            audio: Some(Box::new(synth_audio)),
            renderer: Box::new(PlainTextBoxRenderer),
            fonts: Box::new(BasicFontProvider::default()),
        }
    }

    /// Resets the editor to a fresh blank ASS script.
    pub fn new_script(&mut self) {
        let mut script = ScriptFile::new_default();
        script.ensure_events_section_mut().insert_event(
            0,
            AssEvent::dialogue(AssTime::ZERO, AssTime::from_cs(500), ""),
        );
        self.shell.doc = DocumentController::new(script);
        self.current_path = None;
        "Untitled.ass".clone_into(&mut self.shell.script_name);
        self.sync_ui_from_doc();
    }

    /// Opens an `.ass`, `.ssa`, `.srt`, `.wav`, or `.mkv` file from disk (never silent on errors).
    pub fn open_file(&mut self, path: &Path) -> Result<(), AppError> {
        let ext = path
            .extension()
            .and_then(|e| e.to_str())
            .unwrap_or_default()
            .to_ascii_lowercase();

        if ext == "mkv" {
            let _ = self.open_mkv_subtitles(path)?;
            return Ok(());
        }
        if ext == "wav" {
            self.open_wav_audio(path)?;
            return Ok(());
        }
        if ext == "srt" {
            let raw = fs::read_to_string(path)?;
            let script = parse_srt_to_script(&raw)?;
            self.shell.doc = DocumentController::new(script);
            self.current_path = None;
            let stem = path
                .file_stem()
                .and_then(|s| s.to_str())
                .unwrap_or("imported");
            self.shell.script_name = format!("{stem}.ass");
            self.sync_ui_from_doc();
            return Ok(());
        }

        let doc =
            DocumentController::open_file(path).map_err(|e| AppError::Document(e.to_string()))?;
        self.shell.doc = doc;
        self.current_path = Some(path.to_path_buf());
        if let Some(name) = path.file_name().and_then(|s| s.to_str()) {
            name.clone_into(&mut self.shell.script_name);
        }
        self.sync_ui_from_doc();
        Ok(())
    }

    /// Saves the active script to `current_path` atomically (creating an autobackup if enabled).
    pub fn save(&mut self) -> Result<PathBuf, AppError> {
        let dest = self
            .current_path
            .clone()
            .ok_or_else(|| AppError::Document("no file path set; use Save As".to_owned()))?;
        self.save_as(&dest)?;
        Ok(dest)
    }

    /// Saves the active script to `dest` atomically via `autosave::write_atomic`.
    pub fn save_as(&mut self, dest: &Path) -> Result<(), AppError> {
        self.shell
            .doc
            .flush_amend()
            .map_err(|e| AppError::Document(e.to_string()))?;
        let text = serialize_ass(self.shell.doc.script());
        write_atomic(dest, text.as_bytes()).map_err(|e| AppError::Autosave(e.to_string()))?;
        if self.autosave_cfg.backup_dir.exists() {
            let _ = create_autobackup(dest, &self.autosave_cfg.backup_dir);
        }
        self.shell
            .doc
            .save_as(dest)
            .map_err(|e| AppError::Document(e.to_string()))?;
        self.current_path = Some(dest.to_path_buf());
        if let Some(name) = dest.file_name().and_then(|s| s.to_str()) {
            name.clone_into(&mut self.shell.script_name);
        }
        Ok(())
    }

    /// Polls the autosave interval and writes an atomic autosave snapshot when due.
    pub fn poll_autosave(&mut self, now_ns: u64) -> Result<Option<PathBuf>, AppError> {
        if !self.shell.should_autosave(now_ns) {
            return Ok(None);
        }
        self.shell
            .doc
            .flush_amend()
            .map_err(|e| AppError::Document(e.to_string()))?;
        let bytes = serialize_ass(self.shell.doc.script()).into_bytes();
        let saved = write_autosave_now(
            &self.autosave_cfg.autosave_dir,
            &self.shell.script_name,
            &bytes,
            self.autosave_cfg.max_keep,
            None,
        )
        .map_err(|e| AppError::Autosave(e.to_string()))?;
        self.shell.doc.mark_autosaved();
        self.shell.last_autosave_ns = now_ns;
        Ok(Some(saved))
    }

    /// Scans `autosave_dir` for crash-recovery snapshots and opens the Crash Recovery dialog if
    /// any are found.
    pub fn check_crash_recovery(&mut self) -> usize {
        let stem = Path::new(&self.shell.script_name)
            .file_stem()
            .and_then(|s| s.to_str())
            .unwrap_or(&self.shell.script_name);
        let Ok(report) = discover_recoveries(
            &self.autosave_cfg.autosave_dir,
            stem,
            self.current_path.as_deref(),
        ) else {
            return 0;
        };
        self.recovery_candidates = report.candidates;
        self.shell.dialogs.recovery_files = self
            .recovery_candidates
            .iter()
            .map(|c| c.path.display().to_string())
            .collect();
        if !self.recovery_candidates.is_empty() {
            self.shell.dialogs.open = Some(ActiveDialog::CrashRecovery);
        }
        self.recovery_candidates.len()
    }

    /// Opens the crash-recovery candidate at `index`.
    pub fn recover_candidate(&mut self, index: usize) -> Result<(), AppError> {
        let path = self
            .recovery_candidates
            .get(index)
            .map(|c| c.path.clone())
            .ok_or_else(|| AppError::Autosave("invalid recovery candidate index".to_owned()))?;
        self.open_file(&path)?;
        self.shell.dialogs.open = None;
        Ok(())
    }

    /// Handles a window close request: prompts if unsaved changes exist, otherwise permits close.
    pub fn request_close(&mut self) -> bool {
        if self.shell.doc.is_dirty() {
            self.close_prompt_open = true;
            false
        } else {
            true
        }
    }

    /// Scans a real `.mkv` file's subtitle tracks (`mkv-subs` + `mkv-import`), populates
    /// `shell.mkv_dialog`, and opens the MKV Import dialog.
    pub fn open_mkv_subtitles(&mut self, mkv_path: &Path) -> Result<usize, AppError> {
        let file = File::open(mkv_path)?;
        let mut reader = BufReader::new(file);
        let tracks = list_tracks(&mut reader).map_err(|e| AppError::Mkv(e.to_string()))?;
        let mut picker = TrackPickerModel::from_tracks(&tracks);
        if let Some(first_num) = picker
            .rows()
            .iter()
            .find(|r| r.selectable)
            .map(|r| r.track_number)
        {
            let file2 = File::open(mkv_path)?;
            let _ = picker.load_preview(BufReader::new(file2), first_num, 5);
        }
        let count = picker.rows().len();
        self.shell.mkv_dialog = MkvImportDialogState::from_picker(picker);
        self.mkv_source_path = Some(mkv_path.to_path_buf());
        self.shell.dialogs.open = Some(ActiveDialog::MkvImport);
        Ok(count)
    }

    /// Imports currently selected tracks from `self.mkv_source_path` into the active script.
    pub fn import_selected_mkv_tracks(&mut self) -> Result<usize, AppError> {
        let mkv_path = self
            .mkv_source_path
            .clone()
            .ok_or_else(|| AppError::Mkv("no MKV source path set".to_owned()))?;
        let selected = self.shell.mkv_dialog.picker.selected_track_numbers();
        let mode = self.shell.mkv_dialog.multi_mode;
        let file = File::open(&mkv_path)?;
        let out = import_tracks(BufReader::new(file), &selected, mode)
            .map_err(|e| AppError::Mkv(e.to_string()))?;
        let script = match out {
            ImportOutput::Merged(s) => s,
            ImportOutput::Separate(mut list) => list
                .drain(..)
                .next()
                .map(|(_, s)| s)
                .ok_or_else(|| AppError::Mkv("no tracks imported".to_owned()))?,
        };
        let imported_lines = script.events().count();
        self.shell.doc = DocumentController::new(script);
        self.current_path = None;
        let stem = mkv_path
            .file_stem()
            .and_then(|s| s.to_str())
            .unwrap_or("mkv_subtitles");
        self.shell.script_name = format!("{stem}.ass");
        self.shell.dialogs.open = None;
        self.sync_ui_from_doc();
        Ok(imported_lines)
    }

    /// Loads a `.wav` file via `hound` and attaches it to the audio panel.
    pub fn open_wav_audio(&mut self, wav_path: &Path) -> Result<(), AppError> {
        let wav = WavAudioProvider::from_path(wav_path).map_err(AppError::Audio)?;
        let dur = wav.duration_ms();
        let markers = TimelineMarkerIndex::new(Vec::new(), Vec::new());
        self.shell.attach_audio(dur, markers);
        self.audio = Some(Box::new(wav));
        Ok(())
    }

    /// Loads persisted [`AppSettings`] from `path` and syncs theme/language live.
    pub fn load_settings(&mut self, path: &Path) -> Result<(), AppError> {
        let raw = fs::read_to_string(path)?;
        let loaded = AppSettings::load_json(&raw).map_err(|e| AppError::Settings(e.to_string()))?;
        self.shell.settings = loaded;
        self.settings_path = Some(path.to_path_buf());
        self.apply_settings_to_shell();
        Ok(())
    }

    /// Saves [`AppSettings`] to `path` atomically.
    pub fn save_settings(&self, path: &Path) -> Result<(), AppError> {
        let json = self
            .shell
            .settings
            .save_json()
            .map_err(|e| AppError::Settings(e.to_string()))?;
        write_atomic(path, json.as_bytes()).map_err(|e| AppError::Settings(e.to_string()))
    }

    /// Applies current `shell.settings` to live theme, language, and grid columns.
    pub fn apply_settings_to_shell(&mut self) {
        self.shell.theme.sync_from_prefs(
            &self.shell.settings.appearance,
            self.shell.system_is_dark,
            12 * 60,
        );
        let lang = self.shell.settings.general.language.to_ascii_lowercase();
        if lang.starts_with("vi") {
            self.shell.set_locale(Locale::Vi);
        } else {
            self.shell.set_locale(Locale::En);
        }
        let iface = &self.shell.settings.interface;
        self.shell.grid.model.columns.actor = iface.show_col_actor;
        self.shell.grid.model.columns.effect = iface.show_col_effect;
        self.shell.grid.model.columns.cps = iface.show_col_cps;
        self.shell.grid.model.columns.margin_l = iface.show_col_margins;
        self.shell.grid.model.columns.margin_r = iface.show_col_margins;
        self.shell.grid.model.columns.margin_v = iface.show_col_margins;
    }

    /// Re-synchronizes grid, edit box, and style manager lists from `shell.doc`.
    pub fn sync_ui_from_doc(&mut self) {
        self.shell.grid.sync_from_script(self.shell.doc.script());
        self.shell.editbox.active_line = None;
        let styles: Vec<String> = self
            .shell
            .doc
            .script()
            .styles()
            .map(|st| st.name().to_owned())
            .collect();
        if !styles.is_empty() {
            self.shell.dialogs.script_styles = styles;
        }
    }

    /// Executes a canonical command ID end-to-end against `document` and logic crates.
    #[allow(clippy::too_many_lines)]
    pub fn execute_command(&mut self, cmd_id: &str) -> Result<(), AppError> {
        let sel: Vec<usize> = self.shell.doc.selection().to_vec();
        let active = self.shell.doc.active_line().unwrap_or(0);

        match cmd_id {
            "subtitle/new" => self.new_script(),
            "subtitle/save" => {
                let _ = self.save()?;
            }
            "edit/undo" => {
                if let Some(desc) = self
                    .shell
                    .doc
                    .undo()
                    .map_err(|e| AppError::Document(e.to_string()))?
                {
                    self.sync_ui_from_doc();
                    self.shell.status_message = format!("Undo: {desc}");
                }
            }
            "edit/redo" => {
                if let Some(desc) = self
                    .shell
                    .doc
                    .redo()
                    .map_err(|e| AppError::Document(e.to_string()))?
                {
                    self.sync_ui_from_doc();
                    self.shell.status_message = format!("Redo: {desc}");
                }
            }
            "subtitle/insert/before" => {
                let ev = AssEvent::dialogue(AssTime::ZERO, AssTime::from_cs(500), "");
                let cmd = insert_before(self.shell.doc.script(), active, ev)
                    .map_err(|e| AppError::Edit(e.to_string()))?;
                self.commit_and_sync("insert before", cmd, CommitType::ORDER)?;
            }
            "subtitle/insert/after" => {
                let ev = AssEvent::dialogue(AssTime::ZERO, AssTime::from_cs(500), "");
                let cmd = insert_after(self.shell.doc.script(), active, ev)
                    .map_err(|e| AppError::Edit(e.to_string()))?;
                self.commit_and_sync("insert after", cmd, CommitType::ORDER)?;
            }
            "edit/line/duplicate" => {
                let cmd = duplicate_selected(self.shell.doc.script(), &sel)
                    .map_err(|e| AppError::Edit(e.to_string()))?;
                self.commit_and_sync("duplicate lines", cmd, CommitType::ORDER)?;
            }
            "edit/line/delete" => {
                let cmd = delete_selected(self.shell.doc.script(), &sel)
                    .map_err(|e| AppError::Edit(e.to_string()))?;
                self.commit_and_sync("delete lines", cmd, CommitType::ORDER)?;
            }
            "edit/line/copy" => {
                self.clipboard = copy_lines(self.shell.doc.script(), &sel)
                    .map_err(|e| AppError::Edit(e.to_string()))?;
            }
            "edit/line/cut" => {
                let (clip, cmd) = cut_lines(self.shell.doc.script(), &sel)
                    .map_err(|e| AppError::Edit(e.to_string()))?;
                self.clipboard = clip;
                self.commit_and_sync("cut lines", cmd, CommitType::ORDER)?;
            }
            "edit/line/paste" => {
                let cmd = paste_lines(self.shell.doc.script(), active, &self.clipboard)
                    .map_err(|e| AppError::Edit(e.to_string()))?;
                self.commit_and_sync("paste lines", cmd, CommitType::ORDER)?;
            }
            "edit/line/paste/over" => {
                let cmd = paste_over(
                    self.shell.doc.script(),
                    &sel,
                    &self.clipboard,
                    PasteOverMask::all(),
                )
                .map_err(|e| AppError::Edit(e.to_string()))?;
                self.commit_and_sync("paste over", cmd, CommitType::TEXT)?;
            }
            "edit/line/comment/toggle" => {
                let cmd = toggle_comment_selected(self.shell.doc.script(), &sel)
                    .map_err(|e| AppError::Edit(e.to_string()))?;
                self.commit_and_sync("toggle comment", cmd, CommitType::TEXT)?;
            }
            "subtitle/sort/start" => {
                let cmd = stable_sort(self.shell.doc.script(), SortKey::Start, false, None)
                    .map_err(|e| AppError::Edit(e.to_string()))?;
                self.commit_and_sync("sort by start", cmd, CommitType::ORDER)?;
            }
            "subtitle/sort/end" => {
                let cmd = stable_sort(self.shell.doc.script(), SortKey::End, false, None)
                    .map_err(|e| AppError::Edit(e.to_string()))?;
                self.commit_and_sync("sort by end", cmd, CommitType::ORDER)?;
            }
            "subtitle/sort/style" => {
                let cmd = stable_sort(self.shell.doc.script(), SortKey::Style, false, None)
                    .map_err(|e| AppError::Edit(e.to_string()))?;
                self.commit_and_sync("sort by style", cmd, CommitType::ORDER)?;
            }
            "edit/line/join/concatenate" => {
                let sel_set: BTreeSet<usize> = sel.iter().copied().collect();
                let cmd = join_lines(
                    self.shell.doc.script(),
                    &sel_set,
                    &JoinMode::Concatenate {
                        separator: r"\N".to_owned(),
                    },
                )
                .map_err(|e| AppError::Edit(e.to_string()))?;
                self.commit_and_sync("join lines", cmd, CommitType::ORDER)?;
            }
            "edit/line/split/video" => {
                let cmd = split_line(self.shell.doc.script(), active, SplitMode::ByHardNewline)
                    .map_err(|e| AppError::Edit(e.to_string()))?;
                self.commit_and_sync("split line", cmd, CommitType::ORDER)?;
            }
            "hlouis/select_by_style" => {
                let rep = quick_select_by_style(self.shell.doc.script(), active, false);
                if !rep.selection.is_empty() {
                    self.shell
                        .doc
                        .set_selection(rep.active_line, &rep.selection);
                }
                self.shell.status_message = rep.message_en;
            }
            "hlouis/clean_whitespace" => {
                let sel_set: BTreeSet<usize> = sel.iter().copied().collect();
                let rep = clean_whitespace(
                    self.shell.doc.script(),
                    &sel_set,
                    self.shell.dialogs.hlouis_clean_opts,
                );
                if rep.changed_count > 0 {
                    self.commit_and_sync("HLouis clean whitespace", rep.command, CommitType::TEXT)?;
                }
                self.shell.status_message = rep.message_en;
            }
            "automation/template_doctor" => {
                self.shell.dialogs.template_diags = lint_script(self.shell.doc.script());
                self.shell.dialogs.open = Some(ActiveDialog::TemplateDoctor);
            }
            "automation/template_doctor/fix_all" => {
                let fixed = fix_all_safe(self.shell.doc.script());
                if fixed.fixed_lines > 0 {
                    self.commit_and_sync(
                        "Template Doctor fix all safe",
                        fixed.command,
                        CommitType::TEXT,
                    )?;
                }
            }
            "automation/kara_templater/apply" => {
                let mut clone = self.shell.doc.script().clone();
                let engine = KaraTemplater::new(EngineConfig::default());
                let metrics = FallbackTextExtents;
                let report = engine
                    .apply(&mut clone, &metrics)
                    .map_err(|e| AppError::Edit(e.to_string()))?;
                self.shell.doc = DocumentController::new(clone);
                self.sync_ui_from_doc();
                self.shell.status_message = format!(
                    "Karaoke Templater: {} lines generated",
                    report.generated_fx_lines
                );
            }
            "automation/kara_templater/strip_fx" => {
                let mut clone = self.shell.doc.script().clone();
                let removed = strip_fx_lines(&mut clone);
                self.shell.doc = DocumentController::new(clone);
                self.sync_ui_from_doc();
                self.shell.status_message = format!("Removed {removed} fx line(s)");
            }
            "audio/commit" => {
                let r = self.shell.audio.active_selection;
                #[allow(clippy::cast_possible_truncation, clippy::cast_sign_loss)]
                let s_cs = (r.start_ms.max(0) / 10) as u32;
                #[allow(clippy::cast_possible_truncation, clippy::cast_sign_loss)]
                let e_cs = (r.end_ms.max(0) / 10) as u32;
                self.shell
                    .doc
                    .amend_time(active, true, "audio timing start", AssTime::from_cs(s_cs))
                    .map_err(|e| AppError::Document(e.to_string()))?;
                self.shell
                    .doc
                    .amend_time(active, false, "audio timing end", AssTime::from_cs(e_cs))
                    .map_err(|e| AppError::Document(e.to_string()))?;
                self.sync_ui_from_doc();
            }
            _ => {
                self.shell.execute_command(cmd_id);
            }
        }
        Ok(())
    }

    /// Runs `ass_tools::shift_times` through `document` undo/redo.
    pub fn apply_shift_times(
        &mut self,
        amount_cs: u32,
        forward: bool,
        all_rows: bool,
    ) -> Result<(), AppError> {
        let sel_set: BTreeSet<usize> = self.shell.doc.selection().iter().copied().collect();
        let opts = ShiftOptions {
            amount: ShiftAmount::TimeCs(amount_cs),
            direction: if forward {
                ShiftDirection::Forward
            } else {
                ShiftDirection::Backward
            },
            target: ShiftTarget::Both,
            affect: if all_rows {
                ShiftAffect::All
            } else {
                ShiftAffect::Selected(&sel_set)
            },
        };
        let cmd = shift_times(self.shell.doc.script(), opts)
            .map_err(|e| AppError::Edit(e.to_string()))?;
        self.commit_and_sync("shift times", cmd, CommitType::TIMES)
    }

    /// Runs `ass_tools::timing_post_process` through `document` undo/redo.
    pub fn apply_tpp(&mut self, opts: TppOptions<'_>) -> Result<(), AppError> {
        let cmd = timing_post_process(self.shell.doc.script(), opts);
        self.commit_and_sync("timing post-processor", cmd, CommitType::TIMES)
    }

    /// Runs `ass_tools::find_and_replace` through `document` undo/redo.
    pub fn apply_find_replace(&mut self, opts: FindReplaceOptions<'_>) -> Result<usize, AppError> {
        let (cmd, count) = find_and_replace(self.shell.doc.script(), opts)
            .map_err(|e| AppError::Edit(e.to_string()))?;
        if count > 0 {
            self.commit_and_sync("find and replace", cmd, CommitType::TEXT)?;
        }
        Ok(count)
    }

    /// Runs `ass_tools::select_lines` and updates active selection.
    pub fn apply_select_lines(&mut self, opts: SelectLinesOptions<'_>) -> Result<usize, AppError> {
        let cur: BTreeSet<usize> = self.shell.doc.selection().iter().copied().collect();
        let matched = select_lines(self.shell.doc.script(), &cur, opts)
            .map_err(|e| AppError::Edit(e.to_string()))?;
        let vec_sel: Vec<usize> = matched.into_iter().collect();
        let active = vec_sel.first().copied();
        self.shell.doc.set_selection(active, &vec_sel);
        Ok(vec_sel.len())
    }

    /// Runs `ass_tools::resample_resolution` and updates the document.
    pub fn apply_resample(&mut self, opts: &ResampleOptions) -> Result<(), AppError> {
        let mut clone = self.shell.doc.script().clone();
        let cmd = resample_resolution(&clone, opts).map_err(|e| AppError::Edit(e.to_string()))?;
        cmd.apply(&mut clone)
            .map_err(|e| AppError::Document(e.to_string()))?;
        self.shell.doc = DocumentController::new(clone);
        self.sync_ui_from_doc();
        Ok(())
    }

    fn commit_and_sync(
        &mut self,
        desc: &str,
        cmd: EditCommand,
        commit_type: CommitType,
    ) -> Result<(), AppError> {
        self.shell
            .doc
            .commit(desc, cmd, commit_type)
            .map_err(|e| AppError::Document(e.to_string()))?;
        self.sync_ui_from_doc();
        Ok(())
    }

    /// Renders one complete application frame into `ui` and processes drag-and-drop + prompts.
    pub fn show(&mut self, ui: &mut Ui, clock: &dyn Clock) {
        let vid_adapter = VideoSupplierAdapter(self.video.as_ref());
        let aud_adapter = self.audio.as_deref().map(AudioColumnAdapter);
        let aud_ref: Option<&dyn ui_audio::AudioColumnProvider> = aud_adapter
            .as_ref()
            .map(|a| a as &dyn ui_audio::AudioColumnProvider);

        self.shell.show(ui, Some(&vid_adapter), aud_ref, clock);

        // Process any dropped files from the OS.
        let dropped = std::mem::take(&mut self.shell.dropped_files);
        for p in dropped {
            if let Err(err) = self.open_file(&p) {
                self.shell.status_message = format!("Open failed: {err}");
            }
        }

        // Unsaved changes prompt on close.
        if self.close_prompt_open {
            let mut close_now = false;
            egui::Window::new("Unsaved Changes")
                .order(Order::Foreground)
                .collapsible(false)
                .show(ui.ctx(), |ui| {
                    ui.label(format!(
                        "Save changes to '{}' before closing?",
                        self.shell.script_name
                    ));
                    ui.horizontal(|ui| {
                        if ui.button("Save").clicked() {
                            let _ = self.save();
                            close_now = true;
                        }
                        if ui.button("Discard").clicked() {
                            close_now = true;
                        }
                        if ui.button("Cancel").clicked() {
                            self.close_prompt_open = false;
                        }
                    });
                });
            if close_now {
                self.close_prompt_open = false;
            }
        }
    }
}

/// Parses an `.srt` string into a [`ScriptFile`], converting SRT formatting tags via
/// [`mkv_import::srt_text_to_ass_into`].
pub fn parse_srt_to_script(raw: &str) -> Result<ScriptFile, AppError> {
    let mut script = ScriptFile::new_default();
    let sec = script.ensure_events_section_mut();
    let normalized = raw.replace("\r\n", "\n");
    let mut text_buf = String::with_capacity(128);
    let mut idx = 0usize;

    for block in normalized.split("\n\n") {
        let lines: Vec<&str> = block
            .lines()
            .map(str::trim)
            .filter(|l| !l.is_empty())
            .collect();
        if lines.is_empty() {
            continue;
        }
        let Some(time_pos) = lines.iter().position(|l| l.contains("-->")) else {
            continue;
        };
        let parts: Vec<&str> = lines[time_pos].split("-->").collect();
        if parts.len() != 2 {
            continue;
        }
        let start = parse_srt_ts(parts[0].trim())
            .ok_or_else(|| AppError::Document(format!("invalid SRT start: {}", parts[0])))?;
        let end = parse_srt_ts(parts[1].trim())
            .ok_or_else(|| AppError::Document(format!("invalid SRT end: {}", parts[1])))?;
        let body = lines[time_pos + 1..].join("\n");
        text_buf.clear();
        srt_text_to_ass_into(&body, &mut text_buf);
        let mut ev = AssEvent::dialogue(start, end, text_buf.as_str());
        ev.set_kind(EventKind::Dialogue);
        sec.insert_event(idx, ev);
        idx += 1;
    }

    if idx == 0 {
        return Err(AppError::Document(
            "SRT file contained 0 valid subtitle cues".to_owned(),
        ));
    }
    Ok(script)
}

fn parse_srt_ts(raw: &str) -> Option<AssTime> {
    let clean = raw.split_whitespace().next()?;
    let (hms, ms_part) = clean.split_once(',').or_else(|| clean.split_once('.'))?;
    let mut it = hms.split(':');
    let h: u32 = it.next()?.parse().ok()?;
    let m: u32 = it.next()?.parse().ok()?;
    let s: u32 = it.next()?.parse().ok()?;
    let ms: u32 = ms_part.parse().ok()?;
    let total_cs = ((h * 3600 + m * 60 + s) * 100) + (ms + 5) / 10;
    Some(AssTime::from_cs(total_cs))
}

/// Helper to build a populated demo script with styles, karaoke, signs, comments, and `\N`.
#[must_use]
pub fn build_demo_script(line_count: usize) -> ScriptFile {
    let header = "[Script Info]\nScriptType: v4.00+\nPlayResX: 1920\nPlayResY: 1080\n\n[V4+ Styles]\nFormat: Name, Fontname, Fontsize, PrimaryColour, SecondaryColour, OutlineColour, BackColour, Bold, Italic, Underline, StrikeOut, ScaleX, ScaleY, Spacing, Angle, BorderStyle, Outline, Shadow, Alignment, MarginL, MarginR, MarginV, Encoding\nStyle: Default,Arial,48,&H00FFFFFF,&H000000FF,&H00000000,&H00000000,0,0,0,0,100,100,0,0,1,2,2,2,10,10,10,1\nStyle: Sign,Arial,42,&H00FFE080,&H000000FF,&H00000000,&H00000000,-1,0,0,0,100,100,0,0,1,2,0,8,10,10,10,1\nStyle: Romaji,Arial,44,&H00FFFFFF,&H0000FFFF,&H00000000,&H00000000,0,0,0,0,100,100,0,0,1,2,2,8,10,10,10,1\nStyle: Translation,Arial,44,&H00FFFFFF,&H000000FF,&H00000000,&H00000000,0,0,0,0,100,100,0,0,1,2,2,2,10,10,10,1\n\n[Events]\nFormat: Layer, Start, End, Style, Name, MarginL, MarginR, MarginV, Effect, Text\n";
    let mut script = parse_ass(header).value;
    let sec = script.ensure_events_section_mut();
    for i in 0..line_count {
        #[allow(clippy::cast_possible_truncation)]
        let s = (i as u32) * 5;
        let mut ev = match i % 20 {
            0 => {
                let mut e = AssEvent::dialogue(
                    AssTime::from_cs(s),
                    AssTime::from_cs(s + 180),
                    r"{\an8\pos(960,80)\blur2}Typeset sign / Tiêu đề cảnh",
                );
                e.set_style("Sign");
                e
            }
            5 => {
                let mut e = AssEvent::dialogue(
                    AssTime::from_cs(s),
                    AssTime::from_cs(s + 240),
                    r"{\k40}Yu{\k50}me {\k60}no {\k90}tsuzuki",
                );
                e.set_style("Romaji");
                e.set_effect("karaoke");
                e
            }
            10 => {
                let mut e = AssEvent::dialogue(
                    AssTime::from_cs(s),
                    AssTime::from_cs(s + 100),
                    "QC note: kiểm tra lại tên riêng nhân vật",
                );
                e.set_kind(EventKind::Comment);
                e.set_style("Translation");
                e
            }
            _ => AssEvent::dialogue(
                AssTime::from_cs(s),
                AssTime::from_cs(s + 120),
                r"{\i1}Sample subtitle dialogue line\NPhụ đề tiếng Việt",
            ),
        };
        if i % 10 == 3 {
            ev.set_style("Translation");
        }
        sec.insert_event(i, ev);
    }
    script
}

/// Populates `HLouis` style counts and Template Doctor diagnostics on `app`.
pub fn refresh_demo_diagnostics(app: &mut EditorApp) {
    app.shell.dialogs.hlouis_styles = build_style_counts(app.shell.doc.script());
    app.shell.dialogs.template_diags = lint_script(app.shell.doc.script());
}
