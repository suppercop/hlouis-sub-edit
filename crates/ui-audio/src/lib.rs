//! Audio waveform / spectrum panel (`egui 0.37`) built on [`audio_peaks`], [`audio_view`],
//! [`timing_controller`], [`karaoke_timing`], and [`theme::Palette`].
//!
//! Mirrors Aegisub 3.2.2 / 3.5.1 `src/audio_display.cpp` & `src/audio_box.cpp`:
//! - Dark top time ruler with Aegisub labels (`0:05:12`, then `13`, `14`, ...).
//! - Waveform or Aegisub spectrum palette (dark blue background, cyan/white energy) rendered into
//!   a cached [`egui::TextureHandle`] with strict dirty-key tracking (0 re-uploads when idle).
//! - Dark horizontal scrollbar under the waveform/spectrum with a draggable blue thumb.
//! - Right-hand 3 aligned vertical sliders (H-zoom, V-zoom, Volume) with blue thumbs + pink/red
//!   `Link` toggle button directly below them.
//! - ~20 coloured 16×16 Aegisub audio toolbar icons & toggles (prev/next line, play selection,
//!   play line, stop, play before/after start, first/last 500 ms, play to end, commit ✓, go to
//!   next, lead-in/out, auto-commit, auto-next, auto-scroll, spectrum, karaoke, timing options).

#![forbid(unsafe_code)]
#![allow(clippy::cast_precision_loss)]
#![cfg_attr(not(test), deny(clippy::unwrap_used, clippy::expect_used))]

use audio_peaks::PeakBucket;
use audio_view::{
    AudioFrameInput, AudioFrameLayout, AudioViewport, ColorRole, HitTarget, TimelineMarkerIndex,
    build_frame_layout, compute_ruler_ticks,
};
use egui::{
    Align2, Color32, ColorImage, FontId, Galley, Pos2, Rect, Sense, Stroke, TextureHandle,
    TextureOptions, Ui, Vec2,
};
use i18n_catalog::Catalog;
use karaoke_timing::KaraokeTimingController;
use spectrum::{ColorLut, RealtimeSpectrumRenderer, SpectrumColorScheme, SpectrumViewportSpec};
use std::fmt::Write as _;
use std::ops::Range;
use std::sync::Arc;
use theme::{Palette, Rgba8};
use timing_controller::{
    DialogueTimingController, DragTarget, PlayRangeKind, TimeRangeMs, threshold_ms_from_px,
};
use ui_theme::{rgba, snap_px};

/// Trait supplying waveform peak columns (and optional mono PCM samples for the real FFT
/// spectrogram) for a requested `[start_ms, end_ms]` range into caller-owned buffers, and
/// optionally pre-rendered STFT spectrum pixels.
pub trait AudioColumnProvider {
    /// Total audio duration in milliseconds.
    fn duration_ms(&self) -> i64;
    /// Populates `out` with `width_px` [`PeakBucket`]s covering `[start_ms, end_ms]`.
    fn query_columns_into(
        &self,
        start_ms: i64,
        end_ms: i64,
        width_px: usize,
        out: &mut Vec<PeakBucket>,
    );
    /// Optional spectrum renderer writing `width_px * height_px` row-major [`Color32`] pixels
    /// for `[start_ms, end_ms]` into `out`. Returns `true` if filled, else falls back to peaks.
    fn render_spectrum_into(
        &self,
        _start_ms: i64,
        _end_ms: i64,
        _width_px: usize,
        _height_px: usize,
        _palette: &Palette,
        _out: &mut Vec<Color32>,
    ) -> bool {
        false
    }
    /// Sample rate in Hz (`0` when raw PCM is unavailable, falling back to the provider's own
    /// spectrum renderer / peak columns).
    fn sample_rate(&self) -> u32 {
        0
    }
    /// Reads up to `count` mono `i16` PCM samples starting at `start_sample` into `out`.
    fn read_mono_i16(&self, _start_sample: usize, _count: usize, out: &mut Vec<i16>) {
        out.clear();
    }
}

/// Action emitted by the audio panel toolbar or mouse interactions.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum AudioPanelAction {
    PrevLine,
    NextLine,
    Play(PlayRangeKind),
    PlayToEnd,
    Stop,
    Commit,
    CommitAndNext,
    AddLeadIn,
    AddLeadOut,
    OpenTimingOptions,
    SelectionChanged(TimeRangeMs),
}

/// Active magnetic snap target shown while dragging a timing boundary.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct SnapHint {
    pub time_ms: i64,
    pub is_keyframe: bool,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
struct TextureCacheKey {
    start_ms: i64,
    end_ms: i64,
    width_px: usize,
    height_px: usize,
    spectrum_mode: bool,
    v_zoom_bits: u32,
    ms_per_px_bits: u64,
    palette_rev: u64,
}

/// Baked ruler tick texture cache key.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
struct RulerCacheKey {
    start_ms: i64,
    end_ms: i64,
    phys_w: usize,
    phys_h: usize,
    palette_rev: u64,
    ppp_bits: u32,
}

/// Ruler label cache key; `generation` is the [`ui_theme::GalleyEpoch`] generation so labels laid
/// out before an atlas reset are never reused.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
struct RulerLabelKey {
    start_ms: i64,
    end_ms: i64,
    ms_per_px_bits: u64,
    width_bits: u32,
    ppp_bits: u32,
    generation: u64,
}

/// Persistent UI state + cached waveform texture + reusable scratch buffers for the audio panel.
#[allow(clippy::struct_excessive_bools)]
pub struct AudioPanelState {
    pub viewport: AudioViewport,
    pub spectrum_mode: bool,
    pub karaoke_mode: bool,
    pub auto_commit: bool,
    pub auto_next: bool,
    pub auto_scroll: bool,
    pub auto_snap_keyframes: bool,
    pub snap_to_lines: bool,
    pub snap_distance_px: f32,
    pub h_zoom: f32,
    pub v_zoom: f32,
    pub volume: f32,
    pub link_zoom_volume: bool,
    pub play_cursor_ms: Option<i64>,
    pub video_marker_ms: Option<i64>,
    pub active_selection: TimeRangeMs,
    pub inactive_spans: Vec<TimeRangeMs>,
    pub dragging: Option<DragTarget>,
    pub active_snap_hint: Option<SnapHint>,
    // Cached GPU texture + dirty key + ring-buffer head (0 re-uploads when unchanged,
    // partial-strip set_partial on horizontal scroll instead of full-buffer clone).
    texture: Option<TextureHandle>,
    cache_key: Option<TextureCacheKey>,
    ring_head: usize,
    texture_uploads: u64,
    // Cached physical-resolution time ruler tick texture (0 per-frame tick line segments).
    ruler_texture: Option<TextureHandle>,
    ruler_cache_key: Option<RulerCacheKey>,
    ruler_pixel_buf: Vec<Color32>,
    // Reusable scratch buffers (0 heap allocations in steady state).
    columns_buf: Vec<PeakBucket>,
    partial_columns_buf: Vec<PeakBucket>,
    pixel_buf: Vec<Color32>,
    partial_pixel_buf: Vec<Color32>,
    spectrum_renderer: RealtimeSpectrumRenderer,
    spectrum_lut: Option<(u64, ColorLut)>,
    spectrum_rgba_buf: Vec<Rgba8>,
    frame_layout: AudioFrameLayout,
    ruler_buf: String,
    last_computed_columns: usize,
    total_computed_columns: u64,
    galley_epoch: ui_theme::GalleyEpoch,
    ruler_key: Option<RulerLabelKey>,
    ruler_ticks: Vec<(f32, Arc<Galley>)>,
}

impl Default for AudioPanelState {
    fn default() -> Self {
        Self::new(60_000)
    }
}

impl AudioPanelState {
    /// Drops the cached waveform/spectrum texture so the next frame re-queries the provider
    /// (call when more decoded audio became available for the already visible range).
    pub fn invalidate_texture(&mut self) {
        self.cache_key = None;
    }

    /// Creates an audio panel state initialized for `duration_ms`.
    #[must_use]
    pub fn new(duration_ms: i64) -> Self {
        Self {
            viewport: AudioViewport::new(800.0, 140.0, duration_ms.max(1_000)),
            spectrum_mode: true,
            karaoke_mode: false,
            auto_commit: false,
            auto_next: true,
            auto_scroll: true,
            auto_snap_keyframes: true,
            snap_to_lines: true,
            snap_distance_px: 10.0,
            // Matches the viewport default of 10 ms/px through `ms_per_px = 12 / h_zoom`.
            h_zoom: 3.2,
            v_zoom: 1.4,
            volume: 1.2,
            link_zoom_volume: true,
            play_cursor_ms: None,
            video_marker_ms: None,
            active_selection: TimeRangeMs {
                start_ms: 0,
                end_ms: 2_000,
            },
            inactive_spans: Vec::with_capacity(4),
            dragging: None,
            active_snap_hint: None,
            texture: None,
            cache_key: None,
            ring_head: 0,
            texture_uploads: 0,
            ruler_texture: None,
            ruler_cache_key: None,
            ruler_pixel_buf: Vec::with_capacity(2048 * 32),
            columns_buf: Vec::with_capacity(2048),
            partial_columns_buf: Vec::with_capacity(256),
            pixel_buf: Vec::with_capacity(2048 * 160),
            partial_pixel_buf: Vec::with_capacity(256 * 160),
            spectrum_renderer: RealtimeSpectrumRenderer::new(512),
            spectrum_lut: None,
            spectrum_rgba_buf: Vec::with_capacity(2048 * 160),
            frame_layout: AudioFrameLayout::with_capacity(2048),
            ruler_buf: String::with_capacity(16),
            last_computed_columns: 0,
            total_computed_columns: 0,
            galley_epoch: ui_theme::GalleyEpoch::default(),
            ruler_key: None,
            ruler_ticks: Vec::with_capacity(32),
        }
    }

    /// Number of texture uploads performed so far (0 on idle/unchanged frames).
    #[inline]
    #[must_use]
    pub const fn texture_uploads(&self) -> u64 {
        self.texture_uploads
    }

    /// Number of audio columns computed during the most recent frame (`0` on idle frames,
    /// `N` on an `N`-pixel horizontal scroll).
    #[inline]
    #[must_use]
    pub const fn last_computed_columns(&self) -> usize {
        self.last_computed_columns
    }

    /// Cumulative count of audio columns computed across all frames.
    #[inline]
    #[must_use]
    pub const fn total_computed_columns(&self) -> u64 {
        self.total_computed_columns
    }

    fn ensure_spectrum_lut(&mut self, palette: &Palette, palette_rev: u64) {
        if self
            .spectrum_lut
            .as_ref()
            .is_none_or(|(r, _)| *r != palette_rev)
        {
            let lut = ColorLut::build(SpectrumColorScheme::Perceptual, palette);
            self.spectrum_lut = Some((palette_rev, lut));
        }
    }

    /// Rasterizes the full `0..width_px` waveform or fallback spectrum into `self.pixel_buf`
    /// without allocating once sized.
    pub fn rasterize_columns(&mut self, width_px: usize, height_px: usize, palette: &Palette) {
        self.rasterize_columns_range(width_px, height_px, 0..width_px, palette);
    }

    /// Rasterizes only the columns in `col_range` into `self.pixel_buf` (anti-aliased min/max
    /// waveform with RMS layer, subtle vertical gradient, and clear zero line; or perceptual
    /// spectrum fallback when raw PCM is unavailable). Zero heap allocations once sized.
    #[allow(clippy::too_many_lines)]
    pub fn rasterize_columns_range(
        &mut self,
        width_px: usize,
        height_px: usize,
        col_range: Range<usize>,
        palette: &Palette,
    ) {
        let total = width_px.saturating_mul(height_px);
        let bg_base = if self.spectrum_mode {
            Color32::from_rgb(11, 15, 26)
        } else {
            rgba(palette.audio_bg)
        };
        if self.pixel_buf.len() != total {
            self.pixel_buf.resize(total, bg_base);
        }
        if width_px == 0 || height_px == 0 {
            return;
        }
        let c0 = col_range.start.min(width_px);
        let c1 = col_range.end.min(width_px);
        if c0 >= c1 {
            return;
        }

        let mid_y_idx = height_px / 2;
        let mid_y = (height_px as f32) * 0.5;
        let wave_peak = rgba(palette.audio_waveform);
        let wave_rms = brighten_color(wave_peak, 34);
        let zero_line_col = blend_color(bg_base, rgba(palette.border), 0.42);

        // Perceptual eye-comfort stops used when spectrum mode falls back to peak columns.
        let stops: [Color32; 5] = [
            Color32::from_rgb(18, 34, 62),
            Color32::from_rgb(26, 78, 108),
            Color32::from_rgb(64, 132, 126),
            Color32::from_rgb(176, 136, 74),
            Color32::from_rgb(226, 206, 168),
        ];

        let max_y_f = (height_px - 1) as f32;
        let inv_mid = 1.0 / mid_y.max(1.0);

        for x in c0..c1 {
            let col = self.columns_buf.get(x).copied().unwrap_or(PeakBucket {
                min: 0,
                max: 0,
                rms: 0,
            });
            if self.spectrum_mode {
                for y in 0..height_px {
                    self.pixel_buf[y * width_px + x] = bg_base;
                }
                let max_f = f32::from(col.max) / 32_768.0;
                let min_f = f32::from(col.min) / 32_768.0;
                let amp = (max_f.abs().max(min_f.abs()) * self.v_zoom).clamp(0.0, 1.0);
                #[allow(clippy::cast_possible_truncation, clippy::cast_sign_loss)]
                let bar_h = ((amp * (height_px as f32)) as usize).min(height_px);
                for dy in 0..bar_h {
                    let y = height_px - 1 - dy;
                    let t = (dy as f32) / (height_px.max(1) as f32);
                    #[allow(clippy::cast_possible_truncation, clippy::cast_sign_loss)]
                    let stop_idx = ((t * 4.0) as usize).min(4);
                    self.pixel_buf[y * width_px + x] = stops[stop_idx];
                }
            } else {
                // Subtle vertical background gradient + clear horizontal zero line.
                for y in 0..height_px {
                    let dist = ((y as f32) - mid_y).abs() * inv_mid;
                    #[allow(clippy::cast_possible_truncation)]
                    let delta = (6.0 - dist * 12.0) as i16;
                    self.pixel_buf[y * width_px + x] = if y == mid_y_idx {
                        zero_line_col
                    } else {
                        shade_color(bg_base, delta)
                    };
                }

                // Anti-aliased min/max peak envelope (no horizontal smoothing so transients stay sharp).
                let max_f = (f32::from(col.max) / 32_768.0 * self.v_zoom).clamp(-1.0, 1.0);
                let min_f = (f32::from(col.min) / 32_768.0 * self.v_zoom).clamp(-1.0, 1.0);
                let y_hi = (mid_y - max_f * (mid_y - 1.0)).clamp(0.0, max_y_f);
                let y_lo = (mid_y - min_f * (mid_y - 1.0)).clamp(0.0, max_y_f);
                let y_top_f = y_hi.min(y_lo);
                let y_bot_f = y_hi.max(y_lo);

                #[allow(clippy::cast_possible_truncation, clippy::cast_sign_loss)]
                let top_i = (y_top_f.floor() as usize).min(height_px - 1);
                #[allow(clippy::cast_possible_truncation, clippy::cast_sign_loss)]
                let bot_i = (y_bot_f.floor() as usize).min(height_px - 1);

                if top_i == bot_i {
                    let cov = (y_bot_f - y_top_f).clamp(0.35, 1.0);
                    let bg_px = self.pixel_buf[top_i * width_px + x];
                    self.pixel_buf[top_i * width_px + x] = blend_color(bg_px, wave_peak, cov);
                } else {
                    let top_cov = (1.0 - (y_top_f - top_i as f32)).clamp(0.0, 1.0);
                    let bot_cov = (y_bot_f - bot_i as f32).clamp(0.0, 1.0);
                    let bg_top = self.pixel_buf[top_i * width_px + x];
                    self.pixel_buf[top_i * width_px + x] = blend_color(bg_top, wave_peak, top_cov);
                    for y in (top_i + 1)..bot_i {
                        let dist = ((y as f32) - mid_y).abs() * inv_mid;
                        let col_y = blend_color(wave_peak, wave_rms, dist * 0.35);
                        self.pixel_buf[y * width_px + x] = col_y;
                    }
                    let bg_bot = self.pixel_buf[bot_i * width_px + x];
                    self.pixel_buf[bot_i * width_px + x] = blend_color(bg_bot, wave_peak, bot_cov);
                }

                // Inner RMS energy layer (brighter core with anti-aliased edges).
                let rms_f = (f32::from(col.rms) / 32_768.0 * self.v_zoom).clamp(0.0, 1.0);
                if rms_f > 0.002 {
                    let r_top_f = (mid_y - rms_f * (mid_y - 1.0)).clamp(0.0, max_y_f);
                    let r_bot_f = (mid_y + rms_f * (mid_y - 1.0)).clamp(0.0, max_y_f);
                    #[allow(clippy::cast_possible_truncation, clippy::cast_sign_loss)]
                    let r_top = (r_top_f.floor() as usize).min(height_px - 1);
                    #[allow(clippy::cast_possible_truncation, clippy::cast_sign_loss)]
                    let r_bot = (r_bot_f.floor() as usize).min(height_px - 1);
                    if r_top < r_bot {
                        let tc = (1.0 - (r_top_f - r_top as f32)).clamp(0.0, 1.0);
                        let bc = (r_bot_f - r_bot as f32).clamp(0.0, 1.0);
                        let p_top = self.pixel_buf[r_top * width_px + x];
                        self.pixel_buf[r_top * width_px + x] =
                            blend_color(p_top, wave_rms, tc * 0.85);
                        for y in (r_top + 1)..r_bot {
                            self.pixel_buf[y * width_px + x] = wave_rms;
                        }
                        let p_bot = self.pixel_buf[r_bot * width_px + x];
                        self.pixel_buf[r_bot * width_px + x] =
                            blend_color(p_bot, wave_rms, bc * 0.85);
                    }
                }
            }
        }
    }
}

/// Applies magnetic keyframe / neighbouring-line snapping to `raw_ms` unless `shift_held` is true.
#[must_use]
pub fn apply_snap(
    raw_ms: i64,
    shift_held: bool,
    state: &AudioPanelState,
    markers: &TimelineMarkerIndex,
) -> (i64, Option<SnapHint>) {
    if shift_held || (!state.auto_snap_keyframes && !state.snap_to_lines) {
        return (raw_ms, None);
    }
    let threshold_ms = threshold_ms_from_px(state.snap_distance_px, state.viewport.ms_per_px);
    if let Some((snapped_ms, is_keyframe)) = markers.snap_target(
        raw_ms,
        threshold_ms,
        state.auto_snap_keyframes,
        state.snap_to_lines,
    ) {
        (
            snapped_ms,
            Some(SnapHint {
                time_ms: snapped_ms,
                is_keyframe,
            }),
        )
    } else {
        (raw_ms, None)
    }
}

#[inline]
fn blend_color(bg: Color32, fg: Color32, alpha: f32) -> Color32 {
    let a = alpha.clamp(0.0, 1.0);
    let inv = 1.0 - a;
    #[allow(clippy::cast_possible_truncation, clippy::cast_sign_loss)]
    let r = (f32::from(bg.r()) * inv + f32::from(fg.r()) * a).clamp(0.0, 255.0) as u8;
    #[allow(clippy::cast_possible_truncation, clippy::cast_sign_loss)]
    let g = (f32::from(bg.g()) * inv + f32::from(fg.g()) * a).clamp(0.0, 255.0) as u8;
    #[allow(clippy::cast_possible_truncation, clippy::cast_sign_loss)]
    let b = (f32::from(bg.b()) * inv + f32::from(fg.b()) * a).clamp(0.0, 255.0) as u8;
    Color32::from_rgb(r, g, b)
}

#[inline]
fn shade_color(c: Color32, delta: i16) -> Color32 {
    let r = (i16::from(c.r()) + delta).clamp(0, 255);
    let g = (i16::from(c.g()) + delta).clamp(0, 255);
    let b = (i16::from(c.b()) + delta).clamp(0, 255);
    #[allow(clippy::cast_possible_truncation, clippy::cast_sign_loss)]
    Color32::from_rgb(r as u8, g as u8, b as u8)
}

#[inline]
fn brighten_color(c: Color32, amount: u8) -> Color32 {
    Color32::from_rgb(
        c.r().saturating_add(amount),
        c.g().saturating_add(amount),
        c.b().saturating_add(amount),
    )
}

#[inline]
fn role_color(role: ColorRole, pal: &Palette) -> Color32 {
    match role {
        ColorRole::SelectionFill => {
            let s = rgba(pal.audio_selection);
            Color32::from_rgba_unmultiplied(s.r(), s.g(), s.b(), 82)
        }
        ColorRole::VideoFrameArea => rgba(pal.audio_selection),
        ColorRole::InactiveFill => {
            let c = rgba(pal.audio_inactive);
            Color32::from_rgba_unmultiplied(c.r(), c.g(), c.b(), 64)
        }
        ColorRole::InactiveBoundary => rgba(pal.audio_inactive),
        ColorRole::SelectionStartMarker => Color32::from_rgb(238, 72, 72),
        ColorRole::SelectionEndMarker => Color32::from_rgb(64, 148, 255),
        ColorRole::LineBoundary => rgba(pal.audio_line_boundary),
        // Bright pink/magenta keyframe line, clearly visible over waveform, spectrum, and selection
        // in Light, Dark, and EyeComfort themes.
        ColorRole::KeyframeLine => Color32::from_rgb(255, 48, 200),
        ColorRole::PlayCursor => rgba(pal.audio_play_cursor),
        ColorRole::VideoCursor => rgba(pal.audio_video_marker),
        ColorRole::KaraokeBoundary
        | ColorRole::KaraokeLabelBg
        | ColorRole::KaraokeLabelActiveBg => rgba(pal.audio_karaoke_label),
        ColorRole::WaveformPeak | ColorRole::WaveformRms => rgba(pal.audio_waveform),
    }
}

#[allow(clippy::too_many_arguments)]
fn ensure_ruler_texture(
    state: &mut AudioPanelState,
    ctx: &egui::Context,
    start_ms: i64,
    end_ms: i64,
    phys_w: usize,
    phys_h: usize,
    ppp: f32,
    palette: &Palette,
    palette_rev: u64,
) {
    let key = RulerCacheKey {
        start_ms,
        end_ms,
        phys_w,
        phys_h,
        palette_rev,
        ppp_bits: ppp.to_bits(),
    };
    if state.ruler_cache_key == Some(key) && state.ruler_texture.is_some() {
        return;
    }

    let is_light =
        u16::from(palette.grid_bg.r) + u16::from(palette.grid_bg.g) + u16::from(palette.grid_bg.b)
            > 384;
    let bg = if is_light {
        Color32::from_rgb(22, 26, 34)
    } else {
        shade_color(rgba(palette.panel_bg), -8)
    };
    let border_col = Color32::from_rgb(104, 152, 214);
    let minor_col = Color32::from_rgb(112, 138, 174);
    let medium_col = Color32::from_rgb(142, 178, 224);
    let major_col = Color32::from_rgb(184, 212, 248);

    let total = phys_w.saturating_mul(phys_h);
    if state.ruler_pixel_buf.len() == total {
        state.ruler_pixel_buf.fill(bg);
    } else {
        state.ruler_pixel_buf.resize(total, bg);
    }

    #[allow(clippy::cast_possible_truncation, clippy::cast_sign_loss)]
    let tick_w = (ppp.round() as usize).clamp(1, 4);
    let span_ms = (end_ms - start_ms).max(1) as f64;
    let spec = compute_ruler_ticks(state.viewport.ms_per_px);

    // Bottom horizontal baseline.
    let base_y0 = phys_h.saturating_sub(tick_w);
    for y in base_y0..phys_h {
        let row = y * phys_w;
        state.ruler_pixel_buf[row..row + phys_w].fill(border_col);
    }

    // Adaptive minor (0.10 s) / medium (0.50 s) / major (1.00 s) vertical ticks baked into texture.
    let first_tick = if start_ms >= 0 {
        ((start_ms + spec.minor_ms - 1) / spec.minor_ms) * spec.minor_ms
    } else {
        (start_ms / spec.minor_ms) * spec.minor_ms
    };
    let mut t = first_tick;
    let minor_h = ((phys_h * 26) / 100).max(tick_w + 1);
    let medium_h = ((phys_h * 44) / 100).max(minor_h + 1);
    let major_h = ((phys_h * 66) / 100).max(medium_h + 1);

    while t <= end_ms {
        let frac = ((t - start_ms) as f64 / span_ms).clamp(0.0, 1.0);
        #[allow(clippy::cast_possible_truncation, clippy::cast_sign_loss)]
        let x0 = ((frac * (phys_w.saturating_sub(1) as f64)).round() as usize).min(phys_w - 1);
        let (th, col) = if t % spec.major_ms == 0 {
            (major_h, major_col)
        } else if t % spec.medium_ms == 0 {
            (medium_h, medium_col)
        } else {
            (minor_h, minor_col)
        };
        let y0 = phys_h.saturating_sub(th);
        let x1 = (x0 + tick_w).min(phys_w);
        for y in y0..phys_h {
            let row = y * phys_w;
            state.ruler_pixel_buf[(row + x0)..(row + x1)].fill(col);
        }
        t = t.saturating_add(spec.minor_ms);
    }

    let img = ColorImage {
        size: [phys_w, phys_h],
        source_size: Vec2::new(phys_w as f32, phys_h as f32),
        pixels: state.ruler_pixel_buf.clone(),
    };
    if let Some(ref mut tex) = state.ruler_texture {
        tex.set(img, TextureOptions::NEAREST);
    } else {
        state.ruler_texture =
            Some(ctx.load_texture("hlouis_audio_ruler", img, TextureOptions::NEAREST));
    }
    state.ruler_cache_key = Some(key);
}

/// Custom aligned vertical slider matching Aegisub: thin light track + short wide blue bar thumb.
fn vertical_blue_slider(
    ui: &mut Ui,
    value: &mut f32,
    range: std::ops::RangeInclusive<f32>,
    height: f32,
    pal: &Palette,
    tooltip: &str,
) -> bool {
    let (rect, resp) = ui.allocate_exact_size(Vec2::new(16.0, height), Sense::click_and_drag());
    let mut changed = false;
    let min_v = *range.start();
    let max_v = *range.end();
    if (resp.clicked() || resp.dragged())
        && let Some(pos) = resp.interact_pointer_pos()
    {
        let frac = 1.0 - ((pos.y - rect.top()) / rect.height().max(1.0)).clamp(0.0, 1.0);
        *value = (min_v + frac * (max_v - min_v)).clamp(min_v, max_v);
        changed = true;
    }
    let frac = ((*value - min_v) / (max_v - min_v).max(f32::EPSILON)).clamp(0.0, 1.0);
    let is_light =
        u16::from(pal.grid_bg.r) + u16::from(pal.grid_bg.g) + u16::from(pal.grid_bg.b) > 384;
    let p = ui.painter();
    let track = Rect::from_center_size(rect.center(), Vec2::new(3.5, (height - 4.0).max(8.0)));
    let track_bg = if is_light {
        Color32::from_rgb(232, 234, 238)
    } else {
        rgba(pal.grid_bg)
    };
    p.rect_filled(track, 1.0, track_bg);
    p.rect_stroke(
        track,
        1.0,
        Stroke::new(0.8, rgba(pal.border)),
        egui::StrokeKind::Inside,
    );
    let thumb_y = rect.bottom() - 5.0 - frac * (height - 10.0).max(1.0);
    let thumb = Rect::from_center_size(Pos2::new(rect.center().x, thumb_y), Vec2::new(14.0, 7.0));
    let thumb_fill = if is_light {
        Color32::from_rgb(0, 120, 215)
    } else {
        Color32::from_rgb(76, 146, 224)
    };
    let thumb_bd = if is_light {
        Color32::from_rgb(0, 90, 168)
    } else {
        Color32::from_rgb(48, 104, 174)
    };
    p.rect_filled(thumb, 0.5, thumb_fill);
    p.rect_stroke(
        thumb,
        0.5,
        Stroke::new(0.8, thumb_bd),
        egui::StrokeKind::Inside,
    );
    let _ = resp.on_hover_text(tooltip);
    changed
}

#[derive(Clone, Copy)]
enum AudioToolIcon {
    PrevLine,
    NextLine,
    PlaySel,
    PlayLine,
    Stop,
    PlayBeforeStart,
    PlayAfterStart,
    PlayFirst500,
    PlayLast500,
    PlayBeforeEnd,
    PlayAfterEnd,
    PlayToEnd,
    Commit,
    CommitNext,
    LeadIn,
    LeadOut,
    AutoCommit,
    AutoNext,
    AutoScroll,
    Spectrum,
    Karaoke,
    OptionsWrench,
}

#[allow(clippy::too_many_lines)]
fn audio_tool_button(
    ui: &mut Ui,
    icon: AudioToolIcon,
    active: bool,
    pal: &Palette,
    tooltip: &str,
) -> bool {
    let (rect, resp) = ui.allocate_exact_size(Vec2::splat(19.0), Sense::click());
    let p = ui.painter();
    let bg = if active {
        Color32::from_rgb(198, 224, 250)
    } else if resp.hovered() {
        rgba(pal.hover)
    } else {
        rgba(pal.toolbar_bg)
    };
    p.rect_filled(rect, 2.0, bg);
    if active || resp.hovered() {
        p.rect_stroke(
            rect,
            2.0,
            Stroke::new(
                1.0,
                if active {
                    Color32::from_rgb(56, 132, 220)
                } else {
                    rgba(pal.border)
                },
            ),
            egui::StrokeKind::Inside,
        );
    }
    let c = rect.center();
    let blue = Color32::from_rgb(42, 120, 214);
    let green = Color32::from_rgb(38, 168, 84);
    let red = Color32::from_rgb(218, 62, 62);
    let pink = Color32::from_rgb(218, 92, 162);
    let amber = Color32::from_rgb(214, 152, 26);
    let fg = rgba(pal.text_primary);

    match icon {
        AudioToolIcon::PrevLine => {
            p.add(egui::Shape::convex_polygon(
                vec![
                    Pos2::new(c.x + 4.0, c.y - 4.5),
                    Pos2::new(c.x - 4.5, c.y),
                    Pos2::new(c.x + 4.0, c.y + 4.5),
                ],
                blue,
                Stroke::NONE,
            ));
        }
        AudioToolIcon::NextLine => {
            p.add(egui::Shape::convex_polygon(
                vec![
                    Pos2::new(c.x - 4.0, c.y - 4.5),
                    Pos2::new(c.x + 4.5, c.y),
                    Pos2::new(c.x - 4.0, c.y + 4.5),
                ],
                blue,
                Stroke::NONE,
            ));
        }
        AudioToolIcon::PlaySel => {
            p.add(egui::Shape::convex_polygon(
                vec![
                    Pos2::new(c.x - 4.0, c.y - 5.0),
                    Pos2::new(c.x + 5.0, c.y),
                    Pos2::new(c.x - 4.0, c.y + 5.0),
                ],
                green,
                Stroke::NONE,
            ));
        }
        AudioToolIcon::PlayLine => {
            p.rect_stroke(
                Rect::from_center_size(c, Vec2::new(12.0, 10.0)),
                1.0,
                Stroke::new(1.2, blue),
                egui::StrokeKind::Inside,
            );
            p.add(egui::Shape::convex_polygon(
                vec![
                    Pos2::new(c.x - 2.5, c.y - 3.5),
                    Pos2::new(c.x + 3.5, c.y),
                    Pos2::new(c.x - 2.5, c.y + 3.5),
                ],
                green,
                Stroke::NONE,
            ));
        }
        AudioToolIcon::Stop => {
            p.rect_filled(Rect::from_center_size(c, Vec2::splat(8.0)), 1.0, red);
        }
        AudioToolIcon::PlayBeforeStart => {
            p.line_segment(
                [
                    Pos2::new(c.x + 4.0, c.y - 5.0),
                    Pos2::new(c.x + 4.0, c.y + 5.0),
                ],
                Stroke::new(1.8, red),
            );
            p.add(egui::Shape::convex_polygon(
                vec![
                    Pos2::new(c.x - 4.5, c.y - 4.0),
                    Pos2::new(c.x + 2.0, c.y),
                    Pos2::new(c.x - 4.5, c.y + 4.0),
                ],
                green,
                Stroke::NONE,
            ));
        }
        AudioToolIcon::PlayAfterStart | AudioToolIcon::PlayFirst500 => {
            p.line_segment(
                [
                    Pos2::new(c.x - 4.0, c.y - 5.0),
                    Pos2::new(c.x - 4.0, c.y + 5.0),
                ],
                Stroke::new(1.8, red),
            );
            p.add(egui::Shape::convex_polygon(
                vec![
                    Pos2::new(c.x - 2.0, c.y - 4.0),
                    Pos2::new(c.x + 4.5, c.y),
                    Pos2::new(c.x - 2.0, c.y + 4.0),
                ],
                green,
                Stroke::NONE,
            ));
        }
        AudioToolIcon::PlayLast500 | AudioToolIcon::PlayBeforeEnd => {
            p.line_segment(
                [
                    Pos2::new(c.x + 4.0, c.y - 5.0),
                    Pos2::new(c.x + 4.0, c.y + 5.0),
                ],
                Stroke::new(1.8, pink),
            );
            p.add(egui::Shape::convex_polygon(
                vec![
                    Pos2::new(c.x - 4.5, c.y - 4.0),
                    Pos2::new(c.x + 2.0, c.y),
                    Pos2::new(c.x - 4.5, c.y + 4.0),
                ],
                green,
                Stroke::NONE,
            ));
        }
        AudioToolIcon::PlayAfterEnd => {
            p.line_segment(
                [
                    Pos2::new(c.x - 4.0, c.y - 5.0),
                    Pos2::new(c.x - 4.0, c.y + 5.0),
                ],
                Stroke::new(1.8, pink),
            );
            p.add(egui::Shape::convex_polygon(
                vec![
                    Pos2::new(c.x - 2.0, c.y - 4.0),
                    Pos2::new(c.x + 4.5, c.y),
                    Pos2::new(c.x - 2.0, c.y + 4.0),
                ],
                green,
                Stroke::NONE,
            ));
        }
        AudioToolIcon::PlayToEnd => {
            p.add(egui::Shape::convex_polygon(
                vec![
                    Pos2::new(c.x - 5.0, c.y - 4.0),
                    Pos2::new(c.x + 1.0, c.y),
                    Pos2::new(c.x - 5.0, c.y + 4.0),
                ],
                green,
                Stroke::NONE,
            ));
            p.line_segment(
                [Pos2::new(c.x + 2.5, c.y), Pos2::new(c.x + 5.5, c.y)],
                Stroke::new(1.6, blue),
            );
        }
        AudioToolIcon::Commit => {
            p.line_segment(
                [
                    Pos2::new(c.x - 5.0, c.y + 0.5),
                    Pos2::new(c.x - 1.5, c.y + 4.0),
                ],
                Stroke::new(2.0, green),
            );
            p.line_segment(
                [
                    Pos2::new(c.x - 1.5, c.y + 4.0),
                    Pos2::new(c.x + 5.0, c.y - 4.0),
                ],
                Stroke::new(2.0, green),
            );
        }
        AudioToolIcon::CommitNext => {
            p.line_segment(
                [
                    Pos2::new(c.x - 5.5, c.y + 0.5),
                    Pos2::new(c.x - 2.5, c.y + 3.5),
                ],
                Stroke::new(1.8, green),
            );
            p.line_segment(
                [
                    Pos2::new(c.x - 2.5, c.y + 3.5),
                    Pos2::new(c.x + 1.5, c.y - 3.5),
                ],
                Stroke::new(1.8, green),
            );
            p.add(egui::Shape::convex_polygon(
                vec![
                    Pos2::new(c.x + 2.0, c.y - 3.0),
                    Pos2::new(c.x + 6.0, c.y),
                    Pos2::new(c.x + 2.0, c.y + 3.0),
                ],
                blue,
                Stroke::NONE,
            ));
        }
        AudioToolIcon::LeadIn => {
            p.line_segment(
                [
                    Pos2::new(c.x + 4.0, c.y - 5.0),
                    Pos2::new(c.x + 4.0, c.y + 5.0),
                ],
                Stroke::new(1.6, red),
            );
            p.line_segment(
                [Pos2::new(c.x - 5.0, c.y), Pos2::new(c.x + 2.0, c.y)],
                Stroke::new(1.6, blue),
            );
        }
        AudioToolIcon::LeadOut => {
            p.line_segment(
                [
                    Pos2::new(c.x - 4.0, c.y - 5.0),
                    Pos2::new(c.x - 4.0, c.y + 5.0),
                ],
                Stroke::new(1.6, pink),
            );
            p.line_segment(
                [Pos2::new(c.x - 2.0, c.y), Pos2::new(c.x + 5.0, c.y)],
                Stroke::new(1.6, blue),
            );
        }
        AudioToolIcon::AutoCommit => {
            let st = Stroke::new(1.5, green);
            p.line_segment(
                [
                    Pos2::new(c.x - 4.5, c.y + 0.5),
                    Pos2::new(c.x - 1.5, c.y + 3.5),
                ],
                st,
            );
            p.line_segment(
                [
                    Pos2::new(c.x - 1.5, c.y + 3.5),
                    Pos2::new(c.x + 4.5, c.y - 3.5),
                ],
                st,
            );
            p.circle_filled(Pos2::new(c.x - 4.0, c.y - 3.5), 1.5, blue);
        }
        AudioToolIcon::AutoNext => {
            let st = Stroke::new(1.5, blue);
            p.line_segment([Pos2::new(c.x - 4.5, c.y), Pos2::new(c.x + 4.0, c.y)], st);
            p.line_segment(
                [Pos2::new(c.x + 4.0, c.y), Pos2::new(c.x + 1.0, c.y - 3.0)],
                st,
            );
            p.line_segment(
                [Pos2::new(c.x + 4.0, c.y), Pos2::new(c.x + 1.0, c.y + 3.0)],
                st,
            );
            p.circle_filled(Pos2::new(c.x - 4.0, c.y - 4.0), 1.5, green);
        }
        AudioToolIcon::AutoScroll => {
            let st = Stroke::new(1.4, amber);
            p.line_segment(
                [
                    Pos2::new(c.x - 5.0, c.y - 2.0),
                    Pos2::new(c.x + 4.5, c.y - 2.0),
                ],
                st,
            );
            p.line_segment(
                [
                    Pos2::new(c.x + 4.5, c.y - 2.0),
                    Pos2::new(c.x + 2.0, c.y - 4.5),
                ],
                st,
            );
            p.line_segment(
                [
                    Pos2::new(c.x - 4.5, c.y + 2.5),
                    Pos2::new(c.x + 5.0, c.y + 2.5),
                ],
                st,
            );
            p.line_segment(
                [
                    Pos2::new(c.x - 4.5, c.y + 2.5),
                    Pos2::new(c.x - 2.0, c.y + 5.0),
                ],
                st,
            );
        }
        AudioToolIcon::Spectrum => {
            for (dx, h, col) in [
                (-4.0, 4.0, Color32::from_rgb(36, 158, 206)),
                (-1.0, 9.0, Color32::from_rgb(132, 232, 245)),
                (2.0, 7.0, Color32::from_rgb(36, 158, 206)),
                (5.0, 5.0, Color32::from_rgb(20, 96, 180)),
            ] {
                p.line_segment(
                    [
                        Pos2::new(c.x + dx, c.y + 5.0),
                        Pos2::new(c.x + dx, c.y + 5.0 - h),
                    ],
                    Stroke::new(2.0, col),
                );
            }
        }
        AudioToolIcon::Karaoke => {
            let purple = Color32::from_rgb(164, 72, 206);
            p.circle_filled(Pos2::new(c.x - 2.5, c.y + 3.5), 2.3, purple);
            p.line_segment(
                [
                    Pos2::new(c.x - 0.5, c.y + 3.0),
                    Pos2::new(c.x - 0.5, c.y - 5.0),
                ],
                Stroke::new(1.5, purple),
            );
            p.line_segment(
                [
                    Pos2::new(c.x - 0.5, c.y - 5.0),
                    Pos2::new(c.x + 4.5, c.y - 3.0),
                ],
                Stroke::new(1.5, purple),
            );
        }
        AudioToolIcon::OptionsWrench => {
            p.circle_stroke(Pos2::new(c.x - 2.0, c.y - 2.0), 3.0, Stroke::new(1.5, fg));
            p.line_segment(
                [
                    Pos2::new(c.x + 0.5, c.y + 0.5),
                    Pos2::new(c.x + 5.0, c.y + 5.0),
                ],
                Stroke::new(2.0, amber),
            );
        }
    }
    resp.on_hover_text(tooltip).clicked()
}

/// Renders the full Aegisub 3.5.1 audio panel (dark time ruler + waveform/spectrum canvas +
/// dark horizontal scrollbar with blue thumb + 3 aligned vertical sliders with blue thumbs +
/// pink/red Link toggle + ~20 coloured Aegisub audio toolbar icons) into `ui`.
#[allow(clippy::too_many_arguments, clippy::too_many_lines)]
pub fn show_audio_panel(
    ui: &mut Ui,
    state: &mut AudioPanelState,
    provider: &dyn AudioColumnProvider,
    markers: &TimelineMarkerIndex,
    timing: Option<&mut DialogueTimingController>,
    karaoke: Option<&KaraokeTimingController>,
    palette: &Palette,
    palette_rev: u64,
    _catalog: &Catalog,
) -> Option<AudioPanelAction> {
    let mut action: Option<AudioPanelAction> = None;
    let ppp = ui.ctx().pixels_per_point();

    let ruler_h = 16.0;
    let scrollbar_h = 11.0;
    let toolbar_h = 24.0;
    let slider_strip_w = 56.0;
    let total_canvas_h = (ui.available_height() - toolbar_h).max(76.0);
    let wave_h = (total_canvas_h - ruler_h - scrollbar_h).max(48.0);
    let canvas_w = (ui.available_width() - slider_strip_w - 6.0).max(100.0);

    ui.horizontal(|ui| {
        // Left: dark time ruler + waveform/spectrum canvas + dark horizontal scrollbar.
        ui.vertical(|ui| {
            ui.spacing_mut().item_spacing.y = 0.0;

            // 1. Dark top time ruler with physical-resolution baked 0.10 s / 0.50 s / 1.00 s ticks;
            //    second labels come from a galley cache validated by `GalleyEpoch`.
            let (ruler_rect, _) =
                ui.allocate_exact_size(Vec2::new(canvas_w, ruler_h), Sense::hover());

            state.viewport.width_px = canvas_w.max(1.0);
            state.viewport.height_px = wave_h.max(1.0);
            let vis = state.viewport.visible_range_ms();
            let span_ms = (vis.end_ms - vis.start_ms).max(100);

            #[allow(clippy::cast_possible_truncation, clippy::cast_sign_loss)]
            let ruler_phys_w = ((ruler_rect.width() * ppp).round() as usize).clamp(1, 8192);
            #[allow(clippy::cast_possible_truncation, clippy::cast_sign_loss)]
            let ruler_phys_h = ((ruler_rect.height() * ppp).round() as usize).clamp(1, 128);
            ensure_ruler_texture(
                state,
                ui.ctx(),
                vis.start_ms,
                vis.end_ms,
                ruler_phys_w,
                ruler_phys_h,
                ppp,
                palette,
                palette_rev,
            );

            let rp = ui.painter_at(ruler_rect);
            if let Some(ref rtex) = state.ruler_texture {
                rp.image(
                    rtex.id(),
                    ruler_rect,
                    Rect::from_min_max(Pos2::ZERO, Pos2::new(1.0, 1.0)),
                    Color32::WHITE,
                );
            } else {
                rp.rect_filled(ruler_rect, 0.0, Color32::from_rgb(22, 26, 34));
            }

            // Non-overlapping second labels over the baked tick texture (cached galleys).
            let tick_font = FontId::proportional(9.5);
            let tick_col = Color32::from_rgb(164, 206, 250);
            let generation = state.galley_epoch.sync(ui.ctx());
            let rkey = RulerLabelKey {
                start_ms: vis.start_ms,
                end_ms: vis.end_ms,
                ms_per_px_bits: state.viewport.ms_per_px.to_bits(),
                width_bits: canvas_w.to_bits(),
                ppp_bits: ppp.to_bits(),
                generation,
            };
            if state.ruler_key != Some(rkey) {
                state.ruler_ticks.clear();
                let step_ms = compute_ruler_ticks(state.viewport.ms_per_px).major_ms;
                let first_label = if vis.start_ms >= 0 {
                    ((vis.start_ms + step_ms - 1) / step_ms) * step_ms
                } else {
                    (vis.start_ms / step_ms) * step_ms
                };
                let mut t_ms = first_label;
                let mut tick_idx = 0usize;
                let mut last_label_right = -100.0f32;
                while t_ms <= vis.end_ms {
                    let frac = (t_ms - vis.start_ms) as f32 / span_ms as f32;
                    let rel_x = frac * ruler_rect.width();
                    let total_s = (t_ms / 1_000).max(0);
                    let h = total_s / 3600;
                    let m = (total_s / 60) % 60;
                    let s = total_s % 60;
                    state.ruler_buf.clear();
                    let is_full = tick_idx == 0 || s == 0;
                    if is_full {
                        let _ = write!(state.ruler_buf, "{h}:{m:02}:{s:02}");
                    } else {
                        let _ = write!(state.ruler_buf, "{s:02}");
                    }
                    let est_w = if is_full { 44.0 } else { 20.0 };
                    if rel_x >= last_label_right + 6.0 && rel_x + est_w <= ruler_rect.width() + 4.0
                    {
                        let g = ui.fonts_mut(|f| {
                            f.layout_no_wrap(state.ruler_buf.clone(), tick_font.clone(), tick_col)
                        });
                        state.ruler_ticks.push((rel_x, g));
                        last_label_right = rel_x + est_w;
                    }
                    t_ms = t_ms.saturating_add(step_ms);
                    tick_idx += 1;
                }
                state.ruler_key = Some(rkey);
            }
            for (rel_x, g) in &state.ruler_ticks {
                let tx = snap_px(ruler_rect.left() + *rel_x, ppp);
                rp.galley(
                    Pos2::new(tx + 2.0, ruler_rect.top() + 1.0),
                    Arc::clone(g),
                    tick_col,
                );
            }

            // 2. Main waveform / spectrum canvas.
            let (rect, resp) =
                ui.allocate_exact_size(Vec2::new(canvas_w, wave_h), Sense::click_and_drag());

            #[allow(clippy::cast_possible_truncation, clippy::cast_sign_loss)]
            let tex_w = (rect.width().round() as usize).clamp(1, 4096);
            #[allow(clippy::cast_possible_truncation, clippy::cast_sign_loss)]
            let tex_h = (rect.height().round() as usize).clamp(1, 512);

            let key = TextureCacheKey {
                start_ms: vis.start_ms,
                end_ms: vis.end_ms,
                width_px: tex_w,
                height_px: tex_h,
                spectrum_mode: state.spectrum_mode,
                v_zoom_bits: state.v_zoom.to_bits(),
                ms_per_px_bits: state.viewport.ms_per_px.to_bits(),
                palette_rev,
            };

            if state.cache_key == Some(key) {
                state.last_computed_columns = 0;
            } else {
                // Detect pure horizontal scroll by N pixels -> ring-buffer set_partial of ONLY N
                // columns (zero full-buffer copy_within, zero full-texture clone/upload).
                let sr = provider.sample_rate();
                let scroll_shift: Option<isize> = state.cache_key.and_then(|prev| {
                    if state.texture.is_some()
                        && (!key.spectrum_mode || sr > 0)
                        && prev.width_px == key.width_px
                        && prev.height_px == key.height_px
                        && prev.spectrum_mode == key.spectrum_mode
                        && prev.v_zoom_bits == key.v_zoom_bits
                        && prev.ms_per_px_bits == key.ms_per_px_bits
                        && prev.palette_rev == key.palette_rev
                        && prev.start_ms != key.start_ms
                    {
                        let d_f = (key.start_ms - prev.start_ms) as f64
                            / state.viewport.ms_per_px.max(f64::EPSILON);
                        #[allow(clippy::cast_possible_truncation)]
                        let d = d_f.round() as isize;
                        if d != 0 && d.unsigned_abs() < tex_w {
                            return Some(d);
                        }
                    }
                    None
                });

                if let Some(delta_px) = scroll_shift {
                    let d = delta_px.unsigned_abs();
                    let (dirty_range, write_x) = if delta_px > 0 {
                        // Scrolled right: overwrite old leftmost d cols at ring_head, advance head.
                        let wx = state.ring_head;
                        state.ring_head = (state.ring_head + d) % tex_w;
                        ((tex_w - d)..tex_w, wx)
                    } else {
                        // Scrolled left: step head back by d, overwrite those d cols.
                        state.ring_head = (state.ring_head + tex_w - d) % tex_w;
                        (0..d, state.ring_head)
                    };

                    let strip_total = d.saturating_mul(tex_h);
                    state.partial_pixel_buf.resize(strip_total, Color32::BLACK);

                    if state.spectrum_mode && sr > 0 {
                        state.ensure_spectrum_lut(palette, palette_rev);
                        let total = tex_w.saturating_mul(tex_h);
                        if state.spectrum_rgba_buf.len() != total {
                            state
                                .spectrum_rgba_buf
                                .resize(total, Rgba8::rgb(11, 15, 26));
                        }
                        let spec = SpectrumViewportSpec {
                            width_px: tex_w,
                            height_px: tex_h,
                            start_ms: vis.start_ms,
                            end_ms: vis.end_ms,
                            sample_rate: sr,
                            v_zoom: state.v_zoom,
                        };
                        if let Some((_, ref lut)) = state.spectrum_lut {
                            state.spectrum_renderer.render_columns_range_with(
                                spec,
                                dirty_range.clone(),
                                lut,
                                |s0, count, buf| provider.read_mono_i16(s0, count, buf),
                                &mut state.spectrum_rgba_buf,
                            );
                        }
                        for y in 0..tex_h {
                            let src_row = y * tex_w + dirty_range.start;
                            let dst_row = y * d;
                            for i in 0..d {
                                let c = state.spectrum_rgba_buf[src_row + i];
                                state.partial_pixel_buf[dst_row + i] =
                                    Color32::from_rgb(c.r, c.g, c.b);
                            }
                        }
                    } else {
                        let span_f = (vis.end_ms - vis.start_ms).max(1) as f64;
                        #[allow(clippy::cast_possible_truncation)]
                        let sub_s = vis.start_ms
                            + ((dirty_range.start as f64 / tex_w as f64) * span_f).round() as i64;
                        #[allow(clippy::cast_possible_truncation)]
                        let sub_e = vis.start_ms
                            + ((dirty_range.end as f64 / tex_w as f64) * span_f).round() as i64;
                        provider.query_columns_into(
                            sub_s,
                            sub_e.max(sub_s + 1),
                            d,
                            &mut state.partial_columns_buf,
                        );
                        // Swap scratch buffers briefly to rasterize only the d-wide strip.
                        std::mem::swap(&mut state.columns_buf, &mut state.partial_columns_buf);
                        std::mem::swap(&mut state.pixel_buf, &mut state.partial_pixel_buf);
                        state.rasterize_columns_range(d, tex_h, 0..d, palette);
                        std::mem::swap(&mut state.columns_buf, &mut state.partial_columns_buf);
                        std::mem::swap(&mut state.pixel_buf, &mut state.partial_pixel_buf);
                    }

                    if let Some(ref mut tex) = state.texture {
                        let first_w = d.min(tex_w - write_x);
                        let second_w = d - first_w;
                        if second_w == 0 {
                            let img = ColorImage {
                                size: [d, tex_h],
                                source_size: Vec2::new(d as f32, tex_h as f32),
                                pixels: state.partial_pixel_buf[..strip_total].to_vec(),
                            };
                            tex.set_partial([write_x, 0], img, TextureOptions::NEAREST);
                        } else {
                            let mut p1 = Vec::with_capacity(first_w * tex_h);
                            let mut p2 = Vec::with_capacity(second_w * tex_h);
                            for y in 0..tex_h {
                                let row = y * d;
                                p1.extend_from_slice(
                                    &state.partial_pixel_buf[row..(row + first_w)],
                                );
                                p2.extend_from_slice(
                                    &state.partial_pixel_buf[(row + first_w)..(row + d)],
                                );
                            }
                            tex.set_partial(
                                [write_x, 0],
                                ColorImage {
                                    size: [first_w, tex_h],
                                    source_size: Vec2::new(first_w as f32, tex_h as f32),
                                    pixels: p1,
                                },
                                TextureOptions::NEAREST,
                            );
                            tex.set_partial(
                                [0, 0],
                                ColorImage {
                                    size: [second_w, tex_h],
                                    source_size: Vec2::new(second_w as f32, tex_h as f32),
                                    pixels: p2,
                                },
                                TextureOptions::NEAREST,
                            );
                        }
                    }

                    state.cache_key = Some(key);
                    state.texture_uploads += 1;
                    state.last_computed_columns = d;
                    state.total_computed_columns += d as u64;
                } else {
                    state.ring_head = 0;
                    if state.spectrum_mode && sr > 0 {
                        state.ensure_spectrum_lut(palette, palette_rev);
                        let total = tex_w.saturating_mul(tex_h);
                        if state.spectrum_rgba_buf.len() != total {
                            state
                                .spectrum_rgba_buf
                                .resize(total, Rgba8::rgb(11, 15, 26));
                        }
                        if state.pixel_buf.len() != total {
                            state.pixel_buf.resize(total, Color32::from_rgb(11, 15, 26));
                        }
                        let spec = SpectrumViewportSpec {
                            width_px: tex_w,
                            height_px: tex_h,
                            start_ms: vis.start_ms,
                            end_ms: vis.end_ms,
                            sample_rate: sr,
                            v_zoom: state.v_zoom,
                        };
                        if let Some((_, ref lut)) = state.spectrum_lut {
                            state.spectrum_renderer.render_columns_range_with(
                                spec,
                                0..tex_w,
                                lut,
                                |s0, count, buf| provider.read_mono_i16(s0, count, buf),
                                &mut state.spectrum_rgba_buf,
                            );
                        }
                        for y in 0..tex_h {
                            let row = y * tex_w;
                            for x in 0..tex_w {
                                let c = state.spectrum_rgba_buf[row + x];
                                state.pixel_buf[row + x] = Color32::from_rgb(c.r, c.g, c.b);
                            }
                        }
                    } else {
                        // No raw PCM: use the provider's own spectrum renderer (e.g. the native
                        // STFT) in spectrum mode, otherwise (or on failure) the peak columns.
                        let spectrum_ok = state.spectrum_mode
                            && provider.render_spectrum_into(
                                vis.start_ms,
                                vis.end_ms,
                                tex_w,
                                tex_h,
                                palette,
                                &mut state.pixel_buf,
                            )
                            && state.pixel_buf.len() == tex_w.saturating_mul(tex_h);
                        if !spectrum_ok {
                            provider.query_columns_into(
                                vis.start_ms,
                                vis.end_ms,
                                tex_w,
                                &mut state.columns_buf,
                            );
                            state.rasterize_columns_range(tex_w, tex_h, 0..tex_w, palette);
                        }
                    }

                    let img = ColorImage {
                        size: [tex_w, tex_h],
                        source_size: Vec2::new(tex_w as f32, tex_h as f32),
                        pixels: state.pixel_buf.clone(),
                    };
                    if let Some(ref mut tex) = state.texture {
                        tex.set(img, TextureOptions::NEAREST);
                    } else {
                        state.texture = Some(ui.ctx().load_texture(
                            "hlouis_audio_waveform",
                            img,
                            TextureOptions::NEAREST,
                        ));
                    }
                    state.cache_key = Some(key);
                    state.texture_uploads += 1;
                    state.last_computed_columns = tex_w;
                    state.total_computed_columns += tex_w as u64;
                }
            }

            let painter = ui.painter_at(rect);
            if let Some(ref tex) = state.texture {
                if state.ring_head == 0 || state.ring_head >= tex_w {
                    painter.image(
                        tex.id(),
                        rect,
                        Rect::from_min_max(Pos2::ZERO, Pos2::new(1.0, 1.0)),
                        Color32::WHITE,
                    );
                } else {
                    let u_split = state.ring_head as f32 / tex_w as f32;
                    let left_frac = (tex_w - state.ring_head) as f32 / tex_w as f32;
                    let x_split = rect.left() + left_frac * rect.width();
                    painter.image(
                        tex.id(),
                        Rect::from_min_max(rect.min, Pos2::new(x_split, rect.bottom())),
                        Rect::from_min_max(Pos2::new(u_split, 0.0), Pos2::new(1.0, 1.0)),
                        Color32::WHITE,
                    );
                    painter.image(
                        tex.id(),
                        Rect::from_min_max(Pos2::new(x_split, rect.top()), rect.max),
                        Rect::from_min_max(Pos2::ZERO, Pos2::new(u_split, 1.0)),
                        Color32::WHITE,
                    );
                }
            } else {
                painter.rect_filled(rect, 0.0, Color32::from_rgb(11, 15, 26));
            }

            let kara_bounds = karaoke.map_or(&[][..], KaraokeTimingController::boundaries_ms);
            let input = AudioFrameInput {
                viewport: state.viewport,
                selection: state.active_selection,
                inactive_spans: &state.inactive_spans,
                markers,
                play_cursor_ms: state.play_cursor_ms,
                video_marker: state.video_marker_ms.map(|ms| {
                    timing_controller::VideoPositionMarker {
                        position_ms: ms,
                        frame_start_ms: ms,
                        frame_end_ms: ms,
                    }
                }),
                karaoke_boundaries_ms: if state.karaoke_mode { kara_bounds } else { &[] },
                active_karaoke_syllable: None,
            };
            build_frame_layout(&input, None, &mut state.frame_layout);

            if resp.hovered() {
                let (wheel_y, ctrl) = ui.input(|i| (i.smooth_scroll_delta.y, i.modifiers.ctrl));
                if wheel_y.abs() > 0.1 {
                    if ctrl {
                        let factor = if wheel_y > 0.0 { 1.15 } else { 1.0 / 1.15 };
                        state.h_zoom = (state.h_zoom * factor).clamp(0.25, 8.0);
                        state.viewport.ms_per_px =
                            (12.0 / f64::from(state.h_zoom)).clamp(0.5, 200.0);
                    } else {
                        let delta_ms = f64::from(-wheel_y) * state.viewport.ms_per_px * 1.5;
                        let max_scroll = (provider.duration_ms().max(1) as f64
                            - f64::from(state.viewport.width_px) * state.viewport.ms_per_px)
                            .max(0.0);
                        state.viewport.scroll_ms =
                            (state.viewport.scroll_ms + delta_ms).clamp(0.0, max_scroll);
                    }
                }
            }

            for span in &state.frame_layout.spans {
                let sr = Rect::from_min_max(
                    Pos2::new(rect.left() + span.rect.x0, rect.top() + span.rect.y0),
                    Pos2::new(rect.left() + span.rect.x1, rect.top() + span.rect.y1),
                );
                painter.rect_filled(sr, 0.0, role_color(span.role, palette));
            }

            // Pass 1: subtle line boundaries, video cursor, karaoke boundaries.
            let thin_w = 1.0 / ppp.max(0.5);
            for m in &state.frame_layout.markers {
                if matches!(
                    m.role,
                    ColorRole::KeyframeLine
                        | ColorRole::SelectionStartMarker
                        | ColorRole::SelectionEndMarker
                        | ColorRole::PlayCursor
                ) {
                    continue;
                }
                let sx = snap_px(rect.left() + m.x, ppp);
                painter.line_segment(
                    [Pos2::new(sx, rect.top()), Pos2::new(sx, rect.bottom())],
                    Stroke::new(thin_w, role_color(m.role, palette)),
                );
            }

            // Pass 2: bright pink/magenta full-height keyframes drawn over waveform, spectrum & selection.
            let kf_w = (1.5 / ppp.max(0.5)).max(1.0);
            for m in &state.frame_layout.markers {
                if m.role != ColorRole::KeyframeLine {
                    continue;
                }
                let sx = snap_px(rect.left() + m.x, ppp);
                painter.line_segment(
                    [Pos2::new(sx, rect.top()), Pos2::new(sx, rect.bottom())],
                    Stroke::new(kf_w, role_color(m.role, palette)),
                );
            }

            // Pass 3: active selection start/end boundary lines + crisp corner notches (no Vec alloc).
            let edge_w = (2.0 / ppp.max(0.5)).max(1.5);
            for m in &state.frame_layout.markers {
                if !matches!(
                    m.role,
                    ColorRole::SelectionStartMarker
                        | ColorRole::SelectionEndMarker
                        | ColorRole::PlayCursor
                ) {
                    continue;
                }
                let sx = snap_px(rect.left() + m.x, ppp);
                let col = role_color(m.role, palette);
                let w = if m.role == ColorRole::PlayCursor {
                    thin_w
                } else {
                    edge_w
                };
                painter.line_segment(
                    [Pos2::new(sx, rect.top()), Pos2::new(sx, rect.bottom())],
                    Stroke::new(w, col),
                );
                if m.role == ColorRole::SelectionStartMarker {
                    painter.rect_filled(
                        Rect::from_min_size(Pos2::new(sx, rect.top()), Vec2::new(4.0, 4.0)),
                        0.0,
                        col,
                    );
                    painter.rect_filled(
                        Rect::from_min_size(
                            Pos2::new(sx, rect.bottom() - 4.0),
                            Vec2::new(4.0, 4.0),
                        ),
                        0.0,
                        col,
                    );
                } else if m.role == ColorRole::SelectionEndMarker {
                    painter.rect_filled(
                        Rect::from_min_size(Pos2::new(sx - 4.0, rect.top()), Vec2::new(4.0, 4.0)),
                        0.0,
                        col,
                    );
                    painter.rect_filled(
                        Rect::from_min_size(
                            Pos2::new(sx - 4.0, rect.bottom() - 4.0),
                            Vec2::new(4.0, 4.0),
                        ),
                        0.0,
                        col,
                    );
                }
            }

            // Interaction: drag/click start & end boundaries with magnetic keyframe/line snapping
            // (hold Shift to temporarily disable snapping).
            let shift_held = ui.input(|i| i.modifiers.shift);
            if let Some(pos) = resp.interact_pointer_pos() {
                let local_x = pos.x - rect.left();
                let local_y = pos.y - rect.top();
                if resp.drag_started() {
                    match state.frame_layout.hit_test(
                        local_x,
                        local_y,
                        6.0,
                        &state.viewport,
                        state.active_selection,
                        kara_bounds,
                    ) {
                        HitTarget::StartMarker => state.dragging = Some(DragTarget::Start),
                        HitTarget::EndMarker => state.dragging = Some(DragTarget::End),
                        _ => {
                            let raw_ms = state.viewport.x_to_ms(local_x).max(0);
                            let (ms, hint) = apply_snap(raw_ms, shift_held, state, markers);
                            state.active_selection.start_ms = ms;
                            if state.active_selection.end_ms < ms {
                                state.active_selection.end_ms = ms + 500;
                            }
                            state.dragging = Some(DragTarget::End);
                            state.active_snap_hint = hint;
                            action =
                                Some(AudioPanelAction::SelectionChanged(state.active_selection));
                        }
                    }
                }
                if resp.dragged()
                    && let Some(target) = state.dragging
                {
                    let raw_ms = state.viewport.x_to_ms(local_x).max(0);
                    let (ms, hint) = apply_snap(raw_ms, shift_held, state, markers);
                    state.active_snap_hint = hint;
                    match target {
                        DragTarget::Start => {
                            state.active_selection.start_ms = ms.min(state.active_selection.end_ms);
                        }
                        DragTarget::End => {
                            state.active_selection.end_ms = ms.max(state.active_selection.start_ms);
                        }
                        DragTarget::Both { .. } => {}
                    }
                    action = Some(AudioPanelAction::SelectionChanged(state.active_selection));
                }
                if resp.drag_stopped() {
                    state.dragging = None;
                    state.active_snap_hint = None;
                }
            } else if state.dragging.is_none() {
                state.active_snap_hint = None;
            }

            // Snap halo + "KF" badge ONLY while actively dragging (never in the static view).
            if state.dragging.is_some()
                && let Some(hint) = state.active_snap_hint
            {
                let hx = snap_px(rect.left() + state.viewport.ms_to_x(hint.time_ms), ppp);
                if hx >= rect.left() && hx <= rect.right() {
                    let halo_col = if hint.is_keyframe {
                        Color32::from_rgba_unmultiplied(255, 48, 200, 68)
                    } else {
                        Color32::from_rgba_unmultiplied(96, 184, 255, 68)
                    };
                    let badge_bg = if hint.is_keyframe {
                        Color32::from_rgb(214, 36, 168)
                    } else {
                        Color32::from_rgb(48, 122, 210)
                    };
                    painter.rect_filled(
                        Rect::from_center_size(
                            Pos2::new(hx, rect.center().y),
                            Vec2::new(7.0, rect.height()),
                        ),
                        0.0,
                        halo_col,
                    );
                    let badge_rect = Rect::from_min_size(
                        Pos2::new((hx + 3.0).min(rect.right() - 22.0), rect.top() + 3.0),
                        Vec2::new(20.0, 13.0),
                    );
                    painter.rect_filled(badge_rect, 2.0, badge_bg);
                    painter.text(
                        badge_rect.center(),
                        Align2::CENTER_CENTER,
                        if hint.is_keyframe { "KF" } else { "LN" },
                        FontId::proportional(8.5),
                        Color32::WHITE,
                    );
                }
            }

            // 3. Dark horizontal scrollbar under waveform/spectrum with draggable blue thumb.
            let (sb_rect, sb_resp) =
                ui.allocate_exact_size(Vec2::new(canvas_w, scrollbar_h), Sense::click_and_drag());
            let sbp = ui.painter_at(sb_rect);
            sbp.rect_filled(sb_rect, 0.0, Color32::from_rgb(24, 28, 36));
            let total_dur = provider.duration_ms().max(1) as f32;
            let vis_frac = (span_ms as f32 / total_dur).clamp(0.05, 1.0);
            let pos_frac = (vis.start_ms.max(0) as f32 / total_dur).clamp(0.0, 1.0 - vis_frac);
            let thumb_w = (sb_rect.width() * vis_frac).max(24.0);
            let thumb_x = sb_rect.left() + pos_frac * (sb_rect.width() - thumb_w).max(0.0);
            let thumb_rect = Rect::from_min_size(
                Pos2::new(thumb_x, sb_rect.top() + 1.5),
                Vec2::new(thumb_w, (scrollbar_h - 3.0).max(4.0)),
            );
            sbp.rect_filled(thumb_rect, 2.0, Color32::from_rgb(56, 136, 226));
            if (sb_resp.clicked() || sb_resp.dragged())
                && let Some(ppos) = sb_resp.interact_pointer_pos()
            {
                let frac = ((ppos.x - sb_rect.left()) / sb_rect.width().max(1.0)).clamp(0.0, 1.0);
                #[allow(clippy::cast_possible_truncation)]
                let new_start = (frac * total_dur) as i64;
                state.viewport.scroll_ms = new_start.max(0) as f64;
            }
        });

        // Right-hand strip: 3 aligned vertical sliders (H-zoom, V-zoom, Volume) with blue bar
        // thumbs + pink Link icon toggle below them (matching Aegisub).
        ui.vertical(|ui| {
            let slider_h = (total_canvas_h - 20.0).max(44.0);
            ui.horizontal(|ui| {
                ui.spacing_mut().item_spacing.x = 2.0;
                if vertical_blue_slider(
                    ui,
                    &mut state.h_zoom,
                    0.25..=8.0,
                    slider_h,
                    palette,
                    "Horizontal zoom",
                ) {
                    state.viewport.ms_per_px = (12.0 / f64::from(state.h_zoom)).clamp(0.5, 200.0);
                }
                let prev_v = state.v_zoom;
                vertical_blue_slider(
                    ui,
                    &mut state.v_zoom,
                    0.25..=4.0,
                    slider_h,
                    palette,
                    "Vertical zoom",
                );
                if state.link_zoom_volume && (state.v_zoom - prev_v).abs() > f32::EPSILON {
                    state.volume = (state.v_zoom / 2.0).clamp(0.0, 2.0);
                }
                vertical_blue_slider(
                    ui,
                    &mut state.volume,
                    0.0..=2.0,
                    slider_h,
                    palette,
                    "Volume",
                );
            });

            // Pink Link toggle button with vector U-bracket icon (no text), aligned under sliders.
            let (lr, lresp) = ui.allocate_exact_size(Vec2::new(52.0, 16.0), Sense::click());
            let lp = ui.painter();
            let is_light = u16::from(palette.grid_bg.r)
                + u16::from(palette.grid_bg.g)
                + u16::from(palette.grid_bg.b)
                > 384;
            let lbg = if state.link_zoom_volume {
                if is_light {
                    Color32::from_rgb(245, 134, 134)
                } else {
                    Color32::from_rgb(176, 86, 98)
                }
            } else if is_light {
                Color32::from_rgb(240, 214, 218)
            } else {
                rgba(palette.toolbar_bg)
            };
            let lbd = if is_light {
                Color32::from_rgb(172, 72, 82)
            } else {
                rgba(palette.border)
            };
            lp.rect_filled(lr, 1.5, lbg);
            lp.rect_stroke(lr, 1.5, Stroke::new(1.0, lbd), egui::StrokeKind::Inside);

            // Vector U-bracket link icon (cyan core with crisp dark outline, like Aegisub).
            let lc = lr.center();
            let outline = Stroke::new(3.0, Color32::from_rgb(28, 42, 60));
            let core = Stroke::new(1.6, Color32::from_rgb(36, 178, 228));
            let u_segs = [
                [
                    Pos2::new(lc.x - 6.0, lc.y - 3.0),
                    Pos2::new(lc.x - 6.0, lc.y + 3.0),
                ],
                [
                    Pos2::new(lc.x - 6.0, lc.y + 3.0),
                    Pos2::new(lc.x + 6.0, lc.y + 3.0),
                ],
                [
                    Pos2::new(lc.x + 6.0, lc.y + 3.0),
                    Pos2::new(lc.x + 6.0, lc.y - 3.0),
                ],
            ];
            for seg in u_segs {
                lp.line_segment(seg, outline);
            }
            for seg in u_segs {
                lp.line_segment(seg, core);
            }
            if lresp
                .on_hover_text("Link vertical zoom and volume")
                .clicked()
            {
                state.link_zoom_volume = !state.link_zoom_volume;
            }
        });
    });

    // Bottom ~20-icon Aegisub audio toolbar (coloured icons + toggles, no text words).
    ui.horizontal(|ui| {
        ui.spacing_mut().item_spacing.x = 1.5;
        if audio_tool_button(
            ui,
            AudioToolIcon::PrevLine,
            false,
            palette,
            "Previous line (Z)",
        ) {
            action = Some(AudioPanelAction::PrevLine);
        }
        if audio_tool_button(ui, AudioToolIcon::NextLine, false, palette, "Next line (X)") {
            action = Some(AudioPanelAction::NextLine);
        }
        ui.separator();
        if audio_tool_button(
            ui,
            AudioToolIcon::PlaySel,
            false,
            palette,
            "Play selection (S)",
        ) {
            action = Some(AudioPanelAction::Play(PlayRangeKind::Selection));
        }
        if audio_tool_button(
            ui,
            AudioToolIcon::PlayLine,
            false,
            palette,
            "Play current line (R)",
        ) {
            action = Some(AudioPanelAction::Play(PlayRangeKind::Selection));
        }
        if audio_tool_button(ui, AudioToolIcon::Stop, false, palette, "Stop audio (H)") {
            action = Some(AudioPanelAction::Stop);
        }
        ui.separator();
        if audio_tool_button(
            ui,
            AudioToolIcon::PlayBeforeStart,
            false,
            palette,
            "Play 500ms before start (Q)",
        ) {
            action = Some(AudioPanelAction::Play(PlayRangeKind::BeforeStart500ms));
        }
        if audio_tool_button(
            ui,
            AudioToolIcon::PlayAfterStart,
            false,
            palette,
            "Play 500ms after start (E)",
        ) {
            action = Some(AudioPanelAction::Play(PlayRangeKind::First500ms));
        }
        if audio_tool_button(
            ui,
            AudioToolIcon::PlayFirst500,
            false,
            palette,
            "Play first 500ms of selection",
        ) {
            action = Some(AudioPanelAction::Play(PlayRangeKind::First500ms));
        }
        if audio_tool_button(
            ui,
            AudioToolIcon::PlayLast500,
            false,
            palette,
            "Play last 500ms of selection (D)",
        ) {
            action = Some(AudioPanelAction::Play(PlayRangeKind::Last500ms));
        }
        if audio_tool_button(
            ui,
            AudioToolIcon::PlayBeforeEnd,
            false,
            palette,
            "Play 500ms before end",
        ) {
            action = Some(AudioPanelAction::Play(PlayRangeKind::Last500ms));
        }
        if audio_tool_button(
            ui,
            AudioToolIcon::PlayAfterEnd,
            false,
            palette,
            "Play 500ms after end (W)",
        ) {
            action = Some(AudioPanelAction::Play(PlayRangeKind::AfterEnd500ms));
        }
        if audio_tool_button(
            ui,
            AudioToolIcon::PlayToEnd,
            false,
            palette,
            "Play from selection to end of audio (T)",
        ) {
            action = Some(AudioPanelAction::PlayToEnd);
        }
        ui.separator();
        if audio_tool_button(
            ui,
            AudioToolIcon::Commit,
            false,
            palette,
            "Commit timing (G)",
        ) {
            action = Some(AudioPanelAction::Commit);
        }
        if audio_tool_button(
            ui,
            AudioToolIcon::CommitNext,
            false,
            palette,
            "Commit and go to next line",
        ) {
            action = Some(AudioPanelAction::CommitAndNext);
        }
        if audio_tool_button(ui, AudioToolIcon::LeadIn, false, palette, "Add lead-in (C)") {
            if let Some(tc) = timing {
                tc.add_lead_in();
            }
            action = Some(AudioPanelAction::AddLeadIn);
        }
        if audio_tool_button(
            ui,
            AudioToolIcon::LeadOut,
            false,
            palette,
            "Add lead-out (V)",
        ) {
            action = Some(AudioPanelAction::AddLeadOut);
        }
        ui.separator();
        if audio_tool_button(
            ui,
            AudioToolIcon::AutoCommit,
            state.auto_commit,
            palette,
            "Auto-commit changes",
        ) {
            state.auto_commit = !state.auto_commit;
        }
        if audio_tool_button(
            ui,
            AudioToolIcon::AutoNext,
            state.auto_next,
            palette,
            "Auto-go to next line on commit",
        ) {
            state.auto_next = !state.auto_next;
        }
        if audio_tool_button(
            ui,
            AudioToolIcon::AutoScroll,
            state.auto_scroll,
            palette,
            "Auto-scroll audio to active line",
        ) {
            state.auto_scroll = !state.auto_scroll;
        }
        if audio_tool_button(
            ui,
            AudioToolIcon::Spectrum,
            state.spectrum_mode,
            palette,
            "Toggle Spectrum / Waveform display",
        ) {
            state.spectrum_mode = !state.spectrum_mode;
        }
        if audio_tool_button(
            ui,
            AudioToolIcon::Karaoke,
            state.karaoke_mode,
            palette,
            "Toggle Karaoke timing mode",
        ) {
            state.karaoke_mode = !state.karaoke_mode;
        }
        if audio_tool_button(
            ui,
            AudioToolIcon::OptionsWrench,
            false,
            palette,
            "Audio timing options",
        ) {
            action = Some(AudioPanelAction::OpenTimingOptions);
        }
    });

    action
}

#[cfg(test)]
mod tests {
    use super::*;
    use egui_kittest::Harness;
    use i18n_catalog::Locale;
    use theme::ThemeSpec;

    struct SineProvider;
    impl AudioColumnProvider for SineProvider {
        fn duration_ms(&self) -> i64 {
            120_000
        }
        fn query_columns_into(
            &self,
            start_ms: i64,
            end_ms: i64,
            width_px: usize,
            out: &mut Vec<PeakBucket>,
        ) {
            out.clear();
            let span = (end_ms - start_ms).max(1) as f32;
            for i in 0..width_px {
                let t = (start_ms as f32) + span * (i as f32 / width_px.max(1) as f32);
                #[allow(clippy::cast_possible_truncation, clippy::cast_sign_loss)]
                let v = ((t * 0.02).sin().abs() * 20_000.0) as i16;
                out.push(PeakBucket {
                    min: -v,
                    max: v,
                    rms: v.unsigned_abs(),
                });
            }
        }
    }

    #[test]
    fn audio_panel_uploads_texture_once_and_zero_when_unchanged() {
        let mut state = AudioPanelState::new(120_000);
        let provider = SineProvider;
        let markers = TimelineMarkerIndex::new(vec![1000, 3000, 5000], vec![0, 2000, 4000]);
        let pal = theme::palette(ThemeSpec::Light);
        let cat = Catalog::for_locale(Locale::En);
        let fonts = ui_theme::build_font_definitions(&ui_theme::FontConfig::default());
        let mut inited = false;

        let mut harness = Harness::builder()
            .with_accessibility_check(false)
            .build_ui(move |ui| {
                if !inited {
                    ui.ctx().set_fonts(fonts.clone());
                    inited = true;
                }
                let _ = show_audio_panel(
                    ui, &mut state, &provider, &markers, None, None, &pal, 1, &cat,
                );
                assert_eq!(state.texture_uploads(), 1);
            });
        harness.run();
        harness.step();
    }

    fn flatten(shape: &egui::epaint::Shape, out: &mut Vec<egui::epaint::Shape>) {
        if let egui::epaint::Shape::Vec(v) = shape {
            for s in v {
                flatten(s, out);
            }
        } else {
            out.push(shape.clone());
        }
    }

    /// The active-line selection overlay, its start/end markers and the keyframe lines must be
    /// painted over the decoded audio in both spectrum and waveform mode.
    #[test]
    fn selection_overlay_markers_and_keyframes_are_painted_in_both_modes() {
        let pal = theme::palette(ThemeSpec::Dark);
        let cat = Catalog::for_locale(Locale::En);
        let markers = TimelineMarkerIndex::new(vec![58_500, 59_700, 61_000], vec![57_900, 60_800]);
        for spectrum in [true, false] {
            let mut state = AudioPanelState::new(120_000);
            state.spectrum_mode = spectrum;
            state.viewport.scroll_ms = 55_000.0;
            state.active_selection = TimeRangeMs {
                start_ms: 57_900,
                end_ms: 60_800,
            };
            let ctx = egui::Context::default();
            ctx.set_fonts(ui_theme::build_font_definitions(
                &ui_theme::FontConfig::default(),
            ));
            let raw = egui::RawInput {
                screen_rect: Some(Rect::from_min_size(Pos2::ZERO, Vec2::new(1280.0, 300.0))),
                ..Default::default()
            };
            let mut shapes = Vec::new();
            for _ in 0..2 {
                let mut out = ctx.run_ui(raw.clone(), |ui| {
                    let _ = show_audio_panel(
                        ui,
                        &mut state,
                        &SineProvider,
                        &markers,
                        None,
                        None,
                        &pal,
                        1,
                        &cat,
                    );
                });
                out.textures_delta.clear();
                shapes.clear();
                for cs in &out.shapes {
                    flatten(&cs.shape, &mut shapes);
                }
            }
            // Initial zoom shows several seconds, not a sliver.
            let vis = state.viewport.visible_range_ms();
            assert!(
                vis.end_ms - vis.start_ms >= 6_000,
                "visible span too small: {vis:?}"
            );
            let sel_rgb = ui_theme::rgba(pal.audio_selection);
            let sel = Color32::from_rgba_unmultiplied(sel_rgb.r(), sel_rgb.g(), sel_rgb.b(), 82);
            let has_fill = shapes.iter().any(|s| {
                matches!(s, egui::epaint::Shape::Rect(r) if r.fill == sel && r.rect.width() > 20.0)
            });
            assert!(
                has_fill,
                "selection overlay not painted (spectrum={spectrum})"
            );
            let line_colours: Vec<egui::Color32> = shapes
                .iter()
                .filter_map(|s| match s {
                    egui::epaint::Shape::LineSegment { stroke, .. } => Some(stroke.color),
                    _ => None,
                })
                .collect();
            // Bright pink keyframe lines (theme-independent so they stay visible over any audio).
            let key = Color32::from_rgb(255, 48, 200);
            assert!(
                line_colours.iter().filter(|&&c| c == key).count() >= 2,
                "keyframe lines not painted (spectrum={spectrum})"
            );
            for c in [
                Color32::from_rgb(238, 72, 72),
                Color32::from_rgb(64, 148, 255),
            ] {
                assert!(
                    line_colours.contains(&c),
                    "selection start/end marker {c:?} missing (spectrum={spectrum})"
                );
            }
        }
    }

    /// Regression: the cached ruler labels must never survive a font-atlas rebuild.
    #[test]
    fn ruler_galleys_are_fresh_after_atlas_rebuild() {
        let pal = theme::palette(ThemeSpec::Dark);
        let cat = Catalog::for_locale(Locale::En);
        let markers = TimelineMarkerIndex::new(vec![], vec![]);
        let mut state = AudioPanelState::new(120_000);
        let ctx = egui::Context::default();
        ctx.set_fonts(ui_theme::build_font_definitions(
            &ui_theme::FontConfig::default(),
        ));
        let raw = egui::RawInput {
            screen_rect: Some(Rect::from_min_size(Pos2::ZERO, Vec2::new(1000.0, 300.0))),
            ..Default::default()
        };
        let mut frame = |ctx: &egui::Context| {
            let mut out = ctx.run_ui(raw.clone(), |ui| {
                let _ = show_audio_panel(
                    ui,
                    &mut state,
                    &SineProvider,
                    &markers,
                    None,
                    None,
                    &pal,
                    1,
                    &cat,
                );
            });
            out.textures_delta.clear();
            let labels = ui_theme::galley_check::text_galleys(&out.shapes)
                .iter()
                .filter(|g| g.job.text.starts_with("0:00:"))
                .count();
            assert!(labels > 0, "ruler label missing");
            ui_theme::galley_check::count_stale(ctx, &out.shapes)
        };
        assert_eq!(frame(&ctx), 0);
        assert_eq!(frame(&ctx), 0);
        ctx.set_pixels_per_point(1.5);
        assert_eq!(frame(&ctx), 0);
        ctx.set_fonts(egui::FontDefinitions::default());
        assert_eq!(frame(&ctx), 0);
        assert_eq!(frame(&ctx), 0);
    }

    struct SpeechLikePcmProvider {
        samples: Vec<i16>,
        sr: u32,
    }
    impl SpeechLikePcmProvider {
        fn new(duration_ms: i64, sr: u32) -> Self {
            #[allow(clippy::cast_possible_truncation, clippy::cast_sign_loss)]
            let len = ((duration_ms.max(100) as u64 * u64::from(sr)) / 1_000) as usize;
            let mut samples = Vec::with_capacity(len);
            let mut rng = 0x1234_5678u32;
            for i in 0..len {
                rng = rng.wrapping_mul(1_664_525).wrapping_add(1_013_904_223);
                let noise = ((rng >> 16) as f32 / 32_768.0 - 1.0) * 0.015;
                let t = i as f32 / sr as f32;
                #[allow(clippy::cast_possible_truncation, clippy::cast_sign_loss)]
                let env = if ((t * 2.5) as usize).is_multiple_of(3) {
                    0.05
                } else {
                    0.65
                };
                let s = noise
                    + env
                        * (0.5 * (std::f32::consts::TAU * 175.0 * t).sin()
                            + 0.3 * (std::f32::consts::TAU * 720.0 * t).sin()
                            + 0.2 * (std::f32::consts::TAU * 2_200.0 * t).sin());
                #[allow(clippy::cast_possible_truncation)]
                samples.push((s.clamp(-1.0, 1.0) * 30_000.0) as i16);
            }
            Self { samples, sr }
        }
    }
    impl AudioColumnProvider for SpeechLikePcmProvider {
        fn duration_ms(&self) -> i64 {
            #[allow(clippy::cast_possible_wrap)]
            let n = self.samples.len() as i64;
            (n * 1_000) / i64::from(self.sr.max(1))
        }
        fn sample_rate(&self) -> u32 {
            self.sr
        }
        fn read_mono_i16(&self, start_sample: usize, count: usize, out: &mut Vec<i16>) {
            out.clear();
            if start_sample >= self.samples.len() {
                return;
            }
            let end = (start_sample + count).min(self.samples.len());
            out.extend_from_slice(&self.samples[start_sample..end]);
        }
        fn query_columns_into(
            &self,
            start_ms: i64,
            end_ms: i64,
            width_px: usize,
            out: &mut Vec<PeakBucket>,
        ) {
            out.clear();
            let span = (end_ms - start_ms).max(1) as f64;
            let sr_f = f64::from(self.sr);
            let n = self.samples.len();
            for col in 0..width_px {
                let t0 = start_ms as f64 + (col as f64 / width_px.max(1) as f64) * span;
                let t1 = start_ms as f64 + ((col + 1) as f64 / width_px.max(1) as f64) * span;
                #[allow(clippy::cast_possible_truncation, clippy::cast_sign_loss)]
                let s0 = ((t0.max(0.0) * sr_f) / 1_000.0) as usize;
                #[allow(clippy::cast_possible_truncation, clippy::cast_sign_loss)]
                let s1 = (((t1.max(0.0) * sr_f) / 1_000.0) as usize).max(s0 + 1);
                if s0 >= n {
                    out.push(PeakBucket {
                        min: 0,
                        max: 0,
                        rms: 0,
                    });
                    continue;
                }
                let mut mn = i16::MAX;
                let mut mx = i16::MIN;
                let mut sq = 0u64;
                let slice = &self.samples[s0..s1.min(n)];
                for &s in slice {
                    mn = mn.min(s);
                    mx = mx.max(s);
                    let v = i64::from(s);
                    sq = sq.saturating_add((v * v).unsigned_abs());
                }
                #[allow(clippy::cast_possible_truncation, clippy::cast_sign_loss)]
                let rms = ((sq as f64 / slice.len().max(1) as f64).sqrt()) as u16;
                out.push(PeakBucket {
                    min: mn,
                    max: mx,
                    rms,
                });
            }
        }
    }

    #[test]
    fn column_computation_counts_zero_on_idle_and_n_on_scroll() {
        let provider = SpeechLikePcmProvider::new(30_000, 16_000);
        let markers = TimelineMarkerIndex::new(vec![1_000, 2_500, 4_000], vec![500, 2_000]);
        let pal = theme::palette(ThemeSpec::Dark);
        let cat = Catalog::for_locale(Locale::En);
        let ctx = egui::Context::default();
        ctx.set_fonts(ui_theme::build_font_definitions(
            &ui_theme::FontConfig::default(),
        ));
        let raw = egui::RawInput {
            screen_rect: Some(Rect::from_min_size(Pos2::ZERO, Vec2::new(600.0, 200.0))),
            ..Default::default()
        };

        let mut state = AudioPanelState::new(30_000);
        state.viewport.ms_per_px = 5.0; // 1 px = 5 ms
        state.spectrum_mode = true;

        // Frame 1: full screen computed.
        let mut out1 = ctx.run_ui(raw.clone(), |ui| {
            egui::CentralPanel::default().show(ui, |ui| {
                let _ = show_audio_panel(
                    ui, &mut state, &provider, &markers, None, None, &pal, 1, &cat,
                );
            });
        });
        out1.textures_delta.clear();
        assert!(state.last_computed_columns() > 100);
        let uploads_after_first = state.texture_uploads();

        // Frame 2: idle -> 0 columns computed, 0 texture uploads.
        let mut out2 = ctx.run_ui(raw.clone(), |ui| {
            egui::CentralPanel::default().show(ui, |ui| {
                let _ = show_audio_panel(
                    ui, &mut state, &provider, &markers, None, None, &pal, 1, &cat,
                );
            });
        });
        out2.textures_delta.clear();
        assert_eq!(state.last_computed_columns(), 0);
        assert_eq!(state.texture_uploads(), uploads_after_first);

        // Frame 3: scroll right by exactly 12 px (60 ms at 5 ms/px) -> computes only 12 columns.
        state.viewport.scroll_ms += 60.0;
        let mut out3 = ctx.run_ui(raw, |ui| {
            egui::CentralPanel::default().show(ui, |ui| {
                let _ = show_audio_panel(
                    ui, &mut state, &provider, &markers, None, None, &pal, 1, &cat,
                );
            });
        });
        out3.textures_delta.clear();
        assert_eq!(state.last_computed_columns(), 12);
    }

    #[test]
    fn snap_inside_outside_shift_and_zoom() {
        let mut state = AudioPanelState::new(30_000);
        state.auto_snap_keyframes = true;
        state.snap_to_lines = true;
        state.snap_distance_px = 10.0;
        state.viewport.ms_per_px = 5.0; // 10 px = 50 ms
        let markers = TimelineMarkerIndex::new(vec![2_000, 4_000], vec![1_500, 3_000]);

        // Inside 50 ms threshold -> snaps to keyframe 2000.
        let (ms, hint) = apply_snap(2_040, false, &state, &markers);
        assert_eq!(ms, 2_000);
        assert_eq!(
            hint,
            Some(SnapHint {
                time_ms: 2_000,
                is_keyframe: true,
            })
        );

        // Outside 50 ms threshold -> no snap.
        let (ms_out, hint_out) = apply_snap(2_060, false, &state, &markers);
        assert_eq!(ms_out, 2_060);
        assert_eq!(hint_out, None);

        // Shift held -> disables snap even inside threshold.
        let (ms_shift, hint_shift) = apply_snap(2_005, true, &state, &markers);
        assert_eq!(ms_shift, 2_005);
        assert_eq!(hint_shift, None);

        // Zoomed out (20 ms/px -> 10 px = 200 ms): 2_060 is now inside threshold.
        state.viewport.ms_per_px = 20.0;
        let (ms_z, hint_z) = apply_snap(2_060, false, &state, &markers);
        assert_eq!(ms_z, 2_000);
        assert!(hint_z.is_some_and(|h| h.is_keyframe));
    }

    #[test]
    fn audio_panel_renders_waveform_spectrum_and_snap_hint_all_themes_100_200_dpi() {
        let provider = SpeechLikePcmProvider::new(12_000, 16_000);
        let markers = TimelineMarkerIndex::new(vec![1_000, 2_500, 4_000], vec![500, 2_200, 3_800]);
        let cat = Catalog::for_locale(Locale::En);
        let themes = [
            ThemeSpec::Light,
            ThemeSpec::Dark,
            ThemeSpec::EyeComfort {
                base: theme::BaseTheme::Dark,
                strength: 65,
            },
        ];

        for spec in themes {
            let pal = theme::palette(spec);
            for ppp in [1.0f32, 2.0f32] {
                for spectrum_mode in [false, true] {
                    let mut state = AudioPanelState::new(12_000);
                    state.spectrum_mode = spectrum_mode;
                    state.viewport.ms_per_px = 5.0;
                    state.active_selection = TimeRangeMs {
                        start_ms: 1_000,
                        end_ms: 2_500,
                    };
                    state.inactive_spans.push(TimeRangeMs {
                        start_ms: 2_600,
                        end_ms: 3_800,
                    });
                    state.dragging = Some(DragTarget::Start);
                    state.active_snap_hint = Some(SnapHint {
                        time_ms: 1_000,
                        is_keyframe: true,
                    });
                    let fonts = ui_theme::build_font_definitions(&ui_theme::FontConfig::default());
                    let mut inited = false;
                    let mut harness = Harness::builder()
                        .with_size(Vec2::new(720.0, 220.0))
                        .with_pixels_per_point(ppp)
                        .with_accessibility_check(false)
                        .build_ui(|ui| {
                            if !inited {
                                ui.ctx().set_fonts(fonts.clone());
                                inited = true;
                            }
                            let _ = show_audio_panel(
                                ui, &mut state, &provider, &markers, None, None, &pal, 1, &cat,
                            );
                        });
                    harness.run();
                }
            }
        }
    }
}
