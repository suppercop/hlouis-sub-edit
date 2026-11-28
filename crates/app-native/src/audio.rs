//! [`NativeAudio`]: background `BestSource` audio decode into an [`AudioCache`], waveform peaks
//! ([`WaveformPyramid`]), on-demand STFT spectrum, and WASAPI playback through [`AudioOutput`].

use crate::video::{AudioGate, GateState};
use app_core::media::{AudioProvider, MediaEvent};
use audio_cache::{AudioCache, AudioCacheConfig};
use audio_out::{AudioOutput, PcmSource};
use audio_peaks::{PeakBucket, WaveformPyramid};
use media::{
    track_types, AudioProvider as _, BestSourceAudio, BestSourceOptions, IndexStore, MediaError,
};
use spectrum::{ColorLut, SpectrumCache, SpectrumColorScheme, SpectrumQuality};
use std::cell::RefCell;
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::mpsc::{self, Receiver, Sender};
use std::sync::{Arc, Mutex, PoisonError};
use std::thread::JoinHandle;
use std::time::Duration;

const BLOCK_FRAMES: usize = 65_536;
/// RAM budget of the decoded PCM cache; older blocks spill to a temp file beyond it.
const CACHE_RAM_BYTES: usize = 160 * 1024 * 1024;

type Pyramid = Arc<Mutex<WaveformPyramid>>;

enum Msg {
    Loaded {
        cache: Arc<AudioCache<i16>>,
        pyramid: Pyramid,
        sample_rate: u32,
        channels: u16,
        total_frames: u64,
    },
    Progress(String),
    DataChanged,
    Done,
    NoAudio,
    Error(String),
}

struct Loaded {
    cache: Arc<AudioCache<i16>>,
    pyramid: Pyramid,
    sample_rate: u32,
    total_frames: u64,
}

struct SpectrumChunk {
    start_frame: u64,
    end_frame: u64,
    cache: SpectrumCache,
}

struct Scratch {
    chunk: Option<SpectrumChunk>,
    rgba: Vec<u8>,
    mono: Vec<f32>,
    pcm: Vec<i16>,
}

/// Decoded audio with waveform, spectrum and speaker output.
pub struct NativeAudio {
    rx: Receiver<Msg>,
    cancel: Arc<AtomicBool>,
    thread: Option<JoinHandle<()>>,
    loaded: Option<Loaded>,
    out: Option<AudioOutput>,
    channels: u16,
    scratch: RefCell<Scratch>,
    volume: f32,
}

fn convert_to_i16(bytes: &[u8], info: &media::AudioInfo, out: &mut Vec<i16>) {
    out.clear();
    let bps = info.bytes_per_sample as usize;
    if bps == 0 {
        return;
    }
    out.reserve(bytes.len() / bps);
    for c in bytes.chunks_exact(bps) {
        let v: i16 = if info.is_float {
            let f = match bps {
                4 => f64::from(f32::from_le_bytes([c[0], c[1], c[2], c[3]])),
                8 => f64::from_le_bytes([c[0], c[1], c[2], c[3], c[4], c[5], c[6], c[7]]),
                _ => 0.0,
            };
            #[allow(clippy::cast_possible_truncation)]
            {
                (f.clamp(-1.0, 1.0) * 32_767.0) as i16
            }
        } else {
            match bps {
                1 => (i16::from(c[0]) - 128) << 8,
                2 => i16::from_le_bytes([c[0], c[1]]),
                3 => i16::from_le_bytes([c[1], c[2]]),
                4 => i16::from_le_bytes([c[2], c[3]]),
                _ => 0,
            }
        };
        out.push(v);
    }
}

fn decode_thread(
    path: &Path,
    gate: Option<&AudioGate>,
    cancel: &AtomicBool,
    tx: &Sender<Msg>,
    ctx: &egui::Context,
) -> Result<(), String> {
    let store = IndexStore::default_location();
    let mut opts = BestSourceOptions::default();
    if let Some(store) = &store {
        opts.cache = store
            .cache_for(path)
            .map_err(|e| format!("index cache: {e}"))?;
    }
    opts.threads = 2;

    match track_types(path) {
        Ok(types) if !types.contains(&bestsource_audio_type()) => {
            let _ = tx.send(Msg::NoAudio);
            return Ok(());
        }
        Ok(_) => {}
        Err(e) => return Err(format!("cannot inspect streams: {e}")),
    }

    let mut audio = if let Some(gate) = gate {
        // The video worker builds the exact index video-first, then audio; wait for it.
        loop {
            if cancel.load(Ordering::Relaxed) {
                return Ok(());
            }
            match gate.state() {
                GateState::Pending => {
                    let _ = tx.send(Msg::Progress("Waiting for the audio index ...".to_owned()));
                    std::thread::sleep(Duration::from_millis(150));
                }
                GateState::Failed(m) => return Err(format!("audio index unavailable: {m}")),
                GateState::Ready => break,
            }
        }
        BestSourceAudio::open_cached_only(path, &opts).map_err(|e| e.to_string())?
    } else {
        let mut last = -10i64;
        let txp = tx.clone();
        let c2 = ctx.clone();
        BestSourceAudio::open_with_progress(path, &opts, cancel, move |cur, tot| {
            if tot > 0 && cur != i64::MAX {
                let pct = cur * 100 / tot;
                if pct - last >= 5 {
                    last = pct;
                    let _ = txp.send(Msg::Progress(format!("Indexing audio {pct}%")));
                    c2.request_repaint();
                }
            }
        })
        .map_err(|e: MediaError| e.to_string())?
    };

    let info = audio.audio_info().clone();
    let channels = u16::try_from(info.channels).map_err(|_| "too many audio channels")?;
    let total = info.num_samples;
    let mut pyr =
        WaveformPyramid::new(info.sample_rate, channels).map_err(|e| format!("waveform: {e}"))?;
    pyr.reserve_frames(total);
    let pyramid: Pyramid = Arc::new(Mutex::new(pyr));
    let mut cfg = AudioCacheConfig::new(info.sample_rate, channels, total, CACHE_RAM_BYTES);
    cfg.spill_dir = Some(std::env::temp_dir().join("hlouis-sub-edit-audio"));
    if let Some(d) = &cfg.spill_dir {
        let _ = std::fs::create_dir_all(d);
    }
    let cache = Arc::new(AudioCache::<i16>::new(cfg).map_err(|e| format!("audio cache: {e}"))?);
    let _ = tx.send(Msg::Loaded {
        cache: Arc::clone(&cache),
        pyramid: Arc::clone(&pyramid),
        sample_rate: info.sample_rate,
        channels,
        total_frames: total,
    });
    ctx.request_repaint();

    fill(&mut audio, &info, &cache, &pyramid, cancel, tx, ctx)
}

/// Decodes the whole track block by block into the PCM cache and the waveform pyramid.
fn fill(
    audio: &mut BestSourceAudio,
    info: &media::AudioInfo,
    cache: &AudioCache<i16>,
    pyramid: &Pyramid,
    cancel: &AtomicBool,
    tx: &Sender<Msg>,
    ctx: &egui::Context,
) -> Result<(), String> {
    let ch = info.channels as usize;
    let total = info.num_samples;
    let mut bytes = Vec::new();
    let mut pcm = Vec::new();
    let mut frame = 0u64;
    let mut block = 0usize;
    while frame < total {
        if cancel.load(Ordering::Relaxed) {
            return Ok(());
        }
        let count = BLOCK_FRAMES.min(usize::try_from(total - frame).unwrap_or(BLOCK_FRAMES));
        audio
            .samples_into(frame, count, &mut bytes)
            .map_err(|e| format!("audio decode at {frame}: {e}"))?;
        convert_to_i16(&bytes, info, &mut pcm);
        pcm.truncate(count * ch);
        pcm.resize(count * ch, 0);
        pyramid
            .lock()
            .unwrap_or_else(PoisonError::into_inner)
            .push_interleaved_i16(&pcm);
        cache
            .put_block(block, pcm.clone().into_boxed_slice())
            .map_err(|e| format!("audio cache: {e}"))?;
        frame += count as u64;
        block += 1;
        if block == 1 || block.is_multiple_of(8) {
            let _ = tx.send(Msg::DataChanged);
            ctx.request_repaint();
        }
    }
    pyramid
        .lock()
        .unwrap_or_else(PoisonError::into_inner)
        .finish();
    let _ = tx.send(Msg::Done);
    ctx.request_repaint();
    Ok(())
}

/// `AVMEDIA_TYPE_AUDIO`.
const fn bestsource_audio_type() -> i32 {
    1
}

impl NativeAudio {
    /// Starts decoding `path` in the background. With a `gate`, waits for the video worker's
    /// shared exact index instead of indexing the file a second time.
    pub fn open(
        path: &Path,
        ctx: &egui::Context,
        gate: Option<Arc<AudioGate>>,
    ) -> Result<Self, String> {
        let (tx, rx) = mpsc::channel();
        let cancel = Arc::new(AtomicBool::new(false));
        let p: PathBuf = path.to_path_buf();
        let c = Arc::clone(&cancel);
        let ctx2 = ctx.clone();
        let thread = std::thread::Builder::new()
            .name("hlouis-audio-decode".to_owned())
            .spawn(move || {
                if let Err(e) = decode_thread(&p, gate.as_deref(), &c, &tx, &ctx2) {
                    let _ = tx.send(Msg::Error(e));
                    ctx2.request_repaint();
                }
            })
            .map_err(|e| format!("cannot start the audio decoder: {e}"))?;
        Ok(Self {
            rx,
            cancel,
            thread: Some(thread),
            loaded: None,
            out: None,
            channels: 2,
            scratch: RefCell::new(Scratch {
                chunk: None,
                rgba: Vec::new(),
                mono: Vec::new(),
                pcm: Vec::new(),
            }),
            volume: 1.0,
        })
    }

    fn frames_for_ms(&self, ms: i64) -> u64 {
        let Some(l) = &self.loaded else { return 0 };
        u64::try_from(ms.max(0)).unwrap_or(0) * u64::from(l.sample_rate) / 1_000
    }
}

impl Drop for NativeAudio {
    fn drop(&mut self) {
        self.cancel.store(true, Ordering::Relaxed);
        if let Some(t) = self.thread.take() {
            let _ = t.join();
        }
    }
}

impl AudioProvider for NativeAudio {
    fn sample_rate(&self) -> u32 {
        self.loaded.as_ref().map_or(48_000, |l| l.sample_rate)
    }
    fn channels(&self) -> u16 {
        self.channels
    }
    fn duration_ms(&self) -> i64 {
        self.loaded.as_ref().map_or(0, |l| {
            i64::try_from(l.total_frames * 1_000 / u64::from(l.sample_rate.max(1))).unwrap_or(0)
        })
    }
    fn read_mono_i16(&self, start_sample: usize, count: usize, out: &mut Vec<i16>) {
        out.clear();
        let Some(l) = &self.loaded else { return };
        let ch = usize::from(self.channels.max(1));
        let mut sc = self.scratch.borrow_mut();
        sc.pcm.clear();
        sc.pcm.resize(count * ch, 0);
        let n = l
            .cache
            .read_frames(start_sample as u64, &mut sc.pcm)
            .unwrap_or(0);
        for f in sc.pcm.chunks_exact(ch).take(n) {
            let sum: i32 = f.iter().map(|&s| i32::from(s)).sum();
            #[allow(clippy::cast_possible_truncation)]
            out.push((sum / i32::try_from(ch).unwrap_or(1)) as i16);
        }
    }
    fn query_peaks_into(
        &self,
        start_ms: i64,
        end_ms: i64,
        width_px: usize,
        out: &mut Vec<PeakBucket>,
    ) {
        out.clear();
        let Some(l) = &self.loaded else {
            out.resize(width_px, PeakBucket::default());
            return;
        };
        let span_ms = (end_ms - start_ms).max(1);
        // Columns before time zero stay empty.
        let pad = if start_ms < 0 {
            usize::try_from((-start_ms).saturating_mul(i64::try_from(width_px).unwrap_or(0)) / span_ms)
                .unwrap_or(0)
                .min(width_px)
        } else {
            0
        };
        out.resize(pad, PeakBucket::default());
        let rest = width_px - pad;
        if rest == 0 {
            return;
        }
        let s = self.frames_for_ms(start_ms.max(0));
        let e = self.frames_for_ms(end_ms).max(s + 1);
        let mut tmp = Vec::with_capacity(rest);
        l.pyramid
            .lock()
            .unwrap_or_else(PoisonError::into_inner)
            .query_columns(s, e, rest, &mut tmp);
        out.extend_from_slice(&tmp);
    }
    fn poll(&mut self, out: &mut Vec<MediaEvent>) {
        while let Ok(msg) = self.rx.try_recv() {
            match msg {
                Msg::Loaded {
                    cache,
                    pyramid,
                    sample_rate,
                    channels,
                    total_frames,
                } => {
                    self.channels = channels;
                    match AudioOutput::open_default(Arc::clone(&cache) as Arc<dyn PcmSource>) {
                        Ok(o) => {
                            o.set_volume(self.volume);
                            self.out = Some(o);
                        }
                        Err(e) => out.push(MediaEvent::Error(format!(
                            "Audio output unavailable ({e}); waveform works but nothing will play"
                        ))),
                    }
                    self.loaded = Some(Loaded {
                        cache,
                        pyramid,
                        sample_rate,
                        total_frames,
                    });
                    out.push(MediaEvent::AudioReady {
                        duration_ms: self.duration_ms(),
                    });
                }
                Msg::Progress(s) => out.push(MediaEvent::Status(s)),
                Msg::DataChanged => out.push(MediaEvent::AudioDataChanged),
                Msg::Done => {
                    out.push(MediaEvent::AudioDataChanged);
                    out.push(MediaEvent::Status("Audio decoded".to_owned()));
                }
                Msg::NoAudio => out.push(MediaEvent::Status(
                    "This file has no audio track".to_owned(),
                )),
                Msg::Error(e) => out.push(MediaEvent::Error(format!("Audio: {e}"))),
            }
        }
    }
    fn play(&mut self, start_ms: i64, end_ms: i64) {
        let s = self.frames_for_ms(start_ms);
        let e = self.frames_for_ms(end_ms).max(s + 1);
        if let Some(o) = &self.out {
            o.play(s, e);
        }
    }
    fn stop(&mut self) {
        if let Some(o) = &self.out {
            o.stop();
        }
    }
    fn position_ms(&self) -> Option<i64> {
        let o = self.out.as_ref()?;
        let l = self.loaded.as_ref()?;
        if !o.is_playing() {
            return None;
        }
        Some(i64::try_from(o.position() * 1_000 / u64::from(l.sample_rate.max(1))).unwrap_or(0))
    }
    fn set_volume(&mut self, volume: f32) {
        self.volume = volume;
        if let Some(o) = &self.out {
            o.set_volume(volume);
        }
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
        let Some(l) = &self.loaded else { return false };
        if width_px == 0 || height_px == 0 || end_ms <= start_ms {
            return false;
        }
        let quality = SpectrumQuality::Better;
        let hop = quality.base_hop() as u64;
        let sr = u64::from(l.sample_rate);
        let ch = usize::from(self.channels.max(1));
        let want_s = self.frames_for_ms(start_ms.max(0));
        let want_e = self.frames_for_ms(end_ms).min(l.total_frames).max(want_s + 1);
        let span = want_e - want_s;
        let mut sc = self.scratch.borrow_mut();
        let covered = sc
            .chunk
            .as_ref()
            .is_some_and(|c| c.start_frame <= want_s && c.end_frame >= want_e);
        if !covered {
            let c_s = want_s.saturating_sub(span);
            let c_e = (want_e + span).min(l.total_frames);
            let n = usize::try_from(c_e - c_s).unwrap_or(0);
            let Scratch { mono, pcm, .. } = &mut *sc;
            pcm.clear();
            pcm.resize(n * ch, 0);
            let got = l.cache.read_frames(c_s, pcm).unwrap_or(0);
            mono.clear();
            mono.extend(pcm.chunks_exact(ch).take(got).map(|f| {
                let sum: f32 = f.iter().map(|&s| f32::from(s)).sum();
                sum / (32_768.0 * ch as f32)
            }));
            let cache = SpectrumCache::build_from_mono(mono, l.sample_rate, quality, 1.0);
            sc.chunk = Some(SpectrumChunk {
                start_frame: c_s,
                end_frame: c_e,
                cache,
            });
        }
        let Scratch {
            chunk,
            rgba: rgba_buf,
            ..
        } = &mut *sc;
        let Some(chunk) = chunk.as_ref() else {
            return false;
        };
        let cols_total = chunk.cache.total_columns();
        if cols_total == 0 {
            return false;
        }
        let first_col = usize::try_from((want_s - chunk.start_frame) / hop).unwrap_or(0);
        let ncols = usize::try_from(span / hop + 2).unwrap_or(2);
        let ncols = ncols.min(cols_total.saturating_sub(first_col)).max(1);
        let lut = ColorLut::build(SpectrumColorScheme::ThemePalette, palette);
        let mut rgba = std::mem::take(rgba_buf);
        rgba.clear();
        rgba.resize(ncols * height_px * 4, 0);
        chunk
            .cache
            .render_viewport_into(0, first_col, ncols, height_px, None, &lut, &mut rgba);
        out.clear();
        out.reserve(width_px * height_px);
        let _ = sr;
        for y in 0..height_px {
            for x in 0..width_px {
                let col = (x * ncols / width_px).min(ncols - 1);
                let i = (y * ncols + col) * 4;
                out.push(egui::Color32::from_rgb(rgba[i], rgba[i + 1], rgba[i + 2]));
            }
        }
        *rgba_buf = rgba;
        true
    }
}
