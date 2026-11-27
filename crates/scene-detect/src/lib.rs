//! Pure-Rust scene-cut / keyframe detector on Y8 luma planes.
//!
//! Replaces `VapourSynth` WWXD and Xvid `SCXvid` (`src/video_provider_ffmpegsource.cpp` /
//! `libaegisub/common/keyframe.cpp` workflow in Aegisub 3.5.1) with zero native C dependencies
//! and zero per-frame heap allocations in the hot path.
//!
//! Supports two detection modes:
//! - [`DetectionMode::Wwxd`]: fast luma SAD + 64-bin histogram L1 difference with adaptive rolling
//!   threshold, flash suppression, and fade rejection.
//! - [`DetectionMode::ScxvidBlockSad`]: macroblock motion-compensated SAD vs intra-block variance
//!   (modelled on Xvid's 2-pass keyframe decision / `SCXvid`).
//!
//! Output frame indices (`Vec<u32>`) plug directly into [`keyframes::write_aegisub_keyframes`] and
//! [`keyframes::write_plain_keyframes`].

#![forbid(unsafe_code)]
#![cfg_attr(not(test), deny(clippy::unwrap_used, clippy::expect_used))]

use std::sync::atomic::{AtomicBool, Ordering};
use thiserror::Error;

/// Errors returned by [`SceneDetector`].
#[derive(Debug, Error, PartialEq, Eq)]
pub enum SceneDetectError {
    /// Width or height was zero.
    #[error("invalid frame dimensions: {width}x{height}")]
    InvalidDimensions { width: usize, height: usize },
    /// Row stride was smaller than frame width.
    #[error("invalid stride {stride} for width {width}")]
    InvalidStride { stride: usize, width: usize },
    /// Input Y8 plane slice was too short for the given height and stride.
    #[error("y8 plane too short: got {actual} bytes, need at least {required}")]
    BufferTooShort { actual: usize, required: usize },
    /// Frame dimensions changed mid-stream without calling [`SceneDetector::reset`].
    #[error("frame dimensions changed from {expected_w}x{expected_h} to {actual_w}x{actual_h}")]
    DimensionMismatch {
        expected_w: usize,
        expected_h: usize,
        actual_w: usize,
        actual_h: usize,
    },
    /// Processing was cancelled via the supplied [`AtomicBool`].
    #[error("scene detection cancelled at frame {frame}")]
    Cancelled { frame: u32 },
}

/// Detection algorithm variant.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum DetectionMode {
    /// Faithful WWXD-style luma SAD + histogram difference with adaptive rolling baseline
    /// and 1-frame flash suppression.
    Wwxd,
    /// `SCXvid`-style macroblock motion-compensated SAD vs intra-block deviation.
    ScxvidBlockSad,
}

/// Configuration for [`SceneDetector`].
#[derive(Debug, Clone, Copy)]
pub struct SceneDetectConfig {
    /// Algorithm to run.
    pub mode: DetectionMode,
    /// Downscale input Y8 plane by 4× in each axis (16:1 pixel reduction) using a 4×4 box filter
    /// before computing metrics. Strongly recommended for 1080p+ video.
    pub quarter_res: bool,
    /// Normalized pixel-SAD threshold in `0.0..=1.0` (default `0.14`, ~36/255 average luma shift).
    pub sad_threshold: f32,
    /// Normalized L1 histogram difference threshold in `0.0..=2.0` (default `0.45`).
    pub hist_threshold: f32,
    /// Minimum adaptive ratio over recent rolling SAD median/mean (default `2.5`).
    pub adaptive_ratio: f32,
    /// Minimum frames between consecutive scene cuts (default `6`, avoids double-triggers).
    pub min_scene_len: u32,
    /// Enable 1-frame flash suppression (delays cut confirmation by 1 frame so brief camera
    /// flashes returning to the previous scene do not emit false keyframes).
    pub suppress_flashes: bool,
    /// For [`DetectionMode::ScxvidBlockSad`], fraction of macroblocks preferring intra over
    /// motion-compensated inter prediction required to trigger a cut (default `0.55`).
    pub scxvid_intra_ratio: f32,
}

impl Default for SceneDetectConfig {
    fn default() -> Self {
        Self {
            mode: DetectionMode::Wwxd,
            quarter_res: true,
            sad_threshold: 0.14,
            hist_threshold: 0.45,
            adaptive_ratio: 2.5,
            min_scene_len: 6,
            suppress_flashes: true,
            scxvid_intra_ratio: 0.55,
        }
    }
}

/// Per-frame metric snapshot returned by [`SceneDetector::push_frame`].
#[derive(Debug, Clone, Copy)]
pub struct FrameMetrics {
    /// 0-based index of the pushed frame.
    pub frame_index: u32,
    /// Normalized pixel SAD (`0.0..=1.0`) against the previous frame.
    pub sad_norm: f32,
    /// Normalized 64-bin histogram L1 distance (`0.0..=2.0`) against the previous frame.
    pub hist_diff: f32,
    /// Fraction of intra-preferred blocks in [`DetectionMode::ScxvidBlockSad`], else `0.0`.
    pub intra_ratio: f32,
    /// Confirmed keyframe emitted on this step (may be `frame_index - 1` when flash suppression
    /// is enabled).
    pub confirmed_keyframe: Option<u32>,
}

const HIST_BINS: usize = 64;
const ROLLING_WINDOW: usize = 16;

/// Streaming pure-Rust scene-cut detector.
///
/// Reuses all internal plane buffers across frames so steady-state `push_frame` performs zero
/// heap allocations.
pub struct SceneDetector {
    config: SceneDetectConfig,
    src_w: usize,
    src_h: usize,
    work_w: usize,
    work_h: usize,
    /// Double-buffered working planes: `cur_plane` holds latest, `prev_plane` holds `t-1`,
    /// `anchor_plane` holds `t-2` (for flash suppression).
    cur_plane: Vec<u8>,
    prev_plane: Vec<u8>,
    anchor_plane: Vec<u8>,
    cur_hist: [u32; HIST_BINS],
    prev_hist: [u32; HIST_BINS],
    anchor_hist: [u32; HIST_BINS],
    rolling_sad: [f32; ROLLING_WINDOW],
    rolling_pos: usize,
    rolling_len: usize,
    frame_count: u32,
    last_keyframe: u32,
    pending_candidate: Option<u32>,
    keyframes: Vec<u32>,
}

impl SceneDetector {
    /// Creates a new streaming detector with the given configuration.
    #[must_use]
    pub fn new(config: SceneDetectConfig) -> Self {
        Self {
            config,
            src_w: 0,
            src_h: 0,
            work_w: 0,
            work_h: 0,
            cur_plane: Vec::new(),
            prev_plane: Vec::new(),
            anchor_plane: Vec::new(),
            cur_hist: [0; HIST_BINS],
            prev_hist: [0; HIST_BINS],
            anchor_hist: [0; HIST_BINS],
            rolling_sad: [0.0; ROLLING_WINDOW],
            rolling_pos: 0,
            rolling_len: 0,
            frame_count: 0,
            last_keyframe: 0,
            pending_candidate: None,
            keyframes: Vec::new(),
        }
    }

    /// Resets detector state while retaining allocated plane buffers.
    pub fn reset(&mut self) {
        self.src_w = 0;
        self.src_h = 0;
        self.work_w = 0;
        self.work_h = 0;
        self.cur_hist = [0; HIST_BINS];
        self.prev_hist = [0; HIST_BINS];
        self.anchor_hist = [0; HIST_BINS];
        self.rolling_sad = [0.0; ROLLING_WINDOW];
        self.rolling_pos = 0;
        self.rolling_len = 0;
        self.frame_count = 0;
        self.last_keyframe = 0;
        self.pending_candidate = None;
        self.keyframes.clear();
    }

    /// Pushes one Y8 luma frame (`width` × `height`, row stride `stride` bytes).
    ///
    /// # Errors
    /// Returns [`SceneDetectError`] if dimensions/stride/buffer length are invalid or if
    /// dimensions change mid-stream.
    pub fn push_frame(
        &mut self,
        y_plane: &[u8],
        width: usize,
        height: usize,
        stride: usize,
    ) -> Result<FrameMetrics, SceneDetectError> {
        validate_frame(y_plane, width, height, stride)?;

        if self.frame_count == 0 {
            self.init_dimensions(width, height);
        } else if width != self.src_w || height != self.src_h {
            return Err(SceneDetectError::DimensionMismatch {
                expected_w: self.src_w,
                expected_h: self.src_h,
                actual_w: width,
                actual_h: height,
            });
        }

        let frame_idx = self.frame_count;
        self.frame_count += 1;

        // Rotate buffers: anchor <- prev <- cur (zero allocation: pointer swaps).
        std::mem::swap(&mut self.anchor_plane, &mut self.prev_plane);
        std::mem::swap(&mut self.prev_plane, &mut self.cur_plane);
        self.anchor_hist = self.prev_hist;
        self.prev_hist = self.cur_hist;

        // Fill cur_plane (either 4x4 box-downscaled or copied tightly) and compute 64-bin histogram.
        if self.config.quarter_res && width >= 8 && height >= 8 {
            downscale_4x4_and_hist(
                y_plane,
                width,
                height,
                stride,
                self.work_w,
                self.work_h,
                &mut self.cur_plane,
                &mut self.cur_hist,
            );
        } else {
            copy_tight_and_hist(
                y_plane,
                width,
                height,
                stride,
                &mut self.cur_plane,
                &mut self.cur_hist,
            );
        }

        // Frame 0 is always a keyframe (matches Aegisub / WWXD / `SCXvid` convention).
        if frame_idx == 0 {
            self.keyframes.push(0);
            self.last_keyframe = 0;
            return Ok(FrameMetrics {
                frame_index: 0,
                sad_norm: 0.0,
                hist_diff: 0.0,
                intra_ratio: 0.0,
                confirmed_keyframe: Some(0),
            });
        }

        let num_px = self.work_w * self.work_h;
        let sad_norm = normalized_sad(&self.cur_plane, &self.prev_plane);
        let hist_diff = normalized_hist_diff(&self.cur_hist, &self.prev_hist, num_px);

        let intra_ratio = match self.config.mode {
            DetectionMode::Wwxd => 0.0,
            DetectionMode::ScxvidBlockSad => scxvid_intra_block_ratio(
                &self.cur_plane,
                &self.prev_plane,
                self.work_w,
                self.work_h,
            ),
        };

        let mut confirmed_keyframe = None;

        if let Some(was_flash) = self.resolve_pending_candidate(num_px, &mut confirmed_keyframe) {
            if was_flash {
                return Ok(FrameMetrics {
                    frame_index: frame_idx,
                    sad_norm,
                    hist_diff,
                    intra_ratio,
                    confirmed_keyframe: None,
                });
            }
        }

        let baseline = self.rolling_baseline();
        let is_candidate = self.evaluate_cut(sad_norm, hist_diff, intra_ratio, baseline, frame_idx);

        if is_candidate {
            if self.config.suppress_flashes {
                self.pending_candidate = Some(frame_idx);
            } else {
                self.keyframes.push(frame_idx);
                self.last_keyframe = frame_idx;
                confirmed_keyframe = Some(frame_idx);
            }
        } else {
            self.push_rolling_sad(sad_norm);
        }

        Ok(FrameMetrics {
            frame_index: frame_idx,
            sad_norm,
            hist_diff,
            intra_ratio,
            confirmed_keyframe,
        })
    }

    /// Finalizes the stream, flushing any unconfirmed trailing candidate, and returns all
    /// detected keyframe numbers.
    pub fn finish(&mut self) -> &[u32] {
        if let Some(cand) = self.pending_candidate.take() {
            if cand.saturating_sub(self.last_keyframe) >= self.config.min_scene_len {
                self.keyframes.push(cand);
                self.last_keyframe = cand;
            }
        }
        &self.keyframes
    }

    /// Returns all currently confirmed keyframe indices.
    #[must_use]
    pub fn keyframes(&self) -> &[u32] {
        &self.keyframes
    }

    /// Formats the confirmed keyframes as an Aegisub `# keyframe format v1` string via the
    /// `keyframes` crate.
    #[must_use]
    pub fn to_aegisub_keyframes(&self, fps: Option<f64>) -> String {
        keyframes::write_aegisub_keyframes(&self.keyframes, fps)
    }

    /// Convenience driver for worker threads: iterates over a frame source with cooperative
    /// cancellation (`cancel`) and progress reporting (`on_progress(frame_index)`).
    ///
    /// # Errors
    /// Returns [`SceneDetectError::Cancelled`] if `cancel` is set, or any frame validation error.
    pub fn process_stream<'a, I, F>(
        &mut self,
        frames: I,
        width: usize,
        height: usize,
        stride: usize,
        cancel: &AtomicBool,
        mut on_progress: F,
    ) -> Result<Vec<u32>, SceneDetectError>
    where
        I: IntoIterator<Item = &'a [u8]>,
        F: FnMut(u32),
    {
        for plane in frames {
            if cancel.load(Ordering::Relaxed) {
                return Err(SceneDetectError::Cancelled {
                    frame: self.frame_count,
                });
            }
            let metrics = self.push_frame(plane, width, height, stride)?;
            on_progress(metrics.frame_index);
        }
        Ok(self.finish().to_vec())
    }

    fn resolve_pending_candidate(
        &mut self,
        num_px: usize,
        confirmed_keyframe: &mut Option<u32>,
    ) -> Option<bool> {
        let cand = self.pending_candidate.take()?;
        let back_sad = normalized_sad(&self.cur_plane, &self.anchor_plane);
        let back_hist = normalized_hist_diff(&self.cur_hist, &self.anchor_hist, num_px);
        let is_flash = back_sad < self.config.sad_threshold * 0.6
            && back_hist < self.config.hist_threshold * 0.6;
        if !is_flash && cand.saturating_sub(self.last_keyframe) >= self.config.min_scene_len {
            self.keyframes.push(cand);
            self.last_keyframe = cand;
            *confirmed_keyframe = Some(cand);
        }
        Some(is_flash)
    }

    fn init_dimensions(&mut self, width: usize, height: usize) {
        self.src_w = width;
        self.src_h = height;
        if self.config.quarter_res && width >= 8 && height >= 8 {
            self.work_w = width / 4;
            self.work_h = height / 4;
        } else {
            self.work_w = width;
            self.work_h = height;
        }
        let n = self.work_w * self.work_h;
        self.cur_plane.resize(n, 0);
        self.prev_plane.resize(n, 0);
        self.anchor_plane.resize(n, 0);
    }

    fn rolling_baseline(&self) -> f32 {
        if self.rolling_len == 0 {
            return 0.02;
        }
        let sum: f32 = self.rolling_sad[..self.rolling_len].iter().sum();
        #[allow(clippy::cast_precision_loss)]
        let mean = sum / (self.rolling_len as f32);
        mean.max(0.015)
    }

    fn push_rolling_sad(&mut self, sad: f32) {
        self.rolling_sad[self.rolling_pos] = sad;
        self.rolling_pos = (self.rolling_pos + 1) % ROLLING_WINDOW;
        if self.rolling_len < ROLLING_WINDOW {
            self.rolling_len += 1;
        }
    }

    fn evaluate_cut(
        &self,
        sad_norm: f32,
        hist_diff: f32,
        intra_ratio: f32,
        baseline: f32,
        frame_idx: u32,
    ) -> bool {
        if frame_idx.saturating_sub(self.last_keyframe) < self.config.min_scene_len {
            return false;
        }
        match self.config.mode {
            DetectionMode::Wwxd => {
                // WWXD-style decision:
                // 1) Very strong simultaneous pixel & histogram jump (hard cut), OR
                // 2) Both pixel SAD and histogram diff exceed thresholds AND spike well above
                //    the rolling motion/noise baseline (rejects steady pans, noise, and smooth fades).
                let strong_cut = sad_norm >= self.config.sad_threshold * 1.8
                    && hist_diff >= self.config.hist_threshold;
                let adaptive_cut = sad_norm >= self.config.sad_threshold
                    && hist_diff >= self.config.hist_threshold * 0.75
                    && sad_norm >= baseline * self.config.adaptive_ratio;
                strong_cut || adaptive_cut
            }
            DetectionMode::ScxvidBlockSad => {
                // `SCXvid`-style decision: majority of macroblocks prefer intra over motion-comp inter,
                // backed by a meaningful frame-level luma change.
                intra_ratio >= self.config.scxvid_intra_ratio
                    && sad_norm >= self.config.sad_threshold * 0.75
                    && sad_norm >= baseline * 1.8
            }
        }
    }
}

fn validate_frame(
    y_plane: &[u8],
    width: usize,
    height: usize,
    stride: usize,
) -> Result<(), SceneDetectError> {
    if width == 0 || height == 0 {
        return Err(SceneDetectError::InvalidDimensions { width, height });
    }
    if stride < width {
        return Err(SceneDetectError::InvalidStride { stride, width });
    }
    let required = (height - 1) * stride + width;
    if y_plane.len() < required {
        return Err(SceneDetectError::BufferTooShort {
            actual: y_plane.len(),
            required,
        });
    }
    Ok(())
}

/// Auto-vectorizable 4×4 box-filter downscale + 64-bin histogram in a single pass over `out`.
#[allow(clippy::too_many_arguments)]
fn downscale_4x4_and_hist(
    src: &[u8],
    _src_w: usize,
    _src_h: usize,
    stride: usize,
    dst_w: usize,
    dst_h: usize,
    dst: &mut [u8],
    hist: &mut [u32; HIST_BINS],
) {
    hist.fill(0);
    for dy in 0..dst_h {
        let sy = dy * 4;
        let r0 = &src[sy * stride..sy * stride + dst_w * 4];
        let r1 = &src[(sy + 1) * stride..(sy + 1) * stride + dst_w * 4];
        let r2 = &src[(sy + 2) * stride..(sy + 2) * stride + dst_w * 4];
        let r3 = &src[(sy + 3) * stride..(sy + 3) * stride + dst_w * 4];
        let out_row = &mut dst[dy * dst_w..(dy + 1) * dst_w];

        let c0 = r0.as_chunks::<4>().0;
        let c1 = r1.as_chunks::<4>().0;
        let c2 = r2.as_chunks::<4>().0;
        let c3 = r3.as_chunks::<4>().0;

        for (out_px, (((p0, p1), p2), p3)) in out_row
            .iter_mut()
            .zip(c0.iter().zip(c1.iter()).zip(c2.iter()).zip(c3.iter()))
        {
            let sum = u16::from(p0[0])
                + u16::from(p0[1])
                + u16::from(p0[2])
                + u16::from(p0[3])
                + u16::from(p1[0])
                + u16::from(p1[1])
                + u16::from(p1[2])
                + u16::from(p1[3])
                + u16::from(p2[0])
                + u16::from(p2[1])
                + u16::from(p2[2])
                + u16::from(p2[3])
                + u16::from(p3[0])
                + u16::from(p3[1])
                + u16::from(p3[2])
                + u16::from(p3[3]);
            #[allow(clippy::cast_possible_truncation)]
            let avg = ((sum + 8) >> 4) as u8;
            *out_px = avg;
            hist[usize::from(avg >> 2)] += 1;
        }
    }
}

fn copy_tight_and_hist(
    src: &[u8],
    width: usize,
    height: usize,
    stride: usize,
    dst: &mut [u8],
    hist: &mut [u32; HIST_BINS],
) {
    hist.fill(0);
    for y in 0..height {
        let row = &src[y * stride..y * stride + width];
        let out_row = &mut dst[y * width..(y + 1) * width];
        out_row.copy_from_slice(row);
        for &px in row {
            hist[usize::from(px >> 2)] += 1;
        }
    }
}

/// Auto-vectorizable normalized SAD in `0.0..=1.0`.
#[inline]
fn normalized_sad(a: &[u8], b: &[u8]) -> f32 {
    if a.is_empty() {
        return 0.0;
    }
    let mut sum = 0u64;
    // Process in 16-byte chunks so LLVM emits PSADBW / SIMD abs-diff instructions cleanly.
    let (a_chunks, a_rem) = a.as_chunks::<16>();
    let (b_chunks, b_rem) = b.as_chunks::<16>();
    for (ca, cb) in a_chunks.iter().zip(b_chunks.iter()) {
        let mut local = 0u32;
        for i in 0..16 {
            local += u32::from(ca[i].abs_diff(cb[i]));
        }
        sum += u64::from(local);
    }
    for (&pa, &pb) in a_rem.iter().zip(b_rem.iter()) {
        sum += u64::from(pa.abs_diff(pb));
    }
    #[allow(clippy::cast_precision_loss)]
    let denom = (a.len() as f64) * 255.0;
    #[allow(clippy::cast_precision_loss, clippy::cast_possible_truncation)]
    {
        ((sum as f64) / denom) as f32
    }
}

#[inline]
fn normalized_hist_diff(a: &[u32; HIST_BINS], b: &[u32; HIST_BINS], num_px: usize) -> f32 {
    if num_px == 0 {
        return 0.0;
    }
    let mut diff = 0u64;
    for i in 0..HIST_BINS {
        diff += u64::from(a[i].abs_diff(b[i]));
    }
    #[allow(clippy::cast_precision_loss, clippy::cast_possible_truncation)]
    {
        ((diff as f64) / (num_px as f64)) as f32
    }
}

/// `SCXvid`-like macroblock motion-compensated SAD vs intra block deviation.
///
/// Uses 8×8 blocks on the working plane (equivalent to 32×32 on 1080p at 1/4 res) and evaluates a
/// 9-point `[-4, 0, +4]` motion vector search around each block. A block prefers intra when its
/// best motion-compensated inter SAD exceeds its intra mean-absolute-deviation (with a small
/// bias matching Xvid's intra bias).
fn scxvid_intra_block_ratio(cur: &[u8], prev: &[u8], w: usize, h: usize) -> f32 {
    const BLK: usize = 8;
    const SEARCH: [isize; 3] = [-4, 0, 4];

    if w < BLK * 2 || h < BLK * 2 {
        return 0.0;
    }

    let cols_max = (w - BLK) / BLK;
    let rows_max = (h - BLK) / BLK;
    let mut total_blocks = 0u32;
    let mut intra_blocks = 0u32;

    for by in 1..rows_max {
        let y0 = by * BLK;
        for bx in 1..cols_max {
            let x0 = bx * BLK;
            total_blocks += 1;

            // Intra deviation: sum of |px - block_mean|.
            let mut sum = 0u32;
            for dy in 0..BLK {
                let row = &cur[(y0 + dy) * w + x0..(y0 + dy) * w + x0 + BLK];
                for &p in row {
                    sum += u32::from(p);
                }
            }
            #[allow(clippy::cast_possible_truncation)]
            let mean = (sum >> 6) as u8;
            let mut intra_dev = 0u32;
            for dy in 0..BLK {
                let row = &cur[(y0 + dy) * w + x0..(y0 + dy) * w + x0 + BLK];
                for &p in row {
                    intra_dev += u32::from(p.abs_diff(mean));
                }
            }

            // 9-point motion-compensated inter SAD.
            let mut best_inter = u32::MAX;
            for &my in &SEARCH {
                let py0 = y0.saturating_add_signed(my);
                if py0 + BLK > h {
                    continue;
                }
                for &mx in &SEARCH {
                    let px0 = x0.saturating_add_signed(mx);
                    if px0 + BLK > w {
                        continue;
                    }
                    let mut sad = 0u32;
                    for dy in 0..BLK {
                        let crow = &cur[(y0 + dy) * w + x0..(y0 + dy) * w + x0 + BLK];
                        let prow = &prev[(py0 + dy) * w + px0..(py0 + dy) * w + px0 + BLK];
                        for i in 0..BLK {
                            sad += u32::from(crow[i].abs_diff(prow[i]));
                        }
                    }
                    if sad < best_inter {
                        best_inter = sad;
                    }
                }
            }

            // Intra preferred when motion-comp inter SAD is significantly worse than intra deviation
            // + small floor (avoids triggering on flat-to-flat noise).
            if best_inter > intra_dev.saturating_add(512) {
                intra_blocks += 1;
            }
        }
    }

    if total_blocks == 0 {
        return 0.0;
    }
    #[allow(clippy::cast_precision_loss)]
    {
        (intra_blocks as f32) / (total_blocks as f32)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn solid_frame(w: usize, h: usize, stride: usize, val: u8) -> Vec<u8> {
        let mut buf = vec![0u8; h * stride];
        for y in 0..h {
            buf[y * stride..y * stride + w].fill(val);
        }
        buf
    }

    fn pattern_frame(w: usize, h: usize, seed: u8) -> Vec<u8> {
        let mut buf = vec![0u8; w * h];
        for y in 0..h {
            for x in 0..w {
                #[allow(clippy::cast_possible_truncation)]
                let v =
                    ((x.wrapping_mul(3) ^ y.wrapping_mul(5)).wrapping_add(usize::from(seed))) as u8;
                buf[y * w + x] = v;
            }
        }
        buf
    }

    #[test]
    fn synthetic_cuts_fades_flashes_static_and_noise() {
        let w = 160;
        let h = 96;
        let mut det = SceneDetector::new(SceneDetectConfig::default());

        // Frames 0..15: static dark scene (luma 24)
        for _ in 0..15 {
            det.push_frame(&solid_frame(w, h, w, 24), w, h, w)
                .expect("push");
        }
        // Frame 15: hard cut to bright scene (luma 210) for frames 15..30
        for _ in 15..30 {
            det.push_frame(&solid_frame(w, h, w, 210), w, h, w)
                .expect("push");
        }
        // Frame 30: 1-frame white flash (luma 255) -> returns to 210 on frame 31..40
        det.push_frame(&solid_frame(w, h, w, 255), w, h, w)
            .expect("push");
        for _ in 31..40 {
            det.push_frame(&solid_frame(w, h, w, 210), w, h, w)
                .expect("push");
        }
        // Frames 40..60: smooth fade down from 210 -> 30 (~9 luma/frame)
        for step in 0..20u8 {
            let luma = 210 - step * 9;
            det.push_frame(&solid_frame(w, h, w, luma), w, h, w)
                .expect("push");
        }
        // Frames 60..75: noisy static dark scene (luma 30 +/- 4)
        for f in 0..15u8 {
            let mut frame = solid_frame(w, h, w, 30);
            for (i, px) in frame.iter_mut().enumerate() {
                #[allow(clippy::cast_possible_truncation)]
                let n = ((i.wrapping_mul(17).wrapping_add(usize::from(f) * 31)) & 7) as u8;
                *px = px.saturating_add(n);
            }
            det.push_frame(&frame, w, h, w).expect("push");
        }
        // Frame 75: hard cut to textured bright scene for frames 75..85
        for _ in 75..85 {
            det.push_frame(&pattern_frame(w, h, 140), w, h, w)
                .expect("push");
        }

        let kfs = det.finish().to_vec();
        assert_eq!(kfs, vec![0, 15, 75], "got keyframes: {kfs:?}");

        let formatted = det.to_aegisub_keyframes(Some(24000.0 / 1001.0));
        assert!(formatted.starts_with("# keyframe format v1\n"));
        assert!(formatted.contains("15\n"));
        assert!(formatted.contains("75\n"));
    }

    #[test]
    fn scxvid_block_sad_mode_detects_cut_and_supports_stride_and_cancel() {
        let w = 160;
        let h = 96;
        let stride = 192; // padded stride > width
        let cfg = SceneDetectConfig {
            mode: DetectionMode::ScxvidBlockSad,
            quarter_res: false,
            suppress_flashes: false,
            ..SceneDetectConfig::default()
        };
        let mut det = SceneDetector::new(cfg);

        let a = solid_frame(w, h, stride, 20);
        let b = solid_frame(w, h, stride, 220);
        let frames: Vec<&[u8]> = (0..10)
            .map(|_| a.as_slice())
            .chain((0..10).map(|_| b.as_slice()))
            .collect();

        let cancel = AtomicBool::new(false);
        let mut last_prog = 0;
        let kfs = det
            .process_stream(frames, w, h, stride, &cancel, |idx| last_prog = idx)
            .expect("stream");
        assert_eq!(last_prog, 19);
        assert_eq!(kfs, vec![0, 10]);

        // Cancellation test
        det.reset();
        cancel.store(true, Ordering::Relaxed);
        let err = det
            .process_stream([a.as_slice()], w, h, stride, &cancel, |_| {})
            .expect_err("cancelled");
        assert_eq!(err, SceneDetectError::Cancelled { frame: 0 });
    }
}
