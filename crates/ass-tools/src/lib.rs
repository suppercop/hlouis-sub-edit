//! Pure-logic subtitle editing tools matching Aegisub 3.5.1 semantics.
//!
//! All mutating tools return `ass_core::EditCommand` batches so every operation is undoable
//! as a single atomic step through `ass_core::UndoHistory`.

#![forbid(unsafe_code)]
#![cfg_attr(not(test), deny(clippy::unwrap_used, clippy::expect_used))]

pub mod find_replace;
pub mod join_split;
pub mod select;
pub mod shift;
pub mod tpp;

use thiserror::Error;

pub use find_replace::{find_and_replace, FindReplaceOptions, SearchField, SearchScope};
pub use join_split::{join_lines, split_line, JoinMode, SplitMode};
pub use select::{select_lines, MatchMode, SelectField, SelectLinesOptions, SelectionAction};
pub use shift::{shift_times, ShiftAffect, ShiftAmount, ShiftDirection, ShiftOptions, ShiftTarget};
pub use tpp::{timing_post_process, AdjacentLinkConfig, KeyframeSnapConfig, TppOptions};

/// Errors returned by `ass-tools` operations.
#[derive(Debug, Error)]
pub enum ToolsError {
    #[error("invalid regular expression: {0}")]
    InvalidRegex(#[from] regex::Error),
    #[error("invalid event index: {0}")]
    InvalidEventIndex(usize),
    #[error("operation requires at least two selected lines")]
    InsufficientLines,
    #[error("frame-time table cannot be empty when shifting by frames")]
    EmptyFrameTable,
}
