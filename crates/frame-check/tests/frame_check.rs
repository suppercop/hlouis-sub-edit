//! Frame-accuracy self-check on generated clips (no binaries committed): a synthetic CFR 23.976
//! timestamp table, a VFR table, and real Matroska files with raw video muxed in-process and
//! decoded through FFMS2.

#![allow(
    clippy::cast_possible_wrap,
    clippy::cast_sign_loss,
    clippy::cast_possible_truncation,
    clippy::unreadable_literal
)]

use ass_core::parse_ass;
use frame_check::{check_files, check_script, timecodes_from_pts, FrameCheckError, Verdict};
use keyframes::{TimeType, Timecodes};
use std::fmt::Write as _;

const HEADER: &str = "[Script Info]\nScriptType: v4.00+\nPlayResX: 64\nPlayResY: 48\n\n[V4+ Styles]\nFormat: Name, Fontname, Fontsize, PrimaryColour, SecondaryColour, OutlineColour, BackColour, Bold, Italic, Underline, StrikeOut, ScaleX, ScaleY, Spacing, Angle, BorderStyle, Outline, Shadow, Alignment, MarginL, MarginR, MarginV, Encoding\nStyle: Default,Arial,20,&H00FFFFFF,&H000000FF,&H00000000,&H00000000,0,0,0,0,100,100,0,0,1,1,0,5,2,2,2,1\n\n[Events]\nFormat: Layer, Start, End, Style, Name, MarginL, MarginR, MarginV, Effect, Text\n";

fn cfr_pts(n: usize) -> Vec<i64> {
    // 24000/1001 fps, millisecond-rounded like a Matroska/MP4 demuxer reports it.
    (0..n as i64)
        .map(|i| (i * 1_001_000 + 12_000) / 24_000)
        .collect()
}

fn vfr_pts() -> Vec<i64> {
    // 24 fps, then a 10 fps stretch, then 30 fps.
    let mut t = 0i64;
    let mut v = Vec::new();
    for step in [(60, 41), (30, 100), (90, 33)] {
        for _ in 0..step.0 {
            v.push(t);
            t += step.1;
        }
    }
    v
}

/// Builds a script whose event times come from our own frame<->time conversion.
fn script_for(tc: &Timecodes, ranges: &[(u32, u32)]) -> String {
    let mut s = HEADER.to_owned();
    for &(a, b) in ranges {
        let st = ass_core::AssTime::from_cs(tc.frame_to_time_cs(a, TimeType::Start));
        let en = ass_core::AssTime::from_cs(tc.frame_to_time_cs(b, TimeType::End));
        let _ = writeln!(
            s,
            "Dialogue: 0,{},{},Default,,0,0,0,,SIGN {a}-{b}\n",
            st.to_ass_string(),
            en.to_ass_string()
        );
    }
    s
}

const RANGES: &[(u32, u32)] = &[
    (0, 5),
    (10, 10),
    (11, 40),
    (41, 41),
    (59, 61),
    (100, 140),
    (150, 179),
];

#[test]
fn cfr_23976_is_frame_accurate() {
    let pts = cfr_pts(240);
    let tc = timecodes_from_pts(&pts).unwrap();
    let script = parse_ass(&script_for(&tc, RANGES)).value;
    let report = check_script(&script, &pts, &tc, 64, 48).unwrap();
    assert!(report.is_clean(), "{}", report.summary(20));
    assert_eq!(
        report.count(|v| *v == Verdict::Ok),
        RANGES.len(),
        "{}",
        report.summary(20)
    );
}

#[test]
fn vfr_is_frame_accurate() {
    let pts = vfr_pts();
    let tc = timecodes_from_pts(&pts).unwrap();
    let script = parse_ass(&script_for(&tc, RANGES)).value;
    let report = check_script(&script, &pts, &tc, 64, 48).unwrap();
    assert!(report.is_clean(), "{}", report.summary(20));
    assert_eq!(
        report.count(|v| *v == Verdict::Ok),
        RANGES.len(),
        "{}",
        report.summary(20)
    );
}

#[test]
fn wrong_timing_model_is_detected() {
    // Events computed for exact 24 fps but video is 23.976: libass disagrees at the boundaries.
    let pts = cfr_pts(3000);
    let wrong = Timecodes::from_cfr(24, 1).unwrap();
    let script = parse_ass(&script_for(&wrong, &[(2000, 2100), (2500, 2600)])).value;
    let tc = timecodes_from_pts(&pts).unwrap();
    // Check against the *wrong* model's expectation: build with the true model but shifted events.
    let report = check_script(&script, &pts, &tc, 64, 48).unwrap();
    // The true model computes its own expectation from the event times, so it still agrees with
    // libass; a broken model (24 fps table) must not.
    assert!(report.is_clean(), "{}", report.summary(20));
    let pts24: Vec<i64> = (0..3000).map(|i| i * 1000 / 24).collect();
    let bad = check_script(&script, &pts, &timecodes_from_pts(&pts24).unwrap(), 64, 48).unwrap();
    assert!(
        !bad.is_clean(),
        "a wrong timing model must be reported:\n{}",
        bad.summary(20)
    );
}

#[test]
fn degenerate_inputs_are_errors_not_empty_success() {
    let pts = cfr_pts(10);
    let tc = timecodes_from_pts(&pts).unwrap();
    let script = parse_ass(HEADER).value;
    assert!(matches!(
        check_script(&script, &pts, &tc, 64, 48),
        Err(FrameCheckError::NoEvents)
    ));
    assert!(matches!(
        timecodes_from_pts(&[]),
        Err(FrameCheckError::NoFrames)
    ));
}

// ---------- minimal in-process Matroska muxer (raw BGR24 video) ----------

fn vint(v: u64) -> Vec<u8> {
    for len in 1..=8usize {
        if v < (1u64 << (7 * len)) - 1 {
            let mut out = vec![0u8; len];
            let marked = v | (1u64 << (7 * len));
            for (i, b) in out.iter_mut().enumerate() {
                *b = ((marked >> (8 * (len - 1 - i))) & 0xFF) as u8;
            }
            return out;
        }
    }
    unreachable!()
}

fn id_bytes(id: u32) -> Vec<u8> {
    let b = id.to_be_bytes();
    let skip = b.iter().position(|&x| x != 0).unwrap_or(3);
    b[skip..].to_vec()
}

fn elem(id: u32, payload: &[u8]) -> Vec<u8> {
    let mut out = id_bytes(id);
    out.extend(vint(payload.len() as u64));
    out.extend_from_slice(payload);
    out
}

fn uint(id: u32, v: u64) -> Vec<u8> {
    let bytes = v.to_be_bytes();
    let skip = bytes.iter().position(|&x| x != 0).unwrap_or(7);
    elem(id, &bytes[skip..])
}

fn build_mkv(pts: &[i64], w: u32, h: u32) -> Vec<u8> {
    let ebml = elem(
        0x1A45_DFA3,
        &[
            uint(0x4286, 1),
            uint(0x42F7, 1),
            uint(0x42F2, 4),
            uint(0x42F3, 8),
            elem(0x4282, b"matroska"),
            uint(0x4287, 4),
            uint(0x4285, 2),
        ]
        .concat(),
    );
    let mut bih = Vec::new();
    bih.extend(40u32.to_le_bytes());
    bih.extend(w.to_le_bytes());
    bih.extend((h as i32).to_le_bytes());
    bih.extend(1u16.to_le_bytes());
    bih.extend(24u16.to_le_bytes());
    bih.extend(0u32.to_le_bytes()); // BI_RGB
    bih.extend((w * h * 3).to_le_bytes());
    bih.extend([0u8; 16]);
    let video = elem(
        0xE0,
        &[uint(0xB0, u64::from(w)), uint(0xBA, u64::from(h))].concat(),
    );
    let track = elem(
        0xAE,
        &[
            uint(0xD7, 1),
            uint(0x73C5, 1),
            uint(0x83, 1),
            elem(0x86, b"V_MS/VFW/FOURCC"),
            elem(0x63A2, &bih),
            video,
        ]
        .concat(),
    );
    let info = elem(0x1549_A966, &uint(0x2AD7B1, 1_000_000));
    let tracks = elem(0x1654_AE6B, &track);
    let frame = vec![0x40u8; (w * h * 3) as usize];
    let mut clusters = Vec::new();
    for &t in pts {
        let mut block = vint(1);
        block.extend(0i16.to_be_bytes());
        block.push(0x80); // keyframe
        block.extend_from_slice(&frame);
        clusters.extend(elem(
            0x1F43_B675,
            &[uint(0xE7, t as u64), elem(0xA3, &block)].concat(),
        ));
    }
    let segment = elem(0x1853_8067, &[info, tracks, clusters].concat());
    [ebml, segment].concat()
}

fn write_clip(name: &str, pts: &[i64]) -> std::path::PathBuf {
    let dir = std::env::temp_dir().join(format!("frame_check_{}", std::process::id()));
    std::fs::create_dir_all(&dir).unwrap();
    let path = dir.join(name);
    std::fs::write(&path, build_mkv(pts, 64, 48)).unwrap();
    path
}

fn real_clip_check(name: &str, pts: &[i64]) {
    let video = write_clip(name, pts);
    let tc = timecodes_from_pts(pts).unwrap();
    let ass = video.with_extension("ass");
    std::fs::write(&ass, script_for(&tc, RANGES)).unwrap();
    let report = check_files(&video, &ass).unwrap();
    assert_eq!(report.frames, pts.len());
    assert!(report.is_clean(), "{}", report.summary(20));
    assert_eq!(
        report.count(|v| *v == Verdict::Ok),
        RANGES.len(),
        "{}",
        report.summary(20)
    );
    let _ = std::fs::remove_dir_all(video.parent().unwrap());
}

#[test]
fn generated_cfr_mkv_through_ffms2() {
    real_clip_check("cfr.mkv", &cfr_pts(240));
}

#[test]
fn generated_vfr_mkv_through_ffms2() {
    real_clip_check("vfr.mkv", &vfr_pts());
}
