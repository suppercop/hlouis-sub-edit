//! Core ASS/SRT subtitle data model, lossless parser/serializer, override-tag parser,
//! charset detection, and command-based undo/redo history.
//!
//! This crate has zero GUI or native FFI dependencies.

#![forbid(unsafe_code)]
#![cfg_attr(not(test), deny(clippy::unwrap_used, clippy::expect_used))]

pub mod charset;
pub mod history;
pub mod model;
pub mod parser;
pub mod serializer;
pub mod srt;
pub mod tags;
pub mod time;

use thiserror::Error;

pub use charset::{decode_bytes, DetectedEncoding};
pub use history::{EditCommand, UndoHistory, DEFAULT_MAX_HISTORY_BYTES};
pub use model::{
    AssEvent, AssStyle, EventEntry, EventKind, EventsSection, KeyValueEntry, KeyValueSection,
    LineEnding, RawSection, ScriptFile, Section, StyleEntry, StylesSection,
};
pub use parser::{parse_ass, parse_ass_bytes};
pub use serializer::{serialize_ass, serialize_ass_bytes};
pub use srt::{parse_srt, parse_srt_bytes, write_srt};
pub use tags::{
    parse_text_blocks, serialize_text_blocks, EscapeKind, KaraokeKind, OverrideItem, OverrideTag,
    TagArgs, TextBlock,
};
pub use time::AssTime;

/// Errors returned by `ass-core` operations.
#[derive(Debug, Error)]
pub enum AssError {
    #[error("event index {0} is out of range")]
    EventIndexOutOfRange(usize),
    #[error("style index {0} is out of range")]
    StyleIndexOutOfRange(usize),
    #[error("invalid event permutation of length {0}")]
    InvalidPermutation(usize),
    #[error("I/O error: {0}")]
    Io(#[from] std::io::Error),
}

/// Non-fatal diagnostic emitted when parsing malformed or unusual subtitle input.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ParseWarning {
    /// 1-based line number where the warning occurred.
    pub line: usize,
    /// Human-readable description of the issue.
    pub message: String,
}

/// Parsed document together with any non-fatal warnings encountered.
#[derive(Debug, Clone, PartialEq)]
pub struct ParseOutcome<T> {
    pub value: T,
    pub warnings: Vec<ParseWarning>,
}
