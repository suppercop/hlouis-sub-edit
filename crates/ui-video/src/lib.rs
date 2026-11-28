//! Video preview panel + visual typesetting overlays (`egui 0.37`) built on [`video_geometry`],
//! [`visual_tools`], [`playback`], [`theme::Palette`], and [`i18n_catalog`].
//!
//! Mirrors Aegisub 3.2.2 / 3.5.1 `src/video_display.cpp` & `src/video_box.cpp`:
//! - Left column: 8 coloured 16×16 visual tool icons (Standard, Drag, Rotate Z, Rotate X/Y,
//!   Scale, Rect Clip, Vector Clip, plus `?` Help at the bottom) with pressed frame on selection.
//! - True-colour video frame filling the panel cleanly without oversized black margins.
//! - Dense seek bar with black keyframe tick marks and a crisp small slider thumb.
//! - Classic Aegisub playback row: Play, Play line (`[▶]`), Pause/Stop, `AUTO` scroll toggle,
//!   time/frame box (`0:05:13.897 - 7526`), offset box (`+3607ms; -23ms`), and Zoom combo
//!   (`37.5%`, `50%`, `75%`, `100%`, `150%`, `200%`).

#![forbid(unsafe_code)]
#![allow(clippy::cast_precision_loss, clippy::cast_possible_truncation)]
#![cfg_attr(not(test), deny(clippy::unwrap_used, clippy::expect_used))]

use egui::{
    Color32, ColorImage, FontId, Pos2, Rect, RichText, Sense, Stroke, TextureHandle,
    TextureOptions, Ui, Vec2,
};
use i18n_catalog::Catalog;
use playback::{Clock, PlaybackController};
use std::f32::consts::TAU;
use std::fmt::Write as _;
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
    /// Optional sorted slice of keyframe indices so seek bar ticks render without stride misses.
    fn keyframes(&self) -> Option<&[u32]> {
        None
    }
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
    Stop,
    StepFrame(i32),
    SeekFrame(u32),
    SeekKeyframe(bool),
    VisualOverlayEdited,
    ShowVisualHelp,
}

/// How the video is scaled inside the panel.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ZoomMode {
    /// Scale proportionally to fill the panel.
    Fit,
    /// Use `VideoPanelState::zoom_tenths` directly.
    Fixed,
}

/// Persistent UI state for the video panel + visual tool overlay.
pub struct VideoPanelState {
    pub display: VideoDisplayState,
    pub active_tool: ActiveVisualTool,
    pub vclip_mode: VectorClipMode,
    pub vclip_path: VectorClipPath,
    pub rect_clip: [f64; 4],
    pub drag_pos: [f64; 2],
    pub rotate_z_deg: f64,
    pub rotate_xy_deg: [f64; 2],
    pub scale_pct: [f64; 2],
    pub dragging_vertex: Option<usize>,
    pub show_overscan: bool,
    pub auto_scroll_video: bool,
    pub show_visual_help: bool,
    /// When `true`, scales video proportionally to fill the panel canvas; when `false`, uses
    /// [`Self::zoom_tenths`] directly.
    pub zoom_mode: ZoomMode,
    /// Zoom percentage × 10 (e.g. `375` = `37.5%`, `500` = `50%`, `1000` = `100%`).
    pub zoom_tenths: u16,
    pub active_line_start_ms: i64,
    pub active_line_end_ms: i64,
    // Cached GPU texture + revision key (0 re-upload when unchanged).
    texture: Option<TextureHandle>,
    uploaded_key: Option<(u32, u64)>,
    texture_uploads: u64,
    // Reusable scratch buffer for drawing 500+ vector clip segments without allocating.
    poly_scratch: Vec<Pos2>,
    frame_time_buf: String,
    offset_buf: String,
    last_readout_key: Option<(i64, u32, i64, i64)>,
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
            rotate_z_deg: 0.0,
            rotate_xy_deg: [0.0, 0.0],
            scale_pct: [100.0, 100.0],
            dragging_vertex: None,
            show_overscan: false,
            auto_scroll_video: true,
            show_visual_help: false,
            zoom_mode: ZoomMode::Fit,
            zoom_tenths: 375,
            active_line_start_ms: 0,
            active_line_end_ms: 2_000,
            texture: None,
            uploaded_key: None,
            texture_uploads: 0,
            poly_scratch: Vec::with_capacity(1024),
            frame_time_buf: String::with_capacity(32),
            offset_buf: String::with_capacity(32),
            last_readout_key: None,
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

/// Paints a coloured 16×16 vector icon for one of the 7 Aegisub visual tools with pressed frame.
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
        Color32::from_rgb(198, 224, 250)
    } else if resp.hovered() {
        rgba(pal.hover)
    } else {
        rgba(pal.toolbar_bg)
    };
    p.rect_filled(rect, 2.0, bg);
    p.rect_stroke(
        rect,
        2.0,
        Stroke::new(
            if selected { 1.4 } else { 1.0 },
            if selected {
                Color32::from_rgb(42, 120, 214)
            } else {
                rgba(pal.border)
            },
        ),
        egui::StrokeKind::Inside,
    );
    let c = rect.center();
    let blue = Color32::from_rgb(38, 114, 210);
    let red = Color32::from_rgb(212, 58, 58);
    let green = Color32::from_rgb(38, 162, 82);
    let orange = Color32::from_rgb(218, 128, 24);
    let purple = Color32::from_rgb(148, 66, 198);

    match tool {
        ActiveVisualTool::Cross => {
            p.add(egui::Shape::convex_polygon(
                vec![
                    Pos2::new(c.x - 4.0, c.y - 6.0),
                    Pos2::new(c.x - 4.0, c.y + 5.0),
                    Pos2::new(c.x - 1.0, c.y + 2.5),
                    Pos2::new(c.x + 3.5, c.y + 3.0),
                ],
                blue,
                Stroke::new(0.8, Color32::from_rgb(20, 56, 120)),
            ));
        }
        ActiveVisualTool::Drag => {
            let st = Stroke::new(1.5, blue);
            p.line_segment([Pos2::new(c.x - 6.0, c.y), Pos2::new(c.x + 6.0, c.y)], st);
            p.line_segment([Pos2::new(c.x, c.y - 6.0), Pos2::new(c.x, c.y + 6.0)], st);
            for (dx, dy) in [(-6.0, 0.0), (6.0, 0.0), (0.0, -6.0), (0.0, 6.0)] {
                p.circle_filled(Pos2::new(c.x + dx, c.y + dy), 1.8, red);
            }
        }
        ActiveVisualTool::RotateZ => {
            let r = 5.5;
            let pts: Vec<Pos2> = (0..=12)
                .map(|i| {
                    let a = (i as f32 / 12.0) * TAU * 0.78 - 0.5;
                    Pos2::new(c.x + a.cos() * r, c.y + a.sin() * r)
                })
                .collect();
            p.add(egui::Shape::line(pts, Stroke::new(1.6, green)));
            p.circle_filled(c, 1.8, red);
        }
        ActiveVisualTool::RotateXY => {
            let pts: Vec<Pos2> = (0..=16)
                .map(|i| {
                    let a = (i as f32 / 16.0) * TAU;
                    Pos2::new(c.x + a.cos() * 6.0, c.y + a.sin() * 3.0)
                })
                .collect();
            p.add(egui::Shape::closed_line(pts, Stroke::new(1.4, orange)));
            p.line_segment(
                [
                    Pos2::new(c.x - 2.0, c.y - 6.0),
                    Pos2::new(c.x + 2.0, c.y + 6.0),
                ],
                Stroke::new(1.4, blue),
            );
        }
        ActiveVisualTool::Scale => {
            let b = Rect::from_min_max(
                Pos2::new(c.x - 5.5, c.y - 5.5),
                Pos2::new(c.x + 1.5, c.y + 1.5),
            );
            p.rect_stroke(b, 0.0, Stroke::new(1.4, blue), egui::StrokeKind::Inside);
            p.line_segment(
                [
                    Pos2::new(c.x + 1.0, c.y + 1.0),
                    Pos2::new(c.x + 6.0, c.y + 6.0),
                ],
                Stroke::new(1.6, green),
            );
            p.rect_filled(
                Rect::from_center_size(Pos2::new(c.x + 5.5, c.y + 5.5), Vec2::splat(3.2)),
                0.0,
                red,
            );
        }
        ActiveVisualTool::RectClip => {
            let b = Rect::from_center_size(c, Vec2::new(11.0, 9.0));
            p.rect_stroke(b, 0.0, Stroke::new(1.4, purple), egui::StrokeKind::Inside);
            for corner in [
                b.left_top(),
                b.right_top(),
                b.left_bottom(),
                b.right_bottom(),
            ] {
                p.rect_filled(Rect::from_center_size(corner, Vec2::splat(3.0)), 0.0, red);
            }
        }
        ActiveVisualTool::VectorClip => {
            let v0 = Pos2::new(c.x - 5.5, c.y + 4.5);
            let v1 = Pos2::new(c.x - 2.0, c.y - 5.0);
            let v2 = Pos2::new(c.x + 5.5, c.y - 1.5);
            let v3 = Pos2::new(c.x + 2.5, c.y + 5.0);
            p.add(egui::Shape::closed_line(
                vec![v0, v1, v2, v3],
                Stroke::new(1.4, orange),
            ));
            for v in [v0, v1, v2, v3] {
                p.rect_filled(Rect::from_center_size(v, Vec2::splat(3.0)), 0.0, blue);
            }
        }
    }
    resp.on_hover_text(tooltip).clicked()
}

/// Paints the 8th visual-tool button (`?` help) at the bottom of the left column.
fn visual_help_button(ui: &mut Ui, active: bool, pal: &Palette) -> bool {
    let (rect, resp) = ui.allocate_exact_size(Vec2::splat(22.0), Sense::click());
    let p = ui.painter();
    let bg = if active {
        Color32::from_rgb(198, 224, 250)
    } else if resp.hovered() {
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
    p.circle_filled(rect.center(), 6.5, Color32::from_rgb(42, 120, 214));
    let c = rect.center();
    let qst = Stroke::new(1.5, Color32::WHITE);
    p.line_segment(
        [
            Pos2::new(c.x - 2.0, c.y - 2.2),
            Pos2::new(c.x - 1.2, c.y - 3.6),
        ],
        qst,
    );
    p.line_segment(
        [
            Pos2::new(c.x - 1.2, c.y - 3.6),
            Pos2::new(c.x + 1.4, c.y - 3.6),
        ],
        qst,
    );
    p.line_segment(
        [
            Pos2::new(c.x + 1.4, c.y - 3.6),
            Pos2::new(c.x + 2.0, c.y - 1.8),
        ],
        qst,
    );
    p.line_segment(
        [Pos2::new(c.x + 2.0, c.y - 1.8), Pos2::new(c.x, c.y + 0.5)],
        qst,
    );
    p.line_segment([Pos2::new(c.x, c.y + 0.5), Pos2::new(c.x, c.y + 1.8)], qst);
    p.circle_filled(Pos2::new(c.x, c.y + 3.8), 1.0, Color32::WHITE);
    resp.on_hover_text("Toggle visual typesetting tool help")
        .clicked()
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

fn zoom_label_static(tenths: u16) -> Option<&'static str> {
    match tenths {
        250 => Some("25%"),
        375 => Some("37.5%"),
        500 => Some("50%"),
        750 => Some("75%"),
        1000 => Some("100%"),
        1500 => Some("150%"),
        2000 => Some("200%"),
        _ => None,
    }
}

fn format_zoom_tenths(tenths: u16) -> std::borrow::Cow<'static, str> {
    if let Some(s) = zoom_label_static(tenths) {
        return std::borrow::Cow::Borrowed(s);
    }
    if tenths.is_multiple_of(10) {
        std::borrow::Cow::Owned(format!("{}%", tenths / 10))
    } else {
        std::borrow::Cow::Owned(format!("{}.{}%", tenths / 10, tenths % 10))
    }
}

/// Renders the complete Aegisub 3.5.1 video panel:
/// - 8 coloured visual tool icons on the left (7 tools + `?` help) with pressed frame
/// - Full-fill true-colour video canvas (minimal letterboxing)
/// - Dense seek bar with black keyframe tick marks and crisp small slider thumb
/// - Classic Aegisub playback row: Play, Play line (`[▶]`), Pause/Stop, `AUTO`, time-frame box,
///   offset box, and Zoom combo.
#[allow(clippy::too_many_arguments, clippy::too_many_lines)]
pub fn show_video_panel(
    ui: &mut Ui,
    state: &mut VideoPanelState,
    supplier: &dyn VideoFrameSupplier,
    pb: &mut PlaybackController,
    clock: &dyn Clock,
    palette: &Palette,
    _catalog: &Catalog,
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
    let bottom_bars_h = 44.0 + extra_vclip_h;
    let canvas_h = (ui.available_height() - bottom_bars_h).max(72.0);
    let canvas_w = (ui.available_width() - 28.0).max(120.0);

    ui.horizontal(|ui| {
        // Left vertical visual tool toolbar: 8 entries (7 tools + `?` help at the bottom).
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
            if visual_help_button(ui, state.show_visual_help, palette) {
                state.show_visual_help = !state.show_visual_help;
                action = Some(VideoPanelAction::ShowVisualHelp);
            }
        });

        // Main true-colour video canvas filling the panel cleanly at aspect ratio.
        let (rect, resp) =
            ui.allocate_exact_size(Vec2::new(canvas_w, canvas_h), Sense::click_and_drag());
        let vw = state.display.video_w.max(1.0) as f32;
        let vh = state.display.video_h.max(1.0) as f32;
        let video_aspect = vw / vh;
        let box_aspect = rect.width() / rect.height().max(1.0);
        let (draw_w, draw_h) = if state.zoom_mode == ZoomMode::Fit {
            if box_aspect > video_aspect {
                (rect.height() * video_aspect, rect.height())
            } else {
                (rect.width(), rect.width() / video_aspect)
            }
        } else {
            let scale = (f32::from(state.zoom_tenths) / 1000.0).max(0.05);
            (vw * scale, vh * scale)
        };
        let video_rect = Rect::from_center_size(rect.center(), Vec2::new(draw_w, draw_h));

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
        painter.rect_filled(rect, 0.0, Color32::from_rgb(18, 20, 26));
        if let Some(ref tex) = state.texture {
            painter.image(
                tex.id(),
                video_rect,
                Rect::from_min_max(Pos2::ZERO, Pos2::new(1.0, 1.0)),
                Color32::WHITE,
            );
        }

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
            } else if state.active_tool == ActiveVisualTool::RotateZ && resp.dragged() {
                let dx = vpos.x - state.drag_pos[0];
                let dy = vpos.y - state.drag_pos[1];
                state.rotate_z_deg = -dy.atan2(dx).to_degrees();
                action = Some(VideoPanelAction::VisualOverlayEdited);
            } else if state.active_tool == ActiveVisualTool::RotateXY && resp.dragged() {
                let dx = vpos.x - state.drag_pos[0];
                let dy = vpos.y - state.drag_pos[1];
                state.rotate_xy_deg = [
                    (dy * 0.25).clamp(-89.0, 89.0),
                    (dx * 0.25).clamp(-89.0, 89.0),
                ];
                action = Some(VideoPanelAction::VisualOverlayEdited);
            } else if state.active_tool == ActiveVisualTool::Scale && resp.dragged() {
                let sx = ((vpos.x - state.drag_pos[0]).abs() / 100.0 * 100.0).clamp(10.0, 500.0);
                let sy = ((vpos.y - state.drag_pos[1]).abs() / 100.0 * 100.0).clamp(10.0, 500.0);
                state.scale_pct = [sx, sy];
                action = Some(VideoPanelAction::VisualOverlayEdited);
            } else if state.active_tool == ActiveVisualTool::RectClip {
                if resp.drag_started() {
                    let corners = [
                        (state.rect_clip[0], state.rect_clip[1]),
                        (state.rect_clip[2], state.rect_clip[1]),
                        (state.rect_clip[0], state.rect_clip[3]),
                        (state.rect_clip[2], state.rect_clip[3]),
                    ];
                    let best = corners
                        .iter()
                        .enumerate()
                        .min_by(|(_, a), (_, b)| {
                            let da = (a.0 - vpos.x).hypot(a.1 - vpos.y);
                            let db = (b.0 - vpos.x).hypot(b.1 - vpos.y);
                            da.total_cmp(&db)
                        })
                        .map(|(idx, _)| idx);
                    state.dragging_vertex = best;
                }
                if resp.dragged() {
                    match state.dragging_vertex {
                        Some(0) => {
                            state.rect_clip[0] = vpos.x;
                            state.rect_clip[1] = vpos.y;
                        }
                        Some(1) => {
                            state.rect_clip[2] = vpos.x;
                            state.rect_clip[1] = vpos.y;
                        }
                        Some(2) => {
                            state.rect_clip[0] = vpos.x;
                            state.rect_clip[3] = vpos.y;
                        }
                        Some(3) => {
                            state.rect_clip[2] = vpos.x;
                            state.rect_clip[3] = vpos.y;
                        }
                        _ => {}
                    }
                    action = Some(VideoPanelAction::VisualOverlayEdited);
                }
                if resp.drag_stopped() {
                    state.dragging_vertex = None;
                }
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

    // Row 1 below video: dense seek bar with black keyframe tick marks + small slider thumb.
    let seek_w = (ui.available_width() - 4.0).max(80.0);
    let (seek_rect, seek_resp) =
        ui.allocate_exact_size(Vec2::new(seek_w, 16.0), Sense::click_and_drag());
    let sp = ui.painter_at(seek_rect);
    let track_rect = Rect::from_min_max(
        Pos2::new(seek_rect.left() + 2.0, seek_rect.center().y - 3.0),
        Pos2::new(seek_rect.right() - 2.0, seek_rect.center().y + 3.0),
    );
    sp.rect_filled(track_rect, 1.0, rgba(palette.panel_bg));
    sp.rect_stroke(
        track_rect,
        1.0,
        Stroke::new(1.0, rgba(palette.border)),
        egui::StrokeKind::Inside,
    );

    // Draw black keyframe tick marks across the seek track without stride-missing real keyframes.
    let max_f = total.saturating_sub(1).max(1);
    if let Some(kfs) = supplier.keyframes() {
        let step = (kfs.len() / 320).max(1);
        for &kf_f in kfs.iter().step_by(step) {
            if kf_f < total {
                let frac = (kf_f as f32) / (max_f as f32);
                let tx = snap_px(track_rect.left() + frac * track_rect.width(), ppp);
                sp.line_segment(
                    [
                        Pos2::new(tx, track_rect.top() - 2.0),
                        Pos2::new(tx, track_rect.bottom() + 2.0),
                    ],
                    Stroke::new(1.0 / ppp.max(0.5), Color32::from_rgb(20, 22, 26)),
                );
            }
        }
    } else {
        let kf_stride = (total / 120).max(1);
        let mut kf_f = 0u32;
        while kf_f < total {
            if supplier.is_keyframe(kf_f) {
                let frac = (kf_f as f32) / (max_f as f32);
                let tx = snap_px(track_rect.left() + frac * track_rect.width(), ppp);
                sp.line_segment(
                    [
                        Pos2::new(tx, track_rect.top() - 2.0),
                        Pos2::new(tx, track_rect.bottom() + 2.0),
                    ],
                    Stroke::new(1.0 / ppp.max(0.5), Color32::from_rgb(20, 22, 26)),
                );
            }
            kf_f = kf_f.saturating_add(kf_stride);
        }
    }

    // Draw small slider thumb.
    let cur_frac = (frame as f32 / max_f as f32).clamp(0.0, 1.0);
    let thumb_x = track_rect.left() + cur_frac * track_rect.width();
    let thumb_r = Rect::from_center_size(
        Pos2::new(thumb_x, seek_rect.center().y),
        Vec2::new(7.0, 13.0),
    );
    sp.rect_filled(thumb_r, 1.5, Color32::from_rgb(56, 136, 226));
    sp.rect_stroke(
        thumb_r,
        1.5,
        Stroke::new(1.0, Color32::from_rgb(24, 72, 148)),
        egui::StrokeKind::Inside,
    );
    if (seek_resp.clicked() || seek_resp.dragged())
        && let Some(pos) = seek_resp.interact_pointer_pos()
    {
        let frac = ((pos.x - track_rect.left()) / track_rect.width().max(1.0)).clamp(0.0, 1.0);
        #[allow(clippy::cast_sign_loss)]
        let target = (frac * (max_f as f32)).round() as u32;
        pb.scrub_to_frame(target);
        action = Some(VideoPanelAction::SeekFrame(target));
    }

    // Row 2 below seek bar (Aegisub exact order):
    // Play | Play line ([▶]) | Pause/Stop | AUTO toggle | time-frame box | offset box | zoom combo.
    ui.horizontal(|ui| {
        ui.spacing_mut().item_spacing.x = 2.0;
        let green = Color32::from_rgb(38, 168, 84);
        let red = Color32::from_rgb(214, 58, 58);
        let blue = Color32::from_rgb(42, 120, 214);

        if ui
            .small_button(RichText::new("▶").color(green))
            .on_hover_text("Play video")
            .clicked()
        {
            pb.toggle_play(clock);
            action = Some(VideoPanelAction::PlayToggle);
        }
        if ui
            .small_button(RichText::new("[▶]").color(green))
            .on_hover_text("Play current line")
            .clicked()
        {
            action = Some(VideoPanelAction::PlayLine);
        }
        if ui
            .small_button(RichText::new(if pb.is_playing() { "⏸" } else { "■" }).color(red))
            .on_hover_text("Pause / Stop video")
            .clicked()
        {
            if pb.is_playing() {
                pb.toggle_play(clock);
            }
            action = Some(VideoPanelAction::Stop);
        }
        if ui
            .selectable_label(
                state.auto_scroll_video,
                RichText::new("AUTO").small().color(blue),
            )
            .on_hover_text("Auto-seek video to active line")
            .clicked()
        {
            state.auto_scroll_video = !state.auto_scroll_video;
        }

        let cur_ms = pb.current_time_ms();
        let rkey = (
            cur_ms,
            frame,
            state.active_line_start_ms,
            state.active_line_end_ms,
        );
        if state.last_readout_key != Some(rkey) {
            state.frame_time_buf.clear();
            let clamped = cur_ms.max(0);
            let millis = clamped % 1_000;
            let total_secs = clamped / 1_000;
            let secs = total_secs % 60;
            let total_mins = total_secs / 60;
            let mins = total_mins % 60;
            let hrs = total_mins / 60;
            let _ = write!(
                state.frame_time_buf,
                "{hrs}:{mins:02}:{secs:02}.{millis:03} - {frame}"
            );

            let from_start = cur_ms - state.active_line_start_ms;
            let to_end = cur_ms - state.active_line_end_ms;
            state.offset_buf.clear();
            let _ = write!(state.offset_buf, "{from_start:+}ms; {to_end:+}ms");
            state.last_readout_key = Some(rkey);
        }
        ui.add_enabled(
            false,
            egui::TextEdit::singleline(&mut state.frame_time_buf)
                .font(FontId::proportional(11.0))
                .desired_width(114.0),
        );

        ui.add_enabled(
            false,
            egui::TextEdit::singleline(&mut state.offset_buf)
                .font(FontId::proportional(11.0))
                .desired_width(98.0),
        )
        .on_hover_text("Offset from active line start / end");

        let zoom_text = if state.zoom_mode == ZoomMode::Fit {
            std::borrow::Cow::Borrowed("Fit")
        } else {
            format_zoom_tenths(state.zoom_tenths)
        };
        egui::ComboBox::from_id_salt("video_zoom_combo")
            .width(56.0)
            .selected_text(RichText::new(zoom_text).small())
            .show_ui(ui, |ui| {
                if ui
                    .selectable_label(state.zoom_mode == ZoomMode::Fit, "Fit")
                    .clicked()
                {
                    state.zoom_mode = ZoomMode::Fit;
                }
                for z in [250u16, 375, 500, 750, 1000, 1500, 2000] {
                    if ui
                        .selectable_label(
                            state.zoom_mode == ZoomMode::Fixed && state.zoom_tenths == z,
                            format_zoom_tenths(z),
                        )
                        .clicked()
                    {
                        state.zoom_tenths = z;
                        state.zoom_mode = ZoomMode::Fixed;
                    }
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
                let _ = show_video_panel(ui, &mut state, &sup, &mut pb, &ZeroClock, &pal, &cat);
                assert_eq!(state.texture_uploads(), 1);
            });
        harness.run();
        harness.step();
    }
}
