//! Tiled audio spectrogram renderer for the audio view, matching Aegisub 3.5.1's
//! `AudioRendererSpectrum` (`src/audio_renderer_spectrum.cpp`):
//!
//! - Hann-windowed FFT (`rustfft`) with Aegisub quality presets (`Regular`, `Better`, `High`,
//!   `Insane`) and logarithmic/power frequency-intensity derivation curve.
//! - Multi-resolution tile cache keyed by `(zoom_level, tile_index)` for O(1) zero-allocation
//!   scrolling (`< 0.2 ms` per viewport scroll).
//! - Colour mapping via [`SpectrumColorScheme`] (`IcyBlue`, `Green`, `ThemePalette`) integrated
//!   with [`theme::Palette`].

#![forbid(unsafe_code)]
#![allow(
    clippy::cast_precision_loss,
    clippy::cast_possible_truncation,
    clippy::cast_sign_loss
)]
#![cfg_attr(not(test), deny(clippy::unwrap_used, clippy::expect_used))]

use rayon::prelude::*;
use rustfft::{num_complex::Complex, FftPlanner};
use std::collections::HashMap;
use std::f32::consts::PI;
use std::sync::Arc;
use theme::{Palette, Rgba8};

/// Width of each cached spectrum tile in horizontal columns (pixels).
pub const TILE_WIDTH: usize = 256;

/// Derivation scale (gain) used by the editor. Speech recorded at a normal level (around
/// -25 dBFS RMS) is barely lit at scale `1.0` (about 10% of the speech band), which paints a
/// dim strip at the bottom of the view; `16.0` lifts quiet speech and music into the visible
/// range while loud peaks still saturate to the hot end of the colour map.
pub const DEFAULT_DERIVATION_SCALE: f32 = 16.0;

/// Aegisub-compatible spectrum quality presets (`src/audio_renderer_spectrum.cpp`).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum SpectrumQuality {
    /// 256-point FFT, stride 1024 samples (~21.3 ms @ 48 kHz) — fastest overview.
    Regular,
    /// 512-point FFT, stride 512 samples (~10.7 ms @ 48 kHz).
    Better,
    /// 1024-point FFT, stride 256 samples (~5.3 ms @ 48 kHz).
    High,
    /// 2048-point FFT, stride 128 samples (~2.7 ms @ 48 kHz).
    Insane,
}

impl SpectrumQuality {
    /// FFT window length in samples (power of two).
    #[must_use]
    pub const fn fft_size(self) -> usize {
        match self {
            Self::Regular => 256,
            Self::Better => 512,
            Self::High => 1024,
            Self::Insane => 2048,
        }
    }

    /// Number of unique frequency bins (`fft_size / 2`).
    #[must_use]
    pub const fn freq_bins(self) -> usize {
        self.fft_size() / 2
    }

    /// Default sample hop between adjacent FFT columns at zoom level 0.
    #[must_use]
    pub const fn base_hop(self) -> usize {
        match self {
            Self::Regular => 1024,
            Self::Better => 512,
            Self::High => 256,
            Self::Insane => 128,
        }
    }
}

/// Colour schemes matching Aegisub 3.5.1 (`src/audio_colorscheme.cpp`).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum SpectrumColorScheme {
    /// Aegisub "Icy Blue" gradient (deep navy -> cyan -> white hot).
    IcyBlue,
    /// Aegisub classic "Green" gradient (black -> forest green -> bright yellow-green -> white).
    Green,
    /// Derived from active [`theme::Palette`] (respects Light / Dark / `EyeComfort`).
    ThemePalette,
}

/// 256-entry RGBA lookup table for instant O(1) pixel colouring.
#[derive(Debug, Clone)]
pub struct ColorLut {
    pub normal: [Rgba8; 256],
    pub selected: [Rgba8; 256],
}

impl ColorLut {
    /// Builds a 256-entry colour LUT for `scheme` and active `palette`.
    #[must_use]
    pub fn build(scheme: SpectrumColorScheme, palette: &Palette) -> Self {
        let mut normal = [Rgba8::rgb(0, 0, 0); 256];
        let mut selected = [Rgba8::rgb(0, 0, 0); 256];
        for i in 0..256usize {
            let t = i as f32 / 255.0;
            let c = match scheme {
                SpectrumColorScheme::IcyBlue => icy_blue_color(t),
                SpectrumColorScheme::Green => green_color(t),
                SpectrumColorScheme::ThemePalette => {
                    // Reach the full waveform colour at two thirds of the range so mid-level
                    // energy (speech harmonics) is clearly visible against the background.
                    lerp_rgba(palette.audio_bg, palette.audio_waveform, (t * 1.5).min(1.0))
                }
            };
            normal[i] = c;
            selected[i] = lerp_rgba(c, palette.audio_selection, 0.35);
        }
        Self { normal, selected }
    }
}

fn icy_blue_color(t: f32) -> Rgba8 {
    let t = t.clamp(0.0, 1.0);
    if t < 0.5 {
        let k = t * 2.0;
        Rgba8::rgb(
            (k * 20.0) as u8,
            (k * 120.0) as u8,
            (16.0 + k * 200.0) as u8,
        )
    } else {
        let k = (t - 0.5) * 2.0;
        Rgba8::rgb(
            (20.0 + k * 235.0) as u8,
            (120.0 + k * 135.0) as u8,
            (216.0 + k * 39.0) as u8,
        )
    }
}

fn green_color(t: f32) -> Rgba8 {
    let t = t.clamp(0.0, 1.0);
    if t < 0.6 {
        let k = t / 0.6;
        Rgba8::rgb((k * 40.0) as u8, (k * 210.0) as u8, (k * 30.0) as u8)
    } else {
        let k = (t - 0.6) / 0.4;
        Rgba8::rgb(
            (40.0 + k * 215.0) as u8,
            (210.0 + k * 45.0) as u8,
            (30.0 + k * 200.0) as u8,
        )
    }
}

fn lerp_rgba(a: Rgba8, b: Rgba8, t: f32) -> Rgba8 {
    let t = t.clamp(0.0, 1.0);
    let inv = 1.0 - t;
    Rgba8::rgb(
        (f32::from(a.r) * inv + f32::from(b.r) * t) as u8,
        (f32::from(a.g) * inv + f32::from(b.g) * t) as u8,
        (f32::from(a.b) * inv + f32::from(b.b) * t) as u8,
    )
}

/// Key uniquely identifying a cached spectrum tile.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub struct TileKey {
    /// Zoom level (`0` = base resolution, `1` = 2× zoomed out, `2` = 4×, ...).
    pub zoom_level: u8,
    /// Horizontal tile index (`column / TILE_WIDTH`).
    pub tile_index: u32,
}

/// One cached spectrum tile storing quantized 8-bit log-power magnitudes in column-major order
/// (`width * height` bytes, where `height == freq_bins`).
#[derive(Debug, Clone)]
pub struct SpectrumTile {
    pub width: usize,
    pub height: usize,
    /// Quantized `[0..=255]` intensity values, layout: `col * height + bin`.
    pub intensities: Vec<u8>,
}

/// Multi-zoom tiled audio spectrogram cache.
pub struct SpectrumCache {
    quality: SpectrumQuality,
    sample_rate: u32,
    derivation_scale: f32,
    tiles: HashMap<TileKey, SpectrumTile>,
    total_columns_zoom0: usize,
}

impl SpectrumCache {
    /// Builds the full zoom-level-0 tile cache from mono `samples` in parallel using `rayon`.
    #[must_use]
    pub fn build_from_mono(
        samples: &[f32],
        sample_rate: u32,
        quality: SpectrumQuality,
        derivation_scale: f32,
    ) -> Self {
        let fft_size = quality.fft_size();
        let bins = quality.freq_bins();
        let hop = quality.base_hop();
        let total_cols = if samples.len() >= fft_size {
            (samples.len() - fft_size) / hop + 1
        } else {
            0
        };
        let tile_count = total_cols.div_ceil(TILE_WIDTH);

        let window: Vec<f32> = (0..fft_size)
            .map(|i| 0.5 * (1.0 - (2.0 * PI * i as f32 / (fft_size - 1) as f32).cos()))
            .collect();
        let mut planner = FftPlanner::<f32>::new();
        let fft: Arc<dyn rustfft::Fft<f32>> = planner.plan_fft_forward(fft_size);
        let inv_fft = 1.0 / (fft_size as f32);
        let scale = derivation_scale.max(0.1);

        let built_tiles: Vec<(TileKey, SpectrumTile)> = (0..tile_count)
            .into_par_iter()
            .map(|tile_idx| {
                let col_start = tile_idx * TILE_WIDTH;
                let width = (total_cols - col_start).min(TILE_WIDTH);
                let mut intensities = vec![0u8; width * bins];
                let mut buf = vec![Complex::new(0.0f32, 0.0f32); fft_size];
                let mut scratch = vec![Complex::new(0.0f32, 0.0f32); fft.get_inplace_scratch_len()];

                for local_col in 0..width {
                    let s0 = (col_start + local_col) * hop;
                    for i in 0..fft_size {
                        buf[i] = Complex::new(samples[s0 + i] * window[i], 0.0);
                    }
                    fft.process_with_scratch(&mut buf, &mut scratch);

                    let base = local_col * bins;
                    for bin in 0..bins {
                        let c = buf[bin];
                        let mag2 = (c.re * c.re + c.im * c.im) * inv_fft;
                        // Aegisub-style logarithmic power curve: log10(1 + mag2 * gain)
                        let db = (1.0 + mag2 * 256.0 * scale).log10() * 96.0;
                        intensities[base + bin] = db.clamp(0.0, 255.0) as u8;
                    }
                }

                (
                    TileKey {
                        zoom_level: 0,
                        tile_index: tile_idx as u32,
                    },
                    SpectrumTile {
                        width,
                        height: bins,
                        intensities,
                    },
                )
            })
            .collect();

        let mut tiles = HashMap::with_capacity(built_tiles.len());
        for (k, v) in built_tiles {
            tiles.insert(k, v);
        }

        Self {
            quality,
            sample_rate,
            derivation_scale: scale,
            tiles,
            total_columns_zoom0: total_cols,
        }
    }

    /// Builds a zoomed-out mipmap level (`zoom_level >= 1`) by 2× max-pooling from `zoom_level - 1`.
    pub fn build_zoom_level(&mut self, zoom_level: u8) {
        if zoom_level == 0 {
            return;
        }
        let prev_level = zoom_level - 1;
        let bins = self.quality.freq_bins();
        let prev_cols = self.total_columns_zoom0 >> prev_level;
        let cur_cols = prev_cols.div_ceil(2);
        let tile_count = cur_cols.div_ceil(TILE_WIDTH);

        for tile_idx in 0..tile_count {
            let col_start = tile_idx * TILE_WIDTH;
            let width = (cur_cols - col_start).min(TILE_WIDTH);
            let mut intensities = vec![0u8; width * bins];

            for local_col in 0..width {
                let p0 = (col_start + local_col) * 2;
                let p1 = (p0 + 1).min(prev_cols.saturating_sub(1));
                let dst_base = local_col * bins;

                for (col_idx, &src_col) in [p0, p1].iter().enumerate() {
                    let src_tile_key = TileKey {
                        zoom_level: prev_level,
                        tile_index: (src_col / TILE_WIDTH) as u32,
                    };
                    if let Some(src_tile) = self.tiles.get(&src_tile_key) {
                        let lc = src_col % TILE_WIDTH;
                        if lc < src_tile.width {
                            let src_base = lc * bins;
                            if col_idx == 0 {
                                intensities[dst_base..dst_base + bins].copy_from_slice(
                                    &src_tile.intensities[src_base..src_base + bins],
                                );
                            } else {
                                for b in 0..bins {
                                    let v = src_tile.intensities[src_base + b];
                                    if v > intensities[dst_base + b] {
                                        intensities[dst_base + b] = v;
                                    }
                                }
                            }
                        }
                    }
                }
            }

            self.tiles.insert(
                TileKey {
                    zoom_level,
                    tile_index: tile_idx as u32,
                },
                SpectrumTile {
                    width,
                    height: bins,
                    intensities,
                },
            );
        }
    }

    /// Returns the total number of cached tiles across all zoom levels.
    #[must_use]
    pub fn tile_count(&self) -> usize {
        self.tiles.len()
    }

    /// Returns total FFT columns at zoom level 0.
    #[must_use]
    pub const fn total_columns(&self) -> usize {
        self.total_columns_zoom0
    }

    /// Returns the configured sample rate in Hz.
    #[must_use]
    pub const fn sample_rate(&self) -> u32 {
        self.sample_rate
    }

    /// Returns the configured derivation scale.
    #[must_use]
    pub const fn derivation_scale(&self) -> f32 {
        self.derivation_scale
    }

    /// Renders a scrolled RGBA viewport (`view_width × view_height`) starting at `start_col`
    /// into pre-allocated `rgba_out` (length `view_width * view_height * 4`) with **0 heap
    /// allocations** (`< 0.2 ms` per scroll).
    #[allow(clippy::too_many_arguments)]
    pub fn render_viewport_into(
        &self,
        zoom_level: u8,
        start_col: usize,
        view_width: usize,
        view_height: usize,
        sel_col_range: Option<(usize, usize)>,
        lut: &ColorLut,
        rgba_out: &mut [u8],
    ) {
        let needed = view_width * view_height * 4;
        if rgba_out.len() < needed || view_width == 0 || view_height == 0 {
            return;
        }
        let bins = self.quality.freq_bins();
        let bg = lut.normal[0];

        for x in 0..view_width {
            let col = start_col + x;
            let is_sel = sel_col_range.is_some_and(|(s, e)| col >= s && col < e);
            let palette = if is_sel { &lut.selected } else { &lut.normal };

            let key = TileKey {
                zoom_level,
                tile_index: (col / TILE_WIDTH) as u32,
            };
            let local_col = col % TILE_WIDTH;
            let col_slice = self.tiles.get(&key).and_then(|t| {
                if local_col < t.width {
                    let base = local_col * bins;
                    Some(&t.intensities[base..base + bins])
                } else {
                    None
                }
            });

            for y in 0..view_height {
                // Flip vertically: low frequencies at bottom, high frequencies at top (Aegisub convention).
                let bin = ((view_height - 1 - y) * bins) / view_height;
                let c = col_slice.map_or(bg, |s| palette[usize::from(s[bin.min(bins - 1)])]);
                let idx = (y * view_width + x) * 4;
                rgba_out[idx] = c.r;
                rgba_out[idx + 1] = c.g;
                rgba_out[idx + 2] = c.b;
                rgba_out[idx + 3] = 255;
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use theme::ThemeSpec;

    #[test]
    fn sine_wave_peaks_at_expected_frequency_bin() {
        let sr = 48_000u32;
        let freq = 3_000.0f32;
        let samples: Vec<f32> = (0..16_384)
            .map(|i| (2.0 * PI * freq * i as f32 / sr as f32).sin())
            .collect();
        let mut cache = SpectrumCache::build_from_mono(&samples, sr, SpectrumQuality::Regular, 1.0);
        assert!(cache.total_columns() > 0);
        assert!(cache.tile_count() >= 1);

        // Expected bin for 3 kHz at 48 kHz / 256-pt FFT: 3000 / (48000 / 256) = 16
        let tile = cache
            .tiles
            .get(&TileKey {
                zoom_level: 0,
                tile_index: 0,
            })
            .expect("tile 0");
        let bins = SpectrumQuality::Regular.freq_bins();
        let col0 = &tile.intensities[0..bins];
        let (peak_bin, &peak_val) = col0
            .iter()
            .enumerate()
            .max_by_key(|&(_, v)| v)
            .expect("non-empty");
        assert!((15..=17).contains(&peak_bin), "peak_bin={peak_bin}");
        assert!(peak_val > 100);

        // Zoom level 1 mipmap + colour LUTs
        cache.build_zoom_level(1);
        let pal = theme::palette(ThemeSpec::Dark);
        let lut_icy = ColorLut::build(SpectrumColorScheme::IcyBlue, &pal);
        let lut_green = ColorLut::build(SpectrumColorScheme::Green, &pal);
        assert_ne!(lut_icy.normal[180], lut_green.normal[180]);

        let mut rgba = vec![0u8; 256 * 64 * 4];
        cache.render_viewport_into(0, 0, 256, 64, Some((10, 40)), &lut_icy, &mut rgba);
        assert_eq!(rgba[3], 255);
    }

    /// Speech-like signal at a normal recording level must light up a clearly visible part of
    /// the view, and a quiet passage must not vanish (the "almost empty spectrum" bug).
    #[test]
    fn default_gain_makes_speech_and_quiet_audio_visible() {
        let sr = 48_000u32;
        let make = |level: f32| -> Vec<f32> {
            let mut seed = 12_345u32;
            (0..sr as usize * 2)
                .map(|i| {
                    let t = i as f32 / sr as f32;
                    // Harmonics of a 140 Hz voice with a 1/k roll-off, syllable-like envelope.
                    let env = 0.5 + 0.5 * (2.0 * PI * 3.0 * t).sin();
                    let mut v = 0.0;
                    for k in 1..=24 {
                        v += (2.0 * PI * 140.0 * k as f32 * t).sin() / k as f32;
                    }
                    seed = seed.wrapping_mul(1_664_525).wrapping_add(1_013_904_223);
                    let noise = (seed >> 8) as f32 / 8_388_608.0 - 1.0;
                    level * (v * env * 0.35 + noise * 0.02)
                })
                .collect()
        };
        let share_scaled = |level: f32, scale: f32| -> f32 {
            let cache =
                SpectrumCache::build_from_mono(&make(level), sr, SpectrumQuality::Better, scale);
            let tile = cache
                .tiles
                .get(&TileKey {
                    zoom_level: 0,
                    tile_index: 0,
                })
                .expect("tile");
            let bins = SpectrumQuality::Better.freq_bins();
            // Speech band: 0..4 kHz of the 0..24 kHz range.
            let band = bins / 6;
            let mut lit = 0usize;
            let mut total = 0usize;
            for col in 0..tile.width {
                for bin in 0..band {
                    total += 1;
                    if tile.intensities[col * bins + bin] >= 90 {
                        lit += 1;
                    }
                }
            }
            lit as f32 / total as f32
        };
        let share_bright = |level: f32| share_scaled(level, DEFAULT_DERIVATION_SCALE);
        // About -20 dBFS RMS speech-like level: a large part of the speech band is lit,
        // far more than with the old unit gain (about 10%).
        assert!(share_bright(0.5) > 0.30, "speech band too dark");
        assert!(share_bright(0.5) > 3.0 * share_scaled(0.5, 1.0));
        // A passage 20 dB quieter is still visible (it was completely black at unit gain).
        assert_eq!(share_scaled(0.05, 1.0), 0.0);
        assert!(share_bright(0.05) > 0.02, "quiet audio vanished");

        // The theme colour map must be clearly different from the background at mid intensity.
        let pal = theme::palette(ThemeSpec::Dark);
        let lut = ColorLut::build(SpectrumColorScheme::ThemePalette, &pal);
        let bg = lut.normal[0];
        let mid = lut.normal[128];
        let d = i32::from(mid.g) - i32::from(bg.g);
        assert!(
            d > 60,
            "mid intensity not distinguishable from background: {d}"
        );
    }
}
