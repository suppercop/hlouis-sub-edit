//! Fixture and unit tests for `crates/keyframes`.

use keyframes::{
    parse_keyframes, parse_timecodes, write_aegisub_keyframes, write_plain_keyframes,
    KeyframeFormat, TimeType, Timecodes,
};
use std::path::Path;

const FIXTURES: &str = concat!(env!("CARGO_MANIFEST_DIR"), "/tests/fixtures");

#[test]
fn parses_all_four_keyframe_formats_and_round_trips() {
    let aeg = std::fs::read_to_string(Path::new(FIXTURES).join("aegisub_v1.txt")).expect("aeg");
    let (kfs, fmt) = parse_keyframes(&aeg).expect("parse aeg");
    assert_eq!(fmt, KeyframeFormat::AegisubV1);
    assert_eq!(kfs, vec![0, 48, 120, 240]);

    let rewritten = write_aegisub_keyframes(&kfs, Some(23.976));
    let (kfs_rt, fmt_rt) = parse_keyframes(&rewritten).expect("rt aeg");
    assert_eq!(fmt_rt, KeyframeFormat::AegisubV1);
    assert_eq!(kfs_rt, kfs);

    let xvid = std::fs::read_to_string(Path::new(FIXTURES).join("xvid_pass1.pass")).expect("xvid");
    let (kfs_xvid, fmt_xvid) = parse_keyframes(&xvid).expect("parse xvid");
    assert_eq!(fmt_xvid, KeyframeFormat::XvidPassLog);
    assert_eq!(kfs_xvid, vec![0, 4]);

    let x264 = std::fs::read_to_string(Path::new(FIXTURES).join("x264_stats.log")).expect("x264");
    let (kfs_x264, fmt_x264) = parse_keyframes(&x264).expect("parse x264");
    assert_eq!(fmt_x264, KeyframeFormat::X264Stats);
    assert_eq!(kfs_x264, vec![0, 3, 4]);

    let plain = std::fs::read_to_string(Path::new(FIXTURES).join("plain_wwxd.txt")).expect("plain");
    let (kfs_plain, fmt_plain) = parse_keyframes(&plain).expect("parse plain");
    assert_eq!(fmt_plain, KeyframeFormat::PlainFrameList);
    assert_eq!(kfs_plain, vec![0, 24, 72, 192]);

    let plain_rt = write_plain_keyframes(&kfs_plain);
    let (kfs_plain2, fmt_plain2) = parse_keyframes(&plain_rt).expect("rt plain");
    assert_eq!(fmt_plain2, KeyframeFormat::PlainFrameList);
    assert_eq!(kfs_plain2, kfs_plain);
}

#[test]
fn cfr_24000_1001_aegisub_start_end_semantics_round_trip() {
    let tc = Timecodes::from_cfr(24_000, 1_001).expect("cfr");
    assert_eq!(tc.frame_to_time_ms(0, TimeType::Exact), 0);
    assert_eq!(tc.frame_to_time_ms(1, TimeType::Exact), 42);
    assert_eq!(tc.frame_to_time_ms(24, TimeType::Exact), 1001);

    // Start of frame 10 -> midpoint between frame 9 (375 ms) and frame 10 (417 ms) = 396 ms -> 40 cs.
    let start_cs = tc.frame_to_time_cs(10, TimeType::Start);
    assert_eq!(start_cs, 40);
    assert_eq!(tc.time_cs_to_frame(start_cs, TimeType::Start), 10);

    // End of frame 20 (inclusive) -> midpoint between frame 20 (834 ms) and frame 21 (876 ms) = 855 ms -> 86 cs.
    let end_cs = tc.frame_to_time_cs(20, TimeType::End);
    assert_eq!(end_cs, 86);
    assert_eq!(tc.time_cs_to_frame(end_cs, TimeType::End), 20);
}

#[test]
fn parses_and_writes_mkvmerge_timecodes_v1_and_v2() {
    let v1_raw = std::fs::read_to_string(Path::new(FIXTURES).join("timecodes_v1.txt")).expect("v1");
    let tc_v1 = parse_timecodes(&v1_raw).expect("parse v1");
    // Frame 0..23 at 29.97 fps (~33.367 ms/f) -> frame 24 starts at ~801 ms.
    let f24 = tc_v1.frame_to_time_ms(24, TimeType::Exact);
    assert!((f24 - 801).abs() <= 1, "f24={f24}");
    // Frame 24..47 at 23.976 fps (~41.708 ms/f) -> frame 48 starts at ~1802 ms.
    let f48 = tc_v1.frame_to_time_ms(48, TimeType::Exact);
    assert!((f48 - 1802).abs() <= 2, "f48={f48}");

    let v1_written = tc_v1.write_v1();
    assert!(v1_written.starts_with("# timecode format v1\n"));

    let v2_raw = std::fs::read_to_string(Path::new(FIXTURES).join("timecodes_v2.txt")).expect("v2");
    let tc_v2 = parse_timecodes(&v2_raw).expect("parse v2");
    assert_eq!(tc_v2.frame_to_time_ms(2, TimeType::Exact), 83);
    assert_eq!(tc_v2.time_ms_to_frame(100, TimeType::Start), 3);
    assert_eq!(tc_v2.time_ms_to_frame(100, TimeType::End), 2);

    let v2_written = tc_v2.write_v2(5);
    let tc_v2_rt = parse_timecodes(&v2_written).expect("rt v2");
    assert_eq!(tc_v2_rt, tc_v2);
}
