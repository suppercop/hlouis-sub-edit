//! Audio waveform / spectrum panel (`egui 0.37`) built on [`audio_peaks`], [`audio_view`],
//! [`timing_controller`], [`karaoke_timing`], and [`theme::Palette`].
//!
//! Mirrors Aegisub 3.2.2 / 3.5.1 `src/audio_display.cpp` & `src/audio_box.cpp`:
//! - Top time ruler (`0:00.0` ticks) synced to the visible viewport.
//! - Renders waveform or 5-stop spectrum columns into a cached [`egui::TextureHandle`] with
//!   strict dirty-key tracking (**zero texture re-upload** when viewport/zoom/mode/theme are
//!   unchanged).
//! - Overlays inactive lines, blue keyframes, active selection, crisp red start / pink end lines,
//!   karaoke syllable boundaries, video-position marker, and play cursor.
//! - Right-hand 3 vertical sliders (H-zoom, V-zoom, Volume) + `Link` toggle.
//! - Compact bottom icon toolbar with vector-painted 16×16 Aegisub audio icons.
//! - Bench target: 2,000 px wide with 20,000 lines `< 1 ms` CPU.

#![forbid(unsafe_code)]
#![allow(clippy::cast_precision_loss)]
#![cfg_attr(not(test), deny(clippy::unwrap_used, clippy::expect_used))]

use audio_peaks::PeakBucket;
use audio_view::{
    AudioFrameInput, AudioFrameLayout, AudioViewport, ColorRole, HitTarget, TimelineMarkerIndex,
    build_frame_layout,
};
use egui::{
    Align2, Color32, ColorImage, FontId, Pos2, Rect, Sense, Stroke, TextureHandle, TextureOptions,
    Ui, Vec2,
};
use i18n_catalog::Catalog;
use karaoke_timing::KaraokeTimingController;
use std::fmt::Write as _;
use theme::Palette;
use timing_controller::{DialogueTimingController, DragTarget, PlayRangeKind, TimeRangeMs};
use ui_theme::{rgba, snap_px};

/// Trait supplying waveform peak columns for a requested `[start_ms, end_ms]` range into a
/// caller-owned buffer (implemented by `audio-peaks` pyramid or mock/demo generators).
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
    Play(PlayRangeKind),
    Stop,
    Commit,
    AddLeadIn,
    AddLeadOut,
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

/// Persistent UI state + cached waveform texture + reusable scratch buffers for the audio panel.
pub struct AudioPanelState {
    pub viewport: AudioViewport,
    pub spectrum_mode: bool,
    pub karaoke_mode: bool,
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
            spectrum_mode: false,
            karaoke_mode: false,
            h_zoom: 1.0,
            v_zoom: 1.0,
            volume: 1.0,
            link_zoom_volume: false,
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
        }
    }

    /// Number of texture uploads performed so far (0 on idle/unchanged frames).
    #[inline]
    #[must_use]
    pub const fn texture_uploads(&self) -> u64 {
        self.texture_uploads
    }

    /// Rasterizes waveform or spectrum columns into `self.pixel_buf` without allocating once
    /// sized.
    pub fn rasterize_columns(&mut self, width_px: usize, height_px: usize, palette: &Palette) {
        let total = width_px.saturating_mul(height_px);
        if self.pixel_buf.len() == total {
            self.pixel_buf.fill(rgba(palette.audio_bg));
        } else {
            self.pixel_buf.resize(total, rgba(palette.audio_bg));
        }
        if width_px == 0 || height_px == 0 {
            return;
        }
        let mid_y = (height_px / 2) as f32;
        let wave_col = rgba(palette.audio_waveform);
        let stops: [Color32; 5] = palette.spectrum_stops.map(rgba);

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
        // Classic Aegisub 3.2.2: red start marker, pink end marker, blue keyframe line.
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

#[derive(Clone, Copy)]
enum AudioIcon {
    PlaySel,
    PlayBefore,
    PlayAfter,
    Stop,
    Commit,
    LeadIn,
    LeadOut,
}

#[allow(clippy::too_many_lines)]
fn audio_icon_button(ui: &mut Ui, icon: AudioIcon, pal: &Palette, tooltip: &str) -> bool {
    let (rect, resp) = ui.allocate_exact_size(Vec2::splat(20.0), Sense::click());
    let p = ui.painter();
    let bg = if resp.hovered() {
        rgba(pal.hover)
    } else {
        rgba(pal.toolbar_bg)
    };
    p.rect_filled(rect, 2.0, bg);
    p.rect_stroke(
        rect,
        2.0,
        Stroke::new(1.0, rgba(pal.border)),
        egui::StrokeKind::Inside,
    );
    let fg = rgba(pal.text_primary);
    let c = rect.center();
    match icon {
        AudioIcon::PlaySel => {
            p.add(egui::Shape::convex_polygon(
                vec![
                    Pos2::new(c.x - 4.0, c.y - 5.0),
                    Pos2::new(c.x + 5.0, c.y),
                    Pos2::new(c.x - 4.0, c.y + 5.0),
                ],
                fg,
                Stroke::NONE,
            ));
        }
        AudioIcon::PlayBefore => {
            p.line_segment(
                [
                    Pos2::new(c.x + 4.0, c.y - 5.0),
                    Pos2::new(c.x + 4.0, c.y + 5.0),
                ],
                Stroke::new(1.5, Color32::from_rgb(235, 65, 65)),
            );
            p.add(egui::Shape::convex_polygon(
                vec![
                    Pos2::new(c.x - 5.0, c.y - 4.0),
                    Pos2::new(c.x + 2.0, c.y),
                    Pos2::new(c.x - 5.0, c.y + 4.0),
                ],
                fg,
                Stroke::NONE,
            ));
        }
        AudioIcon::PlayAfter => {
            p.line_segment(
                [
                    Pos2::new(c.x - 4.0, c.y - 5.0),
                    Pos2::new(c.x - 4.0, c.y + 5.0),
                ],
                Stroke::new(1.5, Color32::from_rgb(235, 110, 185)),
            );
            p.add(egui::Shape::convex_polygon(
                vec![
                    Pos2::new(c.x - 2.0, c.y - 4.0),
                    Pos2::new(c.x + 5.0, c.y),
                    Pos2::new(c.x - 2.0, c.y + 4.0),
                ],
                fg,
                Stroke::NONE,
            ));
        }
        AudioIcon::Stop => {
            p.rect_filled(Rect::from_center_size(c, Vec2::splat(8.0)), 1.0, fg);
        }
        AudioIcon::Commit => {
            let green = Color32::from_rgb(46, 170, 92);
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
        AudioIcon::LeadIn => {
            p.line_segment(
                [
                    Pos2::new(c.x + 4.0, c.y - 5.0),
                    Pos2::new(c.x + 4.0, c.y + 5.0),
                ],
                Stroke::new(1.5, fg),
            );
            p.line_segment(
                [Pos2::new(c.x - 5.0, c.y), Pos2::new(c.x + 2.0, c.y)],
                Stroke::new(1.5, fg),
            );
            p.line_segment(
                [Pos2::new(c.x - 5.0, c.y), Pos2::new(c.x - 2.0, c.y - 3.0)],
                Stroke::new(1.5, fg),
            );
            p.line_segment(
                [Pos2::new(c.x - 5.0, c.y), Pos2::new(c.x - 2.0, c.y + 3.0)],
                Stroke::new(1.5, fg),
            );
        }
        AudioIcon::LeadOut => {
            p.line_segment(
                [
                    Pos2::new(c.x - 4.0, c.y - 5.0),
                    Pos2::new(c.x - 4.0, c.y + 5.0),
                ],
                Stroke::new(1.5, fg),
            );
            p.line_segment(
                [Pos2::new(c.x - 2.0, c.y), Pos2::new(c.x + 5.0, c.y)],
                Stroke::new(1.5, fg),
            );
            p.line_segment(
                [Pos2::new(c.x + 5.0, c.y), Pos2::new(c.x + 2.0, c.y - 3.0)],
                Stroke::new(1.5, fg),
            );
            p.line_segment(
                [Pos2::new(c.x + 5.0, c.y), Pos2::new(c.x + 2.0, c.y + 3.0)],
                Stroke::new(1.5, fg),
            );
        }
    }
    resp.on_hover_text(tooltip).clicked()
}

/// Renders the full Aegisub 3.2.2 audio panel (top time ruler + waveform/spectrum canvas +
/// 3 vertical sliders H-zoom/V-zoom/Volume + Link toggle + compact icon toolbar) into `ui`.
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
    catalog: &Catalog,
) -> Option<AudioPanelAction> {
    let mut action: Option<AudioPanelAction> = None;
    let ppp = ui.ctx().pixels_per_point();

    let ruler_h = 16.0;
    let toolbar_h = 26.0;
    let slider_strip_w = 60.0;
    let total_canvas_h = (ui.available_height() - toolbar_h).max(64.0);
    let wave_h = (total_canvas_h - ruler_h).max(48.0);
    let canvas_w = (ui.available_width() - slider_strip_w - 6.0).max(100.0);

    ui.horizontal(|ui| {
        // Left: top time ruler + waveform/spectrum canvas stacked tightly.
        ui.vertical(|ui| {
            ui.spacing_mut().item_spacing.y = 0.0;

            // 1. Top time ruler.
            let (ruler_rect, _) =
                ui.allocate_exact_size(Vec2::new(canvas_w, ruler_h), Sense::hover());
            let rp = ui.painter_at(ruler_rect);
            rp.rect_filled(ruler_rect, 0.0, rgba(palette.toolbar_bg));
            let ry_bot = snap_px(ruler_rect.bottom(), ppp);
            rp.line_segment(
                [
                    Pos2::new(ruler_rect.left(), ry_bot),
                    Pos2::new(ruler_rect.right(), ry_bot),
                ],
                Stroke::new(1.0 / ppp.max(0.5), rgba(palette.border)),
            );

            state.viewport.width_px = canvas_w.max(1.0);
            state.viewport.height_px = wave_h.max(1.0);
            let vis = state.viewport.visible_range_ms();
            let span_ms = (vis.end_ms - vis.start_ms).max(100);
            let step_ms: i64 = if span_ms <= 3_000 {
                500
            } else if span_ms <= 12_000 {
                1_000
            } else if span_ms <= 30_000 {
                2_000
            } else {
                5_000
            };
            let first_tick = ((vis.start_ms + step_ms - 1) / step_ms) * step_ms;
            let tick_font = FontId::monospace(9.5);
            let tick_col = rgba(palette.text_secondary);
            let mut t_ms = first_tick;
            while t_ms <= vis.end_ms {
                let frac = (t_ms - vis.start_ms) as f32 / span_ms as f32;
                let tx = snap_px(ruler_rect.left() + frac * ruler_rect.width(), ppp);
                rp.line_segment(
                    [
                        Pos2::new(tx, ruler_rect.bottom() - 5.0),
                        Pos2::new(tx, ruler_rect.bottom()),
                    ],
                    Stroke::new(1.0 / ppp.max(0.5), tick_col),
                );
                let total_s = (t_ms / 1_000).max(0);
                let mins = total_s / 60;
                let secs = total_s % 60;
                let tenths = ((t_ms % 1_000).abs()) / 100;
                state.ruler_buf.clear();
                let _ = write!(state.ruler_buf, "{mins}:{secs:02}.{tenths}");
                rp.text(
                    Pos2::new(tx + 2.0, ruler_rect.top() + 1.0),
                    Align2::LEFT_TOP,
                    &state.ruler_buf,
                    tick_font.clone(),
                    tick_col,
                );
                t_ms += step_ms;
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

            // Re-rasterize and re-upload GPU texture ONLY when the dirty key changes!
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

            // Paint cached waveform/spectrum texture.
            let painter = ui.painter_at(rect);
            if let Some(ref tex) = state.texture {
                painter.image(
                    tex.id(),
                    rect,
                    Rect::from_min_max(Pos2::ZERO, Pos2::new(1.0, 1.0)),
                    Color32::WHITE,
                );
            } else {
                painter.rect_filled(rect, 0.0, rgba(palette.audio_bg));
            }

            // Build overlay geometry using audio-view (zero alloc in steady state).
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

            // Paint spans (selection / inactive lines).
            for span in &state.frame_layout.spans {
                let sr = Rect::from_min_max(
                    Pos2::new(rect.left() + span.rect.x0, rect.top() + span.rect.y0),
                    Pos2::new(rect.left() + span.rect.x1, rect.top() + span.rect.y1),
                );
                painter.rect_filled(sr, 0.0, role_color(span.role, palette));
            }

            // Paint crisp pixel-snapped vertical markers (keyframes, red start / pink end, cursors).
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

            // Mouse interaction: drag start/end markers or click to reposition selection.
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
        });

        // Right-hand strip: 3 vertical sliders (H-zoom, V-zoom, Volume) + Link toggle.
        ui.vertical(|ui| {
            let slider_h = (total_canvas_h - 22.0).max(36.0);
            ui.spacing_mut().slider_width = slider_h;
            ui.horizontal(|ui| {
                ui.spacing_mut().item_spacing.x = 2.0;
                if ui
                    .add(
                        egui::Slider::new(&mut state.h_zoom, 0.25..=8.0)
                            .vertical()
                            .show_value(false),
                    )
                    .on_hover_text("Horizontal zoom")
                    .changed()
                {
                    let new_ms_per_px = (12.0 / f64::from(state.h_zoom)).clamp(0.5, 200.0);
                    state.viewport.ms_per_px = new_ms_per_px;
                }
                let prev_v = state.v_zoom;
                ui.add(
                    egui::Slider::new(&mut state.v_zoom, 0.25..=4.0)
                        .vertical()
                        .show_value(false),
                )
                .on_hover_text("Vertical zoom");
                if state.link_zoom_volume && (state.v_zoom - prev_v).abs() > f32::EPSILON {
                    state.volume = (state.v_zoom / 2.0).clamp(0.0, 2.0);
                }
                ui.add(
                    egui::Slider::new(&mut state.volume, 0.0..=2.0)
                        .vertical()
                        .show_value(false),
                )
                .on_hover_text("Audio volume");
            });
            ui.toggle_value(&mut state.link_zoom_volume, "Link")
                .on_hover_text("Link vertical zoom and volume");
        });
    });

    // Bottom compact toolbar (Aegisub audio_box.cpp vector icon controls).
    ui.horizontal(|ui| {
        if audio_icon_button(ui, AudioIcon::PlaySel, palette, "Play selection (S)") {
            action = Some(AudioPanelAction::Play(PlayRangeKind::Selection));
        }
        if audio_icon_button(
            ui,
            AudioIcon::PlayBefore,
            palette,
            "Play 500ms before start (Q)",
        ) {
            action = Some(AudioPanelAction::Play(PlayRangeKind::BeforeStart500ms));
        }
        if audio_icon_button(
            ui,
            AudioIcon::PlayAfter,
            palette,
            "Play 500ms after end (W)",
        ) {
            action = Some(AudioPanelAction::Play(PlayRangeKind::AfterEnd500ms));
        }
        if audio_icon_button(ui, AudioIcon::Stop, palette, "Stop audio (H)") {
            action = Some(AudioPanelAction::Stop);
        }
        if audio_icon_button(ui, AudioIcon::Commit, palette, "Commit timing (G)") {
            action = Some(AudioPanelAction::Commit);
        }
        if audio_icon_button(ui, AudioIcon::LeadIn, palette, "Add lead-in (C)") {
            if let Some(tc) = timing {
                tc.add_lead_in();
            }
            action = Some(AudioPanelAction::AddLeadIn);
        }
        if audio_icon_button(ui, AudioIcon::LeadOut, palette, "Add lead-out (V)") {
            action = Some(AudioPanelAction::AddLeadOut);
        }
        ui.separator();
        ui.toggle_value(
            &mut state.spectrum_mode,
            catalog.get("panel-audio-spectrum").unwrap_or("Spectrum"),
        );
        ui.toggle_value(
            &mut state.karaoke_mode,
            catalog.get("panel-audio-karaoke").unwrap_or("Karaoke"),
        );
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
        let pal = theme::palette(ThemeSpec::Dark);
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
