//! Pure geometry and tag-edit engine for Aegisub 3.5.1 visual typesetting tools
//! (`src/visual_tool_drag.cpp`, `src/visual_tool_rotatez.cpp`, `src/visual_tool_rotatexy.cpp`,
//! `src/visual_tool_scale.cpp`, `src/visual_tool_clip.cpp`, `src/visual_tool_vector_clip.cpp`,
//! `src/visual_tool_cross.cpp`).
//!
//! Produces feature handles, hit-test results, and merged atomic [`ass_core::EditCommand`]s
//! (one command per drag, supporting multi-line selection drags).

#![forbid(unsafe_code)]
#![cfg_attr(not(test), deny(clippy::unwrap_used, clippy::expect_used))]

pub mod vector_clip;

use ass_core::{AssEvent, EditCommand};
use std::fmt::Write as _;
pub use vector_clip::{VectorClipMode, VectorClipPath, VectorVertex, VectorVertexKind};

/// 2D point / vector in script, video, or screen coordinates.
#[derive(Debug, Clone, Copy, Default, PartialEq)]
pub struct Vec2 {
    pub x: f64,
    pub y: f64,
}

impl Vec2 {
    /// Constructs a 2D point.
    #[must_use]
    pub const fn new(x: f64, y: f64) -> Self {
        Self { x, y }
    }

    /// Euclidean distance squared to `other`.
    #[inline]
    #[must_use]
    pub fn dist_sq(self, other: Self) -> f64 {
        let dx = self.x - other.x;
        let dy = self.y - other.y;
        dx * dx + dy * dy
    }
}

/// Active Aegisub visual tool selection.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum VisualToolKind {
    Cross,
    Drag,
    RotateZ,
    RotateXY,
    Scale,
    RectClip,
    VectorClip(VectorClipMode),
}

/// Keyboard modifiers active during a visual drag (matches Aegisub Shift/Ctrl/Alt semantics).
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub struct Modifiers {
    pub shift: bool,
    pub ctrl: bool,
    pub alt: bool,
}

/// Script <-> Video <-> Screen coordinate transform (used by all visual tools).
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct CoordTransform {
    pub play_res_x: f64,
    pub play_res_y: f64,
    pub video_w: f64,
    pub video_h: f64,
    /// Top-left of the rendered video rect inside the viewport (includes letterbox + pan).
    pub viewport_offset: Vec2,
    /// Display zoom factor (screen pixels per video pixel).
    pub zoom: f64,
}

impl CoordTransform {
    /// Converts a script-space coordinate to screen pixels.
    #[inline]
    #[must_use]
    pub fn script_to_screen(&self, p: Vec2) -> Vec2 {
        let vx = p.x * (self.video_w / self.play_res_x.max(1.0));
        let vy = p.y * (self.video_h / self.play_res_y.max(1.0));
        Vec2::new(
            self.viewport_offset.x + vx * self.zoom,
            self.viewport_offset.y + vy * self.zoom,
        )
    }

    /// Converts a screen pixel coordinate to script coordinates.
    #[inline]
    #[must_use]
    pub fn screen_to_script(&self, p: Vec2) -> Vec2 {
        let inv_z = 1.0 / self.zoom.max(1e-6);
        let vx = (p.x - self.viewport_offset.x) * inv_z;
        let vy = (p.y - self.viewport_offset.y) * inv_z;
        Vec2::new(
            vx * (self.play_res_x / self.video_w.max(1.0)),
            vy * (self.play_res_y / self.video_h.max(1.0)),
        )
    }

    /// Converts a screen pixel coordinate to video pixels (for Cross tool readout).
    #[inline]
    #[must_use]
    pub fn screen_to_video(&self, p: Vec2) -> Vec2 {
        let inv_z = 1.0 / self.zoom.max(1e-6);
        Vec2::new(
            (p.x - self.viewport_offset.x) * inv_z,
            (p.y - self.viewport_offset.y) * inv_z,
        )
    }
}

/// Computes the default ASS `\pos` anchor for alignment `\an1..=9` given `PlayRes` and margins
/// (matches Aegisub `VisualTool::GetLinePosition` in `src/visual_tool.cpp`).
#[must_use]
pub fn default_anchor_pos(
    an: u8,
    play_res_x: f64,
    play_res_y: f64,
    margin_l: f64,
    margin_r: f64,
    margin_v: f64,
) -> Vec2 {
    let x = match an {
        1 | 4 | 7 => margin_l,
        3 | 6 | 9 => play_res_x - margin_r,
        _ => f64::midpoint(margin_l, play_res_x - margin_r),
    };
    let y = match an {
        7..=9 => margin_v,
        4..=6 => play_res_y * 0.5,
        _ => play_res_y - margin_v,
    };
    Vec2::new(x, y)
}

/// Cross tool readout at a pointer position.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct CrossReadout {
    pub video_pos: Vec2,
    pub script_pos: Vec2,
}

/// Computes Cross tool coordinates (`src/visual_tool_cross.cpp`).
#[inline]
#[must_use]
pub fn cross_readout(tf: &CoordTransform, screen_pos: Vec2) -> CrossReadout {
    CrossReadout {
        video_pos: tf.screen_to_video(screen_pos),
        script_pos: tf.screen_to_script(screen_pos),
    }
}

/// Interactive handle kind displayed on the video overlay.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum FeatureKind {
    Pos,
    MoveStart,
    MoveEnd,
    Origin,
    RotateHandle,
    ScaleXHandle,
    ScaleYHandle,
    ClipCorner(u8),
    VectorVertex(usize),
}

/// Interactive visual handle in script coordinates.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct FeaturePoint {
    pub kind: FeatureKind,
    pub pos: Vec2,
}

/// Rectangular clip coordinates (`\clip(x1,y1,x2,y2)` or `\iclip(...)`).
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct RectClip {
    pub x1: f64,
    pub y1: f64,
    pub x2: f64,
    pub y2: f64,
    pub inverted: bool,
}

impl RectClip {
    /// Updates one corner (`0=TL, 1=TR, 2=BR, 3=BL`) during drag.
    pub fn drag_corner(&mut self, corner: u8, pos: Vec2) {
        match corner {
            0 => {
                self.x1 = pos.x;
                self.y1 = pos.y;
            }
            1 => {
                self.x2 = pos.x;
                self.y1 = pos.y;
            }
            2 => {
                self.x2 = pos.x;
                self.y2 = pos.y;
            }
            3 => {
                self.x1 = pos.x;
                self.y2 = pos.y;
            }
            _ => {}
        }
    }

    /// Formats as `\clip(x1,y1,x2,y2)` or `\iclip(...)`.
    #[must_use]
    pub fn to_tag(&self) -> String {
        let prefix = if self.inverted { r"\iclip" } else { r"\clip" };
        let (ax, bx) = if self.x1 <= self.x2 {
            (self.x1, self.x2)
        } else {
            (self.x2, self.x1)
        };
        let (ay, by) = if self.y1 <= self.y2 {
            (self.y1, self.y2)
        } else {
            (self.y2, self.y1)
        };
        #[allow(clippy::cast_possible_truncation)]
        {
            format!(
                "{prefix}({},{},{},{})",
                ax.round() as i32,
                ay.round() as i32,
                bx.round() as i32,
                by.round() as i32
            )
        }
    }
}

/// Computes constrained drag delta (Shift constrains to horizontal or vertical axis, matching
/// Aegisub `VisualToolDrag::UpdateDrag`).
#[inline]
#[must_use]
pub fn constrained_delta(start: Vec2, current: Vec2, mods: Modifiers) -> Vec2 {
    let dx = current.x - start.x;
    let dy = current.y - start.y;
    if mods.shift {
        if dx.abs() >= dy.abs() {
            Vec2::new(dx, 0.0)
        } else {
            Vec2::new(0.0, dy)
        }
    } else {
        Vec2::new(dx, dy)
    }
}

/// Computes `\frz` angle in degrees from `origin` to `pointer` (ASS positive `\frz` is
/// counter-clockwise in screen space; Shift snaps to 30° increments as in Aegisub
/// `VisualToolRotateZ`).
#[inline]
#[must_use]
pub fn compute_frz_deg(origin: Vec2, pointer: Vec2, mods: Modifiers) -> f64 {
    let raw = -(pointer.y - origin.y)
        .atan2(pointer.x - origin.x)
        .to_degrees();
    let deg = if mods.shift {
        (raw / 30.0).round() * 30.0
    } else {
        raw
    };
    (deg * 100.0).round() / 100.0
}

/// Computes `(\frx, \fry)` rotation degrees from drag delta around `origin` (Shift locks dominant
/// axis, matching Aegisub `VisualToolRotateXY`).
#[inline]
#[must_use]
pub fn compute_frxy_deg(
    start_rot_x: f64,
    start_rot_y: f64,
    drag_start: Vec2,
    pointer: Vec2,
    mods: Modifiers,
) -> (f64, f64) {
    let delta = constrained_delta(drag_start, pointer, mods);
    let scale = if mods.ctrl { 0.1 } else { 0.5 };
    let frx = ((start_rot_x - delta.y * scale) * 100.0).round() / 100.0;
    let fry = ((start_rot_y + delta.x * scale) * 100.0).round() / 100.0;
    (frx, fry)
}

/// Computes `(\fscx, \fscy)` percentage scales from drag delta (Shift locks uniform aspect ratio,
/// matching Aegisub `VisualToolScale`).
#[inline]
#[must_use]
pub fn compute_scale_pct(
    initial_x: f64,
    initial_y: f64,
    drag_start: Vec2,
    pointer: Vec2,
    mods: Modifiers,
) -> (f64, f64) {
    let dx = pointer.x - drag_start.x;
    let dy = drag_start.y - pointer.y;
    let sx = (initial_x + dx * 0.5).max(1.0);
    let sy = if mods.shift && initial_x > 1e-6 {
        (initial_y * (sx / initial_x)).max(1.0)
    } else {
        (initial_y + dy * 0.5).max(1.0)
    };
    ((sx * 100.0).round() / 100.0, (sy * 100.0).round() / 100.0)
}

/// Hit-tests `features` in screen space within `radius_px`.
#[inline]
#[must_use]
pub fn hit_test_features(
    features: &[FeaturePoint],
    tf: &CoordTransform,
    screen_pointer: Vec2,
    radius_px: f64,
) -> Option<usize> {
    let mut best_d2 = radius_px * radius_px;
    let mut best_idx = None;
    for (i, feat) in features.iter().enumerate() {
        let sp = tf.script_to_screen(feat.pos);
        let d2 = sp.dist_sq(screen_pointer);
        if d2 <= best_d2 {
            best_d2 = d2;
            best_idx = Some(i);
        }
    }
    best_idx
}

/// Replaces or inserts a leading override tag (e.g. `pos`, `org`, `frz`, `fscx`, `clip`) inside
/// the first `{...}` block of `text`.
#[must_use]
pub fn upsert_leading_tag(text: &str, tag_name: &str, tag_with_value: &str) -> String {
    if let Some(rest) = text.strip_prefix('{') {
        if let Some(close) = rest.find('}') {
            let body = &rest[..close];
            if let Some((s, e)) = find_tag_span(body, tag_name) {
                let mut out = String::with_capacity(text.len() + tag_with_value.len());
                out.push('{');
                out.push_str(&body[..s]);
                out.push_str(tag_with_value);
                out.push_str(&body[e..]);
                out.push_str(&rest[close..]);
                return out;
            }
            let mut out = String::with_capacity(text.len() + tag_with_value.len());
            out.push('{');
            out.push_str(body);
            out.push_str(tag_with_value);
            out.push_str(&rest[close..]);
            return out;
        }
    }
    let mut out = String::with_capacity(text.len() + tag_with_value.len() + 2);
    out.push('{');
    out.push_str(tag_with_value);
    out.push('}');
    out.push_str(text);
    out
}

/// Builds one merged atomic [`EditCommand::Batch`] moving multiple selected lines by `delta`
/// (each line's `\pos(x,y)` updated by `+delta`, or initialized from its anchor + `delta`).
#[must_use]
pub fn build_multi_line_drag_command(
    selected_events: &[(usize, &AssEvent, Vec2)],
    delta: Vec2,
) -> EditCommand {
    let mut cmds = Vec::with_capacity(selected_events.len());
    let mut tag_buf = String::with_capacity(32);
    for &(line_idx, ev, initial_pos) in selected_events {
        let nx = ((initial_pos.x + delta.x) * 100.0).round() / 100.0;
        let ny = ((initial_pos.y + delta.y) * 100.0).round() / 100.0;
        tag_buf.clear();
        let _ = write!(tag_buf, "\\pos({nx},{ny})");
        let after = upsert_leading_tag(ev.text(), "pos", &tag_buf);
        if after != ev.text() {
            cmds.push(EditCommand::ModifyEventText {
                index: line_idx,
                before: ev.text().to_owned(),
                after,
            });
        }
    }
    EditCommand::Batch(cmds)
}

fn find_tag_span(body: &str, name: &str) -> Option<(usize, usize)> {
    let bytes = body.as_bytes();
    let target = name.as_bytes();
    let len = bytes.len();
    let mut i = 0usize;
    while i < len {
        if bytes[i] != b'\\' {
            i += 1;
            continue;
        }
        let start = i;
        let mut j = i + 1;
        let mut parens = 0usize;
        while j < len {
            let c = bytes[j];
            if c == b'(' {
                parens += 1;
            } else if c == b')' && parens > 0 {
                parens -= 1;
            } else if parens == 0 && c == b'\\' {
                break;
            }
            j += 1;
        }
        let raw = &bytes[start + 1..j];
        if raw.starts_with(target) {
            let rest = &raw[target.len()..];
            if rest.is_empty() || !rest[0].is_ascii_alphabetic() {
                return Some((start, j));
            }
        }
        i = j;
    }
    None
}

#[cfg(test)]
mod tests {
    use super::*;
    use ass_core::AssTime;

    #[test]
    fn coord_transform_anchor_rotate_scale_rect_clip_and_multi_drag() {
        let tf = CoordTransform {
            play_res_x: 1920.0,
            play_res_y: 1080.0,
            video_w: 1920.0,
            video_h: 1080.0,
            viewport_offset: Vec2::new(10.0, 20.0),
            zoom: 0.5,
        };
        let s = tf.script_to_screen(Vec2::new(100.0, 200.0));
        assert_eq!(s, Vec2::new(60.0, 120.0));
        assert_eq!(tf.screen_to_script(s), Vec2::new(100.0, 200.0));

        assert_eq!(
            default_anchor_pos(8, 1920.0, 1080.0, 10.0, 10.0, 20.0),
            Vec2::new(960.0, 20.0)
        );
        assert_eq!(
            compute_frz_deg(
                Vec2::new(0.0, 0.0),
                Vec2::new(0.0, -100.0),
                Modifiers {
                    shift: true,
                    ..Modifiers::default()
                }
            ),
            90.0
        );
        let (sx, sy) = compute_scale_pct(
            100.0,
            50.0,
            Vec2::new(0.0, 0.0),
            Vec2::new(40.0, 0.0),
            Modifiers {
                shift: true,
                ..Modifiers::default()
            },
        );
        assert_eq!((sx, sy), (120.0, 60.0));

        let mut rc = RectClip {
            x1: 10.0,
            y1: 20.0,
            x2: 100.0,
            y2: 200.0,
            inverted: false,
        };
        rc.drag_corner(2, Vec2::new(150.0, 250.0));
        assert_eq!(rc.to_tag(), r"\clip(10,20,150,250)");

        let ev = AssEvent::dialogue(
            AssTime::from_cs(0),
            AssTime::from_cs(100),
            r"{\pos(10,20)}Hi",
        );
        let cmd =
            build_multi_line_drag_command(&[(0, &ev, Vec2::new(10.0, 20.0))], Vec2::new(5.0, -5.0));
        match cmd {
            EditCommand::Batch(v) => {
                assert_eq!(v.len(), 1);
                assert!(
                    matches!(&v[0], EditCommand::ModifyEventText { after, .. } if after == r"{\pos(15,15)}Hi")
                );
            }
            _ => panic!("expected batch"),
        }
    }

    #[test]
    fn vector_clip_all_modes_work() {
        let mut path = VectorClipPath::parse("m 0 0 l 100 0 100 100", false);
        assert_eq!(path.vertices.len(), 3);
        assert_eq!(path.hit_test_vertex(Vec2::new(99.0, 1.0), 5.0), Some(1));
        path.drag_vertex(1, Vec2::new(120.0, 10.0));
        assert!(path.convert_segment_at(1)); // line -> bezier
        assert_eq!(path.vertices.len(), 5);
        assert!(path.convert_segment_at(3)); // bezier -> line
        assert_eq!(path.vertices.len(), 3);
        path.insert_on_nearest_segment(Vec2::new(60.0, 5.0));
        assert_eq!(path.vertices.len(), 4);
        assert!(path.remove_vertex(1));
        assert_eq!(path.vertices.len(), 3);

        let mut tag = String::new();
        path.write_tag_into(&mut tag);
        assert!(tag.starts_with(r"\clip(m 0 0 l 120 10 100 100)"));

        // Freehand smooth
        let mut fh = VectorClipPath::default();
        fh.append_freehand(
            &[
                Vec2::new(0.0, 0.0),
                Vec2::new(10.0, 1.0),
                Vec2::new(20.0, 20.0),
                Vec2::new(40.0, 0.0),
            ],
            2.0,
            true,
        );
        fh.write_tag_into(&mut tag);
        assert!(tag.contains(" b "));
    }
}
