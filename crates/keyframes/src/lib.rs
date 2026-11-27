//! Pure-Rust keyframe file I/O (Aegisub v1, `XviD` pass-1, x264/x265 stats, plain Lavf/WWXD lists)
//! and mkvmerge v1/v2 CFR/VFR timecode conversion with Aegisub 3.5.1 start/end rounding semantics.

#![forbid(unsafe_code)]
#![cfg_attr(not(test), deny(clippy::unwrap_used, clippy::expect_used))]

pub mod keyframe_io;
pub mod timecodes;

use thiserror::Error;

pub use keyframe_io::{
    parse_keyframes, write_aegisub_keyframes, write_plain_keyframes, KeyframeFormat,
};
pub use timecodes::{parse_timecodes, TimeType, Timecodes};

/// Errors returned by `keyframes` parsers and converters.
#[derive(Debug, Error)]
pub enum KeyframeError {
    #[error("unrecognized or empty keyframe file")]
    UnrecognizedFormat,
    #[error("invalid keyframe line {line}: {message}")]
    InvalidKeyframeLine { line: usize, message: String },
    #[error("invalid timecode file: {0}")]
    InvalidTimecodes(String),
    #[error("invalid framerate {0}/{1}")]
    InvalidFramerate(u64, u64),
}
