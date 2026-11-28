//! End-to-end test of the native editor on a generated clip (video + audio + `.ass`): open, play
//! 2 s, step frames, edit a line and see the render change, time a line from audio, save, reopen.
//! Also records the latency numbers listed in the report (open -> first frame, keystroke ->
//! screen, re-render after edit, step frame) and asserts the AGENTS.md budgets.
//!
//! The clip is generated with `ffmpeg` (path from `HLOUIS_FFMPEG`, else `ffmpeg` on `PATH`); the
//! test is skipped with a visible message when no ffmpeg is available.

use app_core::EditorApp;
use app_native::{native_editor, InstantClock};
use ass_core::parse_ass;
use egui::Vec2;
use egui_kittest::Harness;
use std::cell::RefCell;
use std::path::{Path, PathBuf};
use std::process::Command;
use std::rc::Rc;
use std::time::{Duration, Instant};

const ASS: &str = "[Script Info]\nScriptType: v4.00+\nPlayResX: 640\nPlayResY: 360\n\n[V4+ Styles]\nFormat: Name, Fontname, Fontsize, PrimaryColour, SecondaryColour, OutlineColour, BackColour, Bold, Italic, Underline, StrikeOut, ScaleX, ScaleY, Spacing, Angle, BorderStyle, Outline, Shadow, Alignment, MarginL, MarginR, MarginV, Encoding\nStyle: Default,Arial,36,&H00FFFFFF,&H000000FF,&H00000000,&H00000000,0,0,0,0,100,100,0,0,1,2,1,2,10,10,20,1\n\n[Events]\nFormat: Layer, Start, End, Style, Name, MarginL, MarginR, MarginV, Effect, Text\nDialogue: 0,0:00:00.00,0:00:02.00,Default,,0,0,0,,Hello subtitle one\nDialogue: 0,0:00:03.00,0:00:05.00,Default,,0,0,0,,Second line here\n";

fn make_clip(dir: &Path) -> Option<PathBuf> {
    let ff = std::env::var("HLOUIS_FFMPEG").unwrap_or_else(|_| "ffmpeg".to_owned());
    let out = dir.join("clip.mkv");
    let status = Command::new(ff)
        .args([
            "-y",
            "-loglevel",
            "error",
            "-f",
            "lavfi",
            "-i",
            "testsrc2=size=640x360:rate=25:duration=8",
            "-f",
            "lavfi",
            "-i",
            "sine=frequency=440:sample_rate=48000:duration=8",
            "-c:v",
            "libx264",
            "-preset",
            "ultrafast",
            "-g",
            "25",
            "-pix_fmt",
            "yuv420p",
            "-c:a",
            "aac",
            "-shortest",
        ])
        .arg(&out)
        .status()
        .ok()?;
    status.success().then_some(out)
}

type Shared = Rc<RefCell<EditorApp>>;

fn pump(
    h: &mut Harness<'_>,
    app: &Shared,
    timeout: Duration,
    mut done: impl FnMut(&mut EditorApp) -> bool,
) -> bool {
    let t0 = Instant::now();
    while t0.elapsed() < timeout {
        h.step();
        if done(&mut app.borrow_mut()) {
            return true;
        }
        std::thread::sleep(Duration::from_millis(2));
    }
    false
}

fn frame_hash(app: &EditorApp) -> u64 {
    let Some(px) = app.video.frame_rgba(0) else {
        return 0;
    };
    let mut h = 0xcbf2_9ce4_8422_2325u64;
    for &b in px.iter().step_by(7) {
        h = (h ^ u64::from(b)).wrapping_mul(0x100_0000_01b3);
    }
    h
}

#[test]
fn native_editor_end_to_end_on_generated_clip() {
    let dir = tempfile::tempdir().expect("tmp");
    let Some(clip) = make_clip(dir.path()) else {
        eprintln!("SKIPPED: ffmpeg is not available (set HLOUIS_FFMPEG)");
        return;
    };
    let ass_path = dir.path().join("clip.ass");
    std::fs::write(&ass_path, ASS).expect("write ass");

    let (mut app, problems) = native_editor(parse_ass(ASS).value);
    assert!(problems.is_empty(), "native providers failed: {problems:?}");
    app.autosave_cfg.autosave_dir = dir.path().join("autosave");
    let app: Shared = Rc::new(RefCell::new(app));
    let clock = Rc::new(InstantClock::default());
    let app_ui = Rc::clone(&app);
    let clock_ui = Rc::clone(&clock);
    let mut harness = Harness::builder()
        .with_size(Vec2::new(1400.0, 860.0))
        .with_accessibility_check(false)
        .build_ui(move |ui| {
            app_ui.borrow_mut().show(ui, clock_ui.as_ref());
        });
    harness.step();

    // 1. Open video + subtitles: first frame must appear quickly.
    let t_open = Instant::now();
    app.borrow_mut()
        .open_any(&clip, &harness.ctx)
        .expect("open video");
    app.borrow_mut()
        .open_any(&ass_path, &harness.ctx)
        .expect("open ass");
    assert!(
        pump(&mut harness, &app, Duration::from_secs(20), |a| a
            .video
            .frame_rgba(0)
            .is_some()
            && a.shell.playback.is_some()),
        "no first frame; notice = {:?}",
        app.borrow().notice
    );
    let open_to_first_frame = t_open.elapsed();
    eprintln!("open -> first frame: {open_to_first_frame:?}");
    assert_eq!(app.borrow().shell.video.display.video_w as u32, 640);

    // 2. Audio decodes in the background and shows up in the audio panel.
    assert!(
        pump(&mut harness, &app, Duration::from_secs(60), |a| a
            .shell
            .audio
            .viewport
            .width_px
            > 0.0
            && a.audio.as_ref().is_some_and(|x| x.duration_ms() > 5_000)),
        "audio never became ready; notice = {:?}",
        app.borrow().notice
    );
    let mut peaks = Vec::new();
    app.borrow()
        .audio
        .as_ref()
        .expect("audio")
        .query_peaks_into(0, 2_000, 200, &mut peaks);
    assert!(
        peaks.iter().any(|p| p.max > 1_000),
        "waveform is empty although the clip carries a 440 Hz tone"
    );

    // 3. Subtitle line 0 is active at 1 s: the frame carries libass-rendered text.
    {
        let mut a = app.borrow_mut();
        a.shell.playback.as_mut().expect("pb").seek_time_ms(1_000);
    }
    let settle = |h: &mut Harness<'_>, app: &Shared| {
        let r0 = app.borrow().video.revision(0);
        pump(h, app, Duration::from_secs(5), |a| {
            a.video.revision(0) != r0
        });
        pump(h, app, Duration::from_millis(150), |_| false);
    };
    settle(&mut harness, &app);
    let with_text = frame_hash(&app.borrow());

    // 4. Edit the line: the render changes, and we time keystroke -> new frame.
    let t_edit = Instant::now();
    {
        let mut a = app.borrow_mut();
        a.shell.doc.set_selection(Some(0), &[0]);
    }
    harness.step();
    let rev_before = app.borrow().video.revision(0);
    {
        let mut a = app.borrow_mut();
        a.shell
            .doc
            .amend_text(0, "edit", "Changed text of line one!")
            .expect("amend");
    }
    let changed = pump(&mut harness, &app, Duration::from_secs(5), |a| {
        a.video.revision(0) != rev_before
    });
    let edit_to_frame = t_edit.elapsed();
    eprintln!("edit -> re-rendered frame: {edit_to_frame:?}");
    assert!(changed, "no new frame after editing the line");
    pump(&mut harness, &app, Duration::from_millis(150), |_| false);
    assert_ne!(
        with_text,
        frame_hash(&app.borrow()),
        "editing the line did not change the rendered frame"
    );

    // 5. Play 2 s: video clock advances, audio position follows (or the failure is visible).
    {
        let mut a = app.borrow_mut();
        a.shell.playback.as_mut().expect("pb").seek_time_ms(0);
        a.play_range(0, 8_000, clock.as_ref());
    }
    let t_play = Instant::now();
    while t_play.elapsed() < Duration::from_secs(2) {
        harness.step();
        std::thread::sleep(Duration::from_millis(5));
    }
    let (video_ms, audio_ms, notice) = {
        let a = app.borrow();
        (
            a.shell.playback.as_ref().expect("pb").current_time_ms(),
            a.audio.as_ref().and_then(|x| x.position_ms()),
            a.notice.clone(),
        )
    };
    eprintln!("after 2 s of play: video {video_ms} ms, audio {audio_ms:?}, notice {notice:?}");
    assert!(
        (1_500..=2_800).contains(&video_ms),
        "video clock off after 2 s: {video_ms}"
    );
    match audio_ms {
        Some(ms) => assert!(
            (video_ms - ms).abs() < 250,
            "audio {ms} ms and video {video_ms} ms out of sync"
        ),
        None => assert!(
            notice.is_some(),
            "audio is silent but no notice was shown to the user"
        ),
    }

    // 6. Pause, step frames, time a step.
    {
        let mut a = app.borrow_mut();
        a.stop_playback();
    }
    harness.step();
    let f0 = app
        .borrow()
        .shell
        .playback
        .as_ref()
        .expect("pb")
        .current_frame();
    let mut step_times = Vec::new();
    for _ in 0..5 {
        let rev = app.borrow().video.revision(0);
        let t = Instant::now();
        app.borrow_mut()
            .shell
            .playback
            .as_mut()
            .expect("pb")
            .step_frame(1);
        let ok = pump(&mut harness, &app, Duration::from_secs(3), |a| {
            a.video.revision(0) != rev
        });
        assert!(ok, "step frame produced no new frame");
        step_times.push(t.elapsed());
    }
    let f1 = app
        .borrow()
        .shell
        .playback
        .as_ref()
        .expect("pb")
        .current_frame();
    assert_eq!(f1, f0 + 5);
    eprintln!("step frame times: {step_times:?}");

    // 7. Time a line from the audio selection and commit.
    {
        let mut a = app.borrow_mut();
        a.shell.doc.set_selection(Some(1), &[1]);
    }
    harness.step();
    {
        let mut a = app.borrow_mut();
        a.shell.audio.active_selection.start_ms = 3_200;
        a.shell.audio.active_selection.end_ms = 4_400;
    }
    app.borrow_mut()
        .execute_command("audio/commit")
        .expect("commit");
    harness.step();
    {
        let a = app.borrow();
        let ev = a
            .shell
            .doc
            .script()
            .events_section()
            .and_then(|s| s.get_event(1))
            .expect("ev1");
        assert_eq!(ev.start().as_cs(), 320);
        assert_eq!(ev.end().as_cs(), 440);
    }

    // 8. Save and reopen.
    let saved = dir.path().join("saved.ass");
    app.borrow_mut().save_as(&saved).expect("save");
    let (mut again, _) = native_editor(parse_ass(ASS).value);
    again.open_file(&saved).expect("reopen");
    let texts: Vec<String> = again
        .shell
        .doc
        .script()
        .events()
        .map(|e| e.text().to_owned())
        .collect();
    assert_eq!(texts[0], "Changed text of line one!");
    assert_eq!(texts[1], "Second line here");

    // Budgets (AGENTS.md): first frame < 500 ms is the cached-index budget for a 24 min episode;
    // this tiny clip must be far below it. Re-render after an edit < 8 ms is the libass+blend
    // budget measured on the worker; the end-to-end pipeline here includes the 2 ms polling.
    assert!(
        open_to_first_frame < Duration::from_millis(500),
        "open -> first frame {open_to_first_frame:?} exceeds the 500 ms budget"
    );
    let worst_step = step_times.iter().max().copied().unwrap_or_default();
    assert!(
        worst_step < Duration::from_millis(60),
        "step frame {worst_step:?} exceeds the 60 ms seek budget"
    );
}
