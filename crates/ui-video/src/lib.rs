//! Video preview panel + visual typesetting overlays (`egui 0.37`) built on [`video_geometry`],
//! [`visual_tools`], [`playback`], [`theme::Palette`], and [`i18n_catalog`].
//!
//! Mirrors Aegisub 3.2.2 / 3.5.1 `src/video_display.cpp` & `src/video_box.cpp`:
//! - Displays RGBA frames from a [`VideoFrameSupplier`] trait (zero native/FFI deps; video frames
//!   are ALWAYS rendered in true colour, exempt from Eye Comfort warming).
//! - Zoom / AR / pan via [`video_geometry::VideoDisplayState`].
//! - All 7 visual tools painted as crisp 16×16 vector icons on the left (`Cross`, `Drag`,
//!   `RotateZ`, `RotateXY`, `Scale`, `RectClip`, `VectorClip`) plus vector-clip sub-toolbar.
//! - Full-width seek bar, compact vector playback buttons, frame/time box, offset box
//!   (`+0ms; -0ms`), and Zoom combo below the seek bar.
//! - Bench target: overlay frame with a 500-point vector clip `< 0.3 ms` CPU.

#![forbid(unsafe_code)]
#![allow(clippy::cast_precision_loss, clippy::cast_possible_truncation)]
#![cfg_attr(not(test), deny(clippy::unwrap_used, clippy::expect_used))]

use egui::{
    Color32, ColorImage, Pos2, Rect, RichText, Sense, Stroke, TextureHandle, TextureOptions, Ui,
    Vec2,
};
use i18n_catalog::Catalog;
use playback::{Clock, PlaybackController};
use std::f32::consts::TAU;
use theme::Palette;
use ui_theme::{rgba, snap_px};
use video_geometry::VideoDisplayState;
use visual_tools::vector_clip::{VectorClipMode, VectorClipPath, VectorVertexKind};

/// Active visual typesetting tool (matches Aegisub's left video toolbar).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum ActiveVisualTool {
    #[default]
    Cross,
    Drag,
    RotateZ,
    RotateXY,
    Scale,
    RectClip,
    VectorClip,
}

/// Borrowed RGBA8 video frame returned by [`VideoFrameSupplier`].
#[derive(Debug, Clone, Copy)]
pub struct RgbaFrameRef<'a> {
    pub width: u32,
    pub height: u32,
    pub rgba: &'a [u8],
}

/// Trait supplying decoded RGBA8 video frames (plus optional composited subtitles) to the UI.
pub trait VideoFrameSupplier {
    /// Total frame count of the video.
    fn total_frames(&self) -> u32;
    /// Returns `true` if `frame` is a keyframe.
    fn is_keyframe(&self, frame: u32) -> bool;
    /// Revision token for `frame` (bump when subtitle overlay changes to trigger re-upload).
    fn frame_revision(&self, frame: u32) -> u64;
    /// Borrows the RGBA8 pixels for `frame`.
    fn frame_rgba(&self, frame: u32) -> Option<RgbaFrameRef<'_>>;
}

/// Action emitted by the video panel controls or visual tool drag.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum VideoPanelAction {
    PlayToggle,
    PlayLine,
    StepFrame(i32),
    SeekFrame(u32),
    SeekKeyframe(bool),
    VisualOverlayEdited,
}

/// Persistent UI state for the video panel + visual tool overlay.
pub struct VideoPanelState {
    pub display: VideoDisplayState,
    pub active_tool: ActiveVisualTool,
    pub vclip_mode: VectorClipMode,
    pub vclip_path: VectorClipPath,
    pub rect_clip: [f64; 4],
    pub drag_pos: [f64; 2],
    pub dragging_vertex: Option<usize>,
    pub show_overscan: bool,
    pub zoom_percent: u16,
    pub active_line_start_ms: i64,
    pub active_line_end_ms: i64,
    // Cached GPU texture + revision key (0 re-upload when unchanged).
    texture: Option<TextureHandle>,
    uploaded_key: Option<(u32, u64)>,
    texture_uploads: u64,
    // Reusable scratch buffer for drawing 500+ vector clip segments without allocating.
    poly_scratch: Vec<Pos2>,
}

impl Default for VideoPanelState {
    fn default() -> Self {
        Self::new(1920, 1080)
    }
}

impl VideoPanelState {
    /// Creates a video panel state initialized for a `video_w × video_h` stream.
    #[must_use]
    pub fn new(video_w: u32, video_h: u32) -> Self {
        Self {
            display: VideoDisplayState::new(
                f64::from(video_w.max(1)),
                f64::from(video_h.max(1)),
                640.0,
                360.0,
            ),
            active_tool: ActiveVisualTool::Cross,
            vclip_mode: VectorClipMode::Drag,
            vclip_path: VectorClipPath::default(),
            rect_clip: [160.0, 120.0, 1760.0, 960.0],
            drag_pos: [960.0, 540.0],
            dragging_vertex: None,
            show_overscan: false,
            zoom_percent: 50,
            active_line_start_ms: 0,
            active_line_end_ms: 2_000,
            texture: None,
            uploaded_key: None,
            texture_uploads: 0,
            poly_scratch: Vec::with_capacity(1024),
        }
    }

    #[inline]
    #[must_use]
    pub const fn texture_uploads(&self) -> u64 {
        self.texture_uploads
    }

    /// Maps a script/video coordinate `(vx, vy)` into screen coordinates inside `video_rect`
    /// with zero heap allocations.
    #[inline]
    #[must_use]
    pub fn video_to_screen(video_rect: Rect, video_w: f32, video_h: f32, vx: f64, vy: f64) -> Pos2 {
        let sx = video_rect.left() + (vx as f32 / video_w.max(1.0)) * video_rect.width();
        let sy = video_rect.top() + (vy as f32 / video_h.max(1.0)) * video_rect.height();
        Pos2::new(sx, sy)
    }

    /// Maps a screen position inside `video_rect` back to video coordinates (0 allocations).
    #[inline]
    #[must_use]
    pub fn screen_to_video(
        video_rect: Rect,
        video_w: f32,
        video_h: f32,
        p: Pos2,
    ) -> visual_tools::Vec2 {
        let vx =
            f64::from((p.x - video_rect.left()) / video_rect.width().max(1.0)) * f64::from(video_w);
        let vy =
            f64::from((p.y - video_rect.top()) / video_rect.height().max(1.0)) * f64::from(video_h);
        visual_tools::Vec2::new(vx, vy)
    }
}

/// Paints a crisp 16×16 vector icon for one of the 7 Aegisub visual typesetting tools.
#[allow(clippy::too_many_lines)]
fn visual_tool_icon_button(
    ui: &mut Ui,
    tool: ActiveVisualTool,
    selected: bool,
    pal: &Palette,
    tooltip: &str,
) -> bool {
    let (rect, resp) = ui.allocate_exact_size(Vec2::splat(22.0), Sense::click());
    let p = ui.painter();
    let bg = if selected {
        rgba(pal.selection_bg)
    } else if resp.hovered() {
        rgba(pal.hover)
    } else {
        rgba(pal.toolbar_bg)
    };
    let fg = if selected {
        rgba(pal.selection_text)
    } else {
        rgba(pal.text_primary)
    };
    p.rect_filled(rect, 2.0, bg);
    p.rect_stroke(
        rect,
        2.0,
        Stroke::new(
            1.0,
            if selected {
                rgba(pal.focus_ring)
            } else {
                rgba(pal.border)
            },
        ),
        egui::StrokeKind::Inside,
    );
    let c = rect.center();
    let st = Stroke::new(1.3, fg);
    match tool {
        ActiveVisualTool::Cross => {
            // Standard pointer arrow.
            p.add(egui::Shape::convex_polygon(
                vec![
                    Pos2::new(c.x - 4.0, c.y - 6.0),
                    Pos2::new(c.x - 4.0, c.y + 5.0),
                    Pos2::new(c.x - 1.0, c.y + 2.5),
                    Pos2::new(c.x + 3.5, c.y + 3.0),
                ],
                fg,
                Stroke::NONE,
            ));
        }
        ActiveVisualTool::Drag => {
            // 4-way drag arrows.
            p.line_segment([Pos2::new(c.x - 6.0, c.y), Pos2::new(c.x + 6.0, c.y)], st);
            p.line_segment([Pos2::new(c.x, c.y - 6.0), Pos2::new(c.x, c.y + 6.0)], st);
            for (dx, dy) in [(-6.0, 0.0), (6.0, 0.0), (0.0, -6.0), (0.0, 6.0)] {
                p.circle_filled(Pos2::new(c.x + dx, c.y + dy), 1.6, fg);
            }
        }
        ActiveVisualTool::RotateZ => {
            // Circular rotation arc + arrowhead.
            let r = 5.5;
            let pts: Vec<Pos2> = (0..=12)
                .map(|i| {
                    let a = (i as f32 / 12.0) * TAU * 0.78 - 0.5;
                    Pos2::new(c.x + a.cos() * r, c.y + a.sin() * r)
                })
                .collect();
            p.add(egui::Shape::line(pts, st));
            p.circle_filled(c, 1.5, fg);
        }
        ActiveVisualTool::RotateXY => {
            // 3D orbit ellipse + tilted axis.
            let pts: Vec<Pos2> = (0..=16)
                .map(|i| {
                    let a = (i as f32 / 16.0) * TAU;
                    Pos2::new(c.x + a.cos() * 6.0, c.y + a.sin() * 3.0)
                })
                .collect();
            p.add(egui::Shape::closed_line(pts, st));
            p.line_segment(
                [
                    Pos2::new(c.x - 2.0, c.y - 6.0),
                    Pos2::new(c.x + 2.0, c.y + 6.0),
                ],
                st,
            );
        }
        ActiveVisualTool::Scale => {
            // Box + diagonal resize arrow.
            let b = Rect::from_min_max(
                Pos2::new(c.x - 5.5, c.y - 5.5),
                Pos2::new(c.x + 1.5, c.y + 1.5),
            );
            p.rect_stroke(b, 0.0, st, egui::StrokeKind::Inside);
            p.line_segment(
                [
                    Pos2::new(c.x + 1.0, c.y + 1.0),
                    Pos2::new(c.x + 6.0, c.y + 6.0),
                ],
                st,
            );
            p.rect_filled(
                Rect::from_center_size(Pos2::new(c.x + 5.5, c.y + 5.5), Vec2::splat(3.0)),
                0.0,
                fg,
            );
        }
        ActiveVisualTool::RectClip => {
            // Rectangle with 4 corner handles.
            let b = Rect::from_center_size(c, Vec2::new(11.0, 9.0));
            p.rect_stroke(b, 0.0, st, egui::StrokeKind::Inside);
            for corner in [
                b.left_top(),
                b.right_top(),
                b.left_bottom(),
                b.right_bottom(),
            ] {
                p.rect_filled(Rect::from_center_size(corner, Vec2::splat(3.0)), 0.0, fg);
            }
        }
        ActiveVisualTool::VectorClip => {
            // Polygon path with vertex nodes.
            let v0 = Pos2::new(c.x - 5.5, c.y + 4.5);
            let v1 = Pos2::new(c.x - 2.0, c.y - 5.0);
            let v2 = Pos2::new(c.x + 5.5, c.y - 1.5);
            let v3 = Pos2::new(c.x + 2.5, c.y + 5.0);
            p.add(egui::Shape::closed_line(vec![v0, v1, v2, v3], st));
            for v in [v0, v1, v2, v3] {
                p.rect_filled(Rect::from_center_size(v, Vec2::splat(3.0)), 0.0, fg);
            }
        }
    }
    resp.on_hover_text(tooltip).clicked()
}

/// Paints the active visual tool overlay onto `painter` inside `video_rect` (0 allocations in
/// steady state; `< 0.3 ms` even for a 500-point vector clip).
pub fn paint_visual_overlay(
    painter: &egui::Painter,
    video_rect: Rect,
    video_w: f32,
    video_h: f32,
    state: &mut VideoPanelState,
    palette: &Palette,
    pixels_per_point: f32,
) {
    let outline_stroke = Stroke::new(
        1.5 / pixels_per_point.max(0.5),
        rgba(palette.overlay_clip_outline),
    );
    let handle_col = rgba(palette.overlay_handle);
    let cross_stroke = Stroke::new(1.0 / pixels_per_point.max(0.5), rgba(palette.overlay_cross));

    match state.active_tool {
        ActiveVisualTool::Cross => {
            let cx = snap_px(video_rect.center().x, pixels_per_point);
            let cy = snap_px(video_rect.center().y, pixels_per_point);
            painter.line_segment(
                [
                    Pos2::new(video_rect.left(), cy),
                    Pos2::new(video_rect.right(), cy),
                ],
                cross_stroke,
            );
            painter.line_segment(
                [
                    Pos2::new(cx, video_rect.top()),
                    Pos2::new(cx, video_rect.bottom()),
                ],
                cross_stroke,
            );
        }
        ActiveVisualTool::Drag
        | ActiveVisualTool::RotateZ
        | ActiveVisualTool::RotateXY
        | ActiveVisualTool::Scale => {
            let p = VideoPanelState::video_to_screen(
                video_rect,
                video_w,
                video_h,
                state.drag_pos[0],
                state.drag_pos[1],
            );
            painter.circle_stroke(p, 6.0, outline_stroke);
            painter.circle_filled(p, 3.0, handle_col);
        }
        ActiveVisualTool::RectClip => {
            let tl = VideoPanelState::video_to_screen(
                video_rect,
                video_w,
                video_h,
                state.rect_clip[0],
                state.rect_clip[1],
            );
            let br = VideoPanelState::video_to_screen(
                video_rect,
                video_w,
                video_h,
                state.rect_clip[2],
                state.rect_clip[3],
            );
            let r = Rect::from_two_pos(tl, br);
            painter.rect_stroke(r, 0.0, outline_stroke, egui::StrokeKind::Inside);
            for corner in [
                r.left_top(),
                r.right_top(),
                r.left_bottom(),
                r.right_bottom(),
            ] {
                painter.circle_filled(corner, 3.5, handle_col);
            }
        }
        ActiveVisualTool::VectorClip => {
            state.poly_scratch.clear();
            let mut prev_pos: Option<Pos2> = None;
            for v in &state.vclip_path.vertices {
                if v.kind == VectorVertexKind::SplineClose {
                    continue;
                }
                let sp = VideoPanelState::video_to_screen(
                    video_rect, video_w, video_h, v.pos.x, v.pos.y,
                );
                if v.kind != VectorVertexKind::Move
                    && let Some(prev) = prev_pos
                {
                    painter.line_segment([prev, sp], outline_stroke);
                }
                painter.rect_filled(
                    Rect::from_center_size(sp, Vec2::splat(5.0)),
                    0.0,
                    handle_col,
                );
                prev_pos = Some(sp);
            }
        }
    }
}

fn format_video_time_ms(ms: i64) -> String {
    let clamped = ms.max(0);
    let millis = clamped % 1_000;
    let total_s = clamped / 1_000;
    let s = total_s % 60;
    let total_m = total_s / 60;
    let m = total_m % 60;
    let h = total_m / 60;
    format!("{h}:{m:02}:{s:02}.{millis:03}")
}

/// Renders the complete Aegisub 3.2.2 video panel (7 vector visual tool icons on the left,
/// true-colour video canvas, vector-clip sub-toolbar, full-width seek bar, playback buttons,
/// frame/time readout box, offset box, and zoom combo).
#[allow(clippy::too_many_arguments, clippy::too_many_lines)]
pub fn show_video_panel(
    ui: &mut Ui,
    state: &mut VideoPanelState,
    supplier: &dyn VideoFrameSupplier,
    pb: &mut PlaybackController,
    clock: &dyn Clock,
    palette: &Palette,
    catalog: &Catalog,
) -> Option<VideoPanelAction> {
    let mut action: Option<VideoPanelAction> = None;
    let ppp = ui.ctx().pixels_per_point();
    let frame = pb.current_frame();
    let total = supplier.total_frames().max(1);

    let extra_vclip_h = if state.active_tool == ActiveVisualTool::VectorClip {
        22.0
    } else {
        0.0
    };
    let bottom_bars_h = 46.0 + extra_vclip_h;
    let canvas_h = (ui.available_height() - bottom_bars_h).max(72.0);
    let canvas_w = (ui.available_width() - 28.0).max(120.0);

    ui.horizontal(|ui| {
        // Left vertical visual tool toolbar (7 crisp 16×16 vector-painted Aegisub icons).
        ui.vertical(|ui| {
            ui.spacing_mut().item_spacing.y = 2.0;
            for (tool, tip) in [
                (ActiveVisualTool::Cross, "Standard mode"),
                (ActiveVisualTool::Drag, "Drag subtitles (\\pos)"),
                (ActiveVisualTool::RotateZ, "Rotate on Z axis (\\frz)"),
                (
                    ActiveVisualTool::RotateXY,
                    "Rotate on X/Y axes (\\frx/\\fry)",
                ),
                (ActiveVisualTool::Scale, "Scale subtitles (\\fscx/\\fscy)"),
                (ActiveVisualTool::RectClip, "Rectangular clip (\\clip)"),
                (ActiveVisualTool::VectorClip, "Vector clip (\\clip)"),
            ] {
                if visual_tool_icon_button(ui, tool, state.active_tool == tool, palette, tip) {
                    state.active_tool = tool;
                }
            }
        });

        // Main true-colour video canvas.
        let (rect, resp) =
            ui.allocate_exact_size(Vec2::new(canvas_w, canvas_h), Sense::click_and_drag());
        state.display.viewport_w = f64::from(rect.width().max(1.0));
        state.display.viewport_h = f64::from(rect.height().max(1.0));
        let layout = state.display.layout();
        let video_rect = Rect::from_min_size(
            Pos2::new(
                rect.left() + layout.video_rect.x as f32,
                rect.top() + layout.video_rect.y as f32,
            ),
            Vec2::new(layout.video_rect.w as f32, layout.video_rect.h as f32),
        );

        // Upload RGBA frame texture ONLY when `(frame, revision)` changes.
        let rev = supplier.frame_revision(frame);
        if state.uploaded_key != Some((frame, rev))
            && let Some(raw) = supplier.frame_rgba(frame)
        {
            let img = ColorImage::from_rgba_unmultiplied(
                [raw.width as usize, raw.height as usize],
                raw.rgba,
            );
            if let Some(ref mut tex) = state.texture {
                tex.set(img, TextureOptions::LINEAR);
            } else {
                state.texture = Some(ui.ctx().load_texture(
                    "hlouis_video_frame",
                    img,
                    TextureOptions::LINEAR,
                ));
            }
            state.uploaded_key = Some((frame, rev));
            state.texture_uploads += 1;
        }

        let painter = ui.painter_at(rect);
        painter.rect_filled(rect, 0.0, Color32::BLACK);
        if let Some(ref tex) = state.texture {
            // True-colour video frame (Color32::WHITE tint = never warmed by Eye Comfort).
            painter.image(
                tex.id(),
                video_rect,
                Rect::from_min_max(Pos2::ZERO, Pos2::new(1.0, 1.0)),
                Color32::WHITE,
            );
        }

        let vw = state.display.video_w as f32;
        let vh = state.display.video_h as f32;

        // Visual tool mouse interaction (Drag / VectorClip).
        if let Some(pos) = resp.interact_pointer_pos() {
            let vpos = VideoPanelState::screen_to_video(video_rect, vw, vh, pos);
            if state.active_tool == ActiveVisualTool::VectorClip {
                if resp.drag_started() {
                    match state.vclip_mode {
                        VectorClipMode::Drag => {
                            state.dragging_vertex = state.vclip_path.hit_test_vertex(vpos, 18.0);
                        }
                        VectorClipMode::Line => {
                            state.vclip_path.append_line(vpos);
                            action = Some(VideoPanelAction::VisualOverlayEdited);
                        }
                        VectorClipMode::Bicubic => {
                            state.vclip_path.append_bicubic(vpos);
                            action = Some(VideoPanelAction::VisualOverlayEdited);
                        }
                        VectorClipMode::Convert => {
                            if let Some(idx) = state.vclip_path.hit_test_vertex(vpos, 18.0) {
                                let _ = state.vclip_path.convert_segment_at(idx);
                                action = Some(VideoPanelAction::VisualOverlayEdited);
                            }
                        }
                        VectorClipMode::Insert => {
                            let _ = state.vclip_path.insert_on_nearest_segment(vpos);
                            action = Some(VideoPanelAction::VisualOverlayEdited);
                        }
                        VectorClipMode::Remove => {
                            if let Some(idx) = state.vclip_path.hit_test_vertex(vpos, 18.0) {
                                let _ = state.vclip_path.remove_vertex(idx);
                                action = Some(VideoPanelAction::VisualOverlayEdited);
                            }
                        }
                        VectorClipMode::Freehand | VectorClipMode::FreehandSmooth => {}
                    }
                }
                if resp.dragged()
                    && let Some(idx) = state.dragging_vertex
                {
                    state.vclip_path.drag_vertex(idx, vpos);
                    action = Some(VideoPanelAction::VisualOverlayEdited);
                }
                if resp.drag_stopped() {
                    state.dragging_vertex = None;
                }
            } else if state.active_tool == ActiveVisualTool::Drag && resp.dragged() {
                state.drag_pos = [vpos.x, vpos.y];
                action = Some(VideoPanelAction::VisualOverlayEdited);
            }
        }

        paint_visual_overlay(&painter, video_rect, vw, vh, state, palette, ppp);
    });

    // Vector-clip sub-toolbar when VectorClip tool is active.
    if state.active_tool == ActiveVisualTool::VectorClip {
        ui.horizontal(|ui| {
            for (mode, lbl) in [
                (VectorClipMode::Drag, "Drag"),
                (VectorClipMode::Line, "Line"),
                (VectorClipMode::Bicubic, "Curve"),
                (VectorClipMode::Convert, "Conv"),
                (VectorClipMode::Insert, "Ins"),
                (VectorClipMode::Remove, "Del"),
                (VectorClipMode::Freehand, "Free"),
                (VectorClipMode::FreehandSmooth, "Smooth"),
            ] {
                if ui.selectable_label(state.vclip_mode == mode, lbl).clicked() {
                    state.vclip_mode = mode;
                }
            }
        });
    }

    // Row 1 below video: full-width seek bar.
    ui.horizontal(|ui| {
        let mut seek_f = frame;
        let slider_w = (ui.available_width() - 8.0).max(80.0);
        ui.spacing_mut().slider_width = slider_w;
        if ui
            .add(egui::Slider::new(&mut seek_f, 0..=total.saturating_sub(1)).show_value(false))
            .changed()
        {
            pb.scrub_to_frame(seek_f);
            action = Some(VideoPanelAction::SeekFrame(seek_f));
        }
    });

    // Row 2 below seek bar: playback buttons | frame/time box | offset box (+0ms; -0ms) | Zoom combo.
    ui.horizontal(|ui| {
        if ui
            .small_button(if pb.is_playing() { "⏸" } else { "▶" })
            .on_hover_text("Play / Pause video")
            .clicked()
        {
            pb.toggle_play(clock);
            action = Some(VideoPanelAction::PlayToggle);
        }
        if ui
            .small_button("▶|")
            .on_hover_text("Play current line")
            .clicked()
        {
            action = Some(VideoPanelAction::PlayLine);
        }
        if ui
            .small_button("◀")
            .on_hover_text("Previous frame")
            .clicked()
        {
            pb.step_frame(-1);
            action = Some(VideoPanelAction::StepFrame(-1));
        }
        if ui.small_button("▶").on_hover_text("Next frame").clicked() {
            pb.step_frame(1);
            action = Some(VideoPanelAction::StepFrame(1));
        }
        if ui
            .small_button("|◀")
            .on_hover_text("Previous keyframe")
            .clicked()
        {
            let _ = pb.seek_keyframe(false);
            action = Some(VideoPanelAction::SeekKeyframe(false));
        }
        if ui
            .small_button("▶|")
            .on_hover_text("Next keyframe")
            .clicked()
        {
            let _ = pb.seek_keyframe(true);
            action = Some(VideoPanelAction::SeekKeyframe(true));
        }

        let cur_ms = pb.current_time_ms();
        let kf_tag = if supplier.is_keyframe(frame) {
            catalog.get("panel-video-keyframe").unwrap_or("KF")
        } else {
            ""
        };
        let mut frame_time_txt = format!("{} - {frame} {kf_tag}", format_video_time_ms(cur_ms));
        ui.add_enabled(
            false,
            egui::TextEdit::singleline(&mut frame_time_txt)
                .font(egui::FontId::monospace(11.0))
                .desired_width(126.0),
        );

        let from_start = cur_ms - state.active_line_start_ms;
        let to_end = cur_ms - state.active_line_end_ms;
        let mut offset_txt = format!("{from_start:+}ms; {to_end:+}ms");
        ui.add_enabled(
            false,
            egui::TextEdit::singleline(&mut offset_txt)
                .font(egui::FontId::monospace(11.0))
                .desired_width(104.0),
        )
        .on_hover_text("Offset from active line start / end");

        egui::ComboBox::from_id_salt("video_zoom_combo")
            .width(54.0)
            .selected_text(RichText::new(format!("{}%", state.zoom_percent)).small())
            .show_ui(ui, |ui| {
                for pct in [25u16, 50, 75, 100, 150, 200] {
                    ui.selectable_value(&mut state.zoom_percent, pct, format!("{pct}%"));
                }
            });
    });

    action
}

#[cfg(test)]
mod tests {
    use super::*;
    use egui_kittest::Harness;
    use i18n_catalog::Locale;
    use keyframes::Timecodes;
    use playback::PlaybackConfig;
    use theme::ThemeSpec;

    struct DummySupplier {
        rgba: Vec<u8>,
    }
    impl DummySupplier {
        fn new() -> Self {
            Self {
                rgba: vec![40; 64 * 36 * 4],
            }
        }
    }
    impl VideoFrameSupplier for DummySupplier {
        fn total_frames(&self) -> u32 {
            240
        }
        fn is_keyframe(&self, frame: u32) -> bool {
            frame.is_multiple_of(24)
        }
        fn frame_revision(&self, _frame: u32) -> u64 {
            1
        }
        fn frame_rgba(&self, _frame: u32) -> Option<RgbaFrameRef<'_>> {
            Some(RgbaFrameRef {
                width: 64,
                height: 36,
                rgba: &self.rgba,
            })
        }
    }

    struct ZeroClock;
    impl Clock for ZeroClock {
        fn now_ns(&self) -> u64 {
            0
        }
    }

    #[test]
    fn video_panel_renders_with_kittest_and_caches_frame_texture() {
        let mut state = VideoPanelState::new(1280, 720);
        state.active_tool = ActiveVisualTool::VectorClip;
        state.vclip_path = VectorClipPath::parse("m 100 100 l 400 100 l 400 300 l 100 300", false);
        let sup = DummySupplier::new();
        let tc = Timecodes::from_cfr(24_000, 1_001).expect("cfr");
        let mut pb = PlaybackController::new(tc, 240, vec![0, 24, 48], PlaybackConfig::default());
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
                let _ = show_video_panel(ui, &mut state, &sup, &mut pb, &ZeroClock, &pal, &cat);
                assert_eq!(state.texture_uploads(), 1);
            });
        harness.run();
        harness.step();
    }
}
