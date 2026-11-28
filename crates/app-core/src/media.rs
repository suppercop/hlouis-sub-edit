//! Media provider traits (`VideoProvider`, `AudioProvider`, `SubtitleRenderer`, `FontProvider`)
//! and pure-Rust built-in implementations (`DummyVideoProvider`, `WavAudioProvider` via `hound`,
//! `PlainTextBoxRenderer`, `BasicFontProvider`) so `app-core` builds and runs cleanly on Linux
//! and Windows while native FFMS2/libass providers plug into the exact same traits.

use ass_core::{EventKind, ScriptFile};
use audio_peaks::PeakBucket;
use keyframes::Timecodes;
use std::io::Cursor;
use std::path::Path;
use ui_audio::AudioColumnProvider;
use ui_video::{RgbaFrameRef, VideoFrameSupplier};

/// Axis-aligned subtitle bounding box returned by [`SubtitleRenderer`].
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct SubtitleBBox {
    pub x: u32,
    pub y: u32,
    pub w: u32,
    pub h: u32,
    pub line_index: usize,
}

/// Timing metadata of an opened video, reported once the source is ready (and again when a
/// frame-exact index replaces an approximate fast-open source).
#[derive(Debug, Clone)]
pub struct VideoMeta {
    pub width: u32,
    pub height: u32,
    pub frame_count: u32,
    pub timecodes: Timecodes,
    pub keyframes: Vec<u32>,
}

/// Event produced by a background media provider; always surfaced to the user (never silent).
#[derive(Debug, Clone)]
pub enum MediaEvent {
    /// The video source is ready: attach it to the playback controller.
    VideoReady(VideoMeta),
    /// A frame-exact index replaced the fast-open source: refresh timing metadata in place.
    VideoRefined(VideoMeta),
    /// Decoded audio is available with this total duration.
    AudioReady { duration_ms: i64 },
    /// More decoded audio became available: redraw waveform/spectrum.
    AudioDataChanged,
    /// Informational progress (indexing %, ETA, provider fallback notices, ...).
    Status(String),
    /// A failure that must be shown to the user; the provider fell back or stopped.
    Error(String),
}

/// Trait supplying decoded video frames, timecodes, and keyframes.
pub trait VideoProvider {
    fn width(&self) -> u32;
    fn height(&self) -> u32;
    fn frame_count(&self) -> u32;
    fn timecodes(&self) -> &Timecodes;
    fn keyframes(&self) -> &[u32];
    fn is_keyframe(&self, frame: u32) -> bool {
        self.keyframes().binary_search(&frame).is_ok()
    }
    /// Returns tightly packed RGBA8 pixels (`width * height * 4`) for `frame`.
    fn frame_rgba(&self, frame: u32) -> Option<&[u8]>;
    /// Content revision of the pixels returned by [`Self::frame_rgba`] (bump on any change).
    fn revision(&self, frame: u32) -> u64 {
        u64::from(frame) + 1
    }
    /// Hints that the UI now shows `frame` (asynchronous providers start decoding it).
    fn request_frame(&mut self, _frame: u32) {}
    /// Supplies the current subtitle script (full `.ass` text) for compositing on frames.
    fn set_subtitles(&mut self, _ass_text: String) {}
    /// Drains background events (open progress, ready, errors) into `out`.
    fn poll(&mut self, _out: &mut Vec<MediaEvent>) {}
}

/// Trait supplying PCM audio samples and waveform peak columns.
pub trait AudioProvider {
    fn sample_rate(&self) -> u32;
    fn channels(&self) -> u16;
    fn duration_ms(&self) -> i64;
    fn read_mono_i16(&self, start_sample: usize, count: usize, out: &mut Vec<i16>);
    fn query_peaks_into(
        &self,
        start_ms: i64,
        end_ms: i64,
        width_px: usize,
        out: &mut Vec<PeakBucket>,
    );
    /// Drains background events (decode progress, ready, errors) into `out`.
    fn poll(&mut self, _out: &mut Vec<MediaEvent>) {}
    /// Starts audible playback of `[start_ms, end_ms]` (no-op for silent providers).
    fn play(&mut self, _start_ms: i64, _end_ms: i64) {}
    /// Stops audible playback.
    fn stop(&mut self) {}
    /// Current playback position in ms while audible playback is running, else `None`.
    fn position_ms(&self) -> Option<i64> {
        None
    }
    /// Sets the output gain (`0.0 ..= 1.0`).
    fn set_volume(&mut self, _volume: f32) {}
    /// Renders STFT spectrum pixels for `[start_ms, end_ms]` (`width_px * height_px`, row-major)
    /// into `out`; returns `false` when unsupported so the caller falls back to peaks.
    fn render_spectrum_into(
        &self,
        _start_ms: i64,
        _end_ms: i64,
        _width_px: usize,
        _height_px: usize,
        _palette: &theme::Palette,
        _out: &mut Vec<egui::Color32>,
    ) -> bool {
        false
    }
}

/// Creates native (or any other) media providers on demand; implemented by the platform layer.
pub trait MediaFactory {
    /// Starts opening `path` as a video (returns immediately; readiness arrives as events).
    fn open_video(
        &mut self,
        path: &Path,
        ctx: &egui::Context,
        ass_text: String,
    ) -> Result<Box<dyn VideoProvider>, String>;
    /// Starts decoding the audio of `path` (a video container or a pure audio file).
    fn open_audio(
        &mut self,
        path: &Path,
        ctx: &egui::Context,
    ) -> Result<Box<dyn AudioProvider>, String>;
}

/// What a native file dialog should pick.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum PickKind {
    Subtitles,
    Video,
    Audio,
    Mkv,
}

/// Native file dialogs (implemented with `rfd` by the platform layer).
pub trait FileDialogs {
    fn pick_file(&mut self, kind: PickKind) -> Option<std::path::PathBuf>;
    fn pick_save(&mut self, suggested_name: &str) -> Option<std::path::PathBuf>;
}

/// Trait compositing subtitles over an RGBA8 video frame at `time_ms`.
pub trait SubtitleRenderer {
    fn render_over_frame(
        &self,
        script: &ScriptFile,
        time_ms: i64,
        width: u32,
        height: u32,
        frame_rgba: &mut [u8],
    ) -> Vec<SubtitleBBox>;
}

/// Trait querying installed font families.
pub trait FontProvider {
    fn has_font(&self, family: &str) -> bool;
    fn available_fonts(&self) -> Vec<String>;
}

/// Built-in procedural dummy video provider (solid slate-blue backdrop + crisp 5×7 frame counter).
pub struct DummyVideoProvider {
    width: u32,
    height: u32,
    frame_count: u32,
    timecodes: Timecodes,
    keyframes: Vec<u32>,
    ring: Vec<Vec<u8>>,
}

impl DummyVideoProvider {
    /// Creates a dummy video provider with pre-baked numbered frames (`ring_len` frames cached).
    #[must_use]
    pub fn new(width: u32, height: u32, frame_count: u32, ring_len: usize) -> Self {
        let Ok(tc) = Timecodes::from_cfr(24_000, 1_001) else {
            unreachable!();
        };
        let keyframes: Vec<u32> = (0..frame_count).step_by(24).collect();
        let count = ring_len.max(1);
        let mut ring = Vec::with_capacity(count);
        for f in 0..count {
            #[allow(clippy::cast_possible_truncation)]
            ring.push(render_dummy_frame(width, height, f as u32));
        }
        Self {
            width,
            height,
            frame_count: frame_count.max(1),
            timecodes: tc,
            keyframes,
            ring,
        }
    }
}

impl VideoProvider for DummyVideoProvider {
    fn width(&self) -> u32 {
        self.width
    }
    fn height(&self) -> u32 {
        self.height
    }
    fn frame_count(&self) -> u32 {
        self.frame_count
    }
    fn timecodes(&self) -> &Timecodes {
        &self.timecodes
    }
    fn keyframes(&self) -> &[u32] {
        &self.keyframes
    }
    fn frame_rgba(&self, frame: u32) -> Option<&[u8]> {
        let idx = (frame as usize) % self.ring.len();
        Some(&self.ring[idx])
    }
}

/// Adapter exposing any [`VideoProvider`] as a [`ui_video::VideoFrameSupplier`].
pub struct VideoSupplierAdapter<'a>(pub &'a dyn VideoProvider);

impl VideoFrameSupplier for VideoSupplierAdapter<'_> {
    fn total_frames(&self) -> u32 {
        self.0.frame_count()
    }
    fn is_keyframe(&self, frame: u32) -> bool {
        self.0.is_keyframe(frame)
    }
    fn frame_revision(&self, frame: u32) -> u64 {
        self.0.revision(frame)
    }
    fn keyframes(&self) -> Option<&[u32]> {
        Some(self.0.keyframes())
    }
    fn frame_rgba(&self, frame: u32) -> Option<RgbaFrameRef<'_>> {
        let rgba = self.0.frame_rgba(frame)?;
        Some(RgbaFrameRef {
            width: self.0.width(),
            height: self.0.height(),
            rgba,
        })
    }
}

/// Pure-Rust WAV audio provider backed by [`hound::WavReader`].
pub struct WavAudioProvider {
    sample_rate: u32,
    channels: u16,
    mono_samples: Vec<i16>,
}

impl WavAudioProvider {
    /// Loads a PCM WAV file from `path`.
    pub fn from_path(path: &Path) -> Result<Self, String> {
        let bytes = std::fs::read(path).map_err(|e| format!("failed to read WAV: {e}"))?;
        Self::from_wav_bytes(&bytes)
    }

    /// Parses a PCM WAV buffer in memory via `hound`.
    pub fn from_wav_bytes(bytes: &[u8]) -> Result<Self, String> {
        let mut reader = hound::WavReader::new(Cursor::new(bytes))
            .map_err(|e| format!("invalid WAV header: {e}"))?;
        let spec = reader.spec();
        let ch = spec.channels.max(1) as usize;
        let mut mono = Vec::new();
        match spec.sample_format {
            hound::SampleFormat::Int => {
                let shift = spec.bits_per_sample.saturating_sub(16);
                let mut acc = 0i64;
                let mut cnt = 0usize;
                for s in reader.samples::<i32>() {
                    let v = s.map_err(|e| format!("WAV sample error: {e}"))?;
                    acc += i64::from(v >> shift);
                    cnt += 1;
                    if cnt == ch {
                        #[allow(clippy::cast_possible_truncation)]
                        let avg = (acc / i64::from(spec.channels.max(1)))
                            .clamp(i64::from(i16::MIN), i64::from(i16::MAX))
                            as i16;
                        mono.push(avg);
                        acc = 0;
                        cnt = 0;
                    }
                }
            }
            hound::SampleFormat::Float => {
                let mut acc = 0.0f32;
                let mut cnt = 0usize;
                for s in reader.samples::<f32>() {
                    let v = s.map_err(|e| format!("WAV float sample error: {e}"))?;
                    acc += v;
                    cnt += 1;
                    if cnt == ch {
                        #[allow(clippy::cast_possible_truncation)]
                        let avg = ((acc / ch as f32).clamp(-1.0, 1.0) * 32_767.0) as i16;
                        mono.push(avg);
                        acc = 0.0;
                        cnt = 0;
                    }
                }
            }
        }
        Ok(Self {
            sample_rate: spec.sample_rate.max(1),
            channels: spec.channels.max(1),
            mono_samples: mono,
        })
    }

    /// Generates a synthetic sine-wave audio provider (for demo/headless mode without a WAV file).
    #[must_use]
    pub fn synthetic(duration_ms: i64, sample_rate: u32) -> Self {
        #[allow(clippy::cast_possible_truncation, clippy::cast_sign_loss)]
        let len = ((duration_ms.max(100) as u64 * u64::from(sample_rate)) / 1_000) as usize;
        let capped = len.min(sample_rate as usize * 30);
        let mut mono = Vec::with_capacity(capped);
        for i in 0..capped {
            let t = i as f32 / sample_rate.max(1) as f32;
            #[allow(clippy::cast_possible_truncation)]
            let v = ((t * 440.0 * std::f32::consts::TAU).sin() * (t * 2.5).sin().abs() * 22_000.0)
                as i16;
            mono.push(v);
        }
        Self {
            sample_rate: sample_rate.max(1),
            channels: 1,
            mono_samples: mono,
        }
    }
}

impl AudioProvider for WavAudioProvider {
    fn sample_rate(&self) -> u32 {
        self.sample_rate
    }
    fn channels(&self) -> u16 {
        self.channels
    }
    fn duration_ms(&self) -> i64 {
        let sr = i64::from(self.sample_rate.max(1));
        #[allow(clippy::cast_possible_wrap)]
        let samples = self.mono_samples.len() as i64;
        (samples * 1_000) / sr
    }
    fn read_mono_i16(&self, start_sample: usize, count: usize, out: &mut Vec<i16>) {
        out.clear();
        if start_sample >= self.mono_samples.len() {
            return;
        }
        let end = (start_sample + count).min(self.mono_samples.len());
        out.extend_from_slice(&self.mono_samples[start_sample..end]);
    }
    fn query_peaks_into(
        &self,
        start_ms: i64,
        end_ms: i64,
        width_px: usize,
        out: &mut Vec<PeakBucket>,
    ) {
        out.clear();
        if width_px == 0 {
            return;
        }
        let n = self.mono_samples.len();
        if n == 0 {
            out.resize(
                width_px,
                PeakBucket {
                    min: 0,
                    max: 0,
                    rms: 0,
                },
            );
            return;
        }
        let sr = f64::from(self.sample_rate);
        let span_ms = (end_ms - start_ms).max(1) as f64;
        for col in 0..width_px {
            let t0 = start_ms as f64 + (col as f64 / width_px as f64) * span_ms;
            let t1 = start_ms as f64 + ((col + 1) as f64 / width_px as f64) * span_ms;
            #[allow(clippy::cast_possible_truncation, clippy::cast_sign_loss)]
            let s0 = ((t0.max(0.0) * sr) / 1_000.0) as usize;
            #[allow(clippy::cast_possible_truncation, clippy::cast_sign_loss)]
            let s1 = (((t1.max(0.0) * sr) / 1_000.0) as usize).max(s0 + 1);
            if s0 >= n {
                out.push(PeakBucket {
                    min: 0,
                    max: 0,
                    rms: 0,
                });
                continue;
            }
            let slice = &self.mono_samples[s0..s1.min(n)];
            let mut mn = i16::MAX;
            let mut mx = i16::MIN;
            let mut sq = 0u64;
            let stride = (slice.len() / 64).max(1);
            let mut count = 0u64;
            for &sample in slice.iter().step_by(stride) {
                mn = mn.min(sample);
                mx = mx.max(sample);
                let v = i64::from(sample);
                sq = sq.saturating_add((v * v).unsigned_abs());
                count += 1;
            }
            #[allow(clippy::cast_possible_truncation, clippy::cast_sign_loss)]
            let rms = ((sq as f64 / count.max(1) as f64).sqrt()) as u16;
            out.push(PeakBucket {
                min: mn,
                max: mx,
                rms,
            });
        }
    }
}

/// Adapter exposing any [`AudioProvider`] as a [`ui_audio::AudioColumnProvider`].
pub struct AudioColumnAdapter<'a>(pub &'a dyn AudioProvider);

impl AudioColumnProvider for AudioColumnAdapter<'_> {
    fn duration_ms(&self) -> i64 {
        self.0.duration_ms()
    }
    fn query_columns_into(
        &self,
        start_ms: i64,
        end_ms: i64,
        width_px: usize,
        out: &mut Vec<PeakBucket>,
    ) {
        self.0.query_peaks_into(start_ms, end_ms, width_px, out);
    }
    fn render_spectrum_into(
        &self,
        start_ms: i64,
        end_ms: i64,
        width_px: usize,
        height_px: usize,
        palette: &theme::Palette,
        out: &mut Vec<egui::Color32>,
    ) -> bool {
        self.0
            .render_spectrum_into(start_ms, end_ms, width_px, height_px, palette, out)
    }
}

/// Fallback [`SubtitleRenderer`] drawing translucent bounding boxes over active lines when
/// `libass` is not linked (pure Rust, zero unsafe).
#[derive(Debug, Default)]
pub struct PlainTextBoxRenderer;

impl SubtitleRenderer for PlainTextBoxRenderer {
    fn render_over_frame(
        &self,
        script: &ScriptFile,
        time_ms: i64,
        width: u32,
        height: u32,
        frame_rgba: &mut [u8],
    ) -> Vec<SubtitleBBox> {
        let mut boxes = Vec::new();
        let Ok(t_cs) = u32::try_from(time_ms.max(0) / 10) else {
            return boxes;
        };
        let mut stack = 0u32;
        for (idx, ev) in script.events().enumerate() {
            if ev.kind() == EventKind::Comment {
                continue;
            }
            if ev.start().as_cs() <= t_cs && t_cs < ev.end().as_cs() {
                let bh = (height / 10).max(12);
                let bw = ((width * 3) / 4).max(40);
                let bx = (width.saturating_sub(bw)) / 2;
                let by = height.saturating_sub((stack + 1) * (bh + 4));
                boxes.push(SubtitleBBox {
                    x: bx,
                    y: by,
                    w: bw,
                    h: bh,
                    line_index: idx,
                });
                // Blend translucent dark bar into RGBA buffer.
                for y in by..(by + bh).min(height) {
                    for x in bx..(bx + bw).min(width) {
                        let off = ((y as usize) * (width as usize) + (x as usize)) * 4;
                        if off + 3 < frame_rgba.len() {
                            frame_rgba[off] /= 2;
                            frame_rgba[off + 1] /= 2;
                            frame_rgba[off + 2] /= 2;
                        }
                    }
                }
                stack += 1;
                if stack >= 4 {
                    break;
                }
            }
        }
        boxes
    }
}

/// Built-in cross-platform [`FontProvider`] listing standard subtitle fallback families.
#[derive(Debug, Clone)]
pub struct BasicFontProvider {
    families: Vec<String>,
}

impl Default for BasicFontProvider {
    fn default() -> Self {
        Self {
            families: vec![
                "Arial".to_owned(),
                "Segoe UI".to_owned(),
                "Tahoma".to_owned(),
                "Consolas".to_owned(),
                "DejaVu Sans".to_owned(),
                "Noto Sans".to_owned(),
            ],
        }
    }
}

impl FontProvider for BasicFontProvider {
    fn has_font(&self, family: &str) -> bool {
        self.families.iter().any(|f| f.eq_ignore_ascii_case(family))
    }
    fn available_fonts(&self) -> Vec<String> {
        self.families.clone()
    }
}

fn render_dummy_frame(width: u32, height: u32, frame: u32) -> Vec<u8> {
    let w = width as usize;
    let h = height as usize;
    let mut buf = vec![0u8; w * h * 4];
    #[allow(clippy::cast_possible_truncation)]
    let r_bg = 28u8.wrapping_add(((frame / 12) % 24) as u8);
    for px in buf.as_chunks_mut::<4>().0 {
        px[0] = r_bg;
        px[1] = 36;
        px[2] = 52;
        px[3] = 255;
    }
    let label = format!("F {frame:05}");
    let scale = (w / 96).max(1);
    let char_w = 6 * scale;
    let total_w = label.len() * char_w;
    let start_x = w.saturating_sub(total_w) / 2;
    let start_y = h.saturating_sub(7 * scale) / 2;
    for (ci, ch) in label.chars().enumerate() {
        let glyph = bitmap_5x7(ch);
        for (row, bits) in glyph.iter().enumerate() {
            for col in 0..5usize {
                if (bits >> (4 - col)) & 1 == 1 {
                    for sy in 0..scale {
                        for sx in 0..scale {
                            let px = start_x + ci * char_w + col * scale + sx;
                            let py = start_y + row * scale + sy;
                            if px < w && py < h {
                                let idx = (py * w + px) * 4;
                                buf[idx] = 240;
                                buf[idx + 1] = 244;
                                buf[idx + 2] = 250;
                                buf[idx + 3] = 255;
                            }
                        }
                    }
                }
            }
        }
    }
    buf
}

const fn bitmap_5x7(ch: char) -> [u8; 7] {
    match ch {
        '0' => [0x0E, 0x11, 0x13, 0x15, 0x19, 0x11, 0x0E],
        '1' => [0x04, 0x0C, 0x04, 0x04, 0x04, 0x04, 0x0E],
        '2' => [0x0E, 0x11, 0x01, 0x02, 0x04, 0x08, 0x1F],
        '3' => [0x1F, 0x02, 0x04, 0x02, 0x01, 0x11, 0x0E],
        '4' => [0x02, 0x06, 0x0A, 0x12, 0x1F, 0x02, 0x02],
        '5' => [0x1F, 0x10, 0x1E, 0x01, 0x01, 0x11, 0x0E],
        '6' => [0x06, 0x08, 0x10, 0x1E, 0x11, 0x11, 0x0E],
        '7' => [0x1F, 0x01, 0x02, 0x04, 0x08, 0x08, 0x08],
        '8' => [0x0E, 0x11, 0x11, 0x0E, 0x11, 0x11, 0x0E],
        '9' => [0x0E, 0x11, 0x11, 0x0F, 0x01, 0x02, 0x0C],
        'F' => [0x1F, 0x10, 0x10, 0x1E, 0x10, 0x10, 0x10],
        _ => [0; 7],
    }
}
