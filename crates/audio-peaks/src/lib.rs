//! Pure-Rust audio waveform peak pyramid and STFT spectrogram DSP (zero audio I/O).
//!
//! Reference behaviour (Aegisub 3.5.1 `src/audio_renderer_waveform.cpp` &
//! `src/audio_renderer_spectrum.cpp`):
//! - Incremental multi-resolution min/max/RMS waveform peak pyramid (< 2.5 MB for 24 min 48 kHz).
//! - Incremental Hann-windowed STFT spectrum (`rustfft`, optional `rayon` parallelism) with
//!   dB quantization to `u8`, log-frequency mapping, and Aegisub-style color palettes.

#![forbid(unsafe_code)]
#![cfg_attr(not(test), deny(clippy::unwrap_used, clippy::expect_used))]

pub mod spectrum;
pub mod waveform;

use thiserror::Error;

pub use spectrum::{bin_to_rgba, ColorScheme, SpectrumBuilder, SpectrumConfig};
pub use waveform::{PeakBucket, WaveformPyramid};

/// Errors returned by `audio-peaks`.
#[derive(Debug, Error)]
pub enum AudioPeaksError {
    #[error("invalid channel count {0} (must be >= 1)")]
    InvalidChannels(u16),
    #[error("invalid sample rate {0} Hz (must be in 8000..=192000)")]
    InvalidSampleRate(u32),
    #[error("invalid FFT size {0} (must be a power of two in 256..=4096)")]
    InvalidFftSize(usize),
    #[error("invalid hop size {0} (must be in 1..=fft_size)")]
    InvalidHopSize(usize),
}
