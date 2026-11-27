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
    build_frame_layout,
};
use egui::{
    Align2, Color32, ColorImage, FontId, Galley, Pos2, Rect, Sense, Stroke, TextureHandle,
    TextureOptions, Ui, Vec2,
};
use i18n_catalog::Catalog;
use karaoke_timing::KaraokeTimingController;
use std::fmt::Write as _;
use std::sync::Arc;
use theme::Palette;
use timing_controller::{DialogueTimingController, DragTarget, PlayRangeKind, TimeRangeMs};
use ui_theme::{rgba, snap_px};

/// Trait supplying waveform peak columns for a requested `[start_ms, end_ms]` range into a
/// caller-owned buffer.
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

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
struct TextureCacheKey {
    start_ms: i64,
    end_ms: i64,
    width_px: usize,
    height_px: usize,
    spectrum_mode: bool,
    v_zoom_bits: u32,
    palette_rev: u64,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
struct RulerCacheKey {
    start_ms: i64,
    end_ms: i64,
    width_bits: u32,
    ppp_bits: u32,
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
    pub h_zoom: f32,
    pub v_zoom: f32,
    pub volume: f32,
    pub link_zoom_volume: bool,
    pub play_cursor_ms: Option<i64>,
    pub video_marker_ms: Option<i64>,
    pub active_selection: TimeRangeMs,
    pub dragging: Option<DragTarget>,
    // Cached GPU texture + dirty key (ensures 0 re-uploads when unchanged).
    texture: Option<TextureHandle>,
    cache_key: Option<TextureCacheKey>,
    texture_uploads: u64,
    // Reusable scratch buffers (0 heap allocations in steady state).
    columns_buf: Vec<PeakBucket>,
    pixel_buf: Vec<Color32>,
    frame_layout: AudioFrameLayout,
    ruler_buf: String,
    ruler_key: Option<RulerCacheKey>,
    ruler_ticks: Vec<(f32, Arc<Galley>)>,
}

impl Default for AudioPanelState {
    fn default() -> Self {
        Self::new(60_000)
    }
}

impl AudioPanelState {
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
            h_zoom: 1.0,
            v_zoom: 1.0,
            volume: 1.0,
            link_zoom_volume: true,
            play_cursor_ms: None,
            video_marker_ms: None,
            active_selection: TimeRangeMs {
                start_ms: 0,
                end_ms: 2_000,
            },
            dragging: None,
            texture: None,
            cache_key: None,
            texture_uploads: 0,
            columns_buf: Vec::with_capacity(2048),
            pixel_buf: Vec::with_capacity(2048 * 160),
            frame_layout: AudioFrameLayout::with_capacity(2048),
            ruler_buf: String::with_capacity(16),
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

    /// Rasterizes waveform or classic Aegisub spectrum (dark blue bg, cyan/white energy) into
    /// `self.pixel_buf` without allocating once sized.
    pub fn rasterize_columns(&mut self, width_px: usize, height_px: usize, palette: &Palette) {
        let total = width_px.saturating_mul(height_px);
        let bg = if self.spectrum_mode {
            Color32::from_rgb(8, 16, 40)
        } else {
            rgba(palette.audio_bg)
        };
        if self.pixel_buf.len() == total {
            self.pixel_buf.fill(bg);
        } else {
            self.pixel_buf.resize(total, bg);
        }
        if width_px == 0 || height_px == 0 {
            return;
        }
        let mid_y = (height_px / 2) as f32;
        let wave_col = rgba(palette.audio_waveform);
        // Classic Aegisub spectrum stops: dark blue -> blue -> teal -> bright cyan -> white.
        let stops: [Color32; 5] = [
            Color32::from_rgb(12, 32, 82),
            Color32::from_rgb(20, 74, 152),
            Color32::from_rgb(36, 158, 206),
            Color32::from_rgb(132, 232, 245),
            Color32::from_rgb(242, 252, 255),
        ];

        for (x, col) in self.columns_buf.iter().take(width_px).enumerate() {
            if self.spectrum_mode {
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
                let max_f = f32::from(col.max) / 32_768.0;
                let min_f = f32::from(col.min) / 32_768.0;
                let y_hi = (mid_y - max_f * self.v_zoom * mid_y).clamp(0.0, (height_px - 1) as f32);
                let y_lo = (mid_y - min_f * self.v_zoom * mid_y).clamp(0.0, (height_px - 1) as f32);
                #[allow(clippy::cast_possible_truncation, clippy::cast_sign_loss)]
                let top = (y_hi.min(y_lo) as usize).min(height_px - 1);
                #[allow(clippy::cast_possible_truncation, clippy::cast_sign_loss)]
                let bot = (y_hi.max(y_lo) as usize).min(height_px - 1);
                for y in top..=bot {
                    self.pixel_buf[y * width_px + x] = wave_col;
                }
            }
        }
    }
}

#[inline]
fn role_color(role: ColorRole, pal: &Palette) -> Color32 {
    match role {
        ColorRole::SelectionFill | ColorRole::VideoFrameArea => rgba(pal.audio_selection),
        ColorRole::InactiveFill | ColorRole::InactiveBoundary => rgba(pal.audio_inactive),
        ColorRole::SelectionStartMarker => Color32::from_rgb(235, 65, 65),
        ColorRole::SelectionEndMarker => Color32::from_rgb(235, 110, 185),
        ColorRole::LineBoundary => rgba(pal.audio_line_boundary),
        ColorRole::KeyframeLine => rgba(pal.audio_keyframe),
        ColorRole::PlayCursor => rgba(pal.audio_play_cursor),
        ColorRole::VideoCursor => rgba(pal.audio_video_marker),
        ColorRole::KaraokeBoundary
        | ColorRole::KaraokeLabelBg
        | ColorRole::KaraokeLabelActiveBg => rgba(pal.audio_karaoke_label),
        ColorRole::WaveformPeak | ColorRole::WaveformRms => rgba(pal.audio_waveform),
    }
}

/// Custom aligned vertical slider with dark/subtle track and Aegisub blue thumb.
fn vertical_blue_slider(
    ui: &mut Ui,
    value: &mut f32,
    range: std::ops::RangeInclusive<f32>,
    height: f32,
    tooltip: &str,
) -> bool {
    let (rect, resp) = ui.allocate_exact_size(Vec2::new(14.0, height), Sense::click_and_drag());
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
    let p = ui.painter();
    let track = Rect::from_center_size(rect.center(), Vec2::new(4.0, (height - 4.0).max(8.0)));
    p.rect_filled(track, 2.0, Color32::from_rgb(56, 62, 74));
    let thumb_y = rect.bottom() - 4.0 - frac * (height - 8.0).max(1.0);
    let thumb = Rect::from_center_size(Pos2::new(rect.center().x, thumb_y), Vec2::new(12.0, 6.0));
    p.rect_filled(thumb, 1.5, Color32::from_rgb(56, 136, 226));
    p.rect_stroke(
        thumb,
        1.5,
        Stroke::new(0.8, Color32::from_rgb(24, 72, 148)),
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
    let slider_strip_w = 50.0;
    let total_canvas_h = (ui.available_height() - toolbar_h).max(76.0);
    let wave_h = (total_canvas_h - ruler_h - scrollbar_h).max(48.0);
    let canvas_w = (ui.available_width() - slider_strip_w - 6.0).max(100.0);

    ui.horizontal(|ui| {
        // Left: dark time ruler + waveform/spectrum canvas + dark horizontal scrollbar.
        ui.vertical(|ui| {
            ui.spacing_mut().item_spacing.y = 0.0;

            // 1. Dark top time ruler ("0:05:12" then "13", "14", ...).
            let (ruler_rect, _) =
                ui.allocate_exact_size(Vec2::new(canvas_w, ruler_h), Sense::hover());
            let rp = ui.painter_at(ruler_rect);
            rp.rect_filled(ruler_rect, 0.0, Color32::from_rgb(24, 28, 36));
            let ry_bot = snap_px(ruler_rect.bottom(), ppp);
            rp.line_segment(
                [
                    Pos2::new(ruler_rect.left(), ry_bot),
                    Pos2::new(ruler_rect.right(), ry_bot),
                ],
                Stroke::new(1.0 / ppp.max(0.5), Color32::from_rgb(68, 76, 92)),
            );

            state.viewport.width_px = canvas_w.max(1.0);
            state.viewport.height_px = wave_h.max(1.0);
            let vis = state.viewport.visible_range_ms();
            let span_ms = (vis.end_ms - vis.start_ms).max(100);
            let step_ms: i64 = if span_ms <= 15_000 {
                1_000
            } else if span_ms <= 40_000 {
                2_000
            } else {
                5_000
            };
            let first_tick = ((vis.start_ms + step_ms - 1) / step_ms) * step_ms;
            let tick_font = FontId::proportional(9.5);
            let tick_col = Color32::from_rgb(208, 216, 230);
            let rkey = RulerCacheKey {
                start_ms: vis.start_ms,
                end_ms: vis.end_ms,
                width_bits: canvas_w.to_bits(),
                ppp_bits: ppp.to_bits(),
            };
            if state.ruler_key != Some(rkey) {
                state.ruler_ticks.clear();
                let mut t_ms = first_tick;
                let mut tick_idx = 0usize;
                while t_ms <= vis.end_ms {
                    let frac = (t_ms - vis.start_ms) as f32 / span_ms as f32;
                    let rel_x = frac * ruler_rect.width();
                    let total_s = (t_ms / 1_000).max(0);
                    let h = total_s / 3600;
                    let m = (total_s / 60) % 60;
                    let s = total_s % 60;
                    state.ruler_buf.clear();
                    if tick_idx == 0 || s == 0 {
                        let _ = write!(state.ruler_buf, "{h}:{m:02}:{s:02}");
                    } else {
                        let _ = write!(state.ruler_buf, "{s:02}");
                    }
                    let g = ui.fonts_mut(|f| {
                        f.layout_no_wrap(state.ruler_buf.clone(), tick_font.clone(), tick_col)
                    });
                    state.ruler_ticks.push((rel_x, g));
                    t_ms += step_ms;
                    tick_idx += 1;
                }
                state.ruler_key = Some(rkey);
            }
            for (rel_x, g) in &state.ruler_ticks {
                let tx = snap_px(ruler_rect.left() + *rel_x, ppp);
                rp.line_segment(
                    [
                        Pos2::new(tx, ruler_rect.bottom() - 4.0),
                        Pos2::new(tx, ruler_rect.bottom()),
                    ],
                    Stroke::new(1.0 / ppp.max(0.5), tick_col),
                );
                rp.galley(
                    Pos2::new(tx + 2.0, ruler_rect.top() + 2.0),
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
                palette_rev,
            };

            if state.cache_key != Some(key) {
                provider.query_columns_into(
                    vis.start_ms,
                    vis.end_ms,
                    tex_w,
                    &mut state.columns_buf,
                );
                state.rasterize_columns(tex_w, tex_h, palette);
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
            }

            let painter = ui.painter_at(rect);
            if let Some(ref tex) = state.texture {
                painter.image(
                    tex.id(),
                    rect,
                    Rect::from_min_max(Pos2::ZERO, Pos2::new(1.0, 1.0)),
                    Color32::WHITE,
                );
            } else {
                painter.rect_filled(rect, 0.0, Color32::from_rgb(8, 16, 40));
            }

            let kara_bounds = karaoke.map_or(&[][..], KaraokeTimingController::boundaries_ms);
            let input = AudioFrameInput {
                viewport: state.viewport,
                selection: state.active_selection,
                inactive_spans: &[],
                markers,
                play_cursor_ms: state.play_cursor_ms,
                video_marker: None,
                karaoke_boundaries_ms: if state.karaoke_mode { kara_bounds } else { &[] },
                active_karaoke_syllable: None,
            };
            build_frame_layout(&input, None, &mut state.frame_layout);

            for span in &state.frame_layout.spans {
                let sr = Rect::from_min_max(
                    Pos2::new(rect.left() + span.rect.x0, rect.top() + span.rect.y0),
                    Pos2::new(rect.left() + span.rect.x1, rect.top() + span.rect.y1),
                );
                painter.rect_filled(sr, 0.0, role_color(span.role, palette));
            }

            for m in &state.frame_layout.markers {
                let sx = snap_px(rect.left() + m.x, ppp);
                let is_sel_edge = matches!(
                    m.role,
                    ColorRole::SelectionStartMarker | ColorRole::SelectionEndMarker
                );
                let width = if is_sel_edge {
                    2.0 / ppp.max(0.5)
                } else {
                    1.0 / ppp.max(0.5)
                };
                painter.line_segment(
                    [Pos2::new(sx, rect.top()), Pos2::new(sx, rect.bottom())],
                    Stroke::new(width, role_color(m.role, palette)),
                );
            }

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
                            let ms = state.viewport.x_to_ms(local_x).max(0);
                            state.active_selection.start_ms = ms;
                            if state.active_selection.end_ms < ms {
                                state.active_selection.end_ms = ms + 500;
                            }
                            state.dragging = Some(DragTarget::End);
                        }
                    }
                }
                if resp.dragged()
                    && let Some(target) = state.dragging
                {
                    let ms = state.viewport.x_to_ms(local_x).max(0);
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

        // Right-hand strip: 3 aligned vertical sliders (H-zoom, V-zoom, Volume) with blue thumbs
        // + pink/red Link toggle below them.
        ui.vertical(|ui| {
            let slider_h = (total_canvas_h - 20.0).max(44.0);
            ui.horizontal(|ui| {
                ui.spacing_mut().item_spacing.x = 2.0;
                if vertical_blue_slider(
                    ui,
                    &mut state.h_zoom,
                    0.25..=8.0,
                    slider_h,
                    "Horizontal zoom",
                ) {
                    state.viewport.ms_per_px = (12.0 / f64::from(state.h_zoom)).clamp(0.5, 200.0);
                }
                let prev_v = state.v_zoom;
                vertical_blue_slider(ui, &mut state.v_zoom, 0.25..=4.0, slider_h, "Vertical zoom");
                if state.link_zoom_volume && (state.v_zoom - prev_v).abs() > f32::EPSILON {
                    state.volume = (state.v_zoom / 2.0).clamp(0.0, 2.0);
                }
                vertical_blue_slider(ui, &mut state.volume, 0.0..=2.0, slider_h, "Volume");
            });

            // Pink/red Link toggle button below the 3 sliders.
            let (lr, lresp) = ui.allocate_exact_size(Vec2::new(46.0, 16.0), Sense::click());
            let lp = ui.painter();
            let lbg = if state.link_zoom_volume {
                Color32::from_rgb(220, 86, 128)
            } else {
                Color32::from_rgb(245, 212, 224)
            };
            let lfg = if state.link_zoom_volume {
                Color32::WHITE
            } else {
                Color32::from_rgb(165, 42, 82)
            };
            lp.rect_filled(lr, 2.0, lbg);
            lp.rect_stroke(
                lr,
                2.0,
                Stroke::new(1.0, Color32::from_rgb(185, 56, 98)),
                egui::StrokeKind::Inside,
            );
            lp.text(
                lr.center(),
                Align2::CENTER_CENTER,
                "Link",
                FontId::proportional(10.0),
                lfg,
            );
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
}
