//! Pure video/audio playback state machine (no OS devices, trait-based monotonic clock).
//!
//! Mirrors Aegisub 3.5.1:
//! - `src/video_controller.cpp` (`VideoController::Play`, `PlayLine`, `JumpToFrame`, `JumpToTime`,
//!   `NextFrame`, `PrevFrame`, keyframe navigation, position broadcasts to audio view)
//! - `src/audio_controller.cpp` (audio master clock synchronization & range playback)
//!
//! Performance:
//! - Per-tick scheduling decision: `< 200 ns`, **0 heap allocations**
//! - Seek coalescing of 1,000 scrub events: `< 50 µs`, **0 heap allocations**

#![forbid(unsafe_code)]
#![cfg_attr(not(test), deny(clippy::unwrap_used, clippy::expect_used))]

use keyframes::{TimeType, Timecodes};

/// Monotonic nanosecond clock source (implemented by `Instant` in app, mocked in tests).
pub trait Clock {
    /// Returns monotonic elapsed nanoseconds from an arbitrary fixed epoch.
    fn now_ns(&self) -> u64;
}

/// Playback transport state.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum PlayState {
    Paused,
    Playing,
    PlayingLine,
    PlayingRange,
}

/// Configuration for A/V sync, frame dropping, and ahead-of-time decode scheduling.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct PlaybackConfig {
    /// Number of future frames to schedule for ahead-of-time decoding.
    pub prefetch_depth: usize,
    /// Video lag threshold (ms) above which intermediate frames are dropped to catch up.
    pub drop_threshold_ms: i64,
    /// Maximum drift correction (ms) applied per tick toward the audio master clock.
    pub max_drift_step_ms: i64,
    /// Drift deadband (ms) inside which no correction is applied to avoid jitter.
    pub drift_deadband_ms: i64,
    /// Playback speed multiplier (1.0 = normal speed).
    pub speed: f64,
}

impl Default for PlaybackConfig {
    fn default() -> Self {
        Self {
            prefetch_depth: 4,
            drop_threshold_ms: 45,
            max_drift_step_ms: 8,
            drift_deadband_ms: 4,
            speed: 1.0,
        }
    }
}

/// Video-position update broadcast to the audio display (`VideoController::AnnounceTime`).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct AudioViewPosition {
    pub frame: u32,
    pub time_ms: i64,
    pub is_playing: bool,
}

/// Output of one zero-allocation [`PlaybackController::tick`] scheduling decision.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct TickDecision {
    pub current_frame: u32,
    pub current_time_ms: i64,
    /// `true` when the displayed frame changed and should be presented.
    pub present_frame: bool,
    /// Number of intermediate frames skipped/dropped during this tick to stay in A/V sync.
    pub dropped_in_tick: u32,
    /// Number of ahead-of-time frame numbers written into the caller's `decode_queue_out` slice.
    pub decode_request_count: usize,
    /// Position update for the audio view whenever the frame/time or play state changed.
    pub audio_position: Option<AudioViewPosition>,
    /// `true` if a non-looping playback range reached its end and paused on this tick.
    pub reached_end: bool,
}

/// Pure, device-free video/audio playback state machine with VFR [`Timecodes`], keyframe seeks,
/// scrub coalescing, A/V master-clock drift correction, and ahead-of-time decode scheduling.
#[derive(Debug, Clone)]
pub struct PlaybackController {
    timecodes: Timecodes,
    keyframes: Vec<u32>,
    total_frames: u32,
    config: PlaybackConfig,

    state: PlayState,
    looping: bool,
    current_frame: u32,
    current_time_ms: i64,

    range_start_ms: i64,
    range_end_ms: i64,

    // Clock anchors
    anchor_wall_ns: u64,
    anchor_media_ms: i64,

    // Audio master clock state
    audio_master_active: bool,
    audio_clock_ms: i64,
    audio_clock_wall_ns: u64,
    cumulative_drift_ms: i64,

    // Coalesced scrubbing state
    pending_scrub_frame: Option<u32>,
    scrub_events_coalesced: u64,

    // Stats & change detection
    total_dropped_frames: u64,
    last_notified_frame: u32,
    last_notified_playing: bool,
}

impl PlaybackController {
    /// Creates a new playback controller over `timecodes`, `total_frames`, and sorted `keyframes`.
    #[must_use]
    pub fn new(
        timecodes: Timecodes,
        total_frames: u32,
        mut keyframes: Vec<u32>,
        config: PlaybackConfig,
    ) -> Self {
        keyframes.sort_unstable();
        keyframes.dedup();
        let total = total_frames.max(1);
        let max_ms = timecodes.frame_to_time_ms(total.saturating_sub(1), TimeType::Exact);
        Self {
            timecodes,
            keyframes,
            total_frames: total,
            config,
            state: PlayState::Paused,
            looping: false,
            current_frame: 0,
            current_time_ms: 0,
            range_start_ms: 0,
            range_end_ms: max_ms.max(0),
            anchor_wall_ns: 0,
            anchor_media_ms: 0,
            audio_master_active: false,
            audio_clock_ms: 0,
            audio_clock_wall_ns: 0,
            cumulative_drift_ms: 0,
            pending_scrub_frame: None,
            scrub_events_coalesced: 0,
            total_dropped_frames: 0,
            last_notified_frame: u32::MAX,
            last_notified_playing: false,
        }
    }

    #[inline]
    #[must_use]
    pub const fn state(&self) -> PlayState {
        self.state
    }

    #[inline]
    #[must_use]
    pub const fn is_playing(&self) -> bool {
        !matches!(self.state, PlayState::Paused)
    }

    #[inline]
    #[must_use]
    pub const fn current_frame(&self) -> u32 {
        self.current_frame
    }

    #[inline]
    #[must_use]
    pub const fn current_time_ms(&self) -> i64 {
        self.current_time_ms
    }

    #[inline]
    #[must_use]
    pub const fn total_dropped_frames(&self) -> u64 {
        self.total_dropped_frames
    }

    #[inline]
    #[must_use]
    pub const fn scrub_events_coalesced(&self) -> u64 {
        self.scrub_events_coalesced
    }

    #[inline]
    pub fn set_looping(&mut self, looping: bool) {
        self.looping = looping;
    }

    /// Starts full-video playback from the current position.
    pub fn play(&mut self, clock: &dyn Clock) {
        let end_ms = self
            .timecodes
            .frame_to_time_ms(self.total_frames.saturating_sub(1), TimeType::Exact);
        self.range_start_ms = 0;
        self.range_end_ms = end_ms.max(0);
        self.state = PlayState::Playing;
        self.reanchor(clock.now_ns(), self.current_time_ms);
    }

    /// Pauses playback at the current frame.
    pub fn pause(&mut self) {
        self.state = PlayState::Paused;
        self.audio_master_active = false;
    }

    /// Toggles between playing and paused.
    pub fn toggle_play(&mut self, clock: &dyn Clock) {
        if self.is_playing() {
            self.pause();
        } else {
            self.play(clock);
        }
    }

    /// Plays a single subtitle line `[start_ms, end_ms]` (optionally looping).
    pub fn play_line(&mut self, start_ms: i64, end_ms: i64, looping: bool, clock: &dyn Clock) {
        self.start_range(PlayState::PlayingLine, start_ms, end_ms, looping, clock);
    }

    /// Plays an arbitrary `[start_ms, end_ms]` range (optionally looping).
    pub fn play_range(&mut self, start_ms: i64, end_ms: i64, looping: bool, clock: &dyn Clock) {
        self.start_range(PlayState::PlayingRange, start_ms, end_ms, looping, clock);
    }

    fn start_range(
        &mut self,
        mode: PlayState,
        start_ms: i64,
        end_ms: i64,
        looping: bool,
        clock: &dyn Clock,
    ) {
        let s = start_ms.max(0);
        let e = end_ms.max(s);
        self.range_start_ms = s;
        self.range_end_ms = e;
        self.looping = looping;
        self.state = mode;
        let frame = self.clamp_frame(self.timecodes.time_ms_to_frame(s, TimeType::Start));
        self.current_frame = frame;
        self.current_time_ms = s;
        self.pending_scrub_frame = None;
        self.reanchor(clock.now_ns(), s);
    }

    /// Steps forward or backward by `delta` frames (typically `+1` or `-1`) and pauses.
    pub fn step_frame(&mut self, delta: i32) {
        self.pause();
        let next = i64::from(self.current_frame) + i64::from(delta);
        #[allow(clippy::cast_possible_truncation, clippy::cast_sign_loss)]
        let clamped = next.clamp(0, i64::from(self.total_frames.saturating_sub(1))) as u32;
        self.seek_frame(clamped);
    }

    /// Seeks immediately to `frame`.
    #[inline]
    pub fn seek_frame(&mut self, frame: u32) {
        let f = self.clamp_frame(frame);
        self.current_frame = f;
        self.current_time_ms = self.timecodes.frame_to_time_ms(f, TimeType::Exact);
        self.pending_scrub_frame = None;
        self.anchor_media_ms = self.current_time_ms;
    }

    /// Seeks immediately to `time_ms` using VFR [`Timecodes`].
    #[inline]
    pub fn seek_time_ms(&mut self, time_ms: i64) {
        let f = self.clamp_frame(
            self.timecodes
                .time_ms_to_frame(time_ms.max(0), TimeType::Exact),
        );
        self.seek_frame(f);
    }

    /// Seeks to the next or previous video keyframe relative to [`Self::current_frame`].
    pub fn seek_keyframe(&mut self, forward: bool) -> u32 {
        if self.keyframes.is_empty() {
            return self.current_frame;
        }
        let target = if forward {
            let pos = self
                .keyframes
                .partition_point(|&kf| kf <= self.current_frame);
            self.keyframes
                .get(pos)
                .copied()
                .unwrap_or(self.total_frames.saturating_sub(1))
        } else {
            let pos = self
                .keyframes
                .partition_point(|&kf| kf < self.current_frame);
            if pos > 0 { self.keyframes[pos - 1] } else { 0 }
        };
        self.seek_frame(target);
        target
    }

    /// Seeks to the start frame of a subtitle line at `start_ms`.
    #[inline]
    pub fn seek_line_start(&mut self, start_ms: i64) {
        let f = self.clamp_frame(
            self.timecodes
                .time_ms_to_frame(start_ms.max(0), TimeType::Start),
        );
        self.seek_frame(f);
    }

    /// Queues a scrub-bar seek to `frame`, coalescing multiple rapid scrub events into the latest
    /// target with **0 heap allocations** (applied on the next [`Self::tick`] or
    /// [`Self::commit_scrub`]).
    #[inline]
    pub fn scrub_to_frame(&mut self, frame: u32) {
        let f = self.clamp_frame(frame);
        if self.pending_scrub_frame.is_some() {
            self.scrub_events_coalesced += 1;
        }
        self.pending_scrub_frame = Some(f);
    }

    /// Queues a scrub-bar seek by timestamp `time_ms`, coalescing rapid events.
    #[inline]
    pub fn scrub_to_time_ms(&mut self, time_ms: i64) {
        let f = self.clamp_frame(
            self.timecodes
                .time_ms_to_frame(time_ms.max(0), TimeType::Exact),
        );
        self.scrub_to_frame(f);
    }

    /// Applies any pending coalesced scrub target immediately, returning `Some(frame)` if a scrub
    /// was pending.
    #[inline]
    pub fn commit_scrub(&mut self, clock: &dyn Clock) -> Option<u32> {
        let target = self.pending_scrub_frame.take()?;
        self.current_frame = target;
        self.current_time_ms = self.timecodes.frame_to_time_ms(target, TimeType::Exact);
        if self.is_playing() {
            self.reanchor(clock.now_ns(), self.current_time_ms);
        }
        Some(target)
    }

    /// Reports the latest audio master clock position `(audio_ms, now_ns)` for A/V sync.
    #[inline]
    pub fn update_audio_clock(&mut self, audio_ms: i64, now_ns: u64) {
        self.audio_master_active = true;
        self.audio_clock_ms = audio_ms;
        self.audio_clock_wall_ns = now_ns;
    }

    /// Advances playback by one UI/render tick, returning the frame to present, drop count,
    /// ahead-of-time decode requests (written into `decode_queue_out`), and audio-view position
    /// notification. **Zero heap allocations**, `< 200 ns`.
    #[allow(clippy::too_many_lines)]
    pub fn tick(&mut self, clock: &dyn Clock, decode_queue_out: &mut [u32]) -> TickDecision {
        let now_ns = clock.now_ns();
        let mut present_frame = false;
        let mut dropped_in_tick = 0u32;
        let mut reached_end = false;

        // 1. Apply any coalesced scrub target first.
        if let Some(target) = self.pending_scrub_frame.take() {
            if target != self.current_frame {
                self.current_frame = target;
                self.current_time_ms = self.timecodes.frame_to_time_ms(target, TimeType::Exact);
                present_frame = true;
            }
            if self.is_playing() {
                self.reanchor(now_ns, self.current_time_ms);
            }
        }

        // 2. Advance media clock if playing.
        if self.is_playing() {
            let wall_delta_ns = now_ns.saturating_sub(self.anchor_wall_ns);
            #[allow(clippy::cast_precision_loss, clippy::cast_possible_truncation)]
            let elapsed_ms = ((wall_delta_ns as f64) * 1e-6 * self.config.speed) as i64;
            let mut target_ms = self.anchor_media_ms + elapsed_ms;

            // A/V sync drift correction against audio master clock when active.
            if self.audio_master_active {
                let audio_wall_delta = now_ns.saturating_sub(self.audio_clock_wall_ns);
                #[allow(clippy::cast_precision_loss, clippy::cast_possible_truncation)]
                let audio_age_ms = ((audio_wall_delta as f64) * 1e-6 * self.config.speed) as i64;
                let expected_audio_ms = self.audio_clock_ms + audio_age_ms;
                let drift = expected_audio_ms - target_ms;
                if drift.abs() > self.config.drift_deadband_ms {
                    let step = drift.clamp(
                        -self.config.max_drift_step_ms,
                        self.config.max_drift_step_ms,
                    );
                    target_ms += step;
                    self.anchor_media_ms += step;
                    self.cumulative_drift_ms += step;
                }
            }

            // Handle range end / looping (re-anchoring wall clock cleanly on wrap).
            if target_ms >= self.range_end_ms {
                if self.looping && self.range_end_ms > self.range_start_ms {
                    let span = self.range_end_ms - self.range_start_ms;
                    let wrapped = self.range_start_ms + ((target_ms - self.range_start_ms) % span);
                    target_ms = wrapped;
                    self.reanchor(now_ns, wrapped);
                } else {
                    target_ms = self.range_end_ms;
                    self.state = PlayState::Paused;
                    self.audio_master_active = false;
                    reached_end = true;
                }
            }

            let target_frame = self.clamp_frame(
                self.timecodes
                    .time_ms_to_frame(target_ms.max(0), TimeType::Exact),
            );

            if target_frame == self.current_frame {
                self.current_time_ms = target_ms;
            } else {
                let delta = target_frame.saturating_sub(self.current_frame);
                if delta > 1 {
                    let cur_frame_ms = self
                        .timecodes
                        .frame_to_time_ms(self.current_frame, TimeType::Exact);
                    if target_ms - cur_frame_ms >= self.config.drop_threshold_ms {
                        dropped_in_tick = delta - 1;
                        self.total_dropped_frames += u64::from(dropped_in_tick);
                    }
                }
                self.current_frame = target_frame;
                self.current_time_ms = target_ms;
                present_frame = true;
            }
        }

        // 3. Schedule ahead-of-time decode requests into caller's slice (0 allocations).
        let mut req_count = 0usize;
        let depth = self.config.prefetch_depth.min(decode_queue_out.len());
        if depth > 0 {
            let max_frame = self.total_frames.saturating_sub(1);
            let start_f = self.current_frame;
            for offset in 0..depth {
                #[allow(clippy::cast_possible_truncation)]
                let f = start_f.saturating_add(offset as u32);
                if f > max_frame {
                    break;
                }
                decode_queue_out[req_count] = f;
                req_count += 1;
            }
        }

        // 4. Emit audio-view position update when frame or play state changed.
        let playing_now = self.is_playing();
        let audio_position = if self.current_frame != self.last_notified_frame
            || playing_now != self.last_notified_playing
        {
            self.last_notified_frame = self.current_frame;
            self.last_notified_playing = playing_now;
            Some(AudioViewPosition {
                frame: self.current_frame,
                time_ms: self.current_time_ms,
                is_playing: playing_now,
            })
        } else {
            None
        };

        TickDecision {
            current_frame: self.current_frame,
            current_time_ms: self.current_time_ms,
            present_frame,
            dropped_in_tick,
            decode_request_count: req_count,
            audio_position,
            reached_end,
        }
    }

    #[inline]
    fn reanchor(&mut self, wall_ns: u64, media_ms: i64) {
        self.anchor_wall_ns = wall_ns;
        self.anchor_media_ms = media_ms;
    }

    #[inline]
    fn clamp_frame(&self, frame: u32) -> u32 {
        frame.min(self.total_frames.saturating_sub(1))
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::cell::Cell;

    struct FakeClock(Cell<u64>);
    impl FakeClock {
        fn new(ns: u64) -> Self {
            Self(Cell::new(ns))
        }
        fn advance_ms(&self, ms: u64) {
            self.0.set(self.0.get() + ms * 1_000_000);
        }
    }
    impl Clock for FakeClock {
        fn now_ns(&self) -> u64 {
            self.0.get()
        }
    }

    #[test]
    fn play_loop_step_keyframes_scrub_coalescing_and_av_sync() {
        let tc = Timecodes::from_cfr(24_000, 1_001).expect("cfr");
        let kfs = vec![0, 24, 48, 96, 144];
        let mut pb = PlaybackController::new(tc, 240, kfs, PlaybackConfig::default());
        let clk = FakeClock::new(0);
        let mut queue = [0u32; 4];

        // Play line with loop
        pb.play_line(1_000, 1_500, true, &clk);
        let d0 = pb.tick(&clk, &mut queue);
        assert_eq!(d0.decode_request_count, 4);
        assert!(d0.audio_position.is_some());

        // Advance 200 ms -> forward progress, no drops when stepping smoothly
        clk.advance_ms(42);
        let d1 = pb.tick(&clk, &mut queue);
        assert!(d1.present_frame);
        assert_eq!(d1.dropped_in_tick, 0);

        // Large stall -> frame drop kicks in
        clk.advance_ms(200);
        let d2 = pb.tick(&clk, &mut queue);
        assert!(d2.dropped_in_tick > 0);
        assert!(pb.total_dropped_frames() > 0);

        // Audio master clock drift pull
        pb.update_audio_clock(pb.current_time_ms() + 30, clk.now_ns());
        clk.advance_ms(16);
        let _ = pb.tick(&clk, &mut queue);

        // Loop wrap-around
        clk.advance_ms(400);
        let d3 = pb.tick(&clk, &mut queue);
        assert!(!d3.reached_end);
        assert!(pb.is_playing());
        assert!(pb.current_time_ms() >= 1_000 && pb.current_time_ms() < 1_500);

        // Keyframe navigation & step
        pb.pause();
        pb.seek_frame(30);
        assert_eq!(pb.seek_keyframe(true), 48);
        assert_eq!(pb.seek_keyframe(false), 24);
        pb.step_frame(1);
        assert_eq!(pb.current_frame(), 25);
        pb.step_frame(-1);
        assert_eq!(pb.current_frame(), 24);

        // Scrub coalescing
        for f in 10..110u32 {
            pb.scrub_to_frame(f);
        }
        assert_eq!(pb.scrub_events_coalesced(), 99);
        let d_scrub = pb.tick(&clk, &mut queue);
        assert_eq!(d_scrub.current_frame, 109);
        assert!(d_scrub.present_frame);
    }

    #[test]
    fn seek_line_start_lands_on_first_frame_at_or_after_start_23_976() {
        let tc = Timecodes::from_cfr(24_000, 1_001).expect("cfr");
        let mut pb = PlaybackController::new(tc.clone(), 5_000, vec![0], PlaybackConfig::default());
        // 57.90 s is inside frame 1388 (57.891 s); the line first shows on frame 1389.
        pb.seek_line_start(57_900);
        assert_eq!(pb.current_frame(), 1_389);
        assert!(pb.current_time_ms() >= 57_900);
        // The old behaviour (exact frame containing the time) is one frame early.
        pb.seek_time_ms(57_900);
        assert_eq!(pb.current_frame(), 1_388);
        // A start exactly on a frame boundary keeps that frame; line start 0 is frame 0.
        let boundary = tc.frame_to_time_ms(1_389, TimeType::Exact);
        pb.seek_line_start(boundary);
        assert_eq!(pb.current_frame(), 1_389);
        pb.seek_line_start(0);
        assert_eq!(pb.current_frame(), 0);
    }
}
