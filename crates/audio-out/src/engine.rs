//! Device-independent render engine: play-range control, sample-rate conversion, channel mapping,
//! volume and the latency-compensated playback clock.
//!
//! The control side ([`Shared`]) is lock-free: the UI thread writes commands into atomics and the
//! audio callback picks them up at the start of its next buffer. The clock is published by the
//! callback through a single-writer seqlock.

use crate::source::PcmSource;
use rubato::audioadapter_buffers::direct::InterleavedSlice;
use rubato::{Fft, FixedSync, Resampler};
use std::sync::atomic::{fence, AtomicBool, AtomicU32, AtomicU64, Ordering};
use std::sync::Arc;
use std::time::Instant;

/// Source frames fed to the resampler per chunk (≈ 21 ms at 48 kHz).
pub(crate) const RESAMPLER_CHUNK: usize = 1024;

/// Lock-free state shared between the control thread and the audio callback.
pub(crate) struct Shared {
    base: Instant,
    cmd_seq: AtomicU64,
    cmd_start: AtomicU64,
    cmd_end: AtomicU64,
    cmd_play: AtomicBool,
    volume_bits: AtomicU32,
    // Clock seqlock (single writer: the audio callback).
    clk_version: AtomicU64,
    clk_seq: AtomicU64,
    clk_start: AtomicU64,
    clk_emitted: AtomicU64,
    clk_buffer: AtomicU64,
    clk_target: AtomicU64,
    clk_cb_ns: AtomicU64,
    clk_latency_ns: AtomicU64,
    clk_playing: AtomicBool,
    // Callback statistics.
    pub(crate) cb_count: AtomicU64,
    pub(crate) cb_busy_ns: AtomicU64,
    pub(crate) cb_max_ns: AtomicU64,
    pub(crate) cb_frames: AtomicU64,
}

/// Consistent copy of the clock published by the last audio callback.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub(crate) struct ClockSnapshot {
    pub seq: u64,
    pub start: u64,
    /// Device frames emitted since the range started (incl. trailing silence) before the\n    /// latest buffer.
    pub emitted: u64,
    /// Device frames of the range written into the latest buffer.
    pub buffer: u64,
    /// Device frames the whole range produces.
    pub target: u64,
    /// Time of the latest callback (ns since [`Shared::base`]).
    pub cb_ns: u64,
    /// Time from that callback until its first frame is audible.
    pub latency_ns: u64,
    pub playing: bool,
}

impl Shared {
    pub(crate) fn new() -> Self {
        Self {
            base: Instant::now(),
            cmd_seq: AtomicU64::new(0),
            cmd_start: AtomicU64::new(0),
            cmd_end: AtomicU64::new(0),
            cmd_play: AtomicBool::new(false),
            volume_bits: AtomicU32::new(1.0f32.to_bits()),
            clk_version: AtomicU64::new(0),
            clk_seq: AtomicU64::new(0),
            clk_start: AtomicU64::new(0),
            clk_emitted: AtomicU64::new(0),
            clk_buffer: AtomicU64::new(0),
            clk_target: AtomicU64::new(0),
            clk_cb_ns: AtomicU64::new(0),
            clk_latency_ns: AtomicU64::new(0),
            clk_playing: AtomicBool::new(false),
            cb_count: AtomicU64::new(0),
            cb_busy_ns: AtomicU64::new(0),
            cb_max_ns: AtomicU64::new(0),
            cb_frames: AtomicU64::new(0),
        }
    }

    /// Nanoseconds since this state was created (monotonic).
    pub(crate) fn now_ns(&self) -> u64 {
        u64::try_from(self.base.elapsed().as_nanos()).unwrap_or(u64::MAX)
    }

    /// Requests playback of source frames `[start, end)`; returns the command sequence number.
    pub(crate) fn play(&self, start: u64, end: u64) -> u64 {
        self.cmd_start.store(start, Ordering::Relaxed);
        self.cmd_end.store(end.max(start), Ordering::Relaxed);
        self.cmd_play.store(true, Ordering::Relaxed);
        self.cmd_seq.fetch_add(1, Ordering::Release) + 1
    }

    /// Requests a stop; returns the command sequence number.
    pub(crate) fn stop(&self) -> u64 {
        self.cmd_play.store(false, Ordering::Relaxed);
        self.cmd_seq.fetch_add(1, Ordering::Release) + 1
    }

    /// Moves the end of the current range without restarting playback.
    pub(crate) fn set_end(&self, end: u64) {
        let start = self.cmd_start.load(Ordering::Relaxed);
        self.cmd_end.store(end.max(start), Ordering::Release);
    }

    pub(crate) fn set_volume(&self, volume: f32) {
        let v = if volume.is_finite() {
            volume.clamp(0.0, 4.0)
        } else {
            0.0
        };
        self.volume_bits.store(v.to_bits(), Ordering::Relaxed);
    }

    pub(crate) fn volume(&self) -> f32 {
        f32::from_bits(self.volume_bits.load(Ordering::Relaxed))
    }

    pub(crate) fn play_requested(&self) -> bool {
        self.cmd_play.load(Ordering::Relaxed)
    }

    pub(crate) fn command_seq(&self) -> u64 {
        self.cmd_seq.load(Ordering::Acquire)
    }

    pub(crate) fn command_range(&self) -> (u64, u64) {
        (
            self.cmd_start.load(Ordering::Relaxed),
            self.cmd_end.load(Ordering::Relaxed),
        )
    }

    fn publish(&self, s: &ClockSnapshot) {
        let v = self.clk_version.load(Ordering::Relaxed);
        self.clk_version.store(v.wrapping_add(1), Ordering::Relaxed);
        fence(Ordering::Release);
        self.clk_seq.store(s.seq, Ordering::Relaxed);
        self.clk_start.store(s.start, Ordering::Relaxed);
        self.clk_emitted.store(s.emitted, Ordering::Relaxed);
        self.clk_buffer.store(s.buffer, Ordering::Relaxed);
        self.clk_target.store(s.target, Ordering::Relaxed);
        self.clk_cb_ns.store(s.cb_ns, Ordering::Relaxed);
        self.clk_latency_ns.store(s.latency_ns, Ordering::Relaxed);
        self.clk_playing.store(s.playing, Ordering::Relaxed);
        self.clk_version.store(v.wrapping_add(2), Ordering::Release);
    }

    pub(crate) fn snapshot(&self) -> ClockSnapshot {
        loop {
            let v1 = self.clk_version.load(Ordering::Acquire);
            if v1 & 1 == 1 {
                std::hint::spin_loop();
                continue;
            }
            let s = ClockSnapshot {
                seq: self.clk_seq.load(Ordering::Relaxed),
                start: self.clk_start.load(Ordering::Relaxed),
                emitted: self.clk_emitted.load(Ordering::Relaxed),
                buffer: self.clk_buffer.load(Ordering::Relaxed),
                target: self.clk_target.load(Ordering::Relaxed),
                cb_ns: self.clk_cb_ns.load(Ordering::Relaxed),
                latency_ns: self.clk_latency_ns.load(Ordering::Relaxed),
                playing: self.clk_playing.load(Ordering::Relaxed),
            };
            fence(Ordering::Acquire);
            if self.clk_version.load(Ordering::Relaxed) == v1 {
                return s;
            }
        }
    }

    pub(crate) fn record_callback(&self, busy_ns: u64, frames: u64) {
        self.cb_count.fetch_add(1, Ordering::Relaxed);
        self.cb_busy_ns.fetch_add(busy_ns, Ordering::Relaxed);
        self.cb_max_ns.fetch_max(busy_ns, Ordering::Relaxed);
        self.cb_frames.fetch_add(frames, Ordering::Relaxed);
    }
}

/// Source position (fractional frames) heard at `now_ns`, from a clock snapshot.
///
/// The first frame of the latest buffer becomes audible at `cb_ns + latency_ns`; frames emitted
/// earlier are still draining from the device buffer until then.
#[must_use]
pub(crate) fn position_at(s: &ClockSnapshot, now_ns: u64, src_rate: u32, dev_rate: u32) -> f64 {
    let elapsed_s = now_ns.saturating_sub(s.cb_ns) as f64 * 1e-9;
    let latency_s = s.latency_ns as f64 * 1e-9;
    let dev = f64::from(dev_rate);
    let played = s.emitted as f64 + (elapsed_s - latency_s) * dev;
    let upper = (s.emitted + s.buffer).min(s.target) as f64;
    let played = played.clamp(0.0, upper.max(0.0));
    s.start as f64 + played * f64::from(src_rate) / dev
}

/// Device frames produced by `src_frames` source frames.
fn device_frames_for(src_frames: u64, src_rate: u32, dev_rate: u32) -> u64 {
    let num = u128::from(src_frames) * u128::from(dev_rate) + u128::from(src_rate) / 2;
    u64::try_from(num / u128::from(src_rate.max(1))).unwrap_or(u64::MAX)
}

/// Pull-model renderer running on the audio thread. Allocates only in [`Engine::new`].
pub(crate) struct Engine {
    source: Arc<dyn PcmSource>,
    src_ch: usize,
    dev_ch: usize,
    src_rate: u32,
    dev_rate: u32,
    total_frames: u64,
    resampler: Option<Fft<f32>>,
    in_buf: Vec<f32>,
    rs_out: Vec<f32>,
    fifo: Vec<f32>,
    fifo_pos: usize,
    fifo_len: usize,
    cursor: u64,
    start: u64,
    end: u64,
    target: u64,
    emitted: u64,
    skip: usize,
    playing: bool,
    applied_seq: u64,
    gain: f32,
}

impl Engine {
    pub(crate) fn new(
        source: Arc<dyn PcmSource>,
        dev_rate: u32,
        dev_channels: u16,
    ) -> Result<Self, rubato::ResamplerConstructionError> {
        let src_rate = source.sample_rate().max(1);
        let src_ch = usize::from(source.channels().max(1));
        let resampler = if src_rate == dev_rate {
            None
        } else {
            Some(Fft::<f32>::new(
                src_rate as usize,
                dev_rate as usize,
                RESAMPLER_CHUNK,
                src_ch,
                FixedSync::Input,
            )?)
        };
        let out_max = resampler
            .as_ref()
            .map_or(RESAMPLER_CHUNK, Resampler::output_frames_max);
        let in_max = resampler
            .as_ref()
            .map_or(RESAMPLER_CHUNK, Resampler::input_frames_max);
        let fifo_frames = out_max.max(RESAMPLER_CHUNK);
        Ok(Self {
            total_frames: source.total_frames(),
            source,
            src_ch,
            dev_ch: usize::from(dev_channels.max(1)),
            src_rate,
            dev_rate,
            resampler,
            in_buf: vec![0.0; in_max * src_ch],
            rs_out: vec![0.0; out_max * src_ch],
            fifo: vec![0.0; fifo_frames * src_ch],
            fifo_pos: 0,
            fifo_len: 0,
            cursor: 0,
            start: 0,
            end: 0,
            target: 0,
            emitted: 0,
            skip: 0,
            playing: false,
            applied_seq: 0,
            gain: 1.0,
        })
    }

    pub(crate) const fn source_rate(&self) -> u32 {
        self.src_rate
    }

    pub(crate) const fn device_rate(&self) -> u32 {
        self.dev_rate
    }

    fn restart(&mut self, start: u64, end: u64) {
        let end = end.min(self.total_frames);
        self.start = start.min(end);
        self.end = end;
        self.cursor = self.start;
        self.target = device_frames_for(self.end - self.start, self.src_rate, self.dev_rate);
        self.emitted = 0;
        self.fifo_pos = 0;
        self.fifo_len = 0;
        if let Some(rs) = self.resampler.as_mut() {
            rs.reset();
            self.skip = rs.output_delay();
        } else {
            self.skip = 0;
        }
        self.playing = true;
    }

    fn apply_commands(&mut self, shared: &Shared) {
        let seq = shared.command_seq();
        if seq != self.applied_seq {
            self.applied_seq = seq;
            let (start, end) = shared.command_range();
            if shared.cmd_play.load(Ordering::Relaxed) {
                self.restart(start, end);
            } else {
                self.playing = false;
            }
        } else if self.playing {
            let end = shared
                .cmd_end
                .load(Ordering::Acquire)
                .min(self.total_frames);
            if end != self.end {
                self.end = end.max(self.start);
                self.target =
                    device_frames_for(self.end - self.start, self.src_rate, self.dev_rate)
                        .max(self.emitted);
            }
        }
    }

    /// Refills the FIFO with one chunk. Returns `false` if nothing could be produced.
    fn produce(&mut self) -> bool {
        let ch = self.src_ch;
        self.fifo_pos = 0;
        self.fifo_len = 0;
        let Some(rs) = self.resampler.as_mut() else {
            let n = RESAMPLER_CHUNK;
            let dst = &mut self.fifo[..n * ch];
            let avail = usize::try_from(self.end.saturating_sub(self.cursor))
                .unwrap_or(n)
                .min(n);
            let got = if avail > 0 {
                self.source
                    .read_f32(self.cursor, &mut dst[..avail * ch])
                    .min(avail)
            } else {
                0
            };
            dst[got * ch..].fill(0.0);
            self.cursor += n as u64;
            self.fifo_len = n;
            return true;
        };
        let need = rs.input_frames_next();
        let avail = usize::try_from(self.end.saturating_sub(self.cursor))
            .unwrap_or(need)
            .min(need);
        let input = &mut self.in_buf[..need * ch];
        let got = if avail > 0 {
            self.source
                .read_f32(self.cursor, &mut input[..avail * ch])
                .min(avail)
        } else {
            0
        };
        input[got * ch..].fill(0.0);
        self.cursor += need as u64;
        let out_frames = self.rs_out.len() / ch;
        let (Ok(inp), Ok(mut out)) = (
            InterleavedSlice::new(&self.in_buf[..need * ch], ch, need),
            InterleavedSlice::new_mut(&mut self.rs_out[..], ch, out_frames),
        ) else {
            return false;
        };
        let Ok((_, produced)) = rs.process_into_buffer(&inp, &mut out, None) else {
            return false;
        };
        let skip = self.skip.min(produced);
        self.skip -= skip;
        let n = produced - skip;
        self.fifo[..n * ch].copy_from_slice(&self.rs_out[skip * ch..produced * ch]);
        self.fifo_len = n;
        true
    }

    /// Renders one interleaved device buffer and publishes the clock for it.
    pub(crate) fn render(&mut self, shared: &Shared, out: &mut [f32], cb_ns: u64, latency_ns: u64) {
        self.apply_commands(shared);
        let frames = out.len() / self.dev_ch;
        let emitted_before = self.emitted;

        let target_gain = shared.volume();
        let step = if frames > 0 {
            (target_gain - self.gain) / frames as f32
        } else {
            0.0
        };

        let mut written = 0usize;
        while written < frames {
            if !self.playing || self.emitted >= self.target {
                break;
            }
            if self.fifo_len == 0 {
                if !self.produce() {
                    break;
                }
                continue;
            }
            let remaining = usize::try_from(self.target - self.emitted).unwrap_or(usize::MAX);
            let n = (frames - written).min(self.fifo_len).min(remaining);
            let src = &self.fifo[self.fifo_pos * self.src_ch..(self.fifo_pos + n) * self.src_ch];
            let dst = &mut out[written * self.dev_ch..(written + n) * self.dev_ch];
            let mut gain = self.gain + step * written as f32;
            mix_frames(src, self.src_ch, dst, self.dev_ch, &mut gain, step);
            written += n;
            self.emitted += n as u64;
            self.fifo_pos += n;
            self.fifo_len -= n;
        }
        out[written * self.dev_ch..].fill(0.0);
        if self.playing {
            // Trailing silence still advances the device timeline, so the clock keeps moving
            // while the last range frames drain from the device buffer.
            self.emitted += (frames - written) as u64;
        }
        self.gain = target_gain;

        shared.publish(&ClockSnapshot {
            seq: self.applied_seq,
            start: self.start,
            emitted: emitted_before,
            buffer: self.emitted - emitted_before,
            target: self.target,
            cb_ns,
            latency_ns,
            playing: self.playing,
        });
    }
}

/// Maps `src_ch` interleaved frames onto `dst_ch` with a gain ramp.
///
/// Equal counts copy; mono fans out to every channel; more source than device channels
/// downmixes by averaging source channels that fold onto the same device channel.
fn mix_frames(
    src: &[f32],
    src_ch: usize,
    dst: &mut [f32],
    dst_ch: usize,
    gain: &mut f32,
    step: f32,
) {
    for (s, d) in src.chunks_exact(src_ch).zip(dst.chunks_exact_mut(dst_ch)) {
        let g = *gain;
        if src_ch == dst_ch {
            for (o, i) in d.iter_mut().zip(s) {
                *o = i * g;
            }
        } else if src_ch == 1 {
            d.fill(s[0] * g);
        } else if src_ch > dst_ch {
            d.fill(0.0);
            for (i, v) in s.iter().enumerate() {
                d[i % dst_ch] += v;
            }
            let folds = src_ch.div_ceil(dst_ch) as f32;
            for o in d.iter_mut() {
                *o *= g / folds;
            }
        } else {
            for (i, o) in d.iter_mut().enumerate() {
                *o = if i < src_ch { s[i] * g } else { 0.0 };
            }
        }
        *gain += step;
    }
}

#[cfg(test)]
#[allow(clippy::chunks_exact_to_as_chunks)]
mod tests {
    use super::*;
    use audio_cache::{AudioCache, AudioCacheConfig};

    fn tone_cache(rate: u32, channels: u16, frames: u64, freq: f64) -> Arc<AudioCache<f32>> {
        let cfg = AudioCacheConfig::new(rate, channels, frames, 64 << 20);
        let cache = AudioCache::<f32>::new(cfg).unwrap();
        let ch = usize::from(channels);
        let bf = cache.config().block_frames;
        let blocks = usize::try_from(frames).unwrap().div_ceil(bf);
        for b in 0..blocks {
            let n = bf.min(usize::try_from(frames).unwrap() - b * bf);
            let mut data = vec![0.0f32; n * ch];
            for i in 0..n {
                let t = (b * bf + i) as f64 / f64::from(rate);
                #[allow(clippy::cast_possible_truncation)]
                let v = (0.5 * (2.0 * std::f64::consts::PI * freq * t).sin()) as f32;
                data[i * ch..(i + 1) * ch].fill(v);
            }
            cache.put_block(b, data.into_boxed_slice()).unwrap();
        }
        Arc::new(cache)
    }

    fn render_all(engine: &mut Engine, shared: &Shared, dev_ch: usize, max: usize) -> Vec<f32> {
        let mut all = Vec::new();
        let mut buf = vec![0.0f32; 480 * dev_ch];
        for _ in 0..max {
            engine.render(shared, &mut buf, 0, 0);
            all.extend_from_slice(&buf);
        }
        all
    }

    /// Estimates frequency from zero crossings of channel 0.
    fn zero_cross_freq(samples: &[f32], ch: usize, rate: u32) -> f64 {
        let mono: Vec<f32> = samples.chunks_exact(ch).map(|f| f[0]).collect();
        let first = mono.iter().position(|v| v.abs() > 0.1).unwrap_or(0);
        let last = mono
            .iter()
            .rposition(|v| v.abs() > 0.1)
            .unwrap_or(mono.len());
        let seg = &mono[first..last];
        let crossings = seg.windows(2).filter(|w| w[0] < 0.0 && w[1] >= 0.0).count();
        crossings as f64 * f64::from(rate) / seg.len() as f64
    }

    #[test]
    fn same_rate_plays_exact_range_then_silence() {
        let src = tone_cache(48_000, 2, 48_000, 440.0);
        let shared = Shared::new();
        let mut eng = Engine::new(src.clone(), 48_000, 2).unwrap();
        shared.play(1_000, 1_000 + 4_800);
        let out = render_all(&mut eng, &shared, 2, 20);
        let nonzero_frames = out
            .chunks_exact(2)
            .rposition(|f| f[0] != 0.0)
            .map_or(0, |p| p + 1);
        assert!(
            nonzero_frames <= 4_800 && nonzero_frames > 4_700,
            "{nonzero_frames}"
        );
        // Exact sample match with the source.
        let mut expect = vec![0.0f32; 4_800 * 2];
        src.read_frames(1_000, &mut expect).unwrap();
        assert_eq!(&out[..4_800 * 2], &expect[..]);
        let snap = shared.snapshot();
        assert_eq!(snap.target, 4_800);
        assert!(snap.emitted + snap.buffer >= 4_800);
    }

    #[test]
    fn resamples_44k1_to_48k_keeping_pitch_and_length() {
        let src = tone_cache(44_100, 1, 44_100, 1_000.0);
        let shared = Shared::new();
        let mut eng = Engine::new(src, 48_000, 2).unwrap();
        shared.play(0, 44_100);
        let out = render_all(&mut eng, &shared, 2, 110);
        let snap = shared.snapshot();
        assert_eq!(snap.target, 48_000);
        let f = zero_cross_freq(&out, 2, 48_000);
        assert!((f - 1_000.0).abs() < 5.0, "measured {f} Hz");
        // Mono fans out to both device channels.
        assert!(out
            .chunks_exact(2)
            .all(|fr| (fr[0] - fr[1]).abs() < f32::EPSILON));
        // Output is aligned: tone starts within a few frames of the range start.
        let onset = out
            .chunks_exact(2)
            .position(|fr| fr[0].abs() > 0.05)
            .unwrap();
        assert!(onset < 16, "onset at {onset}");
    }

    #[test]
    fn stop_volume_and_live_end_change() {
        let src = tone_cache(48_000, 2, 96_000, 440.0);
        let shared = Shared::new();
        let mut eng = Engine::new(src, 48_000, 2).unwrap();
        let mut buf = vec![0.0f32; 480 * 2];

        shared.set_volume(0.0);
        shared.play(0, 96_000);
        eng.render(&shared, &mut buf, 0, 0); // ramp from 1.0 to 0.0
        eng.render(&shared, &mut buf, 0, 0);
        assert!(buf.iter().all(|v| *v == 0.0));

        shared.set_volume(1.0);
        shared.set_end(1_920);
        eng.render(&shared, &mut buf, 0, 0);
        assert_eq!(shared.snapshot().target, 1_920);
        eng.render(&shared, &mut buf, 0, 0);
        eng.render(&shared, &mut buf, 0, 0);
        assert!(buf.iter().all(|v| *v == 0.0), "past the new end");

        shared.play(0, 96_000);
        eng.render(&shared, &mut buf, 0, 0);
        assert!(buf.iter().any(|v| *v != 0.0));
        shared.stop();
        eng.render(&shared, &mut buf, 0, 0);
        assert!(buf.iter().all(|v| *v == 0.0));
        assert!(!shared.snapshot().playing);
    }

    #[test]
    fn clock_compensates_latency_and_interpolates() {
        let snap = ClockSnapshot {
            seq: 1,
            start: 10_000,
            emitted: 48_000,
            buffer: 480,
            target: 96_000,
            cb_ns: 1_000_000_000,
            latency_ns: 20_000_000,
            playing: true,
        };
        // At callback time, audible position is 20 ms behind what was emitted.
        let p0 = position_at(&snap, 1_000_000_000, 48_000, 48_000);
        assert!((p0 - (10_000.0 + 48_000.0 - 960.0)).abs() < 1e-6);
        // 5 ms later it has advanced by 240 frames.
        let p1 = position_at(&snap, 1_005_000_000, 48_000, 48_000);
        assert!((p1 - p0 - 240.0).abs() < 1e-6);
        // Never runs ahead of what was written.
        let p2 = position_at(&snap, 2_000_000_000, 48_000, 48_000);
        assert!((p2 - (10_000.0 + 48_480.0)).abs() < 1e-6);
        // Source-rate conversion: 44.1 kHz source on a 48 kHz device.
        let p3 = position_at(&snap, 1_020_000_000, 44_100, 48_000);
        assert!((p3 - (10_000.0 + 48_000.0 * 44_100.0 / 48_000.0)).abs() < 1e-6);
    }

    #[test]
    fn downmix_and_upmix() {
        let mut g = 1.0;
        let mut out = [0.0f32; 2];
        mix_frames(&[0.2, 0.4, 0.6, 0.8], 4, &mut out, 2, &mut g, 0.0);
        assert!((out[0] - 0.4).abs() < 1e-6 && (out[1] - 0.6).abs() < 1e-6);
        let mut out6 = [9.0f32; 6];
        mix_frames(&[0.1, 0.2], 2, &mut out6, 6, &mut g, 0.0);
        assert_eq!(out6, [0.1, 0.2, 0.0, 0.0, 0.0, 0.0]);
    }
}
