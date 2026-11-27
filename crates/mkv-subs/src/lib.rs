//! Pure-Rust EBML / Matroska subtitle track lister and streaming extractor.
//!
//! Designed with zero C dependencies, zero `unsafe`, no 32-track limit, and O(1) seek-skipping
//! over non-selected video/audio blocks.

#![forbid(unsafe_code)]
#![cfg_attr(not(test), deny(clippy::unwrap_used, clippy::expect_used))]

pub mod ebml;
pub mod extract;
pub mod tracks;

use thiserror::Error;

pub use extract::{extract_tracks, preview_track, ExtractedTrack};
pub use tracks::{
    list_tracks, CompressionAlgo, ContentEncodingInfo, MkvMetadata, SubtitleCodec, TrackInfo,
    TrackType,
};

/// Errors returned by `mkv-subs`.
#[derive(Debug, Error)]
pub enum MkvError {
    #[error("I/O error: {0}")]
    Io(#[from] std::io::Error),
    #[error("not a valid EBML/Matroska file: {0}")]
    InvalidFormat(String),
    #[error("malformed EBML element: {0}")]
    MalformedEbml(String),
    #[error("track {0} not found in Matroska container")]
    TrackNotFound(u64),
    #[error("track {track} uses unsupported subtitle codec `{codec}` (image subtitles cannot be converted to text)")]
    UnsupportedCodec { track: u64, codec: String },
    #[error("track {track} produced 0 subtitle events: {reason}")]
    EmptyTrack { track: u64, reason: String },
    #[error("zlib decompression failed on track {track}: {source}")]
    Decompress {
        track: u64,
        #[source]
        source: std::io::Error,
    },
    #[error("extraction was cancelled")]
    Cancelled,
}
