//! Aegisub 3.5.1 video display geometry, aspect-ratio modes, zoom/pan, rotation/flip, `HiDPI`
//! scaling, and Dummy Video provider (`src/video_display.cpp`, `src/video_controller.cpp`,
//! `src/video_provider_dummy.cpp`).

#![forbid(unsafe_code)]
#![cfg_attr(not(test), deny(clippy::unwrap_used, clippy::expect_used))]

pub mod dummy;

pub use dummy::{render_dummy_frame_rgba, DummyVideoConfig, DummyVideoError};
use visual_tools::{CoordTransform, Vec2};

/// Aegisub's standard zoom percentage presets (`src/video_display.cpp`).
pub const AEGISUB_ZOOM_PERCENTAGES: &[f64] = &[
    12.5, 25.0, 37.5, 50.0, 62.5, 75.0, 87.5, 100.0, 112.5, 125.0, 137.5, 150.0, 162.5, 175.0,
    187.5, 200.0, 250.0, 300.0, 400.0, 600.0, 800.0,
];

/// Display aspect-ratio override modes (`src/video_controller.cpp` `AspectRatio`).
#[derive(Debug, Clone, Copy, PartialEq)]
pub enum AspectRatioMode {
    Default,
    Fullscreen4x3,
    Widescreen16x9,
    Cinematic235,
    Custom(f64),
}

/// Quarter-turn display rotation.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum Rotation {
    #[default]
    Deg0,
    Deg90,
    Deg180,
    Deg270,
}

/// Axis-aligned rectangle in logical screen pixels.
#[derive(Debug, Clone, Copy, Default, PartialEq)]
pub struct DisplayRect {
    pub x: f64,
    pub y: f64,
    pub w: f64,
    pub h: f64,
}

/// Video viewport state supporting aspect-ratio overrides, zoom, pan (Aegisub 3.4+), `HiDPI` scale,
/// and rotation/flip.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct VideoDisplayState {
    pub video_w: f64,
    pub video_h: f64,
    pub viewport_w: f64,
    pub viewport_h: f64,
    pub dpi_scale: f64,
    pub aspect_mode: AspectRatioMode,
    /// Zoom factor (`1.0 == 100%`).
    pub zoom: f64,
    /// Pan offset in logical viewport pixels relative to centered position.
    pub pan: Vec2,
    pub rotation: Rotation,
    pub flip_h: bool,
    pub flip_v: bool,
}

impl VideoDisplayState {
    /// Creates a default 100%-zoom centered state.
    #[must_use]
    pub const fn new(video_w: f64, video_h: f64, viewport_w: f64, viewport_h: f64) -> Self {
        Self {
            video_w,
            video_h,
            viewport_w,
            viewport_h,
            dpi_scale: 1.0,
            aspect_mode: AspectRatioMode::Default,
            zoom: 1.0,
            pan: Vec2::new(0.0, 0.0),
            rotation: Rotation::Deg0,
            flip_h: false,
            flip_v: false,
        }
    }

    /// Effective display aspect ratio (accounting for [`AspectRatioMode`] and 90°/270° rotation).
    #[inline]
    #[must_use]
    pub fn effective_aspect(&self) -> f64 {
        let base = match self.aspect_mode {
            AspectRatioMode::Default => self.video_w / self.video_h.max(1.0),
            AspectRatioMode::Fullscreen4x3 => 4.0 / 3.0,
            AspectRatioMode::Widescreen16x9 => 16.0 / 9.0,
            AspectRatioMode::Cinematic235 => 2.35,
            AspectRatioMode::Custom(r) => r.max(0.1),
        };
        match self.rotation {
            Rotation::Deg0 | Rotation::Deg180 => base,
            Rotation::Deg90 | Rotation::Deg270 => 1.0 / base,
        }
    }

    /// Computes the rendered video rectangle inside the viewport (logical pixels) and optional
    /// letterbox/pillarbox bars.
    #[inline]
    #[must_use]
    pub fn layout(&self) -> VideoLayout {
        let aspect = self.effective_aspect();
        let (base_w, base_h) = match self.rotation {
            Rotation::Deg0 | Rotation::Deg180 => (self.video_h * aspect, self.video_h),
            Rotation::Deg90 | Rotation::Deg270 => (self.video_w * aspect, self.video_w),
        };
        let draw_w = base_w * self.zoom / self.dpi_scale.max(0.25);
        let draw_h = base_h * self.zoom / self.dpi_scale.max(0.25);
        let x = (self.viewport_w - draw_w) * 0.5 + self.pan.x;
        let y = (self.viewport_h - draw_h) * 0.5 + self.pan.y;

        let video_rect = DisplayRect {
            x,
            y,
            w: draw_w,
            h: draw_h,
        };

        // Compute letterbox (top/bottom) or pillarbox (left/right) bars clipped to viewport.
        let mut bars = [DisplayRect::default(); 2];
        let mut bar_count = 0usize;
        if y > 0.0 {
            bars[0] = DisplayRect {
                x: 0.0,
                y: 0.0,
                w: self.viewport_w,
                h: y.min(self.viewport_h),
            };
            bar_count = 1;
        }
        let bottom = y + draw_h;
        if bottom < self.viewport_h {
            bars[bar_count] = DisplayRect {
                x: 0.0,
                y: bottom.max(0.0),
                w: self.viewport_w,
                h: (self.viewport_h - bottom.max(0.0)).max(0.0),
            };
            bar_count += 1;
        }
        if bar_count == 0 {
            if x > 0.0 {
                bars[0] = DisplayRect {
                    x: 0.0,
                    y: 0.0,
                    w: x.min(self.viewport_w),
                    h: self.viewport_h,
                };
                bar_count = 1;
            }
            let right = x + draw_w;
            if right < self.viewport_w {
                bars[bar_count] = DisplayRect {
                    x: right.max(0.0),
                    y: 0.0,
                    w: (self.viewport_w - right.max(0.0)).max(0.0),
                    h: self.viewport_h,
                };
                bar_count += 1;
            }
        }

        VideoLayout {
            video_rect,
            letterbox_bars: bars,
            letterbox_bar_count: bar_count,
        }
    }

    /// Zooms to `new_zoom` while keeping `anchor_screen` stationary (Aegisub 3.4+ mouse-wheel
    /// zoom around cursor).
    #[inline]
    pub fn zoom_around_point(&mut self, new_zoom: f64, anchor_screen: Vec2) {
        let clamped = new_zoom.clamp(0.125, 8.0);
        let old_rect = self.layout().video_rect;
        if old_rect.w <= 1e-6 || old_rect.h <= 1e-6 {
            self.zoom = clamped;
            return;
        }
        let u = (anchor_screen.x - old_rect.x) / old_rect.w;
        let v = (anchor_screen.y - old_rect.y) / old_rect.h;
        self.zoom = clamped;
        let new_rect = self.layout().video_rect;
        let target_x = anchor_screen.x - u * new_rect.w;
        let target_y = anchor_screen.y - v * new_rect.h;
        self.pan.x += target_x - new_rect.x;
        self.pan.y += target_y - new_rect.y;
    }

    /// Pans the video by `delta` logical pixels.
    #[inline]
    pub fn pan_by(&mut self, delta: Vec2) {
        self.pan.x += delta.x;
        self.pan.y += delta.y;
    }

    /// Resets pan to center and fits video inside the viewport.
    pub fn fit_to_viewport(&mut self) {
        self.pan = Vec2::new(0.0, 0.0);
        let aspect = self.effective_aspect();
        let (base_w, base_h) = match self.rotation {
            Rotation::Deg0 | Rotation::Deg180 => (self.video_h * aspect, self.video_h),
            Rotation::Deg90 | Rotation::Deg270 => (self.video_w * aspect, self.video_w),
        };
        let zx = self.viewport_w * self.dpi_scale / base_w.max(1.0);
        let zy = self.viewport_h * self.dpi_scale / base_h.max(1.0);
        self.zoom = zx.min(zy).clamp(0.125, 8.0);
    }

    /// Converts a screen-space coordinate to normalized video UV `(0..1, 0..1)` accounting for
    /// rotation and horizontal/vertical flip, then into video pixel coordinates.
    #[inline]
    #[must_use]
    pub fn screen_to_video(&self, screen: Vec2) -> Vec2 {
        let rect = self.layout().video_rect;
        let mut u = (screen.x - rect.x) / rect.w.max(1e-6);
        let mut v = (screen.y - rect.y) / rect.h.max(1e-6);
        if self.flip_h {
            u = 1.0 - u;
        }
        if self.flip_v {
            v = 1.0 - v;
        }
        let (vu, vv) = match self.rotation {
            Rotation::Deg0 => (u, v),
            Rotation::Deg90 => (v, 1.0 - u),
            Rotation::Deg180 => (1.0 - u, 1.0 - v),
            Rotation::Deg270 => (1.0 - v, u),
        };
        Vec2::new(vu * self.video_w, vv * self.video_h)
    }

    /// Builds a [`visual_tools::CoordTransform`] directly usable by all `visual-tools` handles.
    #[inline]
    #[must_use]
    pub fn to_visual_coord_transform(&self, play_res_x: f64, play_res_y: f64) -> CoordTransform {
        let rect = self.layout().video_rect;
        CoordTransform {
            play_res_x,
            play_res_y,
            video_w: self.video_w,
            video_h: self.video_h,
            viewport_offset: Vec2::new(rect.x, rect.y),
            zoom: rect.w / self.video_w.max(1.0),
        }
    }
}

/// Computed video rectangle and letterbox/pillarbox bars for one frame.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct VideoLayout {
    pub video_rect: DisplayRect,
    pub letterbox_bars: [DisplayRect; 2],
    pub letterbox_bar_count: usize,
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn aspect_zoom_pan_rotation_and_visual_transform() {
        let mut st = VideoDisplayState::new(1920.0, 1080.0, 1280.0, 800.0);
        st.fit_to_viewport();
        let lay = st.layout();
        // 16:9 inside 1280x800 -> width=1280, height=720, top/bottom letterbox bars of 40 px each
        assert!((lay.video_rect.w - 1280.0).abs() < 1e-4);
        assert!((lay.video_rect.h - 720.0).abs() < 1e-4);
        assert_eq!(lay.letterbox_bar_count, 2);

        // Zoom around cursor keeps cursor's video pixel fixed
        let anchor = Vec2::new(320.0, 220.0);
        let before = st.screen_to_video(anchor);
        st.zoom_around_point(st.zoom * 2.0, anchor);
        let after = st.screen_to_video(anchor);
        assert!((before.x - after.x).abs() < 1e-4);
        assert!((before.y - after.y).abs() < 1e-4);

        let tf = st.to_visual_coord_transform(1920.0, 1080.0);
        let sc = tf.screen_to_script(anchor);
        assert!((sc.x - before.x).abs() < 1e-3);
    }

    #[test]
    fn dummy_uri_roundtrip_and_checkerboard_render() {
        let uri = "?dummy:23.976000:40000:64:32:47:163:254:c";
        let cfg = DummyVideoConfig::parse_uri(uri).expect("parse");
        assert_eq!(cfg.to_uri(), uri);
        let mut buf = vec![0u8; cfg.rgba_byte_len()];
        render_dummy_frame_rgba(&cfg, 0, &mut buf).expect("render");
        // (0,0) has base colour, (8,0) has shifted checkerboard colour
        assert_eq!(&buf[0..4], &[47, 163, 254, 255]);
        assert_eq!(&buf[8 * 4..8 * 4 + 4], &[23, 139, 230, 255]);
    }
}
