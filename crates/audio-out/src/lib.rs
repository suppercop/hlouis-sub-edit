//! Audio output through `cpal` (WASAPI shared mode on Windows).
//!
//! [`AudioOutput`] pulls interleaved PCM from a [`PcmSource`] (implemented for
//! [`audio_cache::AudioCache`]) on the device's real-time callback, converts the sample rate with
//! `rubato` when the device rate differs, maps channels, applies volume and plays a range
//! `[start, end)` of source frames. Its playback clock ([`AudioOutput::position`]) reports the
//! source frame that is audible *now*, compensating for the device latency reported by WASAPI,
//! so the playback controller can use it as the master clock.
//!
//! Control calls are lock-free (atomics); the callback never allocates or locks except inside
//! the source read (`AudioCache` takes a shared read lock).

#![deny(unsafe_code)]
#![cfg_attr(not(test), deny(clippy::unwrap_used, clippy::expect_used))]

mod engine;
#[allow(unsafe_code)]
mod ffi;
mod source;

pub use ffi::{cycles_per_second, process_cpu_cycles, process_cpu_time};
pub use source::PcmSource;

use cpal::traits::{DeviceTrait, HostTrait, StreamTrait};
use engine::{position_at, Engine, Shared};
use std::sync::atomic::Ordering;
use std::sync::Arc;
use std::time::{Duration, Instant};
use thiserror::Error;

/// Errors returned by `audio-out`.
#[derive(Debug, Error)]
pub enum AudioOutError {
    /// No output device is available (e.g. headless CI runner).
    #[error("no audio output device available")]
    NoDevice,
    /// The device configuration could not be queried.
    #[error("audio device configuration error: {0}")]
    Config(String),
    /// The device uses a sample format this crate does not render.
    #[error("unsupported device sample format: {0}")]
    UnsupportedFormat(String),
    /// Resampler setup failed.
    #[error("resampler error: {0}")]
    Resampler(String),
    /// Building or starting the output stream failed.
    #[error("audio stream error: {0}")]
    Stream(String),
}

/// Description of the opened device stream.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct OutputInfo {
    /// Device friendly name.
    pub device_name: String,
    /// Device (mix format) sample rate in Hz.
    pub device_rate: u32,
    /// Device channel count.
    pub device_channels: u16,
    /// Device sample format (e.g. `f32`).
    pub sample_format: String,
    /// Source sample rate in Hz.
    pub source_rate: u32,
    /// `true` when `rubato` converts the source rate to the device rate.
    pub resampling: bool,
}

/// Callback statistics, for latency / CPU reporting.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub struct PlaybackStats {
    /// Number of device callbacks so far.
    pub callbacks: u64,
    /// Device frames rendered so far (including silence).
    pub frames: u64,
    /// Total time spent inside the render callback.
    pub busy: Duration,
    /// Longest single callback.
    pub max_callback: Duration,
    /// Output latency reported for the latest callback (callback â†’ audible).
    pub latency: Duration,
}

impl PlaybackStats {
    /// Average device frames per callback (the WASAPI period).
    #[must_use]
    pub fn frames_per_callback(&self) -> u64 {
        self.frames.checked_div(self.callbacks).unwrap_or(0)
    }
}

/// An open output stream playing from a [`PcmSource`].
///
/// The stream runs continuously while this value lives (silence when stopped) so that `play`
/// starts with no device start-up delay; drop it to release the device.
pub struct AudioOutput {
    stream: cpal::Stream,
    shared: Arc<Shared>,
    info: OutputInfo,
    src_rate: u32,
    dev_rate: u32,
    stopped_at: std::sync::atomic::AtomicU64,
}

impl std::fmt::Debug for AudioOutput {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("AudioOutput")
            .field("info", &self.info)
            .finish_non_exhaustive()
    }
}

impl AudioOutput {
    /// Opens the default output device of the default host (WASAPI on Windows) in shared mode
    /// with its mix format and the smallest shared-mode buffer.
    ///
    /// # Errors
    /// [`AudioOutError::NoDevice`] if there is no output device; other variants if the stream
    /// cannot be created.
    pub fn open_default(source: Arc<dyn PcmSource>) -> Result<Self, AudioOutError> {
        let host = cpal::default_host();
        let device = host
            .default_output_device()
            .ok_or(AudioOutError::NoDevice)?;
        Self::open_device(&device, source)
    }

    /// Opens `device` (see [`AudioOutput::open_default`]).
    ///
    /// # Errors
    /// See [`AudioOutput::open_default`].
    pub fn open_device(
        device: &cpal::Device,
        source: Arc<dyn PcmSource>,
    ) -> Result<Self, AudioOutError> {
        let supported = device
            .default_output_config()
            .map_err(|e| AudioOutError::Config(e.to_string()))?;
        let device_name = device
            .description()
            .map_or_else(|_| "(unknown device)".to_owned(), |d| d.name().to_owned());
        let format = supported.sample_format();
        let mut config = supported.config();
        // `Default` lets WASAPI pick its minimum shared-mode buffer (lowest latency).
        config.buffer_size = cpal::BufferSize::Default;

        let src_rate = source.sample_rate().max(1);
        let dev_rate = config.sample_rate;
        let engine = Engine::new(source, dev_rate, config.channels)
            .map_err(|e| AudioOutError::Resampler(e.to_string()))?;
        let shared = Arc::new(Shared::new());
        let info = OutputInfo {
            device_name,
            device_rate: dev_rate,
            device_channels: config.channels,
            sample_format: format.to_string(),
            source_rate: engine.source_rate(),
            resampling: engine.source_rate() != engine.device_rate(),
        };

        let stream = match format {
            cpal::SampleFormat::F32 => build_f32(device, &config, engine, Arc::clone(&shared))?,
            cpal::SampleFormat::I16 => {
                build_converted::<i16>(device, &config, engine, Arc::clone(&shared))?
            }
            cpal::SampleFormat::I32 => {
                build_converted::<i32>(device, &config, engine, Arc::clone(&shared))?
            }
            cpal::SampleFormat::U16 => {
                build_converted::<u16>(device, &config, engine, Arc::clone(&shared))?
            }
            other => return Err(AudioOutError::UnsupportedFormat(other.to_string())),
        };
        stream
            .play()
            .map_err(|e| AudioOutError::Stream(e.to_string()))?;
        Ok(Self {
            stream,
            shared,
            info,
            src_rate,
            dev_rate,
            stopped_at: std::sync::atomic::AtomicU64::new(0),
        })
    }

    /// Description of the opened stream.
    #[must_use]
    pub const fn info(&self) -> &OutputInfo {
        &self.info
    }

    /// Plays source frames `[start, end)` (clamped to the source length), replacing any
    /// current playback.
    pub fn play(&self, start: u64, end: u64) {
        self.stopped_at.store(start, Ordering::Relaxed);
        self.shared.play(start, end);
    }

    /// Stops playback; [`AudioOutput::position`] freezes at the frame audible now.
    pub fn stop(&self) {
        let pos = self.position();
        self.stopped_at.store(pos, Ordering::Relaxed);
        self.shared.stop();
    }

    /// Moves the end of the current play range without restarting.
    pub fn set_end(&self, end: u64) {
        self.shared.set_end(end);
    }

    /// Sets linear volume (`1.0` = unity, clamped to `0.0..=4.0`); ramped over one buffer.
    pub fn set_volume(&self, volume: f32) {
        self.shared.set_volume(volume);
    }

    /// Current linear volume.
    #[must_use]
    pub fn volume(&self) -> f32 {
        self.shared.volume()
    }

    /// Source frame audible right now (fractional), latency compensated.
    #[must_use]
    pub fn position_f64(&self) -> f64 {
        let snap = self.shared.snapshot();
        let requested = self.shared.command_seq();
        let (start, _) = self.shared.command_range();
        if snap.seq != requested {
            // The callback has not picked up the latest command yet.
            return if self.shared_play_requested() {
                start as f64
            } else {
                self.stopped_at.load(Ordering::Relaxed) as f64
            };
        }
        if !snap.playing {
            return self.stopped_at.load(Ordering::Relaxed) as f64;
        }
        position_at(&snap, self.shared.now_ns(), self.src_rate, self.dev_rate)
    }

    /// Source frame audible right now, latency compensated (the master playback clock).
    #[must_use]
    pub fn position(&self) -> u64 {
        #[allow(clippy::cast_possible_truncation, clippy::cast_sign_loss)]
        {
            self.position_f64().max(0.0) as u64
        }
    }

    /// `true` while a play range is active and its last frame has not been heard yet.
    #[must_use]
    pub fn is_playing(&self) -> bool {
        if !self.shared_play_requested() {
            return false;
        }
        let snap = self.shared.snapshot();
        if snap.seq != self.shared.command_seq() {
            return true;
        }
        let (_, end) = self.shared.command_range();
        snap.playing && self.position() < end.min(snap.start + self.source_frames(snap.target))
    }

    fn source_frames(&self, device_frames: u64) -> u64 {
        let num = u128::from(device_frames) * u128::from(self.src_rate);
        u64::try_from(num / u128::from(self.dev_rate.max(1))).unwrap_or(u64::MAX)
    }

    fn shared_play_requested(&self) -> bool {
        self.shared.play_requested()
    }

    /// Output latency of the latest callback (time from writing a frame until it is audible).
    #[must_use]
    pub fn latency(&self) -> Duration {
        Duration::from_nanos(self.shared.snapshot().latency_ns)
    }

    /// Callback statistics so far.
    #[must_use]
    pub fn stats(&self) -> PlaybackStats {
        let s = &self.shared;
        PlaybackStats {
            callbacks: s.cb_count.load(Ordering::Relaxed),
            frames: s.cb_frames.load(Ordering::Relaxed),
            busy: Duration::from_nanos(s.cb_busy_ns.load(Ordering::Relaxed)),
            max_callback: Duration::from_nanos(s.cb_max_ns.load(Ordering::Relaxed)),
            latency: self.latency(),
        }
    }

    /// Pauses the device stream (no callbacks) without changing the play state.
    ///
    /// # Errors
    /// [`AudioOutError::Stream`] if the device refuses.
    pub fn suspend(&self) -> Result<(), AudioOutError> {
        self.stream
            .pause()
            .map_err(|e| AudioOutError::Stream(e.to_string()))
    }

    /// Resumes a [`AudioOutput::suspend`]ed stream.
    ///
    /// # Errors
    /// [`AudioOutError::Stream`] if the device refuses.
    pub fn resume(&self) -> Result<(), AudioOutError> {
        self.stream
            .play()
            .map_err(|e| AudioOutError::Stream(e.to_string()))
    }
}

/// The [`AudioOutput`] render engine without a device: renders into caller buffers (offline
/// rendering, tests, benchmarks). Same play-range, resampling, channel-mapping and volume path.
pub struct OfflineRenderer {
    engine: Engine,
    shared: Shared,
}

impl std::fmt::Debug for OfflineRenderer {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("OfflineRenderer").finish_non_exhaustive()
    }
}

impl OfflineRenderer {
    /// Renderer producing `device_channels` interleaved `f32` at `device_rate`.
    ///
    /// # Errors
    /// [`AudioOutError::Resampler`] if the rate pair is unsupported.
    pub fn new(
        source: Arc<dyn PcmSource>,
        device_rate: u32,
        device_channels: u16,
    ) -> Result<Self, AudioOutError> {
        Ok(Self {
            engine: Engine::new(source, device_rate.max(1), device_channels)
                .map_err(|e| AudioOutError::Resampler(e.to_string()))?,
            shared: Shared::new(),
        })
    }

    /// See [`AudioOutput::play`].
    pub fn play(&self, start: u64, end: u64) {
        self.shared.play(start, end);
    }

    /// See [`AudioOutput::set_volume`].
    pub fn set_volume(&self, volume: f32) {
        self.shared.set_volume(volume);
    }

    /// Renders the next interleaved buffer (silence when stopped or past the range end).
    pub fn render(&mut self, out: &mut [f32]) {
        self.engine.render(&self.shared, out, 0, 0);
    }
}

fn latency_ns(info: &cpal::OutputCallbackInfo) -> u64 {
    let ts = info.timestamp();
    u64::try_from(
        ts.playback
            .saturating_duration_since(ts.callback)
            .as_nanos(),
    )
    .unwrap_or(0)
}

fn on_error(err: &cpal::Error) {
    // The control side observes failures through the clock stopping; log for diagnostics.
    eprintln!("audio-out: stream error: {err}");
}

fn build_f32(
    device: &cpal::Device,
    config: &cpal::StreamConfig,
    mut engine: Engine,
    shared: Arc<Shared>,
) -> Result<cpal::Stream, AudioOutError> {
    let channels = u64::from(config.channels.max(1));
    device
        .build_output_stream(
            *config,
            move |data: &mut [f32], info: &cpal::OutputCallbackInfo| {
                let t0 = Instant::now();
                let now = shared.now_ns();
                engine.render(&shared, data, now, latency_ns(info));
                let busy = u64::try_from(t0.elapsed().as_nanos()).unwrap_or(u64::MAX);
                shared.record_callback(busy, data.len() as u64 / channels);
            },
            |e| on_error(&e),
            None,
        )
        .map_err(|e| AudioOutError::Stream(e.to_string()))
}

fn build_converted<T>(
    device: &cpal::Device,
    config: &cpal::StreamConfig,
    mut engine: Engine,
    shared: Arc<Shared>,
) -> Result<cpal::Stream, AudioOutError>
where
    T: cpal::SizedSample + cpal::FromSample<f32>,
{
    let channels = u64::from(config.channels.max(1));
    // Pre-sized for 200 ms of 8-channel audio; grows (once) only for unusually large periods.
    let mut scratch = vec![0.0f32; 48_000 / 5 * 8];
    device
        .build_output_stream(
            *config,
            move |data: &mut [T], info: &cpal::OutputCallbackInfo| {
                let t0 = Instant::now();
                if scratch.len() < data.len() {
                    scratch.resize(data.len(), 0.0);
                }
                let buf = &mut scratch[..data.len()];
                let now = shared.now_ns();
                engine.render(&shared, buf, now, latency_ns(info));
                for (d, s) in data.iter_mut().zip(buf.iter()) {
                    *d = <T as cpal::Sample>::from_sample(*s);
                }
                let busy = u64::try_from(t0.elapsed().as_nanos()).unwrap_or(u64::MAX);
                shared.record_callback(busy, data.len() as u64 / channels);
            },
            |e| on_error(&e),
            None,
        )
        .map_err(|e| AudioOutError::Stream(e.to_string()))
}
