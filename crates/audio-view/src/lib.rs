//! Pure geometry, hit-testing, and per-frame layout builder for the Aegisub 3.5.1 audio display
//! (`src/audio_display.cpp` and `src/audio_box.cpp`).
//!
//! Produces flat, caller-reusable buffers of marker X coordinates, highlight rectangles, karaoke
//! syllable label boxes, colour roles, and waveform peak columns (via [`audio_peaks::WaveformPyramid`])
//! with zero per-frame heap allocations once warmed up.

#![forbid(unsafe_code)]
#![cfg_attr(not(test), deny(clippy::unwrap_used, clippy::expect_used))]

use audio_peaks::{PeakBucket, WaveformPyramid};
use timing_controller::{TimeRangeMs, VideoPositionMarker};

/// Semantic colour roles for the audio display renderer (data only; themed by the UI).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum ColorRole {
    SelectionFill,
    SelectionStartMarker,
    SelectionEndMarker,
    InactiveFill,
    InactiveBoundary,
    KeyframeLine,
    LineBoundary,
    PlayCursor,
    VideoCursor,
    VideoFrameArea,
    KaraokeBoundary,
    KaraokeLabelBg,
    KaraokeLabelActiveBg,
    WaveformPeak,
    WaveformRms,
}

/// Axis-aligned floating-point rectangle in audio-widget local pixels.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct RectF {
    pub x0: f32,
    pub y0: f32,
    pub x1: f32,
    pub y1: f32,
}

impl RectF {
    /// Width clamped to `>= 0.0`.
    #[inline]
    #[must_use]
    pub fn width(self) -> f32 {
        (self.x1 - self.x0).max(0.0)
    }

    /// Returns `true` if `(x, y)` lies inside `[x0, x1] × [y0, y1]`.
    #[inline]
    #[must_use]
    pub fn contains(self, x: f32, y: f32) -> bool {
        x >= self.x0 && x <= self.x1 && y >= self.y0 && y <= self.y1
    }
}

/// Vertical marker line to draw at `x` with a semantic [`ColorRole`].
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct MarkerLine {
    pub x: f32,
    pub time_ms: i64,
    pub role: ColorRole,
}

/// Shaded time-span rectangle (selection, inactive line, or video frame-duration area).
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct SpanRect {
    pub rect: RectF,
    pub role: ColorRole,
}

/// Layout rectangle for one karaoke syllable label box along the top/bottom karaoke strip.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct SyllableLabelRect {
    pub index: usize,
    pub rect: RectF,
    pub role: ColorRole,
    pub visible: bool,
}

/// Hit-test outcome for pointer coordinates `(x, y)` inside the audio display.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum HitTarget {
    StartMarker,
    EndMarker,
    SyllableBoundary(usize),
    SyllableLabel(usize),
    SelectionBody,
    Timeline,
}

/// Aegisub-like horizontal zoom levels (`ms_per_pixel`, from zoomed-in to zoomed-out).
pub const AEGISUB_ZOOM_LEVELS_MS_PER_PX: &[f64] = &[
    0.5, 1.0, 2.0, 3.0, 5.0, 8.0, 10.0, 15.0, 20.0, 30.0, 50.0, 80.0, 120.0,
];

/// Viewport state for the audio timeline (mirrors `AudioDisplay` scroll/zoom state).
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct AudioViewport {
    /// Widget width in logical pixels.
    pub width_px: f32,
    /// Widget height in logical pixels.
    pub height_px: f32,
    /// Height reserved at the top for karaoke syllable labels (0 when not in karaoke mode).
    pub karaoke_strip_height_px: f32,
    /// Left edge timestamp in milliseconds (floating-point for smooth scrolling).
    pub scroll_ms: f64,
    /// Target left edge timestamp for smooth scrolling.
    pub target_scroll_ms: f64,
    /// Horizontal scale: milliseconds per logical pixel (`> 0.0`).
    pub ms_per_px: f64,
    /// Vertical waveform amplitude multiplier (`> 0.0`, default `1.0`).
    pub vertical_zoom: f32,
    /// Total audio duration in milliseconds.
    pub total_duration_ms: i64,
}

impl AudioViewport {
    /// Creates a default viewport (`10.0 ms/px`, `1.0x` vertical zoom).
    #[must_use]
    pub const fn new(width_px: f32, height_px: f32, total_duration_ms: i64) -> Self {
        Self {
            width_px,
            height_px,
            karaoke_strip_height_px: 0.0,
            scroll_ms: 0.0,
            target_scroll_ms: 0.0,
            ms_per_px: 10.0,
            vertical_zoom: 1.0,
            total_duration_ms,
        }
    }

    /// Visible time range `[start_ms, end_ms]` in milliseconds.
    #[inline]
    #[must_use]
    #[allow(clippy::cast_possible_truncation)]
    pub fn visible_range_ms(&self) -> TimeRangeMs {
        let start = self.scroll_ms.floor() as i64;
        let span = (f64::from(self.width_px.max(0.0)) * self.ms_per_px).ceil() as i64;
        TimeRangeMs {
            start_ms: start,
            end_ms: start.saturating_add(span),
        }
    }

    /// Converts a timestamp in milliseconds to widget-local X pixel coordinate.
    #[inline]
    #[must_use]
    #[allow(clippy::cast_precision_loss, clippy::cast_possible_truncation)]
    pub fn ms_to_x(&self, time_ms: i64) -> f32 {
        (((time_ms as f64) - self.scroll_ms) / self.ms_per_px) as f32
    }

    /// Converts a widget-local X pixel coordinate to milliseconds.
    #[inline]
    #[must_use]
    #[allow(clippy::cast_possible_truncation)]
    pub fn x_to_ms(&self, x: f32) -> i64 {
        (self.scroll_ms + f64::from(x) * self.ms_per_px).round() as i64
    }

    /// Sets horizontal zoom by index into [`AEGISUB_ZOOM_LEVELS_MS_PER_PX`], keeping `anchor_x`
    /// anchored to the same timestamp.
    pub fn set_zoom_level(&mut self, level_idx: usize, anchor_x: f32) {
        let idx = level_idx.min(AEGISUB_ZOOM_LEVELS_MS_PER_PX.len() - 1);
        let anchor_ms = self.scroll_ms + f64::from(anchor_x) * self.ms_per_px;
        self.ms_per_px = AEGISUB_ZOOM_LEVELS_MS_PER_PX[idx];
        self.scroll_ms = (anchor_ms - f64::from(anchor_x) * self.ms_per_px).max(0.0);
        self.target_scroll_ms = self.scroll_ms;
    }

    /// Sets smooth scroll target (clamped to valid audio range).
    #[allow(clippy::cast_precision_loss)]
    pub fn scroll_to_ms(&mut self, target_ms: f64, immediate: bool) {
        let max_scroll = ((self.total_duration_ms as f64)
            - f64::from(self.width_px.max(0.0)) * self.ms_per_px)
            .max(0.0);
        let clamped = target_ms.clamp(0.0, max_scroll);
        self.target_scroll_ms = clamped;
        if immediate {
            self.scroll_ms = clamped;
        }
    }

    /// Advances smooth scroll interpolation by `dt_sec`. Returns `true` if still animating.
    pub fn step_smooth_scroll(&mut self, dt_sec: f64) -> bool {
        let diff = self.target_scroll_ms - self.scroll_ms;
        if diff.abs() < 0.25 {
            self.scroll_ms = self.target_scroll_ms;
            return false;
        }
        let alpha = 1.0 - (-18.0 * dt_sec.clamp(0.0, 0.1)).exp();
        self.scroll_ms += diff * alpha;
        true
    }

    /// Auto-scroll rule for playback cursor: pages forward when playhead crosses the right 85%
    /// margin (matching Aegisub `AudioDisplay::UpdatePosition`).
    #[allow(clippy::cast_precision_loss)]
    pub fn auto_scroll_for_play_cursor(&mut self, play_ms: i64) {
        let vis_span = f64::from(self.width_px.max(0.0)) * self.ms_per_px;
        let play_f = play_ms as f64;
        if play_f < self.scroll_ms || play_f > self.scroll_ms + vis_span * 0.85 {
            self.scroll_to_ms(play_f - vis_span * 0.15, false);
        }
    }

    /// Auto-scroll rule when selecting a line: centers or aligns selection start with 15% left
    /// margin if it falls outside the visible viewport.
    #[allow(clippy::cast_precision_loss)]
    pub fn auto_scroll_for_selection(&mut self, sel: TimeRangeMs) {
        let vis_span = f64::from(self.width_px.max(0.0)) * self.ms_per_px;
        let s = sel.start_ms as f64;
        let e = sel.end_ms as f64;
        if s < self.scroll_ms || e > self.scroll_ms + vis_span {
            self.scroll_to_ms(s - vis_span * 0.15, false);
        }
    }
}

/// Pre-indexed sorted timeline markers (rebuilt only when script/keyframes change).
#[derive(Debug, Default, Clone)]
pub struct TimelineMarkerIndex {
    /// Sorted keyframe timestamps in ms.
    pub keyframes_ms: Vec<i64>,
    /// Sorted unique line-boundary timestamps in ms (start & end of all lines).
    pub line_boundaries_ms: Vec<i64>,
}

impl TimelineMarkerIndex {
    /// Builds a sorted index from raw keyframe and line-boundary millisecond arrays.
    #[must_use]
    pub fn new(mut keyframes_ms: Vec<i64>, mut line_boundaries_ms: Vec<i64>) -> Self {
        keyframes_ms.sort_unstable();
        keyframes_ms.dedup();
        line_boundaries_ms.sort_unstable();
        line_boundaries_ms.dedup();
        Self {
            keyframes_ms,
            line_boundaries_ms,
        }
    }
}

/// Inputs for a single frame of the audio display.
#[derive(Debug, Clone, Copy)]
pub struct AudioFrameInput<'a> {
    pub viewport: AudioViewport,
    pub selection: TimeRangeMs,
    pub play_cursor_ms: Option<i64>,
    pub video_marker: Option<VideoPositionMarker>,
    pub inactive_spans: &'a [TimeRangeMs],
    /// Optional karaoke syllable boundary timestamps (`len == syllable_count + 1`).
    pub karaoke_boundaries_ms: &'a [i64],
    pub active_karaoke_syllable: Option<usize>,
    pub markers: &'a TimelineMarkerIndex,
}

/// Caller-reusable output buffer populated by [`build_frame_layout`] with zero per-frame heap
/// allocations once capacities stabilize.
#[derive(Debug, Clone)]
pub struct AudioFrameLayout {
    pub visible_range: TimeRangeMs,
    pub selection_rect: RectF,
    pub spans: Vec<SpanRect>,
    pub markers: Vec<MarkerLine>,
    pub karaoke_labels: Vec<SyllableLabelRect>,
    pub waveform_columns: Vec<PeakBucket>,
}

impl Default for AudioFrameLayout {
    fn default() -> Self {
        Self::with_capacity(0)
    }
}

impl AudioFrameLayout {
    /// Creates a layout buffer with pre-reserved capacities suitable for a 2,000-px audio display.
    #[must_use]
    pub fn with_capacity(width_px: usize) -> Self {
        Self {
            visible_range: TimeRangeMs {
                start_ms: 0,
                end_ms: 0,
            },
            selection_rect: RectF {
                x0: 0.0,
                y0: 0.0,
                x1: 0.0,
                y1: 0.0,
            },
            spans: Vec::with_capacity(32),
            markers: Vec::with_capacity(512),
            karaoke_labels: Vec::with_capacity(64),
            waveform_columns: Vec::with_capacity(width_px),
        }
    }

    /// Hit-tests pointer coordinates `(x, y)` against this frame's computed geometry.
    #[must_use]
    pub fn hit_test(
        &self,
        x: f32,
        y: f32,
        hit_radius_px: f32,
        viewport: &AudioViewport,
        selection: TimeRangeMs,
        karaoke_boundaries_ms: &[i64],
    ) -> HitTarget {
        // 1. Karaoke syllable labels (if inside top karaoke strip)
        if y <= viewport.karaoke_strip_height_px {
            for lbl in &self.karaoke_labels {
                if lbl.visible && lbl.rect.contains(x, y) {
                    return HitTarget::SyllableLabel(lbl.index);
                }
            }
        }

        // 2. Active start/end markers
        let sx = viewport.ms_to_x(selection.start_ms);
        let ex = viewport.ms_to_x(selection.end_ms);
        if (x - sx).abs() <= hit_radius_px {
            return HitTarget::StartMarker;
        }
        if (x - ex).abs() <= hit_radius_px {
            return HitTarget::EndMarker;
        }

        // 3. Internal karaoke boundaries
        if karaoke_boundaries_ms.len() > 2 {
            for (b_idx, &t_ms) in karaoke_boundaries_ms
                .iter()
                .enumerate()
                .take(karaoke_boundaries_ms.len() - 1)
                .skip(1)
            {
                let bx = viewport.ms_to_x(t_ms);
                if (x - bx).abs() <= hit_radius_px {
                    return HitTarget::SyllableBoundary(b_idx);
                }
            }
        }

        // 4. Selection body vs timeline background
        if self.selection_rect.contains(x, y) {
            HitTarget::SelectionBody
        } else {
            HitTarget::Timeline
        }
    }
}

/// Computes all visible markers, highlight spans, video frame-duration area, and karaoke syllable
/// label rectangles for one audio-display frame into `out` (zero heap allocations on reuse).
///
/// Uses O(log N) binary search over `input.markers` so 20,000 lines and 3,000 keyframes take only
/// a few microseconds per frame.
pub fn build_frame_layout(
    input: &AudioFrameInput<'_>,
    pyramid: Option<&WaveformPyramid>,
    out: &mut AudioFrameLayout,
) {
    let vp = input.viewport;
    let vis = vp.visible_range_ms();
    out.visible_range = vis;
    out.spans.clear();
    out.markers.clear();
    out.karaoke_labels.clear();

    let body_y0 = vp.karaoke_strip_height_px;
    let body_y1 = vp.height_px;

    // 1. Inactive line highlight spans (clamped to visible range)
    for &sp in input.inactive_spans {
        if sp.end_ms >= vis.start_ms && sp.start_ms <= vis.end_ms {
            let x0 = vp.ms_to_x(sp.start_ms).clamp(0.0, vp.width_px);
            let x1 = vp.ms_to_x(sp.end_ms).clamp(0.0, vp.width_px);
            out.spans.push(SpanRect {
                rect: RectF {
                    x0,
                    y0: body_y0,
                    x1,
                    y1: body_y1,
                },
                role: ColorRole::InactiveFill,
            });
        }
    }

    // 2. Active selection span + start/end markers
    let sel_x0 = vp.ms_to_x(input.selection.start_ms);
    let sel_x1 = vp.ms_to_x(input.selection.end_ms);
    out.selection_rect = RectF {
        x0: sel_x0.clamp(0.0, vp.width_px),
        y0: body_y0,
        x1: sel_x1.clamp(0.0, vp.width_px),
        y1: body_y1,
    };
    if input.selection.end_ms >= vis.start_ms && input.selection.start_ms <= vis.end_ms {
        out.spans.push(SpanRect {
            rect: out.selection_rect,
            role: ColorRole::SelectionFill,
        });
    }
    if input.selection.start_ms >= vis.start_ms && input.selection.start_ms <= vis.end_ms {
        out.markers.push(MarkerLine {
            x: sel_x0,
            time_ms: input.selection.start_ms,
            role: ColorRole::SelectionStartMarker,
        });
    }
    if input.selection.end_ms >= vis.start_ms && input.selection.end_ms <= vis.end_ms {
        out.markers.push(MarkerLine {
            x: sel_x1,
            time_ms: input.selection.end_ms,
            role: ColorRole::SelectionEndMarker,
        });
    }

    if let Some(vm) = input.video_marker {
        push_video_marker(vm, vis, &vp, body_y0, body_y1, out);
    }

    push_visible_markers(
        &input.markers.keyframes_ms,
        vis,
        &vp,
        ColorRole::KeyframeLine,
        &mut out.markers,
    );
    push_visible_markers(
        &input.markers.line_boundaries_ms,
        vis,
        &vp,
        ColorRole::LineBoundary,
        &mut out.markers,
    );
    push_karaoke_layout(input, vis, &vp, out);

    // 7. Play cursor
    if let Some(play_ms) = input.play_cursor_ms {
        if play_ms >= vis.start_ms && play_ms <= vis.end_ms {
            out.markers.push(MarkerLine {
                x: vp.ms_to_x(play_ms),
                time_ms: play_ms,
                role: ColorRole::PlayCursor,
            });
        }
    }

    // 8. Optional waveform peak query via `audio-peaks`
    if let Some(pyr) = pyramid {
        #[allow(clippy::cast_possible_truncation, clippy::cast_sign_loss)]
        let num_cols = vp.width_px.max(0.0).round() as usize;
        let sr = u64::from(pyr.sample_rate());
        let f0 = u64::try_from(vis.start_ms.max(0)).unwrap_or(0) * sr / 1000;
        let f1 = u64::try_from(vis.end_ms.max(0)).unwrap_or(0) * sr / 1000;
        pyr.query_columns(f0, f1, num_cols, &mut out.waveform_columns);
    }
}

fn push_video_marker(
    vm: VideoPositionMarker,
    vis: TimeRangeMs,
    vp: &AudioViewport,
    body_y0: f32,
    body_y1: f32,
    out: &mut AudioFrameLayout,
) {
    if vm.frame_end_ms >= vis.start_ms && vm.frame_start_ms <= vis.end_ms {
        let fx0 = vp.ms_to_x(vm.frame_start_ms).clamp(0.0, vp.width_px);
        let fx1 = vp.ms_to_x(vm.frame_end_ms).clamp(0.0, vp.width_px);
        out.spans.push(SpanRect {
            rect: RectF {
                x0: fx0,
                y0: body_y0,
                x1: fx1,
                y1: body_y1,
            },
            role: ColorRole::VideoFrameArea,
        });
    }
    if vm.position_ms >= vis.start_ms && vm.position_ms <= vis.end_ms {
        out.markers.push(MarkerLine {
            x: vp.ms_to_x(vm.position_ms),
            time_ms: vm.position_ms,
            role: ColorRole::VideoCursor,
        });
    }
}

fn push_visible_markers(
    sorted_ms: &[i64],
    vis: TimeRangeMs,
    vp: &AudioViewport,
    role: ColorRole,
    out: &mut Vec<MarkerLine>,
) {
    let lo = sorted_ms.partition_point(|&t| t < vis.start_ms);
    for &t in &sorted_ms[lo..] {
        if t > vis.end_ms {
            break;
        }
        out.push(MarkerLine {
            x: vp.ms_to_x(t),
            time_ms: t,
            role,
        });
    }
}

fn push_karaoke_layout(
    input: &AudioFrameInput<'_>,
    vis: TimeRangeMs,
    vp: &AudioViewport,
    out: &mut AudioFrameLayout,
) {
    if input.karaoke_boundaries_ms.len() < 2 {
        return;
    }
    let syl_count = input.karaoke_boundaries_ms.len() - 1;
    for i in 0..syl_count {
        let s_ms = input.karaoke_boundaries_ms[i];
        let e_ms = input.karaoke_boundaries_ms[i + 1];
        if i > 0 && s_ms >= vis.start_ms && s_ms <= vis.end_ms {
            out.markers.push(MarkerLine {
                x: vp.ms_to_x(s_ms),
                time_ms: s_ms,
                role: ColorRole::KaraokeBoundary,
            });
        }
        if e_ms >= vis.start_ms && s_ms <= vis.end_ms {
            let x0 = vp.ms_to_x(s_ms).clamp(0.0, vp.width_px);
            let x1 = vp.ms_to_x(e_ms).clamp(0.0, vp.width_px);
            let role = if input.active_karaoke_syllable == Some(i) {
                ColorRole::KaraokeLabelActiveBg
            } else {
                ColorRole::KaraokeLabelBg
            };
            out.karaoke_labels.push(SyllableLabelRect {
                index: i,
                rect: RectF {
                    x0,
                    y0: 0.0,
                    x1,
                    y1: vp.karaoke_strip_height_px,
                },
                role,
                visible: true,
            });
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn viewport_conversions_zoom_scroll_and_hit_test() {
        let mut vp = AudioViewport::new(1000.0, 200.0, 60_000);
        vp.karaoke_strip_height_px = 24.0;
        assert_eq!(vp.ms_to_x(2500), 250.0);
        assert_eq!(vp.x_to_ms(250.0), 2500);

        vp.set_zoom_level(2, 250.0); // 2.0 ms/px anchored at 2500 ms
        assert!((vp.ms_to_x(2500) - 250.0).abs() < 0.01);

        let markers = TimelineMarkerIndex::new(vec![2200, 2800], vec![2000, 3000]);
        let sel = TimeRangeMs {
            start_ms: 2200,
            end_ms: 2800,
        };
        let kara = [2200, 2500, 2800];
        let input = AudioFrameInput {
            viewport: vp,
            selection: sel,
            play_cursor_ms: Some(2500),
            video_marker: Some(VideoPositionMarker {
                position_ms: 2400,
                frame_start_ms: 2380,
                frame_end_ms: 2422,
            }),
            inactive_spans: &[],
            karaoke_boundaries_ms: &kara,
            active_karaoke_syllable: Some(0),
            markers: &markers,
        };

        let mut layout = AudioFrameLayout::with_capacity(1000);
        build_frame_layout(&input, None, &mut layout);
        assert_eq!(layout.karaoke_labels.len(), 2);
        assert!(layout
            .markers
            .iter()
            .any(|m| m.role == ColorRole::KeyframeLine));
        assert!(layout
            .spans
            .iter()
            .any(|s| s.role == ColorRole::VideoFrameArea));

        let sx = vp.ms_to_x(2200);
        assert_eq!(
            layout.hit_test(sx + 1.0, 100.0, 4.0, &vp, sel, &kara),
            HitTarget::StartMarker
        );
        let kx = vp.ms_to_x(2500);
        assert_eq!(
            layout.hit_test(kx, 100.0, 4.0, &vp, sel, &kara),
            HitTarget::SyllableBoundary(1)
        );
        assert_eq!(
            layout.hit_test(vp.ms_to_x(2300), 10.0, 4.0, &vp, sel, &kara),
            HitTarget::SyllableLabel(0)
        );
    }
}
