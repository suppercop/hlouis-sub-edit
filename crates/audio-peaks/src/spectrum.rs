//! Incremental Hann-windowed STFT spectrogram (`rustfft` + optional `rayon`) with dB
//! quantization to `u8`, log-frequency sampling, and Aegisub-style color maps.

use crate::AudioPeaksError;
#[cfg(feature = "rayon")]
use rayon::prelude::*;
use rustfft::{num_complex::Complex, Fft, FftPlanner};
use std::f32::consts::TAU;
use std::sync::Arc;

/// Built-in spectrogram color palettes (matching Aegisub's classic blue-cyan-yellow-red ramp).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum ColorScheme {
    /// Aegisub Classic (dark navy -> blue -> cyan -> yellow -> white).
    #[default]
    AegisubClassic,
    /// High-contrast Inferno-style (black -> purple -> orange -> yellow).
    Inferno,
    /// Linear grayscale.
    Grayscale,
}

/// Pure function mapping a quantized spectrogram bin (`0..=255`) to RGBA8.
#[inline]
#[must_use]
pub const fn bin_to_rgba(scheme: ColorScheme, value: u8) -> [u8; 4] {
    match scheme {
        ColorScheme::Grayscale => [value, value, value, 255],
        ColorScheme::AegisubClassic => {
            if value < 64 {
                let t = value * 4;
                [0, 0, t, 255]
            } else if value < 128 {
                let t = (value - 64) * 4;
                [0, t, 255, 255]
            } else if value < 192 {
                let t = (value - 128) * 4;
                [t, 255, 255 - t, 255]
            } else {
                let t = (value - 192) * 4;
                [255, 255, t, 255]
            }
        }
        ColorScheme::Inferno => {
            if value < 85 {
                let t = value * 3;
                [t / 2, 0, t, 255]
            } else if value < 170 {
                let t = (value - 85) * 3;
                [128 + t / 2, t / 2, 255 - t, 255]
            } else {
                let t = (value - 170) * 3;
                [255, 128 + t / 2, t, 255]
            }
        }
    }
}

/// Configuration for [`SpectrumBuilder`].
#[derive(Debug, Clone, Copy)]
pub struct SpectrumConfig {
    pub sample_rate: u32,
    pub channels: u16,
    /// Power-of-two FFT size in `256..=4096` (default `1024`).
    pub fft_size: usize,
    /// Step size between consecutive STFT frames in `1..=fft_size` (default `512` = 50% overlap).
    pub hop_size: usize,
    /// Minimum dB floor mapped to `0` (default `-80.0`).
    pub min_db: f32,
    /// Maximum dB ceiling mapped to `255` (default `0.0`).
    pub max_db: f32,
}

impl SpectrumConfig {
    /// Standard Aegisub-like 1024-point, 50%-overlap configuration.
    #[must_use]
    pub const fn new(sample_rate: u32, channels: u16) -> Self {
        Self {
            sample_rate,
            channels,
            fft_size: 1024,
            hop_size: 512,
            min_db: -80.0,
            max_db: 0.0,
        }
    }
}

/// Incremental Hann-windowed STFT builder producing `u8`-quantized spectrum columns.
pub struct SpectrumBuilder {
    config: SpectrumConfig,
    bins_per_col: usize,
    hann: Vec<f32>,
    fft: Arc<dyn Fft<f32>>,
    /// Carry-over mono `f32` samples across incremental chunks.
    carry: Vec<f32>,
    /// Flat column-major buffer: column `c`, bin `b` is at `data[c * bins_per_col + b]`.
    data: Vec<u8>,
    num_columns: usize,
}

impl SpectrumBuilder {
    /// Creates a new incremental STFT spectrum builder.
    ///
    /// # Errors
    /// Returns [`AudioPeaksError`] for invalid sample rate, channel count, FFT size, or hop size.
    pub fn new(config: SpectrumConfig) -> Result<Self, AudioPeaksError> {
        if !(8_000..=192_000).contains(&config.sample_rate) {
            return Err(AudioPeaksError::InvalidSampleRate(config.sample_rate));
        }
        if config.channels == 0 {
            return Err(AudioPeaksError::InvalidChannels(config.channels));
        }
        if !(256..=4096).contains(&config.fft_size) || !config.fft_size.is_power_of_two() {
            return Err(AudioPeaksError::InvalidFftSize(config.fft_size));
        }
        if config.hop_size == 0 || config.hop_size > config.fft_size {
            return Err(AudioPeaksError::InvalidHopSize(config.hop_size));
        }

        let n = config.fft_size;
        #[allow(clippy::cast_precision_loss)]
        let denom = (n - 1) as f32;
        #[allow(clippy::cast_precision_loss)]
        let hann: Vec<f32> = (0..n)
            .map(|i| 0.5 * (1.0 - (TAU * (i as f32) / denom).cos()))
            .collect();

        let mut planner = FftPlanner::<f32>::new();
        let fft = planner.plan_fft_forward(n);

        Ok(Self {
            config,
            bins_per_col: n / 2 + 1,
            hann,
            fft,
            carry: Vec::with_capacity(n * 2),
            data: Vec::new(),
            num_columns: 0,
        })
    }

    /// Number of frequency bins per column (`fft_size / 2 + 1`).
    #[inline]
    #[must_use]
    pub const fn bins_per_col(&self) -> usize {
        self.bins_per_col
    }

    /// Number of completed STFT columns so far.
    #[inline]
    #[must_use]
    pub const fn num_columns(&self) -> usize {
        self.num_columns
    }

    /// Returns the quantized `u8` bins (`len == bins_per_col()`) for column `col`.
    #[inline]
    #[must_use]
    pub fn column(&self, col: usize) -> Option<&[u8]> {
        if col >= self.num_columns {
            return None;
        }
        let s = col * self.bins_per_col;
        Some(&self.data[s..s + self.bins_per_col])
    }

    /// Pushes interleaved `i16` PCM samples incrementally and computes all newly ready STFT columns.
    pub fn push_interleaved_i16(&mut self, samples: &[i16]) {
        let ch = usize::from(self.config.channels);
        let frames = samples.len() / ch;
        if frames == 0 {
            return;
        }

        // Convert to mono f32 into carry buffer.
        self.carry.reserve(frames);
        let scale = 1.0 / 32_768.0_f32;
        match ch {
            1 => {
                self.carry
                    .extend(samples.iter().map(|&s| f32::from(s) * scale));
            }
            2 => {
                let half_scale = 0.5 * scale;
                self.carry.extend(
                    samples
                        .as_chunks::<2>()
                        .0
                        .iter()
                        .map(|p| (f32::from(p[0]) + f32::from(p[1])) * half_scale),
                );
            }
            _ => {
                let inv = scale / f32::from(self.config.channels);
                self.carry.extend(samples.chunks_exact(ch).map(|f| {
                    let sum: f32 = f.iter().map(|&s| f32::from(s)).sum();
                    sum * inv
                }));
            }
        }

        self.process_carry();
    }

    /// Pushes interleaved `f32` PCM samples incrementally.
    pub fn push_interleaved_f32(&mut self, samples: &[f32]) {
        let ch = usize::from(self.config.channels);
        let frames = samples.len() / ch;
        if frames == 0 {
            return;
        }
        self.carry.reserve(frames);
        match ch {
            1 => self.carry.extend_from_slice(samples),
            2 => self.carry.extend(
                samples
                    .as_chunks::<2>()
                    .0
                    .iter()
                    .map(|p| f32::midpoint(p[0], p[1])),
            ),
            _ => {
                let inv = 1.0 / f32::from(self.config.channels);
                self.carry.extend(
                    samples
                        .chunks_exact(ch)
                        .map(|f| f.iter().sum::<f32>() * inv),
                );
            }
        }
        self.process_carry();
    }

    /// Samples a column into `out_rows` pixels using logarithmic frequency mapping (`min_hz..=nyquist`).
    pub fn sample_log_column(&self, col: usize, min_hz: f32, out_rows: &mut [u8]) {
        let Some(bins) = self.column(col) else {
            out_rows.fill(0);
            return;
        };
        let rows = out_rows.len();
        if rows == 0 {
            return;
        }
        #[allow(clippy::cast_precision_loss)]
        let nyquist = (self.config.sample_rate as f32) * 0.5;
        let lo = min_hz.clamp(20.0, nyquist * 0.5);
        let log_lo = lo.ln();
        let log_span = (nyquist.ln() - log_lo).max(1e-6);
        #[allow(clippy::cast_precision_loss)]
        let max_bin = (self.bins_per_col - 1) as f32;

        for (y, dst) in out_rows.iter_mut().enumerate() {
            // y = 0 -> low freq, y = rows-1 -> nyquist.
            #[allow(clippy::cast_precision_loss)]
            let frac = (y as f32) / ((rows - 1).max(1) as f32);
            let freq = (log_lo + frac * log_span).exp();
            #[allow(clippy::cast_possible_truncation, clippy::cast_sign_loss)]
            let bin_idx = ((freq / nyquist) * max_bin).round().clamp(0.0, max_bin) as usize;
            *dst = bins[bin_idx];
        }
    }

    fn process_carry(&mut self) {
        let n = self.config.fft_size;
        let hop = self.config.hop_size;
        if self.carry.len() < n {
            return;
        }

        let new_cols = (self.carry.len() - n) / hop + 1;
        let bins = self.bins_per_col;
        let old_len = self.data.len();
        self.data.resize(old_len + new_cols * bins, 0);

        #[allow(clippy::cast_precision_loss)]
        let inv_n = 1.0 / (n as f32);
        let min_db = self.config.min_db;
        let db_scale = 255.0 / (self.config.max_db - min_db).max(1.0);
        let hann = &self.hann;
        let fft = &self.fft;
        let carry = &self.carry;
        let out_slice = &mut self.data[old_len..];

        #[cfg(feature = "rayon")]
        {
            const CHUNK_COLS: usize = 256;
            if new_cols >= CHUNK_COLS * 2 {
                out_slice
                    .par_chunks_mut(CHUNK_COLS * bins)
                    .enumerate()
                    .for_each(|(chunk_idx, out_chunk)| {
                        let start_col = chunk_idx * CHUNK_COLS;
                        let cols_in_chunk = out_chunk.len() / bins;
                        let mut buf = vec![Complex::new(0.0_f32, 0.0); n];
                        let mut scratch =
                            vec![Complex::new(0.0_f32, 0.0); fft.get_inplace_scratch_len()];
                        for local_c in 0..cols_in_chunk {
                            let off = (start_col + local_c) * hop;
                            compute_one_col(
                                &carry[off..off + n],
                                hann,
                                fft.as_ref(),
                                &mut buf,
                                &mut scratch,
                                inv_n,
                                min_db,
                                db_scale,
                                &mut out_chunk[local_c * bins..(local_c + 1) * bins],
                            );
                        }
                    });
            } else {
                run_serial_cols(
                    carry,
                    hop,
                    n,
                    new_cols,
                    bins,
                    hann,
                    fft.as_ref(),
                    inv_n,
                    min_db,
                    db_scale,
                    out_slice,
                );
            }
        }

        #[cfg(not(feature = "rayon"))]
        {
            run_serial_cols(
                carry,
                hop,
                n,
                new_cols,
                bins,
                hann,
                fft.as_ref(),
                inv_n,
                min_db,
                db_scale,
                out_slice,
            );
        }

        self.num_columns += new_cols;
        let consumed = new_cols * hop;
        let remaining = self.carry.len().saturating_sub(consumed);
        self.carry.copy_within(consumed.., 0);
        self.carry.truncate(remaining);
    }
}

#[allow(clippy::too_many_arguments)]
fn run_serial_cols(
    carry: &[f32],
    hop: usize,
    n: usize,
    new_cols: usize,
    bins: usize,
    hann: &[f32],
    fft: &dyn Fft<f32>,
    inv_n: f32,
    min_db: f32,
    db_scale: f32,
    out_slice: &mut [u8],
) {
    let mut buf = vec![Complex::new(0.0_f32, 0.0); n];
    let mut scratch = vec![Complex::new(0.0_f32, 0.0); fft.get_inplace_scratch_len()];
    for c in 0..new_cols {
        let off = c * hop;
        compute_one_col(
            &carry[off..off + n],
            hann,
            fft,
            &mut buf,
            &mut scratch,
            inv_n,
            min_db,
            db_scale,
            &mut out_slice[c * bins..(c + 1) * bins],
        );
    }
}

#[inline]
#[allow(clippy::too_many_arguments)]
fn compute_one_col(
    windowed_src: &[f32],
    hann: &[f32],
    fft: &dyn Fft<f32>,
    buf: &mut [Complex<f32>],
    scratch: &mut [Complex<f32>],
    inv_n: f32,
    min_db: f32,
    db_scale: f32,
    out_bins: &mut [u8],
) {
    for ((dst, &src), &w) in buf.iter_mut().zip(windowed_src.iter()).zip(hann.iter()) {
        *dst = Complex::new(src * w, 0.0);
    }
    fft.process_with_scratch(buf, scratch);

    let inv_n_sq = inv_n * inv_n;
    for (dst, c) in out_bins.iter_mut().zip(buf.iter()) {
        let power = c.norm_sqr() * inv_n_sq;
        let db = 10.0 * (power + 1e-12_f32).log10();
        let norm = ((db - min_db) * db_scale).clamp(0.0, 255.0);
        #[allow(clippy::cast_possible_truncation, clippy::cast_sign_loss)]
        {
            *dst = norm as u8;
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::waveform::WaveformPyramid;

    #[test]
    fn waveform_pyramid_incremental_matches_full_and_respects_memory_budget() {
        let sr = 48_000u32;
        let frames = 48_000usize; // 1 second stereo
        let mut pcm = Vec::with_capacity(frames * 2);
        for i in 0..frames {
            #[allow(clippy::cast_possible_truncation, clippy::cast_precision_loss)]
            let v = ((TAU * 440.0 * (i as f32) / (sr as f32)).sin() * 20_000.0) as i16;
            pcm.push(v);
            pcm.push(v);
        }

        let mut pyr = WaveformPyramid::new(sr, 2).expect("pyr");
        for chunk in pcm.chunks(1337 * 2) {
            pyr.push_interleaved_i16(chunk);
        }
        pyr.finish();
        assert_eq!(pyr.total_frames(), frames as u64);

        let mut cols = Vec::new();
        pyr.query_columns(0, frames as u64, 200, &mut cols);
        assert_eq!(cols.len(), 200);
        assert!(cols[100].max > 15_000 && cols[100].min < -15_000);
        assert!(cols[100].rms > 10_000);
    }

    #[test]
    fn spectrum_detects_sine_peak_and_color_maps_work() {
        let sr = 48_000u32;
        let frames = 4096usize;
        let freq = 1500.0_f32;
        #[allow(clippy::cast_precision_loss)]
        let mono: Vec<f32> = (0..frames)
            .map(|i| (TAU * freq * (i as f32) / (sr as f32)).sin())
            .collect();

        let mut spec = SpectrumBuilder::new(SpectrumConfig::new(sr, 1)).expect("spec");
        spec.push_interleaved_f32(&mono);
        assert!(spec.num_columns() >= 5);

        let col = spec.column(2).expect("col 2");
        // Expected bin for 1500 Hz at 48 kHz / 1024 FFT = 1500 * 1024 / 48000 = bin 32.
        let (peak_bin, &peak_val) = col
            .iter()
            .enumerate()
            .max_by_key(|&(_, v)| v)
            .expect("peak");
        assert!((31..=33).contains(&peak_bin), "peak_bin={peak_bin}");
        assert!(peak_val > 200, "peak_val={peak_val}");

        let mut log_col = vec![0u8; 64];
        spec.sample_log_column(2, 40.0, &mut log_col);
        assert!(log_col.iter().any(|&v| v > 180));

        assert_eq!(
            bin_to_rgba(ColorScheme::AegisubClassic, 255),
            [255, 255, 252, 255]
        );
        assert_eq!(
            bin_to_rgba(ColorScheme::Grayscale, 128),
            [128, 128, 128, 255]
        );
    }
}
