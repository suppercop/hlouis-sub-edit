//! Tests for the unsaved-changes close flow (Save / Discard / Cancel / Save-failure)
//! and verifying that opening video/audio on a fresh script does not mark it modified.

#![forbid(unsafe_code)]

use app_core::EditorApp;
use app_core::media::{
    AudioProvider, DummyVideoProvider, FileDialogs, MediaEvent, MediaFactory, PickKind, VideoMeta,
    VideoProvider, WavAudioProvider,
};
use ass_core::ScriptFile;
use audio_peaks::PeakBucket;
use egui::{
    Event, Modifiers, PointerButton, Pos2, RawInput, Rect, Vec2, ViewportCommand, ViewportId,
};
use egui_kittest::Harness;
use egui_kittest::kittest::Queryable;
use keyframes::Timecodes;
use playback::Clock;
use std::fs;
use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex};
use tempfile::tempdir;

struct ZeroClock;
impl Clock for ZeroClock {
    fn now_ns(&self) -> u64 {
        0
    }
}

struct StubDialogs {
    save_path: Option<PathBuf>,
    save_calls: Arc<Mutex<usize>>,
}

impl FileDialogs for StubDialogs {
    fn pick_file(&mut self, _kind: PickKind) -> Option<PathBuf> {
        None
    }

    fn pick_save(&mut self, _suggested_name: &str) -> Option<PathBuf> {
        if let Ok(mut n) = self.save_calls.lock() {
            *n += 1;
        }
        self.save_path.clone()
    }
}

struct ReadyVideoProvider {
    inner: DummyVideoProvider,
    pending_ready: bool,
}

impl ReadyVideoProvider {
    fn new() -> Self {
        Self {
            inner: DummyVideoProvider::new(640, 360, 24_000, 120),
            pending_ready: true,
        }
    }
}

impl VideoProvider for ReadyVideoProvider {
    fn width(&self) -> u32 {
        self.inner.width()
    }
    fn height(&self) -> u32 {
        self.inner.height()
    }
    fn frame_count(&self) -> u32 {
        self.inner.frame_count()
    }
    fn timecodes(&self) -> &Timecodes {
        self.inner.timecodes()
    }
    fn keyframes(&self) -> &[u32] {
        self.inner.keyframes()
    }
    fn frame_rgba(&self, frame: u32) -> Option<&[u8]> {
        self.inner.frame_rgba(frame)
    }
    fn poll(&mut self, out: &mut Vec<MediaEvent>) {
        if self.pending_ready {
            self.pending_ready = false;
            out.push(MediaEvent::VideoReady(VideoMeta {
                width: self.inner.width(),
                height: self.inner.height(),
                frame_count: self.inner.frame_count(),
                timecodes: self.inner.timecodes().clone(),
                keyframes: self.inner.keyframes().to_vec(),
                sar: (1, 1),
            }));
        }
    }
}

struct ReadyAudioProvider {
    inner: WavAudioProvider,
    pending_ready: bool,
}

impl ReadyAudioProvider {
    fn new() -> Self {
        Self {
            inner: WavAudioProvider::synthetic(10_000, 16_000),
            pending_ready: true,
        }
    }
}

impl AudioProvider for ReadyAudioProvider {
    fn sample_rate(&self) -> u32 {
        self.inner.sample_rate()
    }
    fn channels(&self) -> u16 {
        self.inner.channels()
    }
    fn duration_ms(&self) -> i64 {
        self.inner.duration_ms()
    }
    fn read_mono_i16(&self, start_sample: usize, count: usize, out: &mut Vec<i16>) {
        self.inner.read_mono_i16(start_sample, count, out);
    }
    fn query_peaks_into(
        &self,
        start_ms: i64,
        end_ms: i64,
        width_px: usize,
        out: &mut Vec<PeakBucket>,
    ) {
        self.inner.query_peaks_into(start_ms, end_ms, width_px, out);
    }
    fn poll(&mut self, out: &mut Vec<MediaEvent>) {
        if self.pending_ready {
            self.pending_ready = false;
            out.push(MediaEvent::AudioReady {
                duration_ms: self.inner.duration_ms(),
            });
        }
    }
}

struct StubMediaFactory;

impl MediaFactory for StubMediaFactory {
    fn open_video(
        &mut self,
        _path: &Path,
        _ctx: &egui::Context,
        _ass_text: String,
    ) -> Result<Box<dyn VideoProvider>, String> {
        Ok(Box::new(ReadyVideoProvider::new()))
    }

    fn open_audio(
        &mut self,
        _path: &Path,
        _ctx: &egui::Context,
    ) -> Result<Box<dyn AudioProvider>, String> {
        Ok(Box::new(ReadyAudioProvider::new()))
    }
}

fn dirty_app() -> EditorApp {
    let mut app = EditorApp::new(ScriptFile::new_default());
    app.new_script();
    app.shell
        .doc
        .amend_text(0, "typing", "Unsaved line")
        .expect("amend");
    assert!(app.shell.doc.is_dirty());
    app
}

/// Renders the close prompt, clicks the button with `label`, and returns whether
/// `egui::ViewportCommand::Close` was emitted on that click.
fn run_frame(ctx: &egui::Context, input: RawInput, app: &mut EditorApp) -> bool {
    let mut out = ctx.run_ui(input, |ui| app.show(ui, &ZeroClock));
    let has_close = out
        .viewport_output
        .get(&ViewportId::ROOT)
        .is_some_and(|v| v.commands.contains(&ViewportCommand::Close));
    out.textures_delta.clear();
    has_close
}

fn click_close_prompt_button(app: &mut EditorApp, label: &str) -> bool {
    let screen = Rect::from_min_size(Pos2::ZERO, Vec2::new(1280.0, 800.0));

    // Locate the button center via egui_kittest on an identical layout.
    let btn_center = {
        let mut probe = EditorApp::new(ScriptFile::new_default());
        probe.new_script();
        probe.shell.script_name.clone_from(&app.shell.script_name);
        probe.close_prompt_open = true;
        let mut harness = Harness::builder()
            .with_size(screen.size())
            .with_accessibility_check(false)
            .build_ui_state(
                |ui, state: &mut EditorApp| {
                    state.show(ui, &ZeroClock);
                },
                probe,
            );
        harness.run_steps(2);
        let rect = harness.get_by_label(label).rect();
        Pos2::new(
            f32::midpoint(rect.min.x, rect.max.x),
            f32::midpoint(rect.min.y, rect.max.y),
        )
    };

    let ctx = egui::Context::default();
    let base_input = || RawInput {
        screen_rect: Some(screen),
        ..Default::default()
    };
    // Warm up layout so the window is positioned identically.
    let _ = run_frame(&ctx, base_input(), app);
    let _ = run_frame(&ctx, base_input(), app);

    let mut press = base_input();
    press.events.push(Event::PointerMoved(btn_center));
    press.events.push(Event::PointerButton {
        pos: btn_center,
        button: PointerButton::Primary,
        pressed: true,
        modifiers: Modifiers::NONE,
    });
    let close_press = run_frame(&ctx, press, app);

    let mut release = base_input();
    release.events.push(Event::PointerMoved(btn_center));
    release.events.push(Event::PointerButton {
        pos: btn_center,
        button: PointerButton::Primary,
        pressed: false,
        modifiers: Modifiers::NONE,
    });
    let close_release = run_frame(&ctx, release, app);

    close_press || close_release
}

#[test]
fn close_clean_document_permits_immediately() {
    let mut app = EditorApp::new(ScriptFile::new_default());
    app.new_script();
    assert!(!app.shell.doc.is_dirty());
    assert!(app.request_close());
    assert!(!app.close_prompt_open);
}

#[test]
fn close_dirty_discard_sends_close_and_confirms_next_request() {
    let mut app = dirty_app();
    assert!(!app.request_close());
    assert!(app.close_prompt_open);

    let sent_close = click_close_prompt_button(&mut app, "Discard");
    assert!(sent_close, "Discard must emit ViewportCommand::Close");
    assert!(!app.close_prompt_open);
    assert!(app.close_confirmed);
    assert!(app.shell.doc.is_dirty());
    assert!(
        app.request_close(),
        "subsequent close_requested must be allowed after Discard"
    );
}

#[test]
fn close_dirty_cancel_keeps_app_open() {
    let mut app = dirty_app();
    assert!(!app.request_close());
    assert!(app.close_prompt_open);

    let sent_close = click_close_prompt_button(&mut app, "Cancel");
    assert!(!sent_close, "Cancel must not emit ViewportCommand::Close");
    assert!(!app.close_prompt_open);
    assert!(!app.close_confirmed);
    assert!(app.shell.doc.is_dirty());
    assert!(!app.request_close());
    assert!(app.close_prompt_open);
}

#[test]
fn close_dirty_save_existing_path_saves_and_closes() {
    let dir = tempdir().expect("tempdir");
    let dest = dir.path().join("existing.ass");

    let mut app = dirty_app();
    app.current_path = Some(dest.clone());
    "existing.ass".clone_into(&mut app.shell.script_name);

    assert!(!app.request_close());
    assert!(app.close_prompt_open);

    let sent_close = click_close_prompt_button(&mut app, "Save");
    assert!(sent_close, "Save must emit ViewportCommand::Close");
    assert!(!app.close_prompt_open);
    assert!(app.close_confirmed);
    assert!(!app.shell.doc.is_dirty());
    assert!(app.notice.is_none());
    assert!(app.request_close());

    let saved = fs::read_to_string(&dest).expect("saved file");
    assert!(saved.contains("Unsaved line"));
}

#[test]
fn close_dirty_save_untitled_opens_save_as_and_closes() {
    let dir = tempdir().expect("tempdir");
    let dest = dir.path().join("saved_as.ass");
    let calls = Arc::new(Mutex::new(0usize));

    let mut app = dirty_app();
    app.file_dialogs = Some(Box::new(StubDialogs {
        save_path: Some(dest.clone()),
        save_calls: Arc::clone(&calls),
    }));

    assert!(!app.request_close());
    let sent_close = click_close_prompt_button(&mut app, "Save");
    assert!(
        sent_close,
        "Save As success must emit ViewportCommand::Close"
    );
    assert_eq!(*calls.lock().expect("lock"), 1);
    assert_eq!(app.current_path.as_deref(), Some(dest.as_path()));
    assert!(!app.shell.doc.is_dirty());
    assert!(app.close_confirmed);
    assert!(app.request_close());
    assert!(
        fs::read_to_string(&dest)
            .expect("read")
            .contains("Unsaved line")
    );
}

#[test]
fn close_dirty_save_untitled_cancelled_keeps_app_open() {
    let calls = Arc::new(Mutex::new(0usize));

    let mut app = dirty_app();
    app.file_dialogs = Some(Box::new(StubDialogs {
        save_path: None,
        save_calls: Arc::clone(&calls),
    }));

    assert!(!app.request_close());
    let sent_close = click_close_prompt_button(&mut app, "Save");
    assert!(!sent_close, "cancelled Save As must keep app open");
    assert_eq!(*calls.lock().expect("lock"), 1);
    assert!(!app.close_prompt_open);
    assert!(!app.close_confirmed);
    assert!(app.notice.is_none());
    assert!(app.shell.doc.is_dirty());
    assert!(!app.request_close());
}

#[test]
fn close_dirty_save_failure_keeps_app_open_and_shows_error() {
    let dir = tempdir().expect("tempdir");
    // Point current_path at an existing directory so atomic write fails deterministically.
    let bad_dest = dir.path().to_path_buf();

    let mut app = dirty_app();
    app.current_path = Some(bad_dest);

    assert!(!app.request_close());
    let sent_close = click_close_prompt_button(&mut app, "Save");
    assert!(!sent_close, "Save failure must keep app open");
    assert!(!app.close_prompt_open);
    assert!(!app.close_confirmed);
    assert!(app.shell.doc.is_dirty());
    assert!(
        app.notice
            .as_deref()
            .is_some_and(|m| m.starts_with("Save failed:")),
        "expected error notice, got {:?}",
        app.notice
    );
    assert!(!app.request_close());
}

#[test]
fn close_dirty_save_untitled_without_dialog_keeps_open_and_shows_error() {
    let mut app = dirty_app();
    assert!(app.current_path.is_none());
    assert!(app.file_dialogs.is_none());

    assert!(!app.request_close());
    let sent_close = click_close_prompt_button(&mut app, "Save");
    assert!(!sent_close, "missing Save As dialog must keep app open");
    assert!(!app.close_confirmed);
    assert!(
        app.notice
            .as_deref()
            .is_some_and(|m| m.starts_with("Save failed:")),
        "expected error notice, got {:?}",
        app.notice
    );
    assert!(!app.request_close());
}

#[test]
fn fresh_start_opening_video_and_audio_does_not_mark_script_dirty() {
    let mut app = EditorApp::new(ScriptFile::new_default());
    app.factory = Some(Box::new(StubMediaFactory));
    app.new_script();
    assert!(!app.shell.doc.is_dirty());
    assert_eq!(app.shell.window_title(), "Untitled.ass - hlouis-sub-edit");

    let ctx = egui::Context::default();
    app.open_video(Path::new("episode01.mkv"), &ctx)
        .expect("open_video");

    let screen = Rect::from_min_size(Pos2::ZERO, Vec2::new(1280.0, 800.0));
    for _ in 0..3 {
        let input = RawInput {
            screen_rect: Some(screen),
            ..Default::default()
        };
        let _ = run_frame(&ctx, input, &mut app);
    }

    assert!(
        !app.shell.doc.is_dirty(),
        "opening video/audio alone must not mark the script dirty"
    );
    assert_eq!(app.shell.window_title(), "Untitled.ass - hlouis-sub-edit");
    assert!(app.request_close());
}

struct RecordingAudioProvider {
    inner: WavAudioProvider,
    played_ranges: Arc<Mutex<Vec<(i64, i64)>>>,
}

impl RecordingAudioProvider {
    fn new(played_ranges: Arc<Mutex<Vec<(i64, i64)>>>) -> Self {
        Self {
            inner: WavAudioProvider::synthetic(10_000, 16_000),
            played_ranges,
        }
    }
}

impl AudioProvider for RecordingAudioProvider {
    fn sample_rate(&self) -> u32 {
        self.inner.sample_rate()
    }
    fn channels(&self) -> u16 {
        self.inner.channels()
    }
    fn duration_ms(&self) -> i64 {
        self.inner.duration_ms()
    }
    fn read_mono_i16(&self, start_sample: usize, count: usize, out: &mut Vec<i16>) {
        self.inner.read_mono_i16(start_sample, count, out);
    }
    fn query_peaks_into(
        &self,
        start_ms: i64,
        end_ms: i64,
        width_px: usize,
        out: &mut Vec<PeakBucket>,
    ) {
        self.inner.query_peaks_into(start_ms, end_ms, width_px, out);
    }
    fn play(&mut self, start_ms: i64, end_ms: i64) {
        if let Ok(mut v) = self.played_ranges.lock() {
            v.push((start_ms, end_ms));
        }
    }
}

fn app_with_events(events: &[(u32, u32, &str, i32, &str)]) -> EditorApp {
    let mut script = ScriptFile::new_default();
    let sec = script.ensure_events_section_mut();
    for (i, &(s, e, style, layer, text)) in events.iter().enumerate() {
        let mut ev = ass_core::AssEvent::dialogue(
            ass_core::AssTime::from_cs(s),
            ass_core::AssTime::from_cs(e),
            text,
        );
        ev.set_style(style.to_owned());
        ev.set_layer(layer);
        sec.insert_event(i, ev);
    }
    let mut app = EditorApp::new(script);
    app.shell.settings.timing.default_duration_ms = 2_000;
    app.sync_ui_from_doc();
    app
}

#[test]
fn insert_before_and_after_gap_touching_overlapping() {
    // 1. Gap: [100..200], [500..600]
    let mut app = app_with_events(&[(100, 200, "Sign", 3, "A"), (500, 600, "Alt", 5, "B")]);
    app.shell.doc.set_selection(Some(1), &[1]);
    app.execute_command("subtitle/insert/before")
        .expect("insert before gap");
    assert_eq!(app.shell.doc.active_line(), Some(1));
    assert!(app.shell.editbox.request_focus);
    let inserted = app
        .shell
        .doc
        .script()
        .events_section()
        .and_then(|s| s.get_event(1))
        .expect("inserted");
    assert_eq!(inserted.start().as_cs(), 200);
    assert_eq!(inserted.end().as_cs(), 500);
    assert_eq!(inserted.style(), "Alt");
    assert_eq!(inserted.layer(), 5);

    // Gap after: [100..200], [500..600] -> active=0 -> [200..500], active becomes 1
    let mut app = app_with_events(&[(100, 200, "Sign", 3, "A"), (500, 600, "Alt", 5, "B")]);
    app.shell.doc.set_selection(Some(0), &[0]);
    app.execute_command("subtitle/insert/after")
        .expect("insert after gap");
    assert_eq!(app.shell.doc.active_line(), Some(1));
    assert!(app.shell.editbox.request_focus);
    let inserted = app
        .shell
        .doc
        .script()
        .events_section()
        .and_then(|s| s.get_event(1))
        .expect("inserted");
    assert_eq!(inserted.start().as_cs(), 200);
    assert_eq!(inserted.end().as_cs(), 500);
    assert_eq!(inserted.style(), "Sign");
    assert_eq!(inserted.layer(), 3);

    // 2. Touching: [100..300], [300..500] -> zero-length at 300
    let mut app = app_with_events(&[(100, 300, "Default", 0, "A"), (300, 500, "Default", 0, "B")]);
    app.shell.doc.set_selection(Some(1), &[1]);
    app.execute_command("subtitle/insert/before")
        .expect("insert before touching");
    let ev = app
        .shell
        .doc
        .script()
        .events_section()
        .and_then(|s| s.get_event(1))
        .expect("ev");
    assert_eq!((ev.start().as_cs(), ev.end().as_cs()), (300, 300));

    // 3. Overlapping: [100..400], [300..500] -> zero-length at boundary
    let mut app = app_with_events(&[(100, 400, "Default", 0, "A"), (300, 500, "Default", 0, "B")]);
    app.shell.doc.set_selection(Some(1), &[1]);
    app.execute_command("subtitle/insert/before")
        .expect("insert before overlap");
    let ev = app
        .shell
        .doc
        .script()
        .events_section()
        .and_then(|s| s.get_event(1))
        .expect("ev");
    assert_eq!((ev.start().as_cs(), ev.end().as_cs()), (300, 300));

    let mut app = app_with_events(&[(100, 400, "Default", 0, "A"), (300, 500, "Default", 0, "B")]);
    app.shell.doc.set_selection(Some(0), &[0]);
    app.execute_command("subtitle/insert/after")
        .expect("insert after overlap");
    let ev = app
        .shell
        .doc
        .script()
        .events_section()
        .and_then(|s| s.get_event(1))
        .expect("ev");
    assert_eq!((ev.start().as_cs(), ev.end().as_cs()), (400, 400));
}

#[test]
fn insert_before_and_after_first_last_empty_and_focus() {
    // 4. First line: active starts at 100cs (< default 200cs) -> clamped to 0..100
    let mut app = app_with_events(&[(100, 300, "Lead", 2, "First")]);
    app.shell.doc.set_selection(Some(0), &[0]);
    app.execute_command("subtitle/insert/before")
        .expect("insert before first");
    let ev = app
        .shell
        .doc
        .script()
        .events_section()
        .and_then(|s| s.get_event(0))
        .expect("ev");
    assert_eq!((ev.start().as_cs(), ev.end().as_cs()), (0, 100));
    assert_eq!(ev.style(), "Lead");
    assert_eq!(ev.layer(), 2);

    // 5. Last line: active ends at 300cs -> 300..500 (default 200cs)
    let mut app = app_with_events(&[(100, 300, "Tail", 4, "Last")]);
    app.shell.doc.set_selection(Some(0), &[0]);
    app.execute_command("subtitle/insert/after")
        .expect("insert after last");
    assert_eq!(app.shell.doc.active_line(), Some(1));
    let ev = app
        .shell
        .doc
        .script()
        .events_section()
        .and_then(|s| s.get_event(1))
        .expect("ev");
    assert_eq!((ev.start().as_cs(), ev.end().as_cs()), (300, 500));
    assert_eq!(ev.style(), "Tail");
    assert_eq!(ev.layer(), 4);

    // 6. Empty script (both before and after) + edit box focus on next frame
    let mut app = app_with_events(&[]);
    app.execute_command("subtitle/insert/after")
        .expect("insert after empty");
    assert_eq!(app.shell.doc.active_line(), Some(0));
    assert!(app.shell.editbox.request_focus);
    let ev = app
        .shell
        .doc
        .script()
        .events_section()
        .and_then(|s| s.get_event(0))
        .expect("ev");
    assert_eq!((ev.start().as_cs(), ev.end().as_cs()), (0, 200));

    let ctx = egui::Context::default();
    let screen = Rect::from_min_size(Pos2::ZERO, Vec2::new(1280.0, 800.0));
    let _ = run_frame(
        &ctx,
        RawInput {
            screen_rect: Some(screen),
            ..Default::default()
        },
        &mut app,
    );
    assert!(!app.shell.editbox.request_focus);
    assert!(ctx.egui_wants_keyboard_input(), "edit box must be focused");
}

#[test]
fn space_plays_active_line_outside_text_field_and_types_space_in_edit_box() {
    let played = Arc::new(Mutex::new(Vec::new()));
    let mut app = app_with_events(&[(100, 350, "Default", 0, "Hello")]);
    app.audio = Some(Box::new(RecordingAudioProvider::new(Arc::clone(&played))));

    let ctx = egui::Context::default();
    let screen = Rect::from_min_size(Pos2::ZERO, Vec2::new(1280.0, 800.0));
    let base = || RawInput {
        screen_rect: Some(screen),
        ..Default::default()
    };

    // Warm up without focusing the edit box.
    let _ = run_frame(&ctx, base(), &mut app);
    assert!(!ctx.egui_wants_keyboard_input());

    // Press Space outside text fields -> plays active line selection [1000..3500 ms].
    let mut space_in = base();
    space_in.events.push(Event::Key {
        key: egui::Key::Space,
        physical_key: None,
        pressed: true,
        repeat: false,
        modifiers: Modifiers::NONE,
    });
    let _ = run_frame(&ctx, space_in, &mut app);
    assert_eq!(*played.lock().expect("lock"), vec![(1_000, 3_500)]);

    // Now focus the edit box and type Space -> types a space, does NOT trigger audio playback.
    app.shell.editbox.request_focus = true;
    let _ = run_frame(&ctx, base(), &mut app);
    assert!(ctx.egui_wants_keyboard_input());

    let mut type_space = base();
    type_space.events.push(Event::Key {
        key: egui::Key::Space,
        physical_key: None,
        pressed: true,
        repeat: false,
        modifiers: Modifiers::NONE,
    });
    type_space.events.push(Event::Text(" ".to_owned()));
    let _ = run_frame(&ctx, type_space, &mut app);
    assert_eq!(
        played.lock().expect("lock").len(),
        1,
        "Space inside edit box must not trigger audio playback"
    );
    assert!(
        app.shell.editbox.text_buf.contains(' '),
        "Space inside edit box must type a space, got {:?}",
        app.shell.editbox.text_buf
    );
}
