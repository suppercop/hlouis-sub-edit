//! Index-free video for an instant first frame (plain libavformat/libavcodec through
//! `BestSource`'s decoder), with progressive frame-exact seeking as the shared partial index
//! grows in the background.

use crate::ffi::bestsource::{PartialIndex, RawBsQuick};
use crate::provider::{ProviderKind, VideoProvider};
use crate::{pts_to_ms_rounded, MediaError, VideoInfo};
use std::path::Path;

/// Approximate, index-free video provider that upgrades frame-by-frame to exact seeking as a
/// shared [`PartialIndex`] fills.
pub struct QuickVideo {
    raw: RawBsQuick,
    info: VideoInfo,
    tb_num_ms: i64,
    tb_den: i64,
    partial: Option<PartialIndex>,
}

impl QuickVideo {
    /// Opens `path` and decodes frame 0 (no indexing). `track < 0` = n-th video track.
    pub fn open(path: &Path, track: i32, threads: i32) -> Result<Self, MediaError> {
        Self::open_with_partial(path, track, threads, None)
    }

    /// Opens `path` attached to a shared [`PartialIndex`] for progressive exact seeking.
    pub fn open_with_partial(
        path: &Path,
        track: i32,
        threads: i32,
        partial: Option<PartialIndex>,
    ) -> Result<Self, MediaError> {
        let mut raw = RawBsQuick::open(path, track, threads)?;
        if let Some(p) = partial.as_ref() {
            raw.set_partial_index(Some(p));
        }
        let p = raw.props();
        let width = u32::try_from(p.width).map_err(|_| MediaError::InvalidMetadata)?;
        let height = u32::try_from(p.height).map_err(|_| MediaError::InvalidMetadata)?;
        if width == 0 || height == 0 || p.fps_num <= 0 || p.fps_den <= 0 || p.time_base_den <= 0 {
            return Err(MediaError::InvalidMetadata);
        }
        let fps_num = i64::from(p.fps_num);
        let fps_den = i64::from(p.fps_den);
        let fps = fps_num as f64 / fps_den as f64;
        let tb_num_ms = i64::from(p.time_base_num) * 1000;
        let tb_den = i64::from(p.time_base_den);
        let duration_ms = if p.duration > 0 {
            p.duration as f64 * tb_num_ms as f64 / tb_den as f64
        } else {
            0.0
        };
        let estimate = if p.num_frames_estimate > 0 {
            p.num_frames_estimate
        } else {
            // Truncation is intended: whole frames that fit in the duration.
            #[allow(clippy::cast_possible_truncation)]
            let n = (duration_ms * fps / 1000.0).floor() as i64;
            n
        };
        let num_frames = usize::try_from(estimate.max(1)).unwrap_or(1);
        let first_ms = pts_to_ms_rounded(p.first_pts, tb_num_ms, tb_den);
        let frame_pts_ms = (0..num_frames)
            .map(|i| {
                let i = i64::try_from(i).unwrap_or(i64::MAX);
                first_ms + pts_to_ms_rounded(i.saturating_mul(fps_den), 1000, fps_num)
            })
            .collect();
        let duration_ms = if duration_ms > 0.0 {
            duration_ms
        } else {
            num_frames as f64 * 1000.0 / fps
        };
        Ok(Self {
            raw,
            info: VideoInfo {
                width,
                height,
                num_frames,
                fps_num,
                fps_den,
                fps,
                duration_ms,
                keyframes: Vec::new(),
                frame_pts_ms,
                sar_num: u32::try_from(p.sar_num).unwrap_or(0),
                sar_den: u32::try_from(p.sar_den).unwrap_or(0),
            },
            tb_num_ms,
            tb_den,
            partial,
        })
    }

    /// Replaces or detaches the shared [`PartialIndex`].
    pub fn set_partial_index(&mut self, partial: Option<PartialIndex>) {
        self.raw.set_partial_index(partial.as_ref());
        self.partial = partial;
    }

    /// Number of frames indexed so far in the attached partial index (0 when detached).
    #[must_use]
    pub fn indexed_frames(&self) -> usize {
        self.partial.as_ref().map_or(0, PartialIndex::len)
    }
}

impl VideoProvider for QuickVideo {
    fn kind(&self) -> ProviderKind {
        ProviderKind::Quick
    }

    fn is_frame_exact(&self) -> bool {
        false
    }

    fn is_frame_exact_at(&self, frame_index: usize) -> bool {
        frame_index < self.info.num_frames
            && self
                .partial
                .as_ref()
                .is_some_and(|p| p.is_exact_at(frame_index))
    }

    fn info(&self) -> &VideoInfo {
        &self.info
    }

    fn decode_frame_into(
        &mut self,
        frame_index: usize,
        out_rgba: &mut Vec<u8>,
    ) -> Result<i64, MediaError> {
        if frame_index >= self.info.num_frames {
            return Err(MediaError::FrameOutOfRange {
                frame: frame_index,
                total: self.info.num_frames,
            });
        }
        let total = (self.info.width as usize)
            .checked_mul(self.info.height as usize)
            .and_then(|n| n.checked_mul(4))
            .ok_or(MediaError::InvalidMetadata)?;
        out_rgba.resize(total, 0);
        let n = i64::try_from(frame_index).map_err(|_| MediaError::InvalidMetadata)?;
        let pts = self
            .raw
            .frame_rgba(n, out_rgba, self.info.width, self.info.height)?;
        Ok(pts_to_ms_rounded(pts, self.tb_num_ms, self.tb_den))
    }

    fn next_keyframe(&self, current: usize) -> Option<usize> {
        self.partial
            .as_ref()
            .and_then(|p| p.next_keyframe(current))
            .filter(|&kf| kf < self.info.num_frames)
    }

    fn prev_keyframe(&self, current: usize) -> Option<usize> {
        self.partial
            .as_ref()
            .and_then(|p| p.prev_keyframe(current))
            .filter(|&kf| kf < self.info.num_frames)
    }
}
