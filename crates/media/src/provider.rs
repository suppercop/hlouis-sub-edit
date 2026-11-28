//! The common media provider traits and runtime provider selection.

use crate::bestsource::{track_types, BestSourceAudio, BestSourceOptions, BestSourceProvider};
use crate::ffi::bestsource::PartialIndex;
use crate::ffi::win::{set_current_thread_priority, ThreadPriority};
use crate::index_store::IndexStore;
use crate::quick::QuickVideo;
use crate::{FfmsVideo, MediaError, VideoInfo};
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicBool, AtomicI64, AtomicU8, Ordering};
use std::sync::mpsc::{self, Receiver, TryRecvError};
use std::sync::{Arc, Mutex, PoisonError};
use std::thread::JoinHandle;
use std::time::{Duration, Instant};

/// Which decoding backend serves a track.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Default)]
pub enum ProviderKind {
    /// `BestSource`: frame-accurate, cached index; the default.
    #[default]
    BestSource,
    /// `FFMS2`: fast-open fallback.
    Ffms2,
    /// Index-free libavformat/libavcodec: instant, approximate frame numbers.
    Quick,
}

impl ProviderKind {
    /// Stable name for settings files.
    #[must_use]
    pub fn as_str(self) -> &'static str {
        match self {
            Self::BestSource => "bestsource",
            Self::Ffms2 => "ffms2",
            Self::Quick => "quick",
        }
    }

    /// Parses [`Self::as_str`] (case-insensitive).
    #[must_use]
    pub fn parse(s: &str) -> Option<Self> {
        if s.eq_ignore_ascii_case("bestsource") {
            Some(Self::BestSource)
        } else if s.eq_ignore_ascii_case("ffms2") {
            Some(Self::Ffms2)
        } else if s.eq_ignore_ascii_case("quick") {
            Some(Self::Quick)
        } else {
            None
        }
    }
}

/// A frame-accurate video source producing tightly packed RGBA8 frames.
pub trait VideoProvider: Send {
    /// The backend behind this source.
    fn kind(&self) -> ProviderKind;

    /// Static metadata, timestamps and keyframe indices.
    fn info(&self) -> &VideoInfo;

    /// `true` when frame numbers, timestamps and keyframes are exact (indexed). An
    /// approximate source has no keyframe list and estimates frame numbers from time.
    fn is_frame_exact(&self) -> bool {
        true
    }

    /// `true` when `frame_index` can be decoded frame-accurately right now (for progressive
    /// indexing, frames already covered by the partial index return `true`).
    fn is_frame_exact_at(&self, _frame_index: usize) -> bool {
        self.is_frame_exact()
    }

    /// Decodes frame `frame_index` to RGBA8 (`width * height * 4`) in `out_rgba`, reusing its
    /// allocation. Returns the frame's presentation timestamp in milliseconds.
    fn decode_frame_into(
        &mut self,
        frame_index: usize,
        out_rgba: &mut Vec<u8>,
    ) -> Result<i64, MediaError>;

    /// Next keyframe strictly after `current`.
    fn next_keyframe(&self, current: usize) -> Option<usize> {
        self.info().next_keyframe(current)
    }

    /// Previous keyframe strictly before `current`.
    fn prev_keyframe(&self, current: usize) -> Option<usize> {
        self.info().prev_keyframe(current)
    }
}

/// Static audio track description.
#[derive(Debug, Clone, PartialEq)]
pub struct AudioInfo {
    pub track: i32,
    pub sample_rate: u32,
    pub channels: u32,
    pub bits_per_sample: u32,
    pub bytes_per_sample: u32,
    pub is_float: bool,
    pub num_samples: u64,
    pub start_time_s: f64,
}

impl AudioInfo {
    /// Bytes of one interleaved sample frame (all channels).
    #[must_use]
    pub fn frame_bytes(&self) -> usize {
        self.channels as usize * self.bytes_per_sample as usize
    }

    /// Track duration in milliseconds.
    #[must_use]
    pub fn duration_ms(&self) -> f64 {
        if self.sample_rate == 0 {
            0.0
        } else {
            self.num_samples as f64 * 1000.0 / f64::from(self.sample_rate)
        }
    }
}

/// A sample-accurate audio source producing interleaved samples in the native format.
pub trait AudioProvider: Send {
    fn kind(&self) -> ProviderKind;
    fn audio_info(&self) -> &AudioInfo;
    /// Interleaved samples `[start, start + count)`; out-of-range parts are silence.
    fn samples_into(
        &mut self,
        start: u64,
        count: usize,
        out: &mut Vec<u8>,
    ) -> Result<(), MediaError>;
}

impl VideoProvider for FfmsVideo {
    fn kind(&self) -> ProviderKind {
        ProviderKind::Ffms2
    }

    fn info(&self) -> &VideoInfo {
        FfmsVideo::info(self)
    }

    fn decode_frame_into(
        &mut self,
        frame_index: usize,
        out_rgba: &mut Vec<u8>,
    ) -> Result<i64, MediaError> {
        FfmsVideo::decode_frame_into(self, frame_index, out_rgba)
    }
}

/// Opens `path` with the chosen backend (blocking: indexes unless a cached index exists).
/// [`ProviderKind::Quick`] opens without an index (approximate frame numbers).
pub fn open_video(
    kind: ProviderKind,
    path: &Path,
    opts: &BestSourceOptions,
    cancel: &AtomicBool,
    on_progress: impl FnMut(i64, i64),
) -> Result<Box<dyn VideoProvider>, MediaError> {
    Ok(match kind {
        ProviderKind::BestSource => Box::new(BestSourceProvider::open_with_progress(
            path,
            opts,
            cancel,
            on_progress,
        )?),
        ProviderKind::Ffms2 => Box::new(FfmsVideo::open_with_progress(path, cancel, on_progress)?),
        ProviderKind::Quick => Box::new(QuickVideo::open(path, opts.track, opts.threads)?),
    })
}

/// Decoder threads for background indexing: all cores but two, so the UI and playback keep
/// running smoothly (frame + slice threads are both bounded by this in `FFmpeg`).
#[must_use]
pub fn background_decode_threads() -> i32 {
    let n = std::thread::available_parallelism().map_or(4, std::num::NonZero::get);
    i32::try_from(n.saturating_sub(2).max(1)).unwrap_or(1)
}

/// What [`HybridVideo::poll_upgrade`] observed for the video track.
#[derive(Debug)]
pub enum UpgradeStatus {
    /// `BestSource` is still indexing the video track in the background.
    Pending,
    /// Just switched to `BestSource`; callers should re-read [`VideoProvider::info`].
    Switched,
    /// Already on `BestSource` (or never going to switch).
    Done,
    /// Background `BestSource` open or runtime decode failed; staying on the fallback path.
    Failed(MediaError),
}

/// What [`HybridVideo::poll_audio`] observed for the background audio index pass.
#[derive(Debug)]
pub enum AudioUpgradeStatus {
    /// Video or audio background indexing is still in progress.
    Pending,
    /// `BestSourceAudio` is indexed/loaded and available via [`HybridVideo::audio`].
    Ready,
    /// Source file contains no audio track (or audio indexing is disabled).
    NoAudio,
    /// Audio indexing failed (or video failed before audio started).
    Failed(MediaError),
}

/// Which provider serves frames while `BestSource` indexes.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum FastOpen {
    /// Plain libavformat/libavcodec, no index at all: first frame in ~0.1 s.
    #[default]
    Quick,
    /// `FFMS2` (indexes the container first: ~1 s for a 24-min episode).
    Ffms2,
}

/// Options for [`HybridVideo::open`].
#[derive(Debug, Clone)]
pub struct HybridOptions {
    pub fast: FastOpen,
    /// Options of the background `BestSource` video open (`threads` 0 = [`background_decode_threads`]).
    pub bestsource: BestSourceOptions,
    /// Audio stream index (`< 0` = n-th audio track, `-1` = first).
    pub audio_track: i32,
    /// Whether to index the audio track after video indexing finishes (`true` by default).
    pub index_audio: bool,
    /// Persistent cache; when it already holds an index the exact path opens directly.
    pub store: Option<IndexStore>,
    /// Priority of the indexing thread.
    pub priority: ThreadPriority,
}

impl Default for HybridOptions {
    fn default() -> Self {
        Self {
            fast: FastOpen::Quick,
            bestsource: BestSourceOptions::default(),
            audio_track: -1,
            index_audio: true,
            store: IndexStore::default_location(),
            priority: ThreadPriority::BelowNormal,
        }
    }
}

/// Which indexing pass is currently running in [`HybridVideo`].
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum IndexPhase {
    Video,
    Audio,
}

/// Progress of the background exact index.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct IndexProgress {
    pub phase: IndexPhase,
    /// 0.0 ..= 1.0 (bytes demuxed / file size).
    pub fraction: f32,
    pub elapsed: Duration,
    /// Remaining time extrapolated from the rate so far (`None` until 2 % or 1 s in).
    pub eta: Option<Duration>,
}

const PHASE_VIDEO: u8 = 0;
const PHASE_AUDIO: u8 = 1;

struct Shared {
    phase: AtomicU8,
    cur: AtomicI64,
    total: AtomicI64,
    phase_started: Mutex<Instant>,
}

enum AudioResult {
    Ready(BestSourceAudio),
    NoAudio,
    Failed(MediaError),
}

/// Instant open + progressive exact seeking + frame-exact switch:
///
/// 1. Frames come immediately from [`QuickVideo`] (or `FFMS2`) while `BestSource` indexes audio and
///    then the video track on a below-normal-priority worker thread.
/// 2. Frames already covered by the partial index are decoded frame-accurately right away via the
///    shared [`PartialIndex`]; unindexed frames seek approximately.
/// 3. The background worker indexes the AUDIO track first, so playback audio can start while the
///    video index is still building; it lands in the persistent [`IndexStore`] and is picked up
///    by [`Self::poll_audio`].
/// 4. When video indexing finishes afterwards, [`Self::poll_upgrade`] swaps seamlessly to
///    [`BestSourceProvider`].
/// 5. Any `BestSource` / `QuickVideo` error falls back cleanly with a visible message.
pub struct HybridVideo {
    active: Box<dyn VideoProvider>,
    video_pending: Option<Receiver<Result<BestSourceProvider, MediaError>>>,
    audio_pending: Option<Receiver<AudioResult>>,
    audio: Option<BestSourceAudio>,
    audio_status: AudioState,
    worker: Option<JoinHandle<()>>,
    cancel: Arc<AtomicBool>,
    progress: Arc<Shared>,
    store: Option<IndexStore>,
    path: PathBuf,
    video_track: i32,
    partial: Option<PartialIndex>,
    fallback_notice: Option<String>,
    runtime_fallback_error: Option<MediaError>,
}

enum AudioState {
    Pending,
    Ready,
    NoAudio,
    Failed(String),
}

impl HybridVideo {
    /// Opens `path`. `cancel` aborts the (blocking) fast open; the background index is cancelled
    /// by dropping the returned value. `on_progress` only reports the fast open (FFMS2 indexing).
    pub fn open(
        path: &Path,
        opts: &HybridOptions,
        cancel: &AtomicBool,
        on_progress: impl FnMut(i64, i64),
    ) -> Result<Self, MediaError> {
        let mut bs_opts = opts.bestsource.clone();
        if let Some(store) = &opts.store {
            bs_opts.cache = store
                .cache_for(path)
                .map_err(|e| MediaError::BestSource(format!("index cache: {e}")))?;
        }
        let mut audio_opts = bs_opts.clone();
        audio_opts.track = opts.audio_track;
        audio_opts.threads = if bs_opts.threads > 0 {
            bs_opts.threads.min(4)
        } else {
            background_decode_threads().min(4)
        };
        if bs_opts.threads <= 0 {
            bs_opts.threads = background_decode_threads();
        }

        let progress = Arc::new(Shared {
            phase: AtomicU8::new(PHASE_VIDEO),
            cur: AtomicI64::new(0),
            total: AtomicI64::new(0),
            phase_started: Mutex::new(Instant::now()),
        });
        let bg_cancel = Arc::new(AtomicBool::new(false));
        let mut cached_audio: Option<BestSourceAudio> = None;

        if let Some(store) = &opts.store {
            if store.has_index(path) {
                if let Ok(bs) = BestSourceProvider::open_cached_only(path, &bs_opts) {
                    let _ = store.touch(path);
                    return Self::finish_with_cached_video(
                        path,
                        opts,
                        bs,
                        &audio_opts,
                        progress,
                        bg_cancel,
                    );
                }
                cached_audio = salvage_cached_audio(store, path, opts.index_audio, &audio_opts);
            }
        }

        let partial = PartialIndex::new()?;
        let quick_threads = bs_opts.threads.min(4);
        let (active, fallback_notice) = open_fast_provider(
            path,
            opts.fast,
            bs_opts.track,
            quick_threads,
            &partial,
            cancel,
            on_progress,
        )?;

        let (vtx, vrx) = mpsc::channel();
        let (atx, arx) = mpsc::channel();
        let index_audio = opts.index_audio;
        if index_audio && cached_audio.is_none() {
            progress.phase.store(PHASE_AUDIO, Ordering::Relaxed);
        }
        let worker = spawn_index_worker(WorkerArgs {
            path: path.to_path_buf(),
            bs_opts,
            audio_opts,
            index_audio,
            skip_video: false,
            skip_audio: cached_audio.is_some(),
            priority: opts.priority,
            store: opts.store.clone(),
            partial: Some(partial.clone()),
            bg_cancel: Arc::clone(&bg_cancel),
            progress: Arc::clone(&progress),
            vtx: Some(vtx),
            atx,
        })?;

        Ok(Self {
            active,
            video_pending: Some(vrx),
            audio_pending: if cached_audio.is_some() {
                None
            } else {
                Some(arx)
            },
            audio_status: if cached_audio.is_some() {
                AudioState::Ready
            } else if index_audio {
                AudioState::Pending
            } else {
                AudioState::NoAudio
            },
            audio: cached_audio,
            worker: Some(worker),
            cancel: bg_cancel,
            progress,
            store: opts.store.clone(),
            path: path.to_path_buf(),
            video_track: opts.bestsource.track,
            partial: Some(partial),
            fallback_notice,
            runtime_fallback_error: None,
        })
    }

    fn finish_with_cached_video(
        path: &Path,
        opts: &HybridOptions,
        bs: BestSourceProvider,
        audio_opts: &BestSourceOptions,
        progress: Arc<Shared>,
        bg_cancel: Arc<AtomicBool>,
    ) -> Result<Self, MediaError> {
        let mut audio = None;
        let mut audio_pending = None;
        let mut worker = None;
        let mut audio_status = AudioState::NoAudio;

        if opts.index_audio {
            match BestSourceAudio::open_cached_only(path, audio_opts) {
                Ok(aud) => {
                    audio = Some(aud);
                    audio_status = AudioState::Ready;
                }
                Err(_) => match track_types(path) {
                    Ok(types) if !types.contains(&bestsource_sys::AVMEDIA_TYPE_AUDIO) => {
                        audio_status = AudioState::NoAudio;
                    }
                    Ok(_) => {
                        progress.phase.store(PHASE_AUDIO, Ordering::Relaxed);
                        let (atx, arx) = mpsc::channel();
                        worker = Some(spawn_index_worker(WorkerArgs {
                            path: path.to_path_buf(),
                            bs_opts: opts.bestsource.clone(),
                            audio_opts: audio_opts.clone(),
                            index_audio: true,
                            skip_video: true,
                            skip_audio: false,
                            priority: opts.priority,
                            store: opts.store.clone(),
                            partial: None,
                            bg_cancel: Arc::clone(&bg_cancel),
                            progress: Arc::clone(&progress),
                            vtx: None,
                            atx,
                        })?);
                        audio_pending = Some(arx);
                        audio_status = AudioState::Pending;
                    }
                    Err(e) => {
                        audio_status = AudioState::Failed(e.to_string());
                    }
                },
            }
        }

        Ok(Self {
            active: Box::new(bs),
            video_pending: None,
            audio_pending,
            audio,
            audio_status,
            worker,
            cancel: bg_cancel,
            progress,
            store: opts.store.clone(),
            path: path.to_path_buf(),
            video_track: opts.bestsource.track,
            partial: None,
            fallback_notice: None,
            runtime_fallback_error: None,
        })
    }

    /// Whether background indexing (video or audio) is still running.
    #[must_use]
    pub fn is_indexing(&self) -> bool {
        self.video_pending.is_some() || self.audio_pending.is_some()
    }

    /// Whether the video track specifically is still being indexed.
    #[must_use]
    pub fn is_indexing_video(&self) -> bool {
        self.video_pending.is_some()
    }

    /// Number of video frames indexed so far (full frame count once exact).
    #[must_use]
    pub fn indexed_frames(&self) -> usize {
        if self.active.is_frame_exact() {
            self.active.info().num_frames
        } else {
            self.partial.as_ref().map_or(0, PartialIndex::len)
        }
    }

    /// Non-empty fallback notice if a fast/runtime provider fell back (consumes once read).
    pub fn take_fallback_notice(&mut self) -> Option<String> {
        self.fallback_notice.take()
    }

    /// Progress + ETA of the current background index pass (`None` when not indexing).
    #[must_use]
    pub fn index_progress(&self) -> Option<IndexProgress> {
        if !self.is_indexing() {
            return None;
        }
        let phase = match self.progress.phase.load(Ordering::Relaxed) {
            PHASE_AUDIO => IndexPhase::Audio,
            _ => IndexPhase::Video,
        };
        let cur = self.progress.cur.load(Ordering::Relaxed);
        let tot = self.progress.total.load(Ordering::Relaxed);
        let fraction = if tot > 0 {
            (cur as f64 / tot as f64).clamp(0.0, 1.0)
        } else {
            0.0
        };
        let elapsed = self
            .progress
            .phase_started
            .lock()
            .unwrap_or_else(PoisonError::into_inner)
            .elapsed();
        let eta = (fraction >= 0.02 && elapsed >= Duration::from_secs(1))
            .then(|| elapsed.mul_f64((1.0 - fraction) / fraction));
        #[allow(clippy::cast_possible_truncation)]
        let fraction = fraction as f32;
        Some(IndexProgress {
            phase,
            fraction,
            elapsed,
            eta,
        })
    }

    /// Non-blocking: switches to `BestSource` as soon as video indexing finishes (before audio
    /// indexing finishes).
    pub fn poll_upgrade(&mut self) -> UpgradeStatus {
        if let Some(err) = self.runtime_fallback_error.take() {
            return UpgradeStatus::Failed(err);
        }
        let Some(rx) = &self.video_pending else {
            return UpgradeStatus::Done;
        };
        let status = match rx.try_recv() {
            Err(TryRecvError::Empty) => return UpgradeStatus::Pending,
            Err(TryRecvError::Disconnected) => {
                self.video_pending = None;
                self.join_worker_if_idle();
                return UpgradeStatus::Failed(MediaError::BestSource(
                    "indexing thread died".to_owned(),
                ));
            }
            Ok(Ok(bs)) => {
                self.active = Box::new(bs);
                self.partial = None;
                if let Some(store) = &self.store {
                    let _ = store.touch(&self.path);
                }
                UpgradeStatus::Switched
            }
            Ok(Err(e)) => {
                // Audio was indexed first and is independent: keep whatever it produced.
                self.video_pending = None;
                self.join_worker_if_idle();
                return UpgradeStatus::Failed(e);
            }
        };
        self.video_pending = None;
        self.join_worker_if_idle();
        status
    }

    /// Non-blocking: polls the background audio index pass.
    pub fn poll_audio(&mut self) -> AudioUpgradeStatus {
        if let Some(rx) = &self.audio_pending {
            match rx.try_recv() {
                Err(TryRecvError::Empty) => return AudioUpgradeStatus::Pending,
                Err(TryRecvError::Disconnected) => {
                    self.audio_pending = None;
                    self.audio_status = AudioState::Failed("audio indexing thread died".to_owned());
                    self.join_worker_if_idle();
                    return AudioUpgradeStatus::Failed(MediaError::BestSource(
                        "audio indexing thread died".to_owned(),
                    ));
                }
                Ok(AudioResult::Ready(aud)) => {
                    self.audio = Some(aud);
                    self.audio_status = AudioState::Ready;
                    if let Some(store) = &self.store {
                        let _ = store.touch(&self.path);
                    }
                    self.audio_pending = None;
                    self.join_worker_if_idle();
                    return AudioUpgradeStatus::Ready;
                }
                Ok(AudioResult::NoAudio) => {
                    self.audio_status = AudioState::NoAudio;
                    self.audio_pending = None;
                    self.join_worker_if_idle();
                    return AudioUpgradeStatus::NoAudio;
                }
                Ok(AudioResult::Failed(e)) => {
                    let msg = e.to_string();
                    self.audio_status = AudioState::Failed(msg);
                    self.audio_pending = None;
                    self.join_worker_if_idle();
                    return AudioUpgradeStatus::Failed(e);
                }
            }
        }
        match &self.audio_status {
            AudioState::Pending => AudioUpgradeStatus::Pending,
            AudioState::Ready => AudioUpgradeStatus::Ready,
            AudioState::NoAudio => AudioUpgradeStatus::NoAudio,
            AudioState::Failed(msg) => {
                AudioUpgradeStatus::Failed(MediaError::BestSource(msg.clone()))
            }
        }
    }

    /// Blocks until the background worker (video + audio) finishes, then updates both states.
    pub fn wait_upgrade(&mut self) -> UpgradeStatus {
        if let Some(w) = self.worker.take() {
            let _ = w.join();
        }
        let status = self.poll_upgrade();
        let _ = self.poll_audio();
        status
    }

    /// Borrow the exact `BestSourceAudio` provider once audio indexing has completed.
    #[must_use]
    pub fn audio(&self) -> Option<&BestSourceAudio> {
        self.audio.as_ref()
    }

    /// Mutably borrow the exact `BestSourceAudio` provider once audio indexing has completed.
    pub fn audio_mut(&mut self) -> Option<&mut BestSourceAudio> {
        self.audio.as_mut()
    }

    /// Take ownership of the exact `BestSourceAudio` provider once audio indexing has completed.
    pub fn take_audio(&mut self) -> Option<BestSourceAudio> {
        self.audio.take()
    }

    fn join_worker_if_idle(&mut self) {
        if self.video_pending.is_none() && self.audio_pending.is_none() {
            if let Some(w) = self.worker.take() {
                let _ = w.join();
            }
        }
    }

    fn fallback_after_decode_error(
        &mut self,
        frame_index: usize,
        out_rgba: &mut Vec<u8>,
        err: MediaError,
    ) -> Result<i64, MediaError> {
        let cancel = AtomicBool::new(false);
        let fallback: Option<(Box<dyn VideoProvider>, String)> = match self.active.kind() {
            ProviderKind::BestSource => {
                if let Ok(mut ff) = FfmsVideo::open_with_progress(&self.path, &cancel, |_, _| {}) {
                    if ff.decode_frame_into(frame_index, out_rgba).is_ok() {
                        let msg = format!("BestSource decode failed ({err}); fell back to FFMS2.");
                        self.active = Box::new(ff);
                        self.runtime_fallback_error = Some(err);
                        self.fallback_notice = Some(msg);
                        return self.active.decode_frame_into(frame_index, out_rgba);
                    }
                }
                QuickVideo::open(&self.path, self.video_track, 4)
                    .ok()
                    .map(|q| {
                        (
                            Box::new(q) as Box<dyn VideoProvider>,
                            format!(
                                "BestSource decode failed ({err}); fell back to quick decoder."
                            ),
                        )
                    })
            }
            ProviderKind::Quick => FfmsVideo::open_with_progress(&self.path, &cancel, |_, _| {})
                .ok()
                .map(|ff| {
                    (
                        Box::new(ff) as Box<dyn VideoProvider>,
                        format!("Quick decode failed ({err}); fell back to FFMS2."),
                    )
                }),
            ProviderKind::Ffms2 => None,
        };

        if let Some((mut fb, msg)) = fallback {
            if let Ok(pts) = fb.decode_frame_into(frame_index, out_rgba) {
                self.active = fb;
                self.partial = None;
                self.runtime_fallback_error = Some(err);
                self.fallback_notice = Some(msg);
                return Ok(pts);
            }
        }
        Err(err)
    }
}

/// The video index is missing or stale. A finished audio index (audio is indexed first) is kept
/// and reused; only a wholly unusable cache is dropped so it gets rebuilt.
fn salvage_cached_audio(
    store: &IndexStore,
    path: &Path,
    index_audio: bool,
    audio_opts: &BestSourceOptions,
) -> Option<BestSourceAudio> {
    let audio = if index_audio {
        BestSourceAudio::open_cached_only(path, audio_opts).ok()
    } else {
        None
    };
    if audio.is_none() {
        let _ = store.remove_entries_for(path);
    }
    audio
}

fn open_fast_provider(
    path: &Path,
    fast: FastOpen,
    track: i32,
    quick_threads: i32,
    partial: &PartialIndex,
    cancel: &AtomicBool,
    mut on_progress: impl FnMut(i64, i64),
) -> Result<(Box<dyn VideoProvider>, Option<String>), MediaError> {
    match fast {
        FastOpen::Quick => {
            match QuickVideo::open_with_partial(path, track, quick_threads, Some(partial.clone())) {
                Ok(q) => Ok((Box::new(q), None)),
                Err(quick_err) => {
                    let ff = match FfmsVideo::open_with_progress(path, cancel, &mut on_progress) {
                        Ok(ff) => ff,
                        Err(MediaError::Cancelled) => return Err(MediaError::Cancelled),
                        Err(ff_err) => {
                            return Err(MediaError::BestSource(format!(
                                "{quick_err}; FFMS2 fallback failed: {ff_err}"
                            )));
                        }
                    };
                    Ok((
                        Box::new(ff),
                        Some(format!(
                            "Quick decoder unavailable ({quick_err}); using FFMS2."
                        )),
                    ))
                }
            }
        }
        FastOpen::Ffms2 => match FfmsVideo::open_with_progress(path, cancel, &mut on_progress) {
            Ok(ff) => Ok((Box::new(ff), None)),
            Err(ff_err) => {
                if matches!(ff_err, MediaError::Cancelled) {
                    return Err(ff_err);
                }
                let q = match QuickVideo::open_with_partial(
                    path,
                    track,
                    quick_threads,
                    Some(partial.clone()),
                ) {
                    Ok(q) => q,
                    Err(quick_err) => {
                        return Err(MediaError::Ffms(format!(
                            "{ff_err}; quick fallback failed: {quick_err}"
                        )));
                    }
                };
                Ok((
                    Box::new(q),
                    Some(format!(
                        "FFMS2 unavailable ({ff_err}); using quick decoder."
                    )),
                ))
            }
        },
    }
}

struct WorkerArgs {
    path: PathBuf,
    bs_opts: BestSourceOptions,
    audio_opts: BestSourceOptions,
    index_audio: bool,
    skip_video: bool,
    skip_audio: bool,
    priority: ThreadPriority,
    store: Option<IndexStore>,
    partial: Option<PartialIndex>,
    bg_cancel: Arc<AtomicBool>,
    progress: Arc<Shared>,
    vtx: Option<mpsc::Sender<Result<BestSourceProvider, MediaError>>>,
    atx: mpsc::Sender<AudioResult>,
}

fn begin_phase(progress: &Shared, phase: u8) {
    progress.phase.store(phase, Ordering::Relaxed);
    progress.cur.store(0, Ordering::Relaxed);
    progress.total.store(0, Ordering::Relaxed);
    *progress
        .phase_started
        .lock()
        .unwrap_or_else(PoisonError::into_inner) = Instant::now();
}

/// Audio phase (runs FIRST so playback audio can start while the video index is still building).
fn run_audio_phase(args: &WorkerArgs) {
    if args.skip_audio {
        return;
    }
    if !args.index_audio || args.bg_cancel.load(Ordering::Relaxed) {
        let _ = args.atx.send(AudioResult::NoAudio);
        return;
    }
    match track_types(&args.path) {
        Ok(types) if !types.contains(&bestsource_sys::AVMEDIA_TYPE_AUDIO) => {
            let _ = args.atx.send(AudioResult::NoAudio);
        }
        Ok(_) => {
            begin_phase(&args.progress, PHASE_AUDIO);
            let audio_res = BestSourceAudio::open_with_progress(
                &args.path,
                &args.audio_opts,
                &args.bg_cancel,
                |cur, tot| {
                    if cur != i64::MAX {
                        args.progress.cur.store(cur, Ordering::Relaxed);
                        args.progress.total.store(tot, Ordering::Relaxed);
                    }
                },
            );
            match audio_res {
                Ok(aud) => {
                    if let Some(store) = &args.store {
                        let _ = store.touch(&args.path);
                        let _ = store.enforce_limit_preserving(Some(&args.path));
                    }
                    let _ = args.atx.send(AudioResult::Ready(aud));
                }
                Err(e) => {
                    let _ = args.atx.send(AudioResult::Failed(e));
                }
            }
        }
        Err(e) => {
            let _ = args.atx.send(AudioResult::Failed(e));
        }
    }
}

fn spawn_index_worker(args: WorkerArgs) -> Result<JoinHandle<()>, MediaError> {
    std::thread::Builder::new()
        .name("bestsource-index".to_owned())
        .spawn(move || {
            let _ = set_current_thread_priority(args.priority);
            // Audio first: a failed audio index never blocks the video index.
            run_audio_phase(&args);
            if args.skip_video || args.bg_cancel.load(Ordering::Relaxed) {
                return;
            }
            begin_phase(&args.progress, PHASE_VIDEO);
            let res = BestSourceProvider::open_with_partial_and_progress(
                &args.path,
                &args.bs_opts,
                args.partial.as_ref(),
                &args.bg_cancel,
                |cur, tot| {
                    if cur != i64::MAX {
                        args.progress.cur.store(cur, Ordering::Relaxed);
                        args.progress.total.store(tot, Ordering::Relaxed);
                    }
                },
            );
            if res.is_ok() {
                if let Some(store) = &args.store {
                    let _ = store.touch(&args.path);
                    let _ = store.enforce_limit_preserving(Some(&args.path));
                }
            }
            if let Some(vtx) = &args.vtx {
                let _ = vtx.send(res);
            }
        })
        .map_err(|e| MediaError::BestSource(e.to_string()))
}

impl Drop for HybridVideo {
    fn drop(&mut self) {
        self.cancel.store(true, Ordering::Relaxed);
        if let Some(w) = self.worker.take() {
            let _ = w.join();
        }
    }
}

impl VideoProvider for HybridVideo {
    fn kind(&self) -> ProviderKind {
        self.active.kind()
    }

    fn is_frame_exact(&self) -> bool {
        self.active.is_frame_exact()
    }

    fn is_frame_exact_at(&self, frame_index: usize) -> bool {
        self.active.is_frame_exact_at(frame_index)
    }

    fn info(&self) -> &VideoInfo {
        self.active.info()
    }

    fn decode_frame_into(
        &mut self,
        frame_index: usize,
        out_rgba: &mut Vec<u8>,
    ) -> Result<i64, MediaError> {
        match self.active.decode_frame_into(frame_index, out_rgba) {
            Ok(pts) => Ok(pts),
            Err(err @ (MediaError::Cancelled | MediaError::FrameOutOfRange { .. })) => Err(err),
            Err(err) => self.fallback_after_decode_error(frame_index, out_rgba, err),
        }
    }

    fn next_keyframe(&self, current: usize) -> Option<usize> {
        self.active.next_keyframe(current)
    }

    fn prev_keyframe(&self, current: usize) -> Option<usize> {
        self.active.prev_keyframe(current)
    }
}
