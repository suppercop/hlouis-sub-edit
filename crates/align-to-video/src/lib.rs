//! Aegisub 3.5.1 "Align subtitle to video" flood-fill region detector (`src/dialog_align.cpp` /
//! region bounding-box alignment to `\an1..=9` `\pos(x,y)`).
//!
//! Uses a fast scanline flood-fill over a 1-bit-per-pixel bitset (259 KB for 1080p, L2-resident)
//! so 300×200 regions on 1080p RGBA frames finish well under 1 ms.

#![forbid(unsafe_code)]
#![cfg_attr(not(test), deny(clippy::unwrap_used, clippy::expect_used))]

use ass_core::{AssEvent, EditCommand};
use std::fmt::Write as _;
use thiserror::Error;

/// Errors returned by `align-to-video`.
#[derive(Debug, Error, PartialEq, Eq)]
pub enum AlignError {
    /// Invalid frame dimensions or stride.
    #[error("invalid frame {width}x{height} (stride {stride_bytes} bytes)")]
    InvalidFrame {
        width: usize,
        height: usize,
        stride_bytes: usize,
    },
    /// RGBA buffer too small for height and stride.
    #[error("rgba buffer too short: got {actual} bytes, need at least {required}")]
    BufferTooShort { actual: usize, required: usize },
    /// Click point outside frame bounds.
    #[error("seed point ({x}, {y}) outside {width}x{height}")]
    SeedOutOfBounds {
        x: usize,
        y: usize,
        width: usize,
        height: usize,
    },
    /// Invalid ASS alignment `\an` (expected `1..=9`).
    #[error("invalid alignment {0} (expected 1..=9)")]
    InvalidAlignment(u8),
}

/// Colour matching metric for flood-fill tolerance.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ColorMetric {
    /// `max(|dr|, |dg|, |db|) <= tolerance` (fast, default).
    MaxChannel,
    /// `|dr| + |dg| + |db| <= tolerance * 3`.
    Manhattan,
    /// `dr^2 + dg^2 + db^2 <= tolerance^2 * 3`.
    Euclidean,
}

/// Options for [`align_to_video`].
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct AlignOptions {
    /// Target RGB colour; `None` samples the pixel at the click seed `(seed_x, seed_y)`.
    pub target_rgb: Option<[u8; 3]>,
    /// Per-channel colour tolerance (`0..=255`).
    pub tolerance: u8,
    /// Distance metric.
    pub metric: ColorMetric,
    /// ASS alignment `\an1..=9` determining which point of the bounding box becomes `\pos(x,y)`.
    pub alignment: u8,
    /// Script `PlayResX` / `PlayResY` used to map video pixels to script coordinates.
    pub play_res_x: f64,
    pub play_res_y: f64,
    /// Optional safety cap on visited pixels (`0` = unlimited).
    pub max_pixels: usize,
}

impl Default for AlignOptions {
    fn default() -> Self {
        Self {
            target_rgb: None,
            tolerance: 16,
            metric: ColorMetric::MaxChannel,
            alignment: 5,
            play_res_x: 1920.0,
            play_res_y: 1080.0,
            max_pixels: 0,
        }
    }
}

/// Bounding box of the flood-filled video region (inclusive pixel coordinates).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct RegionBBox {
    pub min_x: usize,
    pub min_y: usize,
    pub max_x: usize,
    pub max_y: usize,
    pub pixel_count: usize,
}

/// Result of [`flood_fill_region`] + `\pos` alignment.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct AlignResult {
    pub bbox: RegionBBox,
    /// Aligned position in video pixels.
    pub video_pos: (f64, f64),
    /// Aligned position in script coordinates (`PlayResX` × `PlayResY`).
    pub script_pos: (f64, f64),
}

/// Reusable scratch buffers for zero-allocation repeated flood fills (e.g. while dragging a
/// tolerance slider in the Align dialog).
#[derive(Debug, Default, Clone)]
pub struct AlignScratch {
    visited_bits: Vec<u64>,
    stack: Vec<(usize, usize)>,
}

impl AlignScratch {
    /// Creates an empty scratch buffer.
    #[must_use]
    pub fn new() -> Self {
        Self::default()
    }
}

/// Runs a scanline flood-fill from `(seed_x, seed_y)` on an RGBA8 frame and computes the aligned
/// `\pos` in video and script coordinates.
///
/// # Errors
/// Returns [`AlignError`] if frame dimensions, stride, buffer size, seed, or alignment are invalid.
#[allow(clippy::too_many_arguments)]
pub fn align_to_video(
    rgba: &[u8],
    width: usize,
    height: usize,
    stride_bytes: usize,
    seed_x: usize,
    seed_y: usize,
    options: AlignOptions,
    scratch: &mut AlignScratch,
) -> Result<AlignResult, AlignError> {
    if !(1..=9).contains(&options.alignment) {
        return Err(AlignError::InvalidAlignment(options.alignment));
    }
    let bbox = flood_fill_region(
        rgba,
        width,
        height,
        stride_bytes,
        seed_x,
        seed_y,
        options.target_rgb,
        options.tolerance,
        options.metric,
        options.max_pixels,
        scratch,
    )?;

    #[allow(clippy::cast_precision_loss)]
    let (x0, y0, x1, y1) = (
        bbox.min_x as f64,
        bbox.min_y as f64,
        (bbox.max_x + 1) as f64,
        (bbox.max_y + 1) as f64,
    );

    let vx = match options.alignment {
        1 | 4 | 7 => x0,
        3 | 6 | 9 => x1,
        _ => f64::midpoint(x0, x1),
    };
    let vy = match options.alignment {
        7..=9 => y0,
        4..=6 => f64::midpoint(y0, y1),
        _ => y1,
    };

    #[allow(clippy::cast_precision_loss)]
    let sx = ((vx * options.play_res_x / (width as f64)) * 100.0).round() / 100.0;
    #[allow(clippy::cast_precision_loss)]
    let sy = ((vy * options.play_res_y / (height as f64)) * 100.0).round() / 100.0;

    Ok(AlignResult {
        bbox,
        video_pos: (vx, vy),
        script_pos: (sx, sy),
    })
}

/// Builds an atomic [`EditCommand::ModifyEventText`] applying `\pos(x,y)` (and `\an` if requested)
/// to `event`.
#[must_use]
pub fn build_align_edit_command(
    line_index: usize,
    event: &AssEvent,
    result: AlignResult,
    set_an_tag: Option<u8>,
) -> Option<EditCommand> {
    let mut tag = String::with_capacity(32);
    if let Some(an) = set_an_tag {
        let _ = write!(tag, "\\an{an}");
    }
    let _ = write!(
        tag,
        "\\pos({},{})",
        result.script_pos.0, result.script_pos.1
    );

    let text = event.text();
    let after = if let Some(rest) = text.strip_prefix('{') {
        if let Some(close) = rest.find('}') {
            let mut out = String::with_capacity(text.len() + tag.len());
            out.push('{');
            out.push_str(&rest[..close]);
            out.push_str(&tag);
            out.push_str(&rest[close..]);
            out
        } else {
            format!("{{{tag}}}{text}")
        }
    } else {
        format!("{{{tag}}}{text}")
    };

    if after == text {
        return None;
    }
    Some(EditCommand::ModifyEventText {
        index: line_index,
        before: text.to_owned(),
        after,
    })
}

/// Fast scanline flood-fill over an RGBA8 frame using a reusable 1-bit-per-pixel bitset.
///
/// # Errors
/// Returns [`AlignError`] if frame geometry, buffer length, or seed coordinates are invalid.
#[allow(clippy::too_many_arguments, clippy::too_many_lines)]
pub fn flood_fill_region(
    rgba: &[u8],
    width: usize,
    height: usize,
    stride_bytes: usize,
    seed_x: usize,
    seed_y: usize,
    target_rgb: Option<[u8; 3]>,
    tolerance: u8,
    metric: ColorMetric,
    max_pixels: usize,
    scratch: &mut AlignScratch,
) -> Result<RegionBBox, AlignError> {
    if width == 0 || height == 0 || stride_bytes < width * 4 {
        return Err(AlignError::InvalidFrame {
            width,
            height,
            stride_bytes,
        });
    }
    let required = (height - 1) * stride_bytes + width * 4;
    if rgba.len() < required {
        return Err(AlignError::BufferTooShort {
            actual: rgba.len(),
            required,
        });
    }
    if seed_x >= width || seed_y >= height {
        return Err(AlignError::SeedOutOfBounds {
            x: seed_x,
            y: seed_y,
            width,
            height,
        });
    }

    let seed_off = seed_y * stride_bytes + seed_x * 4;
    let target = target_rgb.unwrap_or([rgba[seed_off], rgba[seed_off + 1], rgba[seed_off + 2]]);
    let tol = i32::from(tolerance);
    let tol_man = tol * 3;
    let tol_euc_sq = tol * tol * 3;

    let total_px = width * height;
    let words = total_px.div_ceil(64);
    if scratch.visited_bits.len() < words {
        scratch.visited_bits.resize(words, 0);
    } else {
        scratch.visited_bits[..words].fill(0);
    }
    scratch.stack.clear();

    if !pixel_matches(rgba, seed_off, target, tol, tol_man, tol_euc_sq, metric) {
        return Ok(RegionBBox {
            min_x: seed_x,
            min_y: seed_y,
            max_x: seed_x,
            max_y: seed_y,
            pixel_count: 0,
        });
    }

    let mut min_x = seed_x;
    let mut max_x = seed_x;
    let mut min_y = seed_y;
    let mut max_y = seed_y;
    let mut count = 0usize;
    let cap = if max_pixels == 0 {
        usize::MAX
    } else {
        max_pixels
    };

    scratch.stack.push((seed_x, seed_y));

    while let Some((sx, y)) = scratch.stack.pop() {
        if count >= cap {
            break;
        }
        let row_base = y * stride_bytes;
        let bit_row = y * width;

        // Expand left along scanline
        let mut lx = sx;
        while lx > 0
            && !is_visited(&scratch.visited_bits, bit_row + lx - 1)
            && pixel_matches(
                rgba,
                row_base + (lx - 1) * 4,
                target,
                tol,
                tol_man,
                tol_euc_sq,
                metric,
            )
        {
            lx -= 1;
        }

        // Expand right along scanline
        let mut rx = sx;
        while rx < width
            && !is_visited(&scratch.visited_bits, bit_row + rx)
            && pixel_matches(
                rgba,
                row_base + rx * 4,
                target,
                tol,
                tol_man,
                tol_euc_sq,
                metric,
            )
        {
            mark_visited(&mut scratch.visited_bits, bit_row + rx);
            count += 1;
            rx += 1;
        }

        // Also mark [lx..sx)
        for x in lx..sx {
            if !is_visited(&scratch.visited_bits, bit_row + x) {
                mark_visited(&mut scratch.visited_bits, bit_row + x);
                count += 1;
            }
        }

        if rx <= lx {
            continue;
        }
        let span_max_x = rx - 1;
        if lx < min_x {
            min_x = lx;
        }
        if span_max_x > max_x {
            max_x = span_max_x;
        }
        if y < min_y {
            min_y = y;
        }
        if y > max_y {
            max_y = y;
        }

        // Enqueue contiguous matching runs on `y - 1` and `y + 1`.
        if y > 0 {
            enqueue_neighbor_runs(
                rgba,
                width,
                stride_bytes,
                y - 1,
                lx,
                rx,
                target,
                tol,
                tol_man,
                tol_euc_sq,
                metric,
                &scratch.visited_bits,
                &mut scratch.stack,
            );
        }
        if y + 1 < height {
            enqueue_neighbor_runs(
                rgba,
                width,
                stride_bytes,
                y + 1,
                lx,
                rx,
                target,
                tol,
                tol_man,
                tol_euc_sq,
                metric,
                &scratch.visited_bits,
                &mut scratch.stack,
            );
        }
    }

    Ok(RegionBBox {
        min_x,
        min_y,
        max_x,
        max_y,
        pixel_count: count,
    })
}

#[inline]
fn is_visited(bits: &[u64], idx: usize) -> bool {
    (bits[idx >> 6] & (1u64 << (idx & 63))) != 0
}

#[inline]
fn mark_visited(bits: &mut [u64], idx: usize) {
    bits[idx >> 6] |= 1u64 << (idx & 63);
}

#[inline]
fn pixel_matches(
    rgba: &[u8],
    off: usize,
    target: [u8; 3],
    tol: i32,
    tol_man: i32,
    tol_euc_sq: i32,
    metric: ColorMetric,
) -> bool {
    let dr = (i32::from(rgba[off]) - i32::from(target[0])).abs();
    let dg = (i32::from(rgba[off + 1]) - i32::from(target[1])).abs();
    let db = (i32::from(rgba[off + 2]) - i32::from(target[2])).abs();
    match metric {
        ColorMetric::MaxChannel => dr <= tol && dg <= tol && db <= tol,
        ColorMetric::Manhattan => dr + dg + db <= tol_man,
        ColorMetric::Euclidean => dr * dr + dg * dg + db * db <= tol_euc_sq,
    }
}

#[allow(clippy::too_many_arguments)]
fn enqueue_neighbor_runs(
    rgba: &[u8],
    width: usize,
    stride_bytes: usize,
    ny: usize,
    lx: usize,
    rx: usize,
    target: [u8; 3],
    tol: i32,
    tol_man: i32,
    tol_euc_sq: i32,
    metric: ColorMetric,
    visited: &[u64],
    stack: &mut Vec<(usize, usize)>,
) {
    let row_base = ny * stride_bytes;
    let bit_row = ny * width;
    let mut in_run = false;
    for x in lx..rx {
        let matches = !is_visited(visited, bit_row + x)
            && pixel_matches(
                rgba,
                row_base + x * 4,
                target,
                tol,
                tol_man,
                tol_euc_sq,
                metric,
            );
        if matches && !in_run {
            stack.push((x, ny));
            in_run = true;
        } else if !matches {
            in_run = false;
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use ass_core::AssTime;

    #[test]
    fn flood_fills_sign_region_and_aligns_pos_for_an() {
        let w = 200usize;
        let h = 100usize;
        let mut rgba = vec![0u8; w * h * 4];
        // Draw a white 60x40 sign rectangle at (50..110, 20..60)
        for y in 20..60 {
            for x in 50..110 {
                let off = (y * w + x) * 4;
                rgba[off] = 240;
                rgba[off + 1] = 242;
                rgba[off + 2] = 238;
                rgba[off + 3] = 255;
            }
        }

        let mut scratch = AlignScratch::new();
        let res = align_to_video(
            &rgba,
            w,
            h,
            w * 4,
            75,
            35,
            AlignOptions {
                tolerance: 10,
                alignment: 5,
                play_res_x: 1920.0,
                play_res_y: 1080.0,
                ..AlignOptions::default()
            },
            &mut scratch,
        )
        .expect("align");

        assert_eq!(res.bbox.min_x, 50);
        assert_eq!(res.bbox.max_x, 109);
        assert_eq!(res.bbox.min_y, 20);
        assert_eq!(res.bbox.max_y, 59);
        assert_eq!(res.bbox.pixel_count, 60 * 40);
        assert_eq!(res.video_pos, (80.0, 40.0));

        let ev = AssEvent::dialogue(AssTime::from_cs(0), AssTime::from_cs(100), "Sign");
        let cmd = build_align_edit_command(0, &ev, res, Some(5)).expect("cmd");
        assert!(matches!(
            cmd,
            EditCommand::ModifyEventText { after, .. } if after == r"{\an5\pos(768,432)}Sign"
        ));
    }
}
