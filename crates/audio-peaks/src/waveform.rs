//! Incremental multi-resolution waveform peak pyramid (min/max/RMS).

use crate::AudioPeaksError;

/// Base bucket size in audio frames (256 frames = 5.33 ms at 48 kHz).
pub const BASE_BUCKET_FRAMES: usize = 256;

/// Number of mip levels (`256`, `1024`, `4096`, `16384`, `65536` frames per bucket).
const NUM_MIP_LEVELS: usize = 5;

/// Factor between adjacent mip levels.
const MIP_RATIO: usize = 4;

/// Compact 6-byte peak summary for a contiguous span of audio frames.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub struct PeakBucket {
    pub min: i16,
    pub max: i16,
    pub rms: u16,
}

impl PeakBucket {
    /// Merges up to 4 child buckets into a parent mip bucket.
    #[inline]
    #[must_use]
    fn merge_slice(children: &[Self]) -> Self {
        let mut min_v = i16::MAX;
        let mut max_v = i16::MIN;
        let mut sum_sq = 0u64;
        for c in children {
            min_v = min_v.min(c.min);
            max_v = max_v.max(c.max);
            let r = u64::from(c.rms);
            sum_sq += r * r;
        }
        let n = children.len().max(1) as u64;
        let mean_sq = sum_sq / n;
        let rms = integer_sqrt_u64(mean_sq);
        Self {
            min: if children.is_empty() { 0 } else { min_v },
            max: if children.is_empty() { 0 } else { max_v },
            rms,
        }
    }
}

/// Incremental multi-resolution waveform pyramid supporting any channel count and 8–192 kHz.
///
/// For a 24-minute 48 kHz stereo episode (69,120,000 frames), the full 5-level pyramid uses
/// ~2.16 MB of RAM (well under the 8 MB budget).
#[derive(Debug, Clone)]
pub struct WaveformPyramid {
    sample_rate: u32,
    channels: usize,
    total_frames: u64,
    /// Mip levels: level `k` covers `BASE_BUCKET_FRAMES * 4^k` frames per bucket.
    levels: [Vec<PeakBucket>; NUM_MIP_LEVELS],
    /// Partial mono-mixed frame accumulator (< 256 frames) carried across incremental chunks.
    partial_min: i16,
    partial_max: i16,
    partial_sum_sq: u64,
    partial_count: usize,
    /// Number of base buckets already folded into higher mip levels.
    folded_counts: [usize; NUM_MIP_LEVELS],
}

impl WaveformPyramid {
    /// Creates an empty incremental waveform pyramid.
    ///
    /// # Errors
    /// Returns [`AudioPeaksError`] if `sample_rate` is outside `8_000..=192_000` or `channels == 0`.
    pub fn new(sample_rate: u32, channels: u16) -> Result<Self, AudioPeaksError> {
        if !(8_000..=192_000).contains(&sample_rate) {
            return Err(AudioPeaksError::InvalidSampleRate(sample_rate));
        }
        if channels == 0 {
            return Err(AudioPeaksError::InvalidChannels(channels));
        }
        Ok(Self {
            sample_rate,
            channels: usize::from(channels),
            total_frames: 0,
            levels: std::array::from_fn(|_| Vec::new()),
            partial_min: i16::MAX,
            partial_max: i16::MIN,
            partial_sum_sq: 0,
            partial_count: 0,
            folded_counts: [0; NUM_MIP_LEVELS],
        })
    }

    /// Pre-allocates bucket vectors for an expected total frame count to avoid reallocations.
    pub fn reserve_frames(&mut self, expected_frames: u64) {
        let mut stride = BASE_BUCKET_FRAMES as u64;
        for lvl in &mut self.levels {
            let cap = usize::try_from(expected_frames.div_ceil(stride)).unwrap_or(0);
            if cap > lvl.capacity() {
                lvl.reserve(cap - lvl.len());
            }
            stride = stride.saturating_mul(MIP_RATIO as u64);
        }
    }

    /// Sample rate in Hz.
    #[inline]
    #[must_use]
    pub const fn sample_rate(&self) -> u32 {
        self.sample_rate
    }

    /// Total frames ingested so far.
    #[inline]
    #[must_use]
    pub const fn total_frames(&self) -> u64 {
        self.total_frames
    }

    /// Approximate heap memory used by the pyramid in bytes.
    #[must_use]
    pub fn heap_bytes(&self) -> usize {
        self.levels
            .iter()
            .map(|v| v.capacity() * std::mem::size_of::<PeakBucket>())
            .sum()
    }

    /// Pushes interleaved `i16` PCM samples incrementally (zero heap allocation in hot loop).
    pub fn push_interleaved_i16(&mut self, samples: &[i16]) {
        let ch = self.channels;
        let frame_count = samples.len() / ch;
        if frame_count == 0 {
            return;
        }
        self.total_frames = self.total_frames.saturating_add(frame_count as u64);
        let mut frame_idx = 0usize;

        // Finish any partial 256-frame bucket first.
        if self.partial_count > 0 {
            let need = BASE_BUCKET_FRAMES - self.partial_count;
            let take = need.min(frame_count);
            self.accumulate_partial_i16(&samples[..take * ch]);
            frame_idx += take;
            if self.partial_count == BASE_BUCKET_FRAMES {
                self.flush_partial_bucket();
            }
        }

        // Fast SIMD-friendly path for full 256-frame chunks.
        let rem_frames = frame_count - frame_idx;
        let full_buckets = rem_frames / BASE_BUCKET_FRAMES;
        if full_buckets > 0 {
            self.levels[0].reserve(full_buckets);
            let base_slice =
                &samples[frame_idx * ch..(frame_idx + full_buckets * BASE_BUCKET_FRAMES) * ch];
            match ch {
                1 => {
                    for chunk in base_slice.as_chunks::<BASE_BUCKET_FRAMES>().0 {
                        self.levels[0].push(reduce_mono_i16_256(chunk));
                    }
                }
                2 => {
                    for chunk in base_slice.as_chunks::<{ BASE_BUCKET_FRAMES * 2 }>().0 {
                        self.levels[0].push(reduce_stereo_i16_256(chunk));
                    }
                }
                _ => {
                    for chunk in base_slice.chunks_exact(BASE_BUCKET_FRAMES * ch) {
                        self.levels[0].push(reduce_generic_i16(chunk, ch));
                    }
                }
            }
            frame_idx += full_buckets * BASE_BUCKET_FRAMES;
        }

        // Accumulate trailing < 256 frames into partial state.
        if frame_idx < frame_count {
            self.accumulate_partial_i16(&samples[frame_idx * ch..frame_count * ch]);
        }

        self.fold_higher_levels();
    }

    /// Pushes interleaved `f32` PCM samples (in `-1.0..=1.0`) incrementally.
    pub fn push_interleaved_f32(&mut self, samples: &[f32]) {
        let ch = self.channels;
        let frame_count = samples.len() / ch;
        if frame_count == 0 {
            return;
        }
        // Convert in small stack-allocated 1024-sample chunks to reuse the fast i16 path with zero heap alloc.
        let mut stack_buf = [0i16; 1024];
        let chunk_frames = (1024 / ch).max(1);
        let chunk_samples = chunk_frames * ch;
        for slice in samples[..frame_count * ch].chunks(chunk_samples) {
            let n = slice.len();
            for (dst, &src) in stack_buf[..n].iter_mut().zip(slice.iter()) {
                #[allow(clippy::cast_possible_truncation)]
                let scaled = (src.clamp(-1.0, 1.0) * 32_767.0).round() as i16;
                *dst = scaled;
            }
            self.push_interleaved_i16(&stack_buf[..n]);
        }
    }

    /// Flushes any trailing partial bucket (< 256 frames) and finalizes higher mip levels.
    pub fn finish(&mut self) {
        if self.partial_count > 0 {
            self.flush_partial_bucket();
        }
        self.fold_higher_levels();
        // Fold trailing partial groups (< 4) at each higher mip level.
        for lvl in 1..NUM_MIP_LEVELS {
            let src_len = self.levels[lvl - 1].len();
            let done = self.folded_counts[lvl - 1];
            if done < src_len {
                let tail = &self.levels[lvl - 1][done..src_len];
                self.levels[lvl].push(PeakBucket::merge_slice(tail));
                self.folded_counts[lvl - 1] = src_len;
            }
        }
    }

    /// Queries `[start_frame, end_frame)` into `num_columns` display columns, writing directly
    /// into caller-supplied `out` (zero allocation when `out` is reused).
    pub fn query_columns(
        &self,
        start_frame: u64,
        end_frame: u64,
        num_columns: usize,
        out: &mut Vec<PeakBucket>,
    ) {
        out.clear();
        if num_columns == 0 || end_frame <= start_frame || self.levels[0].is_empty() {
            out.resize(num_columns, PeakBucket::default());
            return;
        }
        out.reserve(num_columns);

        let span = end_frame - start_frame;
        let frames_per_col = (span / (num_columns as u64)).max(1);

        // Pick the coarsest mip level whose bucket size <= frames_per_col (or level 0).
        let mut level_idx = 0usize;
        let mut bucket_frames = BASE_BUCKET_FRAMES as u64;
        for (lvl, level_buckets) in self.levels.iter().enumerate().skip(1) {
            let next_stride = bucket_frames * (MIP_RATIO as u64);
            if next_stride <= frames_per_col && !level_buckets.is_empty() {
                level_idx = lvl;
                bucket_frames = next_stride;
            } else {
                break;
            }
        }

        let buckets = &self.levels[level_idx];
        let max_b = buckets.len();

        for col in 0..num_columns {
            let c0 = start_frame + (span * (col as u64)) / (num_columns as u64);
            let c1 = start_frame + (span * ((col + 1) as u64)) / (num_columns as u64);
            let b0 = usize::try_from(c0 / bucket_frames).unwrap_or(max_b);
            let b1 = usize::try_from(c1.div_ceil(bucket_frames))
                .unwrap_or(max_b)
                .max(b0.saturating_add(1))
                .min(max_b);

            if b0 >= max_b {
                out.push(PeakBucket::default());
            } else if b1 == b0 + 1 {
                out.push(buckets[b0]);
            } else {
                out.push(PeakBucket::merge_slice(&buckets[b0..b1]));
            }
        }
    }

    fn accumulate_partial_i16(&mut self, slice: &[i16]) {
        let ch = self.channels;
        for frame in slice.chunks_exact(ch) {
            let sample = mix_frame_i16(frame);
            self.partial_min = self.partial_min.min(sample);
            self.partial_max = self.partial_max.max(sample);
            let s = i64::from(sample);
            self.partial_sum_sq += (s * s).cast_unsigned();
            self.partial_count += 1;
        }
    }

    fn flush_partial_bucket(&mut self) {
        let n = self.partial_count.max(1) as u64;
        let rms = integer_sqrt_u64(self.partial_sum_sq / n);
        self.levels[0].push(PeakBucket {
            min: self.partial_min,
            max: self.partial_max,
            rms,
        });
        self.partial_min = i16::MAX;
        self.partial_max = i16::MIN;
        self.partial_sum_sq = 0;
        self.partial_count = 0;
    }

    fn fold_higher_levels(&mut self) {
        for lvl in 1..NUM_MIP_LEVELS {
            let src_len = self.levels[lvl - 1].len();
            let mut done = self.folded_counts[lvl - 1];
            while done + MIP_RATIO <= src_len {
                let merged = PeakBucket::merge_slice(&self.levels[lvl - 1][done..done + MIP_RATIO]);
                self.levels[lvl].push(merged);
                done += MIP_RATIO;
            }
            self.folded_counts[lvl - 1] = done;
        }
    }
}

#[inline]
fn mix_frame_i16(frame: &[i16]) -> i16 {
    if frame.len() == 1 {
        frame[0]
    } else if frame.len() == 2 {
        #[allow(clippy::cast_possible_truncation)]
        {
            ((i32::from(frame[0]) + i32::from(frame[1])) >> 1) as i16
        }
    } else {
        let sum: i32 = frame.iter().map(|&s| i32::from(s)).sum();
        let len = i32::try_from(frame.len()).unwrap_or(1);
        #[allow(clippy::cast_possible_truncation)]
        {
            (sum / len) as i16
        }
    }
}

/// Auto-vectorizable 256-frame mono reduction.
#[inline]
fn reduce_mono_i16_256(chunk: &[i16]) -> PeakBucket {
    let mut min_v = i16::MAX;
    let mut max_v = i16::MIN;
    let mut sum_sq = 0u64;
    for &s in chunk {
        min_v = min_v.min(s);
        max_v = max_v.max(s);
        let v = i64::from(s);
        sum_sq += (v * v).cast_unsigned();
    }
    PeakBucket {
        min: min_v,
        max: max_v,
        rms: integer_sqrt_u64(sum_sq >> 8),
    }
}

/// Auto-vectorizable 256-frame stereo reduction (512 interleaved `i16` samples).
#[inline]
fn reduce_stereo_i16_256(chunk: &[i16]) -> PeakBucket {
    let mut min_v = i16::MAX;
    let mut max_v = i16::MIN;
    let mut sum_sq = 0u64;
    for pair in chunk.as_chunks::<2>().0 {
        #[allow(clippy::cast_possible_truncation)]
        let m = ((i32::from(pair[0]) + i32::from(pair[1])) >> 1) as i16;
        min_v = min_v.min(m);
        max_v = max_v.max(m);
        let v = i64::from(m);
        sum_sq += (v * v).cast_unsigned();
    }
    PeakBucket {
        min: min_v,
        max: max_v,
        rms: integer_sqrt_u64(sum_sq >> 8),
    }
}

#[inline]
fn reduce_generic_i16(chunk: &[i16], channels: usize) -> PeakBucket {
    let mut min_v = i16::MAX;
    let mut max_v = i16::MIN;
    let mut sum_sq = 0u64;
    for frame in chunk.chunks_exact(channels) {
        let m = mix_frame_i16(frame);
        min_v = min_v.min(m);
        max_v = max_v.max(m);
        let v = i64::from(m);
        sum_sq += (v * v).cast_unsigned();
    }
    PeakBucket {
        min: min_v,
        max: max_v,
        rms: integer_sqrt_u64(sum_sq >> 8),
    }
}

#[inline]
#[allow(
    clippy::cast_possible_truncation,
    clippy::cast_sign_loss,
    clippy::cast_precision_loss
)]
fn integer_sqrt_u64(v: u64) -> u16 {
    (v as f64).sqrt().min(f64::from(u16::MAX)) as u16
}
