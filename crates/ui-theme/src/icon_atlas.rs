//! Lightweight 2x2-supersampled software rasterizer for crisp, cached 16 px / 200% DPI
//! duotone toolbar and visual-tool icon atlases (`0` per-frame allocations once uploaded).

use egui::{Color32, ColorImage, Vec2};
use std::f32::consts::TAU;

/// Normalized `16×16` icon rasterizer that writes directly into a horizontal sprite-atlas buffer.
pub struct IconAtlasBuilder {
    slot_px: usize,
    slot_count: usize,
    atlas_w: usize,
    pixels: Vec<Color32>,
}

impl IconAtlasBuilder {
    /// Creates an empty transparent atlas with `slot_count` square icons of `slot_px × slot_px`.
    #[must_use]
    pub fn new(slot_count: usize, slot_px: usize) -> Self {
        let slot_px = slot_px.clamp(16, 64);
        let atlas_w = slot_px.saturating_mul(slot_count.max(1));
        Self {
            slot_px,
            slot_count: slot_count.max(1),
            atlas_w,
            pixels: vec![Color32::TRANSPARENT; atlas_w.saturating_mul(slot_px)],
        }
    }

    /// Returns a drawing handle scoped to icon `slot` (`0..slot_count`) in `0..16` logical coords.
    pub fn slot(&mut self, slot: usize) -> IconSlot<'_> {
        let clamped = slot.min(self.slot_count - 1);
        IconSlot {
            builder: self,
            slot: clamped,
        }
    }

    /// Consumes the builder and produces an [`egui::ColorImage`] ready for `ctx.load_texture`.
    #[must_use]
    pub fn into_color_image(self) -> ColorImage {
        #[allow(clippy::cast_precision_loss)]
        let source_size = Vec2::new(self.atlas_w as f32, self.slot_px as f32);
        ColorImage {
            size: [self.atlas_w, self.slot_px],
            source_size,
            pixels: self.pixels,
        }
    }
}

/// Drawing handle for a single `16×16` logical icon inside an [`IconAtlasBuilder`].
pub struct IconSlot<'a> {
    builder: &'a mut IconAtlasBuilder,
    slot: usize,
}

impl IconSlot<'_> {
    #[inline]
    fn blend_pixel(&mut self, px: usize, py: usize, src: Color32, coverage: f32) {
        if coverage <= 0.0 || px >= self.builder.slot_px || py >= self.builder.slot_px {
            return;
        }
        let idx = py * self.builder.atlas_w + self.slot * self.builder.slot_px + px;
        let dst = self.builder.pixels[idx];
        let sa = (f32::from(src.a()) / 255.0) * coverage.clamp(0.0, 1.0);
        if sa <= 0.0 {
            return;
        }
        let da = f32::from(dst.a()) / 255.0;
        let out_a = sa + da * (1.0 - sa);
        if out_a <= 0.0001 {
            return;
        }
        let blend = |sc: u8, dc: u8| -> u8 {
            let v = (f32::from(sc) * sa + f32::from(dc) * da * (1.0 - sa)) / out_a;
            #[allow(clippy::cast_possible_truncation, clippy::cast_sign_loss)]
            let b = v.round().clamp(0.0, 255.0) as u8;
            b
        };
        #[allow(clippy::cast_possible_truncation, clippy::cast_sign_loss)]
        let oa = (out_a * 255.0).round().clamp(0.0, 255.0) as u8;
        self.builder.pixels[idx] = Color32::from_rgba_unmultiplied(
            blend(src.r(), dst.r()),
            blend(src.g(), dst.g()),
            blend(src.b(), dst.b()),
            oa,
        );
    }

    fn shade_sdf(&mut self, color: Color32, sdf: impl Fn(f32, f32) -> f32) {
        let n = self.builder.slot_px;
        #[allow(clippy::cast_precision_loss)]
        let scale = 16.0 / (n as f32);
        let aa = scale * 0.65;
        for py in 0..n {
            for px in 0..n {
                // 2x2 supersampling inside the pixel for crisp anti-aliased edges.
                let mut cov = 0.0_f32;
                for sy in [0.25_f32, 0.75] {
                    #[allow(clippy::cast_precision_loss)]
                    let y = (py as f32 + sy) * scale;
                    for sx in [0.25_f32, 0.75] {
                        #[allow(clippy::cast_precision_loss)]
                        let x = (px as f32 + sx) * scale;
                        let d = sdf(x, y);
                        cov += ((0.5 - d / aa).clamp(0.0, 1.0)) * 0.25;
                    }
                }
                if cov > 0.0 {
                    self.blend_pixel(px, py, color, cov);
                }
            }
        }
    }

    /// Paints a filled rounded rectangle in `0..16` icon coordinates.
    pub fn fill_rect(&mut self, x0: f32, y0: f32, x1: f32, y1: f32, radius: f32, color: Color32) {
        let cx = f32::midpoint(x0, x1);
        let cy = f32::midpoint(y0, y1);
        let hx = ((x1 - x0) * 0.5 - radius).max(0.0);
        let hy = ((y1 - y0) * 0.5 - radius).max(0.0);
        self.shade_sdf(color, |x, y| {
            let dx = (x - cx).abs() - hx;
            let dy = (y - cy).abs() - hy;
            let out = dx.max(0.0).hypot(dy.max(0.0));
            let inside = dx.max(dy).min(0.0);
            out + inside - radius
        });
    }

    /// Paints a stroked rounded rectangle in `0..16` icon coordinates.
    #[allow(clippy::too_many_arguments)]
    pub fn stroke_rect(
        &mut self,
        x0: f32,
        y0: f32,
        x1: f32,
        y1: f32,
        radius: f32,
        width: f32,
        color: Color32,
    ) {
        let cx = f32::midpoint(x0, x1);
        let cy = f32::midpoint(y0, y1);
        let hx = ((x1 - x0) * 0.5 - radius).max(0.0);
        let hy = ((y1 - y0) * 0.5 - radius).max(0.0);
        let hw = width * 0.5;
        self.shade_sdf(color, |x, y| {
            let dx = (x - cx).abs() - hx;
            let dy = (y - cy).abs() - hy;
            let d = dx.max(0.0).hypot(dy.max(0.0)) + dx.max(dy).min(0.0) - radius;
            d.abs() - hw
        });
    }

    /// Paints a filled circle in `0..16` icon coordinates.
    pub fn fill_circle(&mut self, cx: f32, cy: f32, r: f32, color: Color32) {
        self.shade_sdf(color, |x, y| (x - cx).hypot(y - cy) - r);
    }

    /// Paints a stroked circle in `0..16` icon coordinates.
    pub fn stroke_circle(&mut self, cx: f32, cy: f32, r: f32, width: f32, color: Color32) {
        let hw = width * 0.5;
        self.shade_sdf(color, |x, y| ((x - cx).hypot(y - cy) - r).abs() - hw);
    }

    /// Paints a round-capped line segment in `0..16` icon coordinates.
    pub fn line(&mut self, x0: f32, y0: f32, x1: f32, y1: f32, width: f32, color: Color32) {
        let dx = x1 - x0;
        let dy = y1 - y0;
        let len_sq = dx * dx + dy * dy;
        let hw = width * 0.5;
        self.shade_sdf(color, |x, y| {
            let t = if len_sq > 1e-6 {
                (((x - x0) * dx + (y - y0) * dy) / len_sq).clamp(0.0, 1.0)
            } else {
                0.0
            };
            let px = x0 + t * dx;
            let py = y0 + t * dy;
            (x - px).hypot(y - py) - hw
        });
    }

    /// Paints a filled triangle in `0..16` icon coordinates.
    pub fn fill_tri(&mut self, p0: [f32; 2], p1: [f32; 2], p2: [f32; 2], color: Color32) {
        self.shade_sdf(color, |x, y| tri_sdf(x, y, p0, p1, p2));
    }

    /// Paints an elliptical arc from angle `a0` to `a1` (radians) in `0..16` icon coordinates.
    #[allow(clippy::too_many_arguments)]
    pub fn arc(
        &mut self,
        cx: f32,
        cy: f32,
        rx: f32,
        ry: f32,
        a0: f32,
        a1: f32,
        width: f32,
        color: Color32,
    ) {
        let steps = 16usize;
        let mut prev = [cx + a0.cos() * rx, cy + a0.sin() * ry];
        for i in 1..=steps {
            #[allow(clippy::cast_precision_loss)]
            let t = (i as f32) / (steps as f32);
            let a = a0 + (a1 - a0) * t;
            let cur = [cx + a.cos() * rx, cy + a.sin() * ry];
            self.line(prev[0], prev[1], cur[0], cur[1], width, color);
            prev = cur;
        }
    }

    /// Paints a closed ellipse outline in `0..16` icon coordinates.
    pub fn stroke_ellipse(
        &mut self,
        cx: f32,
        cy: f32,
        rx: f32,
        ry: f32,
        width: f32,
        color: Color32,
    ) {
        self.arc(cx, cy, rx, ry, 0.0, TAU, width, color);
    }
}

#[inline]
fn tri_sdf(px: f32, py: f32, p0: [f32; 2], p1: [f32; 2], p2: [f32; 2]) -> f32 {
    let e0 = [p1[0] - p0[0], p1[1] - p0[1]];
    let e1 = [p2[0] - p1[0], p2[1] - p1[1]];
    let e2 = [p0[0] - p2[0], p0[1] - p2[1]];
    let v0 = [px - p0[0], py - p0[1]];
    let v1 = [px - p1[0], py - p1[1]];
    let v2 = [px - p2[0], py - p2[1]];

    let proj = |vec: [f32; 2], edge: [f32; 2]| -> [f32; 2] {
        let denom = (edge[0] * edge[0] + edge[1] * edge[1]).max(1e-6);
        let frac = ((vec[0] * edge[0] + vec[1] * edge[1]) / denom).clamp(0.0, 1.0);
        [vec[0] - edge[0] * frac, vec[1] - edge[1] * frac]
    };
    let pq0 = proj(v0, e0);
    let pq1 = proj(v1, e1);
    let pq2 = proj(v2, e2);

    let winding = (e0[0] * e2[1] - e0[1] * e2[0]).signum();
    let d0 = pq0[0] * pq0[0] + pq0[1] * pq0[1];
    let d1 = pq1[0] * pq1[0] + pq1[1] * pq1[1];
    let d2 = pq2[0] * pq2[0] + pq2[1] * pq2[1];
    let c0 = winding * (v0[0] * e0[1] - v0[1] * e0[0]);
    let c1 = winding * (v1[0] * e1[1] - v1[1] * e1[0]);
    let c2 = winding * (v2[0] * e2[1] - v2[1] * e2[0]);

    let min_d = d0.min(d1).min(d2).sqrt();
    let min_c = c0.min(c1).min(c2);
    if min_c > 0.0 { -min_d } else { min_d }
}

/// Computes the UV rectangle for `slot` out of `slot_count` in a horizontal sprite atlas.
#[inline]
#[must_use]
pub fn slot_uv(slot: usize, slot_count: usize) -> egui::Rect {
    #[allow(clippy::cast_precision_loss)]
    let n = slot_count.max(1) as f32;
    #[allow(clippy::cast_precision_loss)]
    let u0 = (slot as f32) / n;
    #[allow(clippy::cast_precision_loss)]
    let u1 = ((slot + 1) as f32) / n;
    egui::Rect::from_min_max(egui::Pos2::new(u0, 0.0), egui::Pos2::new(u1, 1.0))
}
