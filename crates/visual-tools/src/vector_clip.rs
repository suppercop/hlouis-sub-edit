//! Aegisub 3.5.1 Vector Clip tool (`src/visual_tool_vector_clip.cpp` & `src/spline.cpp`).
//!
//! Supports all Aegisub vector-clip sub-modes:
//! - Drag point / control handle
//! - Append line (`l`)
//! - Append bicubic bezier (`b`)
//! - Convert line <-> bezier
//! - Insert point on segment
//! - Remove point
//! - Freehand draw + Ramer-Douglas-Peucker freehand smoothing

use crate::Vec2;
use std::fmt::Write as _;

/// Sub-mode for the Vector Clip tool (matches Aegisub `VisualToolVectorClip` toolbar).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum VectorClipMode {
    Drag,
    Line,
    Bicubic,
    Convert,
    Insert,
    Remove,
    Freehand,
    FreehandSmooth,
}

/// Kind of vertex inside a vector clip contour.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum VectorVertexKind {
    /// Contour start (`m x y`).
    Move,
    /// Straight line endpoint (`l x y`).
    Line,
    /// Cubic bezier control point 1 (first pair of `b x1 y1 x2 y2 x3 y3`).
    BezierCtrl1,
    /// Cubic bezier control point 2 (second pair of `b`).
    BezierCtrl2,
    /// Cubic bezier endpoint (third pair of `b`).
    BezierEnd,
    /// Cubic B-spline control point (`s x1 y1 x2 y2 x3 y3 ...`).
    Spline,
    /// Extended B-spline control point (`p x y`).
    SplineExtend,
    /// Closes an open B-spline (`c` — no coordinates, `pos` unused).
    SplineClose,
}

/// A single editable vertex in a vector clip path.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct VectorVertex {
    pub kind: VectorVertexKind,
    pub pos: Vec2,
}

/// Editable vector clip geometry (flat vertex array for L1-cache-resident hit-testing & drag).
#[derive(Debug, Clone, Default, PartialEq)]
pub struct VectorClipPath {
    pub inverted: bool,
    pub scale: u32,
    pub vertices: Vec<VectorVertex>,
    /// Preserved verbatim input payload so untouched clips round-trip byte-identically.
    raw_original: Option<String>,
}

impl VectorClipPath {
    /// Parses an ASS vector drawing payload (the interior of `\clip(...)` or `\iclip(...)`).
    #[must_use]
    pub fn parse(payload: &str, inverted: bool) -> Self {
        let trimmed = payload.trim();
        let (scale, body) = if let Some((first, rest)) = trimmed.split_once(',') {
            if let Ok(sc) = first.trim().parse::<u32>() {
                (sc.max(1), rest.trim())
            } else {
                (1, trimmed)
            }
        } else {
            (1, trimmed)
        };

        let mut vertices = Vec::new();
        let mut tokens = body.split_whitespace().peekable();
        let mut cmd = 'm';

        while let Some(tok) = tokens.next() {
            if tok.len() == 1 {
                let c = tok.as_bytes()[0].to_ascii_lowercase() as char;
                if c == 'c' {
                    vertices.push(VectorVertex {
                        kind: VectorVertexKind::SplineClose,
                        pos: Vec2::new(0.0, 0.0),
                    });
                    cmd = 'c';
                    continue;
                }
                if matches!(c, 'm' | 'n' | 'l' | 'b' | 's' | 'p') {
                    cmd = c;
                    continue;
                }
            }
            let Ok(x) = tok.parse::<f64>() else {
                continue;
            };
            let Some(y_tok) = tokens.next() else {
                break;
            };
            let Ok(y) = y_tok.parse::<f64>() else {
                continue;
            };

            match cmd {
                'm' | 'n' => {
                    vertices.push(VectorVertex {
                        kind: VectorVertexKind::Move,
                        pos: Vec2::new(x, y),
                    });
                    cmd = 'l';
                }
                'l' => {
                    vertices.push(VectorVertex {
                        kind: VectorVertexKind::Line,
                        pos: Vec2::new(x, y),
                    });
                }
                'b' => {
                    let Some(x2) = tokens.next().and_then(|t| t.parse::<f64>().ok()) else {
                        break;
                    };
                    let Some(y2) = tokens.next().and_then(|t| t.parse::<f64>().ok()) else {
                        break;
                    };
                    let Some(x3) = tokens.next().and_then(|t| t.parse::<f64>().ok()) else {
                        break;
                    };
                    let Some(y3) = tokens.next().and_then(|t| t.parse::<f64>().ok()) else {
                        break;
                    };
                    vertices.push(VectorVertex {
                        kind: VectorVertexKind::BezierCtrl1,
                        pos: Vec2::new(x, y),
                    });
                    vertices.push(VectorVertex {
                        kind: VectorVertexKind::BezierCtrl2,
                        pos: Vec2::new(x2, y2),
                    });
                    vertices.push(VectorVertex {
                        kind: VectorVertexKind::BezierEnd,
                        pos: Vec2::new(x3, y3),
                    });
                }
                's' => {
                    vertices.push(VectorVertex {
                        kind: VectorVertexKind::Spline,
                        pos: Vec2::new(x, y),
                    });
                }
                'p' => {
                    vertices.push(VectorVertex {
                        kind: VectorVertexKind::SplineExtend,
                        pos: Vec2::new(x, y),
                    });
                }
                _ => {}
            }
        }

        Self {
            inverted,
            scale,
            vertices,
            raw_original: Some(payload.to_owned()),
        }
    }

    /// Hit-tests `query` against all vertices within `radius_sq` (`radius * radius`).
    /// Zero allocations; linear scan over contiguous `Vec<VectorVertex>` (< 1 µs for 500 pts).
    #[inline]
    #[must_use]
    pub fn hit_test_vertex(&self, query: Vec2, radius: f64) -> Option<usize> {
        let mut best_d2 = radius * radius;
        let mut best_idx = None;
        for (i, v) in self.vertices.iter().enumerate() {
            if v.kind == VectorVertexKind::SplineClose {
                continue;
            }
            let dx = v.pos.x - query.x;
            let dy = v.pos.y - query.y;
            let d2 = dx * dx + dy * dy;
            if d2 <= best_d2 {
                best_d2 = d2;
                best_idx = Some(i);
            }
        }
        best_idx
    }

    /// Moves vertex `index` to `new_pos` in O(1) with zero heap allocations.
    #[inline]
    pub fn drag_vertex(&mut self, index: usize, new_pos: Vec2) {
        if let Some(v) = self.vertices.get_mut(index) {
            v.pos = new_pos;
            self.raw_original = None;
        }
    }

    /// Appends a line vertex (or `Move` if path is empty).
    pub fn append_line(&mut self, pos: Vec2) {
        let kind = if self.vertices.is_empty() {
            VectorVertexKind::Move
        } else {
            VectorVertexKind::Line
        };
        self.vertices.push(VectorVertex { kind, pos });
        self.raw_original = None;
    }

    /// Appends a cubic bezier segment from the previous vertex to `end_pos` with 1/3 & 2/3
    /// default control handles (matching Aegisub's Bicubic tool).
    pub fn append_bicubic(&mut self, end_pos: Vec2) {
        let Some(prev) = self.vertices.last().map(|v| v.pos) else {
            self.vertices.push(VectorVertex {
                kind: VectorVertexKind::Move,
                pos: end_pos,
            });
            return;
        };
        let c1 = Vec2::new(
            prev.x + (end_pos.x - prev.x) / 3.0,
            prev.y + (end_pos.y - prev.y) / 3.0,
        );
        let c2 = Vec2::new(
            prev.x + 2.0 * (end_pos.x - prev.x) / 3.0,
            prev.y + 2.0 * (end_pos.y - prev.y) / 3.0,
        );
        self.vertices.push(VectorVertex {
            kind: VectorVertexKind::BezierCtrl1,
            pos: c1,
        });
        self.vertices.push(VectorVertex {
            kind: VectorVertexKind::BezierCtrl2,
            pos: c2,
        });
        self.vertices.push(VectorVertex {
            kind: VectorVertexKind::BezierEnd,
            pos: end_pos,
        });
        self.raw_original = None;
    }

    /// Converts a `Line` vertex at `index` into a cubic bezier (`Ctrl1, Ctrl2, End`), or a
    /// `BezierEnd` at `index` back into a `Line` (removing its two preceding control points).
    pub fn convert_segment_at(&mut self, index: usize) -> bool {
        let Some(v) = self.vertices.get(index).copied() else {
            return false;
        };
        match v.kind {
            VectorVertexKind::Line if index > 0 => {
                let prev = self.vertices[index - 1].pos;
                let end = v.pos;
                let c1 = Vec2::new(
                    prev.x + (end.x - prev.x) / 3.0,
                    prev.y + (end.y - prev.y) / 3.0,
                );
                let c2 = Vec2::new(
                    prev.x + 2.0 * (end.x - prev.x) / 3.0,
                    prev.y + 2.0 * (end.y - prev.y) / 3.0,
                );
                self.vertices[index].kind = VectorVertexKind::BezierEnd;
                self.vertices.insert(
                    index,
                    VectorVertex {
                        kind: VectorVertexKind::BezierCtrl2,
                        pos: c2,
                    },
                );
                self.vertices.insert(
                    index,
                    VectorVertex {
                        kind: VectorVertexKind::BezierCtrl1,
                        pos: c1,
                    },
                );
                self.raw_original = None;
                true
            }
            VectorVertexKind::BezierEnd if index >= 2 => {
                if self.vertices[index - 1].kind == VectorVertexKind::BezierCtrl2
                    && self.vertices[index - 2].kind == VectorVertexKind::BezierCtrl1
                {
                    self.vertices[index].kind = VectorVertexKind::Line;
                    self.vertices.drain(index - 2..index);
                    self.raw_original = None;
                    return true;
                }
                false
            }
            VectorVertexKind::Spline => self.convert_spline_to_bezier_at(index),
            _ => false,
        }
    }

    /// Converts a cubic Bezier segment (`b c1 c2 end`, whose endpoint is at `index`) into an
    /// equivalent uniform cubic B-spline segment (`s s1 s2 s3` starting from `Move`/`prev` = `s0`),
    /// matching Aegisub `src/visual_tool_vector_clip.cpp` / `src/spline.cpp`.
    ///
    /// Uses the exact inverse of the uniform cubic B-spline basis matrix:
    /// - `S0 = 6*B0 - 7*B1 + 2*B2`
    /// - `S1 = 2*B1 - B2`
    /// - `S2 = -B1 + 2*B2`
    /// - `S3 = 2*B1 - 7*B2 + 6*B3`
    pub fn convert_bezier_to_spline_at(&mut self, index: usize) -> bool {
        if index < 3 || index >= self.vertices.len() {
            return false;
        }
        if self.vertices[index].kind != VectorVertexKind::BezierEnd
            || self.vertices[index - 1].kind != VectorVertexKind::BezierCtrl2
            || self.vertices[index - 2].kind != VectorVertexKind::BezierCtrl1
        {
            return false;
        }
        let b0 = self.vertices[index - 3].pos;
        let b1 = self.vertices[index - 2].pos;
        let b2 = self.vertices[index - 1].pos;
        let b3 = self.vertices[index].pos;

        let s0 = Vec2::new(
            6.0 * b0.x - 7.0 * b1.x + 2.0 * b2.x,
            6.0 * b0.y - 7.0 * b1.y + 2.0 * b2.y,
        );
        let s1 = Vec2::new(2.0 * b1.x - b2.x, 2.0 * b1.y - b2.y);
        let s2 = Vec2::new(-b1.x + 2.0 * b2.x, -b1.y + 2.0 * b2.y);
        let s3 = Vec2::new(
            2.0 * b1.x - 7.0 * b2.x + 6.0 * b3.x,
            2.0 * b1.y - 7.0 * b2.y + 6.0 * b3.y,
        );

        self.vertices[index - 3].pos = s0;
        self.vertices[index - 2] = VectorVertex {
            kind: VectorVertexKind::Spline,
            pos: s1,
        };
        self.vertices[index - 1] = VectorVertex {
            kind: VectorVertexKind::Spline,
            pos: s2,
        };
        self.vertices[index] = VectorVertex {
            kind: VectorVertexKind::Spline,
            pos: s3,
        };
        self.raw_original = None;
        true
    }

    /// Converts a uniform cubic B-spline segment (`s s1 s2 s3` following `s0`) into an equivalent
    /// cubic Bezier segment (`b b1 b2 b3` starting at `b0`), matching Aegisub `src/spline.cpp`.
    ///
    /// `index` may point to any of the three `Spline` control vertices (`s1`, `s2`, `s3`) of the
    /// segment; the segment's 4 control points `(s0, s1, s2, s3)` are converted via:
    /// - `B0 = (S0 + 4*S1 + S2) / 6`
    /// - `B1 = (2*S1 + S2) / 3`
    /// - `B2 = (S1 + 2*S2) / 3`
    /// - `B3 = (S1 + 4*S2 + S3) / 6`
    pub fn convert_spline_to_bezier_at(&mut self, index: usize) -> bool {
        if index >= self.vertices.len() || self.vertices[index].kind != VectorVertexKind::Spline {
            return false;
        }
        // Locate the start of the contiguous `Spline` run containing `index`.
        let mut run_start = index;
        while run_start > 0 && self.vertices[run_start - 1].kind == VectorVertexKind::Spline {
            run_start -= 1;
        }
        if run_start == 0 || run_start + 2 >= self.vertices.len() {
            return false;
        }
        if self.vertices[run_start + 1].kind != VectorVertexKind::Spline
            || self.vertices[run_start + 2].kind != VectorVertexKind::Spline
        {
            return false;
        }

        let s0 = self.vertices[run_start - 1].pos;
        let s1 = self.vertices[run_start].pos;
        let s2 = self.vertices[run_start + 1].pos;
        let s3 = self.vertices[run_start + 2].pos;

        let b0 = Vec2::new(
            (s0.x + 4.0 * s1.x + s2.x) / 6.0,
            (s0.y + 4.0 * s1.y + s2.y) / 6.0,
        );
        let b1 = Vec2::new((2.0 * s1.x + s2.x) / 3.0, (2.0 * s1.y + s2.y) / 3.0);
        let b2 = Vec2::new((s1.x + 2.0 * s2.x) / 3.0, (s1.y + 2.0 * s2.y) / 3.0);
        let b3 = Vec2::new(
            (s1.x + 4.0 * s2.x + s3.x) / 6.0,
            (s1.y + 4.0 * s2.y + s3.y) / 6.0,
        );

        self.vertices[run_start - 1].pos = b0;
        self.vertices[run_start] = VectorVertex {
            kind: VectorVertexKind::BezierCtrl1,
            pos: b1,
        };
        self.vertices[run_start + 1] = VectorVertex {
            kind: VectorVertexKind::BezierCtrl2,
            pos: b2,
        };
        self.vertices[run_start + 2] = VectorVertex {
            kind: VectorVertexKind::BezierEnd,
            pos: b3,
        };
        self.raw_original = None;
        true
    }

    /// Inserts a new `Line` vertex on the nearest segment to `pos`.
    pub fn insert_on_nearest_segment(&mut self, pos: Vec2) -> Option<usize> {
        if self.vertices.len() < 2 {
            self.append_line(pos);
            return Some(self.vertices.len() - 1);
        }
        let mut best_i = None;
        let mut best_d2 = f64::INFINITY;
        for i in 1..self.vertices.len() {
            if matches!(
                self.vertices[i].kind,
                VectorVertexKind::BezierCtrl1 | VectorVertexKind::BezierCtrl2
            ) {
                continue;
            }
            let a = self.vertices[i - 1].pos;
            let b = self.vertices[i].pos;
            let d2 = point_to_segment_dist_sq(pos, a, b);
            if d2 < best_d2 {
                best_d2 = d2;
                best_i = Some(i);
            }
        }
        if let Some(idx) = best_i {
            self.vertices.insert(
                idx,
                VectorVertex {
                    kind: VectorVertexKind::Line,
                    pos,
                },
            );
            self.raw_original = None;
        }
        best_i
    }

    /// Removes the vertex at `index` (and its bezier control handles if `BezierEnd`).
    pub fn remove_vertex(&mut self, index: usize) -> bool {
        let Some(v) = self.vertices.get(index).copied() else {
            return false;
        };
        match v.kind {
            VectorVertexKind::BezierEnd
                if index >= 2
                    && self.vertices[index - 1].kind == VectorVertexKind::BezierCtrl2
                    && self.vertices[index - 2].kind == VectorVertexKind::BezierCtrl1 =>
            {
                self.vertices.drain(index - 2..=index);
                self.raw_original = None;
                true
            }
            VectorVertexKind::BezierCtrl1 | VectorVertexKind::BezierCtrl2 => false,
            _ => {
                self.vertices.remove(index);
                if index == 0 && !self.vertices.is_empty() {
                    self.vertices[0].kind = VectorVertexKind::Move;
                }
                self.raw_original = None;
                true
            }
        }
    }

    /// Appends a freehand polyline stroke, optionally simplifying with Ramer-Douglas-Peucker
    /// (`tolerance_px > 0.0`) and converting to smooth cubic beziers when `smooth == true`.
    pub fn append_freehand(&mut self, stroke: &[Vec2], tolerance_px: f64, smooth: bool) {
        if stroke.is_empty() {
            return;
        }
        let simplified = if tolerance_px > 0.0 && stroke.len() > 2 {
            rdp_simplify(stroke, tolerance_px)
        } else {
            stroke.to_vec()
        };
        if simplified.is_empty() {
            return;
        }
        self.raw_original = None;
        self.vertices.push(VectorVertex {
            kind: VectorVertexKind::Move,
            pos: simplified[0],
        });
        if !smooth || simplified.len() < 3 {
            for &pt in &simplified[1..] {
                self.vertices.push(VectorVertex {
                    kind: VectorVertexKind::Line,
                    pos: pt,
                });
            }
            return;
        }
        // Catmull-Rom to cubic Bezier conversion for smooth freehand curves.
        for i in 0..simplified.len() - 1 {
            let p0 = simplified[i.saturating_sub(1)];
            let p1 = simplified[i];
            let p2 = simplified[i + 1];
            let p3 = simplified[(i + 2).min(simplified.len() - 1)];
            let c1 = Vec2::new(p1.x + (p2.x - p0.x) / 6.0, p1.y + (p2.y - p0.y) / 6.0);
            let c2 = Vec2::new(p2.x - (p3.x - p1.x) / 6.0, p2.y - (p3.y - p1.y) / 6.0);
            self.vertices.push(VectorVertex {
                kind: VectorVertexKind::BezierCtrl1,
                pos: c1,
            });
            self.vertices.push(VectorVertex {
                kind: VectorVertexKind::BezierCtrl2,
                pos: c2,
            });
            self.vertices.push(VectorVertex {
                kind: VectorVertexKind::BezierEnd,
                pos: p2,
            });
        }
    }

    /// Formats the vector clip tag payload into `out` (reusing `out`'s allocation).
    pub fn write_tag_into(&self, out: &mut String) {
        out.clear();
        out.push_str(if self.inverted { r"\iclip(" } else { r"\clip(" });
        if let Some(ref raw) = self.raw_original {
            out.push_str(raw);
            out.push(')');
            return;
        }
        if self.scale > 1 {
            let _ = write!(out, "{},", self.scale);
        }
        let mut last_cmd = '\0';
        let mut i = 0usize;
        let len = self.vertices.len();
        while i < len {
            let v = self.vertices[i];
            match v.kind {
                VectorVertexKind::Move => {
                    if last_cmd != '\0' {
                        out.push(' ');
                    }
                    let _ = write!(out, "m {} {}", round_i32(v.pos.x), round_i32(v.pos.y));
                    last_cmd = 'm';
                    i += 1;
                }
                VectorVertexKind::Line => {
                    if last_cmd != 'l' {
                        out.push_str(" l");
                        last_cmd = 'l';
                    }
                    let _ = write!(out, " {} {}", round_i32(v.pos.x), round_i32(v.pos.y));
                    i += 1;
                }
                VectorVertexKind::BezierCtrl1 if i + 2 < len => {
                    let c2 = self.vertices[i + 1].pos;
                    let end = self.vertices[i + 2].pos;
                    if last_cmd != 'b' {
                        out.push_str(" b");
                        last_cmd = 'b';
                    }
                    let _ = write!(
                        out,
                        " {} {} {} {} {} {}",
                        round_i32(v.pos.x),
                        round_i32(v.pos.y),
                        round_i32(c2.x),
                        round_i32(c2.y),
                        round_i32(end.x),
                        round_i32(end.y),
                    );
                    i += 3;
                }
                VectorVertexKind::Spline => {
                    if last_cmd != 's' {
                        out.push_str(" s");
                        last_cmd = 's';
                    }
                    let _ = write!(out, " {} {}", round_i32(v.pos.x), round_i32(v.pos.y));
                    i += 1;
                }
                VectorVertexKind::SplineExtend => {
                    if last_cmd != 'p' {
                        out.push_str(" p");
                        last_cmd = 'p';
                    }
                    let _ = write!(out, " {} {}", round_i32(v.pos.x), round_i32(v.pos.y));
                    i += 1;
                }
                VectorVertexKind::SplineClose => {
                    out.push_str(" c");
                    last_cmd = 'c';
                    i += 1;
                }
                _ => {
                    i += 1;
                }
            }
        }
        out.push(')');
    }
}

#[inline]
#[allow(clippy::cast_possible_truncation)]
fn round_i32(v: f64) -> i32 {
    v.round() as i32
}

fn point_to_segment_dist_sq(p: Vec2, a: Vec2, b: Vec2) -> f64 {
    let abx = b.x - a.x;
    let aby = b.y - a.y;
    let len2 = abx * abx + aby * aby;
    if len2 < 1e-12 {
        let dx = p.x - a.x;
        let dy = p.y - a.y;
        return dx * dx + dy * dy;
    }
    let t = (((p.x - a.x) * abx + (p.y - a.y) * aby) / len2).clamp(0.0, 1.0);
    let px = a.x + t * abx;
    let py = a.y + t * aby;
    let dx = p.x - px;
    let dy = p.y - py;
    dx * dx + dy * dy
}

fn rdp_simplify(pts: &[Vec2], eps: f64) -> Vec<Vec2> {
    if pts.len() <= 2 {
        return pts.to_vec();
    }
    let eps2 = eps * eps;
    let a = pts[0];
    let b = pts[pts.len() - 1];
    let mut max_d2 = 0.0;
    let mut max_i = 0usize;
    for (i, &p) in pts.iter().enumerate().take(pts.len() - 1).skip(1) {
        let d2 = point_to_segment_dist_sq(p, a, b);
        if d2 > max_d2 {
            max_d2 = d2;
            max_i = i;
        }
    }
    if max_d2 > eps2 {
        let mut left = rdp_simplify(&pts[..=max_i], eps);
        let right = rdp_simplify(&pts[max_i..], eps);
        left.pop();
        left.extend_from_slice(&right);
        left
    } else {
        vec![a, b]
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn converts_bezier_to_bspline_and_back_losslessly() {
        let mut path = VectorClipPath::parse("m 10 20 b 40 80 120 90 180 30", false);
        let orig: Vec<Vec2> = path.vertices.iter().map(|v| v.pos).collect();

        assert!(path.convert_bezier_to_spline_at(3));
        assert_eq!(path.vertices[1].kind, VectorVertexKind::Spline);
        assert_eq!(path.vertices[2].kind, VectorVertexKind::Spline);
        assert_eq!(path.vertices[3].kind, VectorVertexKind::Spline);

        let mut tag = String::new();
        path.write_tag_into(&mut tag);
        assert!(tag.contains(" s "), "expected s command in {tag}");

        assert!(path.convert_spline_to_bezier_at(2));
        assert_eq!(path.vertices[1].kind, VectorVertexKind::BezierCtrl1);
        assert_eq!(path.vertices[2].kind, VectorVertexKind::BezierCtrl2);
        assert_eq!(path.vertices[3].kind, VectorVertexKind::BezierEnd);

        for (i, want) in orig.iter().enumerate() {
            let got = path.vertices[i].pos;
            assert!(
                (got.x - want.x).abs() < 1e-6 && (got.y - want.y).abs() < 1e-6,
                "vertex {i} mismatch: got {got:?}, want {want:?}"
            );
        }
    }
}
