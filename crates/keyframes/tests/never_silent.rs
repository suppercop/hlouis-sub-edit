//! Never-silent-error rule: inputs that would "succeed with 0 items" must return a typed error.

use keyframes::{parse_keyframes, KeyframeError};

#[test]
fn header_only_aegisub_file_is_an_error() {
    let err = parse_keyframes("# keyframe format v1\nfps 23.976\n").unwrap_err();
    assert!(matches!(err, KeyframeError::NoKeyframes { .. }), "{err:?}");
    assert!(err.to_string().contains("no keyframes"));
}

#[test]
fn xvid_log_without_intra_frames_is_an_error() {
    let err = parse_keyframes("# XviD 2pass stat file\np 1\np 2\n");
    assert!(err.is_err());
}

#[test]
fn x264_stats_without_idr_is_an_error() {
    let err = parse_keyframes("in:0 out:0 type:P\nin:1 out:1 type:P\n").unwrap_err();
    assert!(matches!(err, KeyframeError::NoKeyframes { .. }), "{err:?}");
}

#[test]
fn x264_bad_in_field_is_reported_with_line() {
    let err = parse_keyframes("in:0 out:0 type:I\nin:zz out:1 type:I\n").unwrap_err();
    assert!(
        matches!(err, KeyframeError::InvalidKeyframeLine { line: 2, .. }),
        "{err:?}"
    );
}

#[test]
fn non_empty_file_still_parses() {
    let (kf, _) = parse_keyframes("# keyframe format v1\nfps 24\n0\n48\n").unwrap();
    assert_eq!(kf, vec![0, 48]);
}
