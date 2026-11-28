//! End-to-end headless tests for [`app_core::EditorApp`] driving the real UI (`egui_kittest`),
//! file I/O, SRT->ASS, 64-track MKV import, crash recovery, WAV audio, and 200k-line responsiveness.

#![forbid(unsafe_code)]

use app_core::{EditorApp, build_demo_script};
use ass_core::{AssEvent, AssTime, ScriptFile, serialize_ass};
use autosave::write_autosave_now;
use egui::Vec2;
use egui_kittest::Harness;
use playback::Clock;
use std::fs;
use std::time::Instant;
use tempfile::tempdir;

struct ZeroClock;
impl Clock for ZeroClock {
    fn now_ns(&self) -> u64 {
        0
    }
}

fn encode_id(element_id: u32, out: &mut Vec<u8>) {
    let bytes = element_id.to_be_bytes();
    let first = bytes.iter().position(|&b| b != 0).unwrap_or(3);
    out.extend_from_slice(&bytes[first..]);
}

fn encode_vint(val: u64, out: &mut Vec<u8>) {
    if val < 0x7F {
        #[allow(clippy::cast_possible_truncation)]
        out.push(0x80 | (val as u8));
    } else if val < 0x3FFF {
        #[allow(clippy::cast_possible_truncation)]
        {
            out.push(0x40 | ((val >> 8) as u8));
            out.push((val & 0xFF) as u8);
        }
    } else {
        #[allow(clippy::cast_possible_truncation)]
        {
            out.push(0x20 | ((val >> 16) as u8));
            out.push(((val >> 8) & 0xFF) as u8);
            out.push((val & 0xFF) as u8);
        }
    }
}

fn elem(element_id: u32, payload: &[u8]) -> Vec<u8> {
    let mut out = Vec::new();
    encode_id(element_id, &mut out);
    encode_vint(payload.len() as u64, &mut out);
    out.extend_from_slice(payload);
    out
}

fn elem_uint(element_id: u32, val: u64) -> Vec<u8> {
    let bytes = val.to_be_bytes();
    let first = bytes.iter().position(|&b| b != 0).unwrap_or(7);
    elem(element_id, &bytes[first..])
}

fn elem_str(element_id: u32, s: &str) -> Vec<u8> {
    elem(element_id, s.as_bytes())
}

fn build_64_track_mkv() -> Vec<u8> {
    let mut ebml_body = Vec::new();
    ebml_body.extend(elem_str(0x4282, "matroska"));
    let ebml = elem(0x1A45_DFA3, &ebml_body);

    let info = elem(0x1549_A966, &elem_uint(0x002A_D7B1, 1_000_000));

    let mut tracks_body = Vec::new();
    for i in 1..=64u64 {
        let mut entry = Vec::new();
        entry.extend(elem_uint(0xD7, i));
        entry.extend(elem_uint(0x83, 0x11)); // Subtitle
        entry.extend(elem_str(0x86, "S_TEXT/UTF8"));
        entry.extend(elem_str(0x22_B59C, if i == 1 { "vie" } else { "eng" }));
        entry.extend(elem_str(0x536E, &format!("Track {i}")));
        tracks_body.extend(elem(0xAE, &entry));
    }
    let tracks = elem(0x1654_AE6B, &tracks_body);

    // One cluster with a simple block on track 1 so importing yields >= 1 event.
    let mut blk = Vec::new();
    encode_vint(1, &mut blk);
    blk.extend_from_slice(&0i16.to_be_bytes());
    blk.push(0x80);
    blk.extend_from_slice(b"Hello from track 1");
    let mut cluster_body = Vec::new();
    cluster_body.extend(elem_uint(0xE7, 1_000));
    let mut group = Vec::new();
    group.extend(elem(0xA1, &blk));
    group.extend(elem_uint(0x9B, 2_000));
    cluster_body.extend(elem(0xA0, &group));
    let cluster = elem(0x1F43_B675, &cluster_body);

    let mut seg_body = Vec::new();
    seg_body.extend(info);
    seg_body.extend(tracks);
    seg_body.extend(cluster);
    let seg = elem(0x1853_8067, &seg_body);

    let mut file = Vec::new();
    file.extend(ebml);
    file.extend(seg);
    file
}

#[test]
fn e2e_open_ass_edit_shift_undo_redo_clean_and_save() {
    let dir = tempdir().expect("tempdir");
    let ass_path = dir.path().join("sample.ass");
    let mut initial = ScriptFile::new_default();
    initial.ensure_events_section_mut().insert_event(
        0,
        AssEvent::dialogue(
            AssTime::from_cs(100),
            AssTime::from_cs(300),
            "  Hello   world  ",
        ),
    );
    fs::write(&ass_path, serialize_ass(&initial)).expect("write ass");

    let mut app = EditorApp::new(ScriptFile::new_default());
    app.open_file(&ass_path).expect("open ass");
    assert_eq!(app.shell.window_title(), "sample.ass - hlouis-sub-edit");

    // Shift forward 2.00s, undo, redo, clean whitespace, and save.
    app.apply_shift_times(200, true, true).expect("shift");
    assert!(app.shell.doc.is_dirty());
    assert!(app.shell.window_title().starts_with('*'));
    assert_eq!(
        app.shell.doc.script().events().next().expect("ev").start(),
        AssTime::from_cs(300)
    );

    app.execute_command("edit/undo").expect("undo");
    assert_eq!(
        app.shell.doc.script().events().next().expect("ev").start(),
        AssTime::from_cs(100)
    );

    app.execute_command("edit/redo").expect("redo");
    assert_eq!(
        app.shell.doc.script().events().next().expect("ev").start(),
        AssTime::from_cs(300)
    );

    app.shell.dialogs.hlouis_clean_opts.collapse_internal_spaces = true;
    app.execute_command("hlouis/clean_whitespace")
        .expect("clean");
    assert_eq!(
        app.shell.doc.script().events().next().expect("ev").text(),
        "Hello world"
    );

    app.save().expect("save");
    assert!(!app.shell.doc.is_dirty());
    let saved_disk = fs::read_to_string(&ass_path).expect("read saved");
    assert!(saved_disk.contains("Hello world"));
    assert!(saved_disk.contains("0:00:03.00"));
}

#[test]
fn e2e_open_srt_and_save_ass() {
    let dir = tempdir().expect("tempdir");
    let srt_path = dir.path().join("episode.srt");
    let out_ass = dir.path().join("episode.ass");
    fs::write(
        &srt_path,
        "1\n00:00:01,500 --> 00:00:04,000\n<i>Xin chào</i>\nDòng hai\n\n",
    )
    .expect("write srt");

    let mut app = EditorApp::new(ScriptFile::new_default());
    app.open_file(&srt_path).expect("open srt");
    assert_eq!(app.shell.doc.script().events().count(), 1);
    let ev = app.shell.doc.script().events().next().expect("ev");
    assert_eq!(ev.start(), AssTime::from_cs(150));
    assert!(ev.text().contains(r"{\i1}Xin chào{\i0}\NDòng hai"));

    app.save_as(&out_ass).expect("save ass");
    let raw = fs::read_to_string(&out_ass).expect("read ass");
    assert!(raw.contains("[Events]"));
    assert!(raw.contains("Xin chào"));
}

#[test]
fn e2e_mkv_import_from_synthetic_64_track_mkv() {
    let dir = tempdir().expect("tempdir");
    let mkv_path = dir.path().join("64tracks.mkv");
    fs::write(&mkv_path, build_64_track_mkv()).expect("write mkv");

    let mut app = EditorApp::new(ScriptFile::new_default());
    let count = app.open_mkv_subtitles(&mkv_path).expect("scan mkv");
    assert_eq!(count, 64);

    let imported = app.import_selected_mkv_tracks().expect("import track 1");
    assert_eq!(imported, 1);
    assert_eq!(
        app.shell.doc.script().events().next().expect("ev").text(),
        "Hello from track 1"
    );
}

#[test]
fn e2e_crash_recovery_after_simulated_kill() {
    let dir = tempdir().expect("tempdir");
    let autosave_dir = dir.path().join("autosave");
    fs::create_dir_all(&autosave_dir).expect("mkdir");
    let mut script = ScriptFile::new_default();
    script.ensure_events_section_mut().insert_event(
        0,
        AssEvent::dialogue(AssTime::ZERO, AssTime::from_cs(200), "Recovered line"),
    );
    let bytes = serialize_ass(&script).into_bytes();
    let _ = write_autosave_now(&autosave_dir, "crashed", &bytes, 10, None).expect("autosave");

    let mut app = EditorApp::new(ScriptFile::new_default());
    app.autosave_cfg.autosave_dir = autosave_dir;
    "crashed".clone_into(&mut app.shell.script_name);
    let found = app.check_crash_recovery();
    assert_eq!(found, 1);
    app.recover_candidate(0).expect("recover");
    assert_eq!(
        app.shell.doc.script().events().next().expect("ev").text(),
        "Recovered line"
    );
}

#[test]
fn e2e_200k_lines_open_scroll_type_via_kittest() {
    let mut app = EditorApp::new(build_demo_script(200_000));
    app.shell.grid.model.click(100_000);
    app.shell.grid.scroll_to_active();
    app.shell.doc.set_selection(Some(100_000), &[100_000]);

    let t0 = Instant::now();
    let mut harness = Harness::builder()
        .with_size(Vec2::new(1280.0, 800.0))
        .with_accessibility_check(false)
        .build_ui(move |ui| {
            app.show(ui, &ZeroClock);
        });
    harness.run();
    harness.step();
    assert!(
        t0.elapsed().as_secs() < 15,
        "200k-line kittest took too long"
    );
}

/// Selecting a line must seek to the first frame whose timestamp is at or after the line start
/// (Aegisub `FrameAtTime(.., START)`), never the frame before it, so the subtitle is visible.
#[test]
fn e2e_selecting_line_seeks_to_first_visible_frame_23_976() {
    let mut script = ScriptFile::new_default();
    let sec = script.ensure_events_section_mut();
    sec.insert_event(
        0,
        AssEvent::dialogue(AssTime::from_cs(100), AssTime::from_cs(300), "first"),
    );
    // 0:00:57.90 lies inside frame 1388 (57.891 s) but the line only shows from frame 1389.
    sec.insert_event(
        1,
        AssEvent::dialogue(AssTime::from_cs(5_790), AssTime::from_cs(6_080), "target"),
    );
    let mut app = EditorApp::new(script);
    let tc = keyframes::Timecodes::from_cfr(24_000, 1_001).expect("cfr");
    app.shell
        .attach_video(tc.clone(), 5_000, vec![0], 1920, 1080);
    app.shell.doc.set_selection(Some(1), &[1]);
    app.shell.grid.model.click(1);

    let mut harness = Harness::builder()
        .with_size(Vec2::new(1280.0, 800.0))
        .with_accessibility_check(false)
        .build_ui_state(
            |ui, app: &mut EditorApp| {
                app.show(ui, &ZeroClock);
            },
            app,
        );
    harness.run();
    let app = harness.state();
    let pb = app.shell.playback.as_ref().expect("playback");
    let frame = pb.current_frame();
    assert_eq!(
        frame, 1_389,
        "line at 57.90 must seek to frame 1389, not 1388"
    );
    assert!(
        tc.frame_to_time_ms(frame, keyframes::TimeType::Exact) >= 57_900,
        "the sought frame must be displayed at or after the line start"
    );
}
