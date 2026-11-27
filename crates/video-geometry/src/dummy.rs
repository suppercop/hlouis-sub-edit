//! Aegisub 3.5.1 Dummy Video provider (`src/video_provider_dummy.cpp`).
//!
//! Parses and formats `?dummy:<fps>:<frames>:<width>:<height>:<r>:<g>:<b>:<c|>` URIs and fills
//! caller-supplied RGBA8 buffers at `memcpy` speed (< 1 ms for 1080p) using two L1-cached
//! scanline templates for checkerboard mode.

use std::fmt::Write as _;
use thiserror::Error;

/// Errors returned by dummy video parsing/rendering.
#[derive(Debug, Error, PartialEq, Eq)]
pub enum DummyVideoError {
    /// Invalid `?dummy:...` URI string.
    #[error("invalid ?dummy URI: {0}")]
    InvalidUri(String),
    /// Output RGBA buffer was smaller than `width * height * 4`.
    #[error("rgba buffer too small: got {actual} bytes, need {required}")]
    BufferTooSmall { actual: usize, required: usize },
}

/// Configuration for Aegisub's Dummy Video generator.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct DummyVideoConfig {
    pub fps: f64,
    pub frames: u32,
    pub width: u32,
    pub height: u32,
    pub red: u8,
    pub green: u8,
    pub blue: u8,
    pub checkerboard: bool,
}

impl DummyVideoConfig {
    /// Parses an Aegisub `?dummy:fps:frames:width:height:r:g:b:[c]` URI.
    ///
    /// # Errors
    /// Returns [`DummyVideoError::InvalidUri`] if the URI is malformed or dimensions are zero.
    pub fn parse_uri(uri: &str) -> Result<Self, DummyVideoError> {
        let rest = uri
            .strip_prefix("?dummy:")
            .ok_or_else(|| DummyVideoError::InvalidUri(uri.to_owned()))?;
        let parts: Vec<&str> = rest.split(':').collect();
        if parts.len() < 7 || parts.len() > 8 {
            return Err(DummyVideoError::InvalidUri(uri.to_owned()));
        }
        let fps: f64 = parts[0]
            .parse()
            .map_err(|_| DummyVideoError::InvalidUri(uri.to_owned()))?;
        let frames: u32 = parts[1]
            .parse()
            .map_err(|_| DummyVideoError::InvalidUri(uri.to_owned()))?;
        let width: u32 = parts[2]
            .parse()
            .map_err(|_| DummyVideoError::InvalidUri(uri.to_owned()))?;
        let height: u32 = parts[3]
            .parse()
            .map_err(|_| DummyVideoError::InvalidUri(uri.to_owned()))?;
        let red: u8 = parts[4]
            .parse()
            .map_err(|_| DummyVideoError::InvalidUri(uri.to_owned()))?;
        let green: u8 = parts[5]
            .parse()
            .map_err(|_| DummyVideoError::InvalidUri(uri.to_owned()))?;
        let blue: u8 = parts[6]
            .parse()
            .map_err(|_| DummyVideoError::InvalidUri(uri.to_owned()))?;
        let checkerboard = parts.get(7).is_some_and(|s| *s == "c" || *s == "C");

        if !fps.is_finite() || fps <= 0.0 || width == 0 || height == 0 {
            return Err(DummyVideoError::InvalidUri(uri.to_owned()));
        }

        Ok(Self {
            fps,
            frames,
            width,
            height,
            red,
            green,
            blue,
            checkerboard,
        })
    }

    /// Formats this configuration into an Aegisub `?dummy:...` URI string.
    #[must_use]
    pub fn to_uri(&self) -> String {
        let mut out = String::with_capacity(64);
        let flag = if self.checkerboard { "c" } else { "" };
        let _ = write!(
            out,
            "?dummy:{:.6}:{}:{}:{}:{}:{}:{}:{flag}",
            self.fps, self.frames, self.width, self.height, self.red, self.green, self.blue
        );
        out
    }

    /// Required RGBA buffer byte length (`width * height * 4`).
    #[must_use]
    pub const fn rgba_byte_len(&self) -> usize {
        (self.width as usize) * (self.height as usize) * 4
    }
}

/// Renders dummy video frame `frame_index` into caller-provided `out_rgba` (`width * height * 4`
/// bytes) using L1-cached scanline templates so 1080p fills run at memcpy speed (< 1 ms).
///
/// # Errors
/// Returns [`DummyVideoError::BufferTooSmall`] if `out_rgba.len() < cfg.rgba_byte_len()`.
pub fn render_dummy_frame_rgba(
    cfg: &DummyVideoConfig,
    _frame_index: u32,
    out_rgba: &mut [u8],
) -> Result<(), DummyVideoError> {
    let w = cfg.width as usize;
    let h = cfg.height as usize;
    let row_bytes = w * 4;
    let required = row_bytes * h;
    if out_rgba.len() < required {
        return Err(DummyVideoError::BufferTooSmall {
            actual: out_rgba.len(),
            required,
        });
    }
    if required == 0 {
        return Ok(());
    }

    let c1 = [cfg.red, cfg.green, cfg.blue, 255];

    if !cfg.checkerboard {
        // Fill first scanline once, then memcpy to all remaining rows.
        let (first_row, rest) = out_rgba[..required].split_at_mut(row_bytes);
        for px in first_row.as_chunks_mut::<4>().0 {
            *px = c1;
        }
        for row in rest.chunks_exact_mut(row_bytes) {
            row.copy_from_slice(first_row);
        }
        return Ok(());
    }

    // Aegisub checkerboard: 8x8 tiles alternating base colour with a slightly darkened/lightened tone.
    let c2 = [
        shift_checker(cfg.red),
        shift_checker(cfg.green),
        shift_checker(cfg.blue),
        255,
    ];

    let mut row_a = vec![0u8; row_bytes];
    let mut row_b = vec![0u8; row_bytes];
    for (x, (pa, pb)) in row_a
        .as_chunks_mut::<4>()
        .0
        .iter_mut()
        .zip(row_b.as_chunks_mut::<4>().0.iter_mut())
        .enumerate()
    {
        if (x & 8) == 0 {
            *pa = c1;
            *pb = c2;
        } else {
            *pa = c2;
            *pb = c1;
        }
    }

    for (y, row) in out_rgba[..required].chunks_exact_mut(row_bytes).enumerate() {
        if (y & 8) == 0 {
            row.copy_from_slice(&row_a);
        } else {
            row.copy_from_slice(&row_b);
        }
    }

    Ok(())
}

#[inline]
const fn shift_checker(v: u8) -> u8 {
    if v >= 24 {
        v - 24
    } else {
        v + 24
    }
}
