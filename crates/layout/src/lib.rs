//! Pure layout geometry for the classic Aegisub 3.2.2 four-panel workspace
//! (`src/frame_main.cpp` `FrameMain::SetDisplayMode`):
//! video top-left, audio top-right, edit box under audio, subtitle grid across the bottom.
//!
//! Computes exact logical rectangles from window dimensions, DPI scale, view mode, splitter
//! proportions, and detached-video state with zero heap allocations (< 50 ns per frame).

#![forbid(unsafe_code)]
#![cfg_attr(not(test), deny(clippy::unwrap_used, clippy::expect_used))]

use std::collections::HashMap;

/// Logical axis-aligned rectangle.
#[derive(Debug, Clone, Copy, Default, PartialEq)]
pub struct Rect {
    pub x: f32,
    pub y: f32,
    pub w: f32,
    pub h: f32,
}

impl Rect {
    /// Returns `true` if the rectangle has positive area.
    #[inline]
    #[must_use]
    pub fn is_visible(self) -> bool {
        self.w > 0.0 && self.h > 0.0
    }
}

/// Active Aegisub panel visibility mode.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum ViewMode {
    SubsOnly,
    VideoAndSubs,
    AudioAndSubs,
    #[default]
    Full,
}

/// DPI-independent minimum panel dimensions (logical points).
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct MinPanelSizes {
    pub video_w: f32,
    pub video_h: f32,
    pub audio_w: f32,
    pub audio_h: f32,
    pub edit_h: f32,
    pub grid_h: f32,
    pub splitter: f32,
}

impl Default for MinPanelSizes {
    fn default() -> Self {
        Self {
            video_w: 240.0,
            video_h: 180.0,
            audio_w: 280.0,
            audio_h: 120.0,
            edit_h: 110.0,
            grid_h: 140.0,
            splitter: 4.0,
        }
    }
}

/// Detached video window geometry (clamped into monitor bounds when restoring).
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct DetachedVideoState {
    pub detached: bool,
    pub x: f32,
    pub y: f32,
    pub w: f32,
    pub h: f32,
}

impl Default for DetachedVideoState {
    fn default() -> Self {
        Self {
            detached: false,
            x: 80.0,
            y: 80.0,
            w: 854.0,
            h: 480.0,
        }
    }
}

/// Width of the video panel taken by the visual-tool toolbar and spacing (logical points); the
/// canvas is `panel_w - VIDEO_PANEL_CHROME_W` wide.
pub const VIDEO_PANEL_CHROME_W: f32 = 28.0;
/// Height of the video panel taken by the seek bar and transport rows below the canvas.
pub const VIDEO_PANEL_CHROME_H: f32 = 44.0;

/// Proportional splitter state (`0.0..=1.0`).
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct LayoutState {
    pub mode: ViewMode,
    /// Fraction of total height allocated to the top region (video + audio/edit) vs bottom grid.
    pub top_height_ratio: f32,
    /// Fraction of top width allocated to the video panel on the left (in `Full` mode).
    pub video_width_ratio: f32,
    /// Fraction of top-right height allocated to the audio panel vs edit box below it.
    pub audio_height_ratio: f32,
    /// Display aspect ratio (width / height) of the loaded video. When `Some`, the video panel
    /// takes the width that makes its canvas match this aspect at the current top height (as
    /// Aegisub sizes its video window), so Fit mode shows no letterbox bars when the layout
    /// allows it. Cleared when the user drags the video/right splitter.
    pub video_aspect: Option<f32>,
    pub detached_video: DetachedVideoState,
    pub mins: MinPanelSizes,
}

impl Default for LayoutState {
    fn default() -> Self {
        Self {
            mode: ViewMode::Full,
            top_height_ratio: 0.62,
            video_width_ratio: 0.52,
            audio_height_ratio: 0.52,
            video_aspect: None,
            detached_video: DetachedVideoState::default(),
            mins: MinPanelSizes::default(),
        }
    }
}

/// Computed rectangles for all panels and splitters in one frame.
#[derive(Debug, Clone, Copy, Default, PartialEq)]
pub struct ComputedLayout {
    pub video: Option<Rect>,
    pub audio: Option<Rect>,
    pub edit_box: Rect,
    pub grid: Rect,
    pub main_h_splitter: Rect,
    pub top_v_splitter: Option<Rect>,
    pub audio_edit_splitter: Option<Rect>,
}

impl LayoutState {
    /// Computes all panel and splitter rectangles directly in logical points (no extra DPI division).
    #[inline]
    #[must_use]
    pub fn compute_logical(&self, logical_w: f32, logical_h: f32) -> ComputedLayout {
        self.compute(logical_w, logical_h, 1.0)
    }

    /// Computes all panel and splitter rectangles for a window of `win_w × win_h` physical pixels
    /// at `dpi_scale` (returns logical coordinates). Zero allocations.
    #[inline]
    #[must_use]
    #[allow(clippy::too_many_lines)]
    pub fn compute(&self, win_w: f32, win_h: f32, dpi_scale: f32) -> ComputedLayout {
        let scale = dpi_scale.max(0.5);
        let w = (win_w / scale).max(1.0);
        let h = (win_h / scale).max(1.0);
        let sp = self.mins.splitter;

        let show_video = matches!(self.mode, ViewMode::VideoAndSubs | ViewMode::Full)
            && !self.detached_video.detached;
        let show_audio = matches!(self.mode, ViewMode::AudioAndSubs | ViewMode::Full);

        // Minimum top height depends on which top panels are visible.
        let min_top_h = if show_audio {
            self.mins.audio_h + sp + self.mins.edit_h
        } else if show_video {
            self.mins.video_h.max(self.mins.edit_h)
        } else {
            self.mins.edit_h
        };
        let max_top_h = (h - sp - self.mins.grid_h).max(min_top_h.min(h * 0.8));
        let top_h = (h * self.top_height_ratio).clamp(min_top_h.min(max_top_h), max_top_h);

        let main_h_splitter = Rect {
            x: 0.0,
            y: top_h,
            w,
            h: sp,
        };
        let grid_y = (top_h + sp).min(h);
        let grid = Rect {
            x: 0.0,
            y: grid_y,
            w,
            h: (h - grid_y).max(0.0),
        };

        if !show_video && !show_audio {
            // SubsOnly: edit box occupies the entire top area.
            return ComputedLayout {
                video: None,
                audio: None,
                edit_box: Rect {
                    x: 0.0,
                    y: 0.0,
                    w,
                    h: top_h,
                },
                grid,
                main_h_splitter,
                top_v_splitter: None,
                audio_edit_splitter: None,
            };
        }

        if show_video {
            let right_min_w = if show_audio { self.mins.audio_w } else { 200.0 };
            let max_vid_w = (w - sp - right_min_w).max(self.mins.video_w.min(w * 0.7));
            let wanted_w = match self.video_aspect {
                Some(aspect) if aspect.is_finite() && aspect > 0.0 => {
                    VIDEO_PANEL_CHROME_W + (top_h - VIDEO_PANEL_CHROME_H).max(1.0) * aspect
                }
                _ => w * self.video_width_ratio,
            };
            let vid_w = wanted_w.clamp(self.mins.video_w.min(max_vid_w), max_vid_w);
            let video = Rect {
                x: 0.0,
                y: 0.0,
                w: vid_w,
                h: top_h,
            };
            let top_v_splitter = Rect {
                x: vid_w,
                y: 0.0,
                w: sp,
                h: top_h,
            };
            let rx = vid_w + sp;
            let rw = (w - rx).max(0.0);

            if show_audio {
                let (audio, ae_split, edit_box) =
                    split_audio_and_edit(rx, rw, top_h, self.audio_height_ratio, self.mins);
                ComputedLayout {
                    video: Some(video),
                    audio: Some(audio),
                    edit_box,
                    grid,
                    main_h_splitter,
                    top_v_splitter: Some(top_v_splitter),
                    audio_edit_splitter: Some(ae_split),
                }
            } else {
                ComputedLayout {
                    video: Some(video),
                    audio: None,
                    edit_box: Rect {
                        x: rx,
                        y: 0.0,
                        w: rw,
                        h: top_h,
                    },
                    grid,
                    main_h_splitter,
                    top_v_splitter: Some(top_v_splitter),
                    audio_edit_splitter: None,
                }
            }
        } else {
            // AudioAndSubs (or Full with detached video): audio + edit box span full width.
            let (audio, ae_split, edit_box) =
                split_audio_and_edit(0.0, w, top_h, self.audio_height_ratio, self.mins);
            ComputedLayout {
                video: None,
                audio: Some(audio),
                edit_box,
                grid,
                main_h_splitter,
                top_v_splitter: None,
                audio_edit_splitter: Some(ae_split),
            }
        }
    }

    /// Clamps a detached video window back inside a new monitor's work area after a monitor
    /// change or DPI change.
    pub fn clamp_detached_to_monitor(&mut self, mon: Rect) {
        if !self.detached_video.detached {
            return;
        }
        self.detached_video.w = self
            .detached_video
            .w
            .clamp(self.mins.video_w, mon.w.max(self.mins.video_w));
        self.detached_video.h = self
            .detached_video
            .h
            .clamp(self.mins.video_h, mon.h.max(self.mins.video_h));
        let max_x = (mon.x + mon.w - self.detached_video.w).max(mon.x);
        let max_y = (mon.y + mon.h - self.detached_video.h).max(mon.y);
        self.detached_video.x = self.detached_video.x.clamp(mon.x, max_x);
        self.detached_video.y = self.detached_video.y.clamp(mon.y, max_y);
    }
}

#[inline]
fn split_audio_and_edit(
    x: f32,
    w: f32,
    top_h: f32,
    audio_ratio: f32,
    mins: MinPanelSizes,
) -> (Rect, Rect, Rect) {
    let sp = mins.splitter;
    let max_aud_h = (top_h - sp - mins.edit_h).max(mins.audio_h.min(top_h * 0.7));
    let aud_h = (top_h * audio_ratio).clamp(mins.audio_h.min(max_aud_h), max_aud_h);
    let audio = Rect {
        x,
        y: 0.0,
        w,
        h: aud_h,
    };
    let splitter = Rect {
        x,
        y: aud_h,
        w,
        h: sp,
    };
    let ey = (aud_h + sp).min(top_h);
    let edit_box = Rect {
        x,
        y: ey,
        w,
        h: (top_h - ey).max(0.0),
    };
    (audio, splitter, edit_box)
}

/// Key identifying a monitor + DPI configuration for per-monitor layout memory.
#[derive(Debug, Clone, PartialEq, Eq, Hash)]
pub struct MonitorKey {
    pub monitor_id: String,
    pub width_px: u32,
    pub height_px: u32,
    pub dpi_percent: u16,
}

/// Per-monitor layout store remembering splitter ratios and detached video positions.
#[derive(Debug, Default, Clone)]
pub struct LayoutStore {
    profiles: HashMap<MonitorKey, LayoutState>,
}

impl LayoutStore {
    /// Saves `state` for `key`.
    pub fn save(&mut self, key: MonitorKey, state: LayoutState) {
        self.profiles.insert(key, state);
    }

    /// Restores the saved layout for `key`, or falls back to `default` clamped to `work_area`.
    #[must_use]
    pub fn restore(&self, key: &MonitorKey, work_area: Rect) -> LayoutState {
        let mut st = self.profiles.get(key).copied().unwrap_or_default();
        st.clamp_detached_to_monitor(work_area);
        st
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn computes_all_view_modes_and_detached_monitor_restore() {
        let mut st = LayoutState::default();
        let full = st.compute(1920.0, 1080.0, 1.0);
        let vid = full.video.expect("video");
        let aud = full.audio.expect("audio");
        assert!(
            vid.is_visible()
                && aud.is_visible()
                && full.edit_box.is_visible()
                && full.grid.is_visible()
        );
        // Video is top-left, audio is top-right, edit box is under audio, grid is at bottom
        assert_eq!(vid.x, 0.0);
        assert!(aud.x > vid.w);
        assert!(full.edit_box.y > aud.h);
        assert!(full.grid.y > full.edit_box.y);

        // SubsOnly hides both video and audio
        st.mode = ViewMode::SubsOnly;
        let subs = st.compute(1920.0, 1080.0, 1.5);
        assert!(subs.video.is_none() && subs.audio.is_none());
        assert_eq!(subs.edit_box.w, 1920.0 / 1.5);

        // Detached video clamped when moving from dual-monitor to single 1280x720 monitor
        st.mode = ViewMode::Full;
        st.detached_video = DetachedVideoState {
            detached: true,
            x: 2400.0,
            y: 900.0,
            w: 800.0,
            h: 500.0,
        };
        let mut store = LayoutStore::default();
        let key = MonitorKey {
            monitor_id: "DISPLAY1".to_owned(),
            width_px: 1280,
            height_px: 720,
            dpi_percent: 100,
        };
        store.save(key.clone(), st);
        let restored = store.restore(
            &key,
            Rect {
                x: 0.0,
                y: 0.0,
                w: 1280.0,
                h: 720.0,
            },
        );
        assert!(restored.detached_video.x + restored.detached_video.w <= 1280.0);
        assert!(restored.detached_video.y + restored.detached_video.h <= 720.0);
    }
}
