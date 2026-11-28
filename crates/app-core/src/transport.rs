//! Media transport glue: connects [`EditorApp`] to asynchronous video/audio providers
//! (open from menus, drag and drop and the command line; play/step/seek; subtitle re-render after
//! every edit; audio-master A/V sync; audio timing commit).
//!
//! Everything here is provider-agnostic: it only talks to the [`VideoProvider`],
//! [`AudioProvider`], [`MediaFactory`] and [`FileDialogs`] traits, so the pure-Rust fallbacks and the
//! native Windows providers behave identically.

use crate::media::{MediaEvent, PickKind, VideoMeta};
use crate::{AppError, EditorApp};
use ass_core::{AssTime, serialize_ass};
use audio_view::TimelineMarkerIndex;
use keyframes::{TimeType, Timecodes};
use playback::Clock;
use std::path::{Path, PathBuf};
use timing_controller::{
    DialogueTimingController, PlayRangeKind, TimeRangeMs, TimingControllerConfig,
};
use ui_audio::AudioPanelAction;
use ui_video::VideoPanelAction;

/// File extensions opened as video.
pub const VIDEO_EXTENSIONS: &[&str] = &[
    "mkv", "mp4", "m4v", "avi", "mov", "webm", "ts", "m2ts", "mts", "wmv", "flv", "ogv", "mpg",
    "mpeg",
];
/// File extensions opened as audio-only sources.
pub const AUDIO_EXTENSIONS: &[&str] = &[
    "wav", "flac", "mp3", "aac", "m4a", "ogg", "opus", "wma", "mka", "ac3", "dts", "eac3",
];
/// File extensions opened as subtitles.
pub const SUBTITLE_EXTENSIONS: &[&str] = &["ass", "ssa", "srt"];

/// Per-session transport bookkeeping (what was last pushed to the providers).
#[derive(Debug, Default)]
pub struct TransportState {
    last_commit: Option<u64>,
    requested_frame: Option<u32>,
    last_active_line: Option<usize>,
    was_playing: bool,
    audio_running: bool,
    /// Wall-clock playback started by an audio-only action (no video attached).
    pub audio_only_end_ms: Option<i64>,
}

impl TransportState {
    /// Forces the next frame to re-push the script and re-request the displayed frame.
    pub fn invalidate(&mut self) {
        self.last_commit = None;
        self.requested_frame = None;
    }
}

fn ext_of(path: &Path) -> String {
    path.extension()
        .and_then(|e| e.to_str())
        .unwrap_or_default()
        .to_ascii_lowercase()
}

/// `true` when `path` has a known video container extension.
#[must_use]
pub fn is_video_path(path: &Path) -> bool {
    VIDEO_EXTENSIONS.contains(&ext_of(path).as_str())
}

/// `true` when `path` has a known audio-only extension.
#[must_use]
pub fn is_audio_path(path: &Path) -> bool {
    AUDIO_EXTENSIONS.contains(&ext_of(path).as_str())
}

fn timecodes_from(meta_pts: &[i64], fps_num: u64, fps_den: u64) -> Option<Timecodes> {
    if meta_pts.len() > 1
        && let Ok(tc) = Timecodes::from_vfr_ms(meta_pts.to_vec())
    {
        return Some(tc);
    }
    Timecodes::from_cfr(fps_num.max(1), fps_den.max(1)).ok()
}

/// Builds [`Timecodes`] from per-frame presentation timestamps, falling back to CFR.
#[must_use]
pub fn timecodes_for(frame_pts_ms: &[i64], fps_num: u64, fps_den: u64) -> Option<Timecodes> {
    timecodes_from(frame_pts_ms, fps_num, fps_den)
}

impl EditorApp {
    /// Records a visible, user-facing notice (status bar, event log and a dismissable dialog).
    pub fn notify_error(&mut self, msg: impl Into<String>) {
        let msg = msg.into();
        self.shell.status_message.clone_from(&msg);
        self.shell.dialogs.log_lines.push(format!("ERROR {msg}"));
        self.notice = Some(msg);
    }

    /// Records an informational status line (status bar + event log).
    pub fn notify_status(&mut self, msg: impl Into<String>) {
        let msg = msg.into();
        self.shell.status_message.clone_from(&msg);
        self.shell.dialogs.log_lines.push(format!("INFO  {msg}"));
        if self.shell.dialogs.log_lines.len() > 2_000 {
            self.shell.dialogs.log_lines.drain(..1_000);
        }
    }

    /// Opens `path` as a video through the native media factory (asynchronous; the first frame
    /// appears as soon as the provider reports it). Also starts decoding its audio track.
    pub fn open_video(&mut self, path: &Path, ctx: &egui::Context) -> Result<(), AppError> {
        let ass_text = self.render_ass_text();
        let Some(factory) = self.factory.as_mut() else {
            return Err(AppError::Media(
                "no native video backend is available in this build".to_owned(),
            ));
        };
        let video = factory
            .open_video(path, ctx, ass_text)
            .map_err(AppError::Media)?;
        // Drop the previous source first (cancels its background indexing).
        self.video = video;
        self.video_path = Some(path.to_path_buf());
        self.shell.detach_video();
        self.transport.invalidate();
        if let Some(name) = path.file_name().and_then(|n| n.to_str()) {
            self.notify_status(format!("Opening video {name} ..."));
        }
        // Audio of the same container (silently skipped by the provider when there is none).
        if self.audio_path.as_deref() != Some(path) {
            match self.factory.as_mut().map(|f| f.open_audio(path, ctx)) {
                Some(Ok(audio)) => {
                    self.audio = Some(audio);
                    self.audio_path = Some(path.to_path_buf());
                    self.shell.detach_audio();
                }
                Some(Err(e)) => self.notify_error(format!("Audio: {e}")),
                None => {}
            }
        }
        Ok(())
    }

    /// Opens `path` as an audio-only source (waveform, spectrum and playback).
    pub fn open_audio(&mut self, path: &Path, ctx: &egui::Context) -> Result<(), AppError> {
        if let Some(factory) = self.factory.as_mut() {
            let audio = factory.open_audio(path, ctx).map_err(AppError::Media)?;
            self.audio = Some(audio);
            self.audio_path = Some(path.to_path_buf());
            self.shell.detach_audio();
            if let Some(name) = path.file_name().and_then(|n| n.to_str()) {
                self.notify_status(format!("Decoding audio {name} ..."));
            }
            Ok(())
        } else if ext_of(path) == "wav" {
            self.open_wav_audio(path)
        } else {
            Err(AppError::Media(
                "no native audio backend is available in this build".to_owned(),
            ))
        }
    }

    /// Opens any supported media or subtitle file by extension (drag and drop / command line).
    pub fn open_any(&mut self, path: &Path, ctx: &egui::Context) -> Result<(), AppError> {
        if is_video_path(path) {
            self.open_video(path, ctx)
        } else if is_audio_path(path) {
            self.open_audio(path, ctx)
        } else {
            self.open_file(path)?;
            self.transport.invalidate();
            Ok(())
        }
    }

    /// Serialises the current script (including an un-flushed edit-box amend) for the renderer.
    #[must_use]
    pub fn render_ass_text(&self) -> String {
        let doc = &self.shell.doc;
        let script = doc.script();
        if let Some(active) = doc.active_line()
            && let Some(effective) = doc.effective_event_text(active)
            && script
                .events_section()
                .and_then(|s| s.get_event(active))
                .is_some_and(|ev| ev.text() != effective)
        {
            let mut patched = script.clone();
            if let Some(ev) = patched
                .events_section_mut()
                .and_then(|s| s.get_event_mut(active))
            {
                ev.set_text(effective.to_owned());
            }
            return serialize_ass(&patched);
        }
        serialize_ass(script)
    }

    fn apply_meta(&mut self, meta: &VideoMeta, refined: bool) {
        let keep = self
            .shell
            .playback
            .as_ref()
            .map_or(0, playback::PlaybackController::current_frame);
        self.shell.attach_video(
            meta.timecodes.clone(),
            meta.frame_count,
            meta.keyframes.clone(),
            meta.width,
            meta.height,
        );
        self.shell.set_video_sar(meta.sar.0, meta.sar.1);
        if refined && let Some(pb) = self.shell.playback.as_mut() {
            pb.seek_frame(keep);
        }
        self.refresh_markers(&meta.keyframes, &meta.timecodes);
        self.transport.invalidate();
    }

    fn refresh_markers(&mut self, keyframes: &[u32], tc: &Timecodes) {
        let kf_ms: Vec<i64> = keyframes
            .iter()
            .map(|&f| tc.frame_to_time_ms(f, TimeType::Exact))
            .collect();
        self.keyframe_ms = kf_ms;
        self.rebuild_markers();
    }

    /// Rebuilds keyframe + line-boundary markers for the audio view.
    pub fn rebuild_markers(&mut self) {
        let mut bounds = Vec::new();
        for ev in self.shell.doc.script().events() {
            bounds.push(i64::from(ev.start().as_cs()) * 10);
            bounds.push(i64::from(ev.end().as_cs()) * 10);
        }
        self.shell.audio_markers = TimelineMarkerIndex::new(self.keyframe_ms.clone(), bounds);
    }

    fn handle_media_event(&mut self, ev: MediaEvent) {
        match ev {
            MediaEvent::VideoReady(meta) => {
                self.apply_meta(&meta, false);
                self.notify_status(format!(
                    "Video ready: {}x{}, {} frames",
                    meta.width, meta.height, meta.frame_count
                ));
            }
            MediaEvent::VideoRefined(meta) => self.apply_meta(&meta, true),
            MediaEvent::AudioReady { duration_ms } => {
                let markers = std::mem::take(&mut self.shell.audio_markers);
                self.shell.attach_audio(duration_ms, markers);
                self.rebuild_markers();
                self.notify_status(format!("Audio ready ({} s)", duration_ms / 1_000));
            }
            MediaEvent::AudioDataChanged => self.shell.audio.invalidate_texture(),
            MediaEvent::Status(s) => self.notify_status(s),
            MediaEvent::Error(e) => self.notify_error(e),
        }
    }

    /// Per-frame work before the UI is laid out: drain provider events and feed the audio master
    /// clock into the playback controller.
    pub fn media_pre_show(&mut self, clock: &dyn Clock) {
        let mut events = Vec::new();
        self.video.poll(&mut events);
        if let Some(a) = self.audio.as_mut() {
            a.poll(&mut events);
        }
        for ev in events {
            self.handle_media_event(ev);
        }
        if self.transport.audio_running
            && let Some(pos) = self.audio.as_ref().and_then(|a| a.position_ms())
            && let Some(pb) = self.shell.playback.as_mut()
            && pb.is_playing()
        {
            pb.update_audio_clock(pos, clock.now_ns());
        }
    }

    fn active_event_times(&self) -> Option<(usize, i64, i64)> {
        let idx = self.shell.doc.active_line()?;
        let ev = self.shell.doc.script().events_section()?.get_event(idx)?;
        Some((
            idx,
            i64::from(ev.start().as_cs()) * 10,
            i64::from(ev.end().as_cs()) * 10,
        ))
    }

    fn range_for(&self, kind: PlayRangeKind) -> Option<TimeRangeMs> {
        let idx = self.shell.doc.active_line()?;
        let ev = self.shell.doc.script().events_section()?.get_event(idx)?;
        let sel = self.shell.audio.active_selection;
        let mut ctl = DialogueTimingController::new(
            TimingControllerConfig::default(),
            self.audio.as_ref().map_or(0, |a| a.duration_ms()),
        );
        let mut shown = ev.clone();
        shown.set_start(AssTime::from_cs(
            u32::try_from(sel.start_ms.max(0) / 10).unwrap_or(0),
        ));
        shown.set_end(AssTime::from_cs(
            u32::try_from(sel.end_ms.max(0) / 10).unwrap_or(0),
        ));
        ctl.select_line(idx, &shown);
        let mut r = ctl.play_range(kind);
        if kind == PlayRangeKind::CurrentLine {
            r = TimeRangeMs {
                start_ms: i64::from(ev.start().as_cs()) * 10,
                end_ms: i64::from(ev.end().as_cs()) * 10,
            };
        }
        Some(r)
    }

    /// Starts audible playback of `[start_ms, end_ms]` and (when a video is attached) moves the
    /// playback controller to the same range so picture and sound run together.
    pub fn play_range(&mut self, start_ms: i64, end_ms: i64, clock: &dyn Clock) {
        let start_ms = start_ms.max(0);
        let end_ms = end_ms.max(start_ms + 1);
        if let Some(pb) = self.shell.playback.as_mut() {
            pb.play_range(start_ms, end_ms, false, clock);
        }
        self.start_audio(start_ms, end_ms);
    }

    fn start_audio(&mut self, start_ms: i64, end_ms: i64) {
        if let Some(a) = self.audio.as_mut() {
            a.play(start_ms, end_ms);
            self.transport.audio_running = true;
            self.transport.audio_only_end_ms = Some(end_ms);
        } else {
            self.notify_status("No audio is loaded; playing video only");
        }
    }

    /// Stops audio and video playback.
    pub fn stop_playback(&mut self) {
        if let Some(a) = self.audio.as_mut() {
            a.stop();
        }
        self.transport.audio_running = false;
        self.transport.audio_only_end_ms = None;
        if let Some(pb) = self.shell.playback.as_mut() {
            pb.pause();
        }
        self.shell.audio.play_cursor_ms = None;
    }

    fn handle_video_action(&mut self, act: VideoPanelAction, clock: &dyn Clock) {
        match act {
            VideoPanelAction::PlayToggle => {
                let playing = self
                    .shell
                    .playback
                    .as_ref()
                    .is_some_and(playback::PlaybackController::is_playing);
                if playing {
                    let t = self
                        .shell
                        .playback
                        .as_ref()
                        .map_or(0, playback::PlaybackController::current_time_ms);
                    let end = self
                        .audio
                        .as_ref()
                        .map_or(i64::MAX / 4, |a| a.duration_ms());
                    self.start_audio(t, end);
                } else {
                    self.stop_audio_only();
                }
            }
            VideoPanelAction::PlayLine => {
                if let Some((_, s, e)) = self.active_event_times() {
                    self.play_range(s, e, clock);
                }
            }
            VideoPanelAction::Stop | VideoPanelAction::StepFrame(_) => self.stop_audio_only(),
            VideoPanelAction::SeekFrame(_) | VideoPanelAction::SeekKeyframe(_) => {
                let playing = self
                    .shell
                    .playback
                    .as_ref()
                    .is_some_and(playback::PlaybackController::is_playing);
                if playing {
                    let t = self
                        .shell
                        .playback
                        .as_ref()
                        .map_or(0, playback::PlaybackController::current_time_ms);
                    let end = self
                        .audio
                        .as_ref()
                        .map_or(i64::MAX / 4, |a| a.duration_ms());
                    self.start_audio(t, end);
                }
            }
            VideoPanelAction::VisualOverlayEdited | VideoPanelAction::ShowVisualHelp => {}
        }
    }

    fn stop_audio_only(&mut self) {
        if let Some(a) = self.audio.as_mut() {
            a.stop();
        }
        self.transport.audio_running = false;
        self.shell.audio.play_cursor_ms = None;
    }

    fn select_line(&mut self, idx: usize) {
        let count = self.shell.grid.model.row_count();
        if idx < count {
            self.shell.doc.set_selection(Some(idx), &[idx]);
        }
    }

    fn handle_audio_action(&mut self, act: AudioPanelAction, clock: &dyn Clock) {
        match act {
            AudioPanelAction::Play(kind) => {
                if let Some(r) = self.range_for(kind) {
                    self.play_range(r.start_ms, r.end_ms, clock);
                }
            }
            AudioPanelAction::PlayToEnd => {
                if let Some(r) = self.range_for(PlayRangeKind::ToEnd) {
                    self.play_range(r.start_ms, r.end_ms, clock);
                }
            }
            AudioPanelAction::Stop => self.stop_playback(),
            AudioPanelAction::Commit => {
                if let Err(e) = self.execute_command("audio/commit") {
                    self.notify_error(format!("Commit failed: {e}"));
                }
            }
            AudioPanelAction::CommitAndNext => {
                let cur = self.shell.doc.active_line().unwrap_or(0);
                if let Err(e) = self.execute_command("audio/commit") {
                    self.notify_error(format!("Commit failed: {e}"));
                }
                self.select_line(cur + 1);
            }
            AudioPanelAction::PrevLine => {
                let cur = self.shell.doc.active_line().unwrap_or(0);
                self.select_line(cur.saturating_sub(1));
            }
            AudioPanelAction::NextLine => {
                let cur = self.shell.doc.active_line().unwrap_or(0);
                self.select_line(cur + 1);
            }
            AudioPanelAction::AddLeadIn | AudioPanelAction::AddLeadOut => {
                let lead = if act == AudioPanelAction::AddLeadIn {
                    -300
                } else {
                    300
                };
                let sel = &mut self.shell.audio.active_selection;
                if lead < 0 {
                    sel.start_ms = (sel.start_ms + lead).max(0);
                } else {
                    sel.end_ms += lead;
                }
            }
            AudioPanelAction::OpenTimingOptions => {
                self.shell.dialogs.open = Some(ui_dialogs::ActiveDialog::Preferences);
            }
            AudioPanelAction::SelectionChanged(_) => {}
        }
    }

    /// Handles command IDs the shell forwards (open/save/audio...). Failures are always shown.
    fn handle_pending_commands(&mut self, ctx: &egui::Context) {
        let cmds = std::mem::take(&mut self.shell.pending_commands);
        for cmd in cmds {
            let res = self.run_host_command(&cmd, ctx);
            if let Err(e) = res {
                self.notify_error(format!("{cmd}: {e}"));
            }
        }
    }

    fn pick(&mut self, kind: PickKind) -> Option<PathBuf> {
        self.file_dialogs.as_mut().and_then(|d| d.pick_file(kind))
    }

    fn run_host_command(&mut self, cmd: &str, ctx: &egui::Context) -> Result<(), AppError> {
        match cmd {
            "subtitle/open" => {
                if let Some(p) = self.pick(PickKind::Subtitles) {
                    self.open_file(&p)?;
                    self.transport.invalidate();
                }
            }
            "subtitle/save" => {
                if self.current_path.is_some() {
                    self.save()?;
                } else {
                    self.run_host_command("subtitle/save/as", ctx)?;
                }
            }
            "subtitle/save/as" => {
                let name = self.shell.script_name.clone();
                if let Some(dest) = self.file_dialogs.as_mut().and_then(|d| d.pick_save(&name)) {
                    self.save_as(&dest)?;
                }
            }
            "subtitle/open/video" => {
                let src = if let Some(p) = self.video_path.clone().filter(|p| ext_of(p) == "mkv") {
                    Some(p)
                } else {
                    self.pick(PickKind::Mkv)
                };
                if let Some(p) = src {
                    self.open_mkv_subtitles(&p)?;
                }
            }
            "video/open" => {
                if let Some(p) = self.pick(PickKind::Video) {
                    self.open_video(&p, ctx)?;
                }
            }
            "audio/open" => {
                if let Some(p) = self.pick(PickKind::Audio) {
                    self.open_audio(&p, ctx)?;
                }
            }
            "audio/open/video" => {
                if let Some(p) = self.video_path.clone() {
                    self.open_audio(&p, ctx)?;
                }
            }
            "mkv/import" => {
                let n = self.import_selected_mkv_tracks()?;
                self.notify_status(format!("Imported {n} subtitle line(s) from video"));
                self.transport.invalidate();
            }
            "video/close" => {
                self.shell.detach_video();
                self.video_path = None;
            }
            "audio/close" => {
                self.stop_playback();
                self.audio = None;
                self.audio_path = None;
                self.shell.detach_audio();
            }
            other => {
                // Remaining IDs are handled by the editing command layer.
                self.execute_command(other)?;
            }
        }
        Ok(())
    }

    /// Per-frame work after the UI was laid out: handle forwarded commands and panel actions,
    /// keep audio cursor / selection / video frame / subtitle script in sync.
    pub fn media_post_show(&mut self, ctx: &egui::Context, clock: &dyn Clock) {
        self.handle_pending_commands(ctx);

        if let Some(act) = self.shell.last_video_action.take() {
            self.handle_video_action(act, clock);
        }
        if let Some(act) = self.shell.last_audio_action.take() {
            self.handle_audio_action(act, clock);
        }

        // Playback ended (range finished or user paused): silence the audio.
        let playing = self
            .shell
            .playback
            .as_ref()
            .is_some_and(playback::PlaybackController::is_playing);
        if self.transport.was_playing && !playing && self.transport.audio_running {
            self.stop_audio_only();
        }
        self.transport.was_playing = playing;

        // Audio-only playback ended by itself.
        if self.transport.audio_running {
            let still = self.audio.as_ref().and_then(|a| a.position_ms());
            if let Some(pos) = still {
                self.shell.audio.play_cursor_ms = Some(pos);
                let vp = &mut self.shell.audio.viewport;
                let right = vp.scroll_ms + f64::from(vp.width_px) * vp.ms_per_px;
                #[allow(clippy::cast_precision_loss)]
                let p = pos as f64;
                if self.shell.audio.auto_scroll && (p < vp.scroll_ms || p > right) {
                    vp.scroll_ms = (p - 200.0 * vp.ms_per_px).max(0.0);
                    vp.target_scroll_ms = vp.scroll_ms;
                }
                ctx.request_repaint();
            } else {
                self.transport.audio_running = false;
                self.shell.audio.play_cursor_ms = None;
            }
        }

        // Active line changed: mirror it into the audio selection and (AUTO) the video position.
        let active = self.active_event_times();
        let active_idx = active.map(|a| a.0);
        if active_idx != self.transport.last_active_line {
            self.transport.last_active_line = active_idx;
            if let Some((_, s, e)) = active {
                self.shell.audio.active_selection = TimeRangeMs {
                    start_ms: s,
                    end_ms: e,
                };
                let vp = &mut self.shell.audio.viewport;
                let width_ms = f64::from(vp.width_px) * vp.ms_per_px;
                #[allow(clippy::cast_precision_loss)]
                let centre = (s + e) as f64 / 2.0;
                vp.scroll_ms = (centre - width_ms / 2.0).max(0.0);
                vp.target_scroll_ms = vp.scroll_ms;
                if self.shell.video.auto_scroll_video
                    && !playing
                    && let Some(pb) = self.shell.playback.as_mut()
                {
                    pb.seek_line_start(s);
                }
            }
        }

        // Displayed frame changed: ask the provider for it.
        if let Some(f) = self
            .shell
            .playback
            .as_ref()
            .map(playback::PlaybackController::current_frame)
            && self.transport.requested_frame != Some(f)
        {
            self.transport.requested_frame = Some(f);
            self.video.request_frame(f);
        }

        // Script changed (including un-flushed typing): re-render subtitles on the frame.
        let commit = self.shell.doc.commit_id();
        if self.transport.last_commit != Some(commit) {
            self.transport.last_commit = Some(commit);
            if self.shell.playback.is_some() || self.video_path.is_some() {
                let text = self.render_ass_text();
                self.video.set_subtitles(text);
            }
            if self.audio.is_some() {
                self.rebuild_markers();
            }
        }
    }
}
