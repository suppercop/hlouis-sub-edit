//! The common media provider traits and runtime provider selection.

use crate::bestsource::{BestSourceOptions, BestSourceProvider};
use crate::ffi::win::{set_current_thread_priority, ThreadPriority};
use crate::index_store::IndexStore;
use crate::quick::QuickVideo;
use crate::{FfmsVideo, MediaError, VideoInfo};
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicBool, AtomicI64, Ordering};
use std::sync::mpsc::{self, Receiver, TryRecvError};
use std::sync::Arc;
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

/// What [`HybridVideo::poll_upgrade`] observed.
#[derive(Debug)]
pub enum UpgradeStatus {
    /// `BestSource` is still indexing in the background.
    Pending,
    /// Just switched to `BestSource`; callers should re-read [`VideoProvider::info`].
    Switched,
    /// Already on `BestSource` (or never going to switch).
    Done,
    /// Background `BestSource` open failed; staying on the fast path.
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
    /// Options of the background `BestSource` open (`threads` 0 = [`background_decode_threads`]).
    pub bestsource: BestSourceOptions,
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
            store: IndexStore::default_location(),
            priority: ThreadPriority::BelowNormal,
        }
    }
}

/// Progress of the background exact index.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct IndexProgress {
    /// 0.0 ..= 1.0 (bytes demuxed / file size).
    pub fraction: f32,
    pub elapsed: Duration,
    /// Remaining time extrapolated from the rate so far (`None` until 2 % or 1 s in).
    pub eta: Option<Duration>,
}

struct Shared {
    cur: AtomicI64,
    total: AtomicI64,
}

/// Instant open + frame-exact switch: frames come from a fast provider immediately (see
/// [`FastOpen`]) while `BestSource` indexes on a below-normal-priority worker thread; once the
/// index is ready [`Self::poll_upgrade`] switches to it. With a cached index the exact provider
/// opens directly. Indexing is cancelled when this value is dropped (file closed).
///
/// `BestSource` R23 builds its index in one pass inside its constructor and cannot serve exact
/// frames from a partial index, so exact operations become available only once it finishes.
pub struct HybridVideo {
    active: Box<dyn VideoProvider>,
    pending: Option<Receiver<Result<BestSourceProvider, MediaError>>>,
    worker: Option<JoinHandle<()>>,
    cancel: Arc<AtomicBool>,
    progress: Arc<Shared>,
    started: Instant,
    store: Option<IndexStore>,
    path: PathBuf,
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
        let progress = Arc::new(Shared {
            cur: AtomicI64::new(0),
            total: AtomicI64::new(0),
        });
        let bg_cancel = Arc::new(AtomicBool::new(false));
        let assemble = |active: Box<dyn VideoProvider>,
                        pending: Option<Receiver<Result<BestSourceProvider, MediaError>>>,
                        worker: Option<JoinHandle<()>>| Self {
            active,
            pending,
            worker,
            cancel: Arc::clone(&bg_cancel),
            progress: Arc::clone(&progress),
            started: Instant::now(),
            store: opts.store.clone(),
            path: path.to_path_buf(),
        };

        if opts.store.as_ref().is_some_and(|s| s.has_index(path)) {
            let bs = BestSourceProvider::open_with_progress(path, &bs_opts, cancel, |_, _| {})?;
            if let Some(store) = &opts.store {
                let _ = store.touch(path);
            }
            return Ok(assemble(Box::new(bs), None, None));
        }

        let active: Box<dyn VideoProvider> = match opts.fast {
            FastOpen::Quick => Box::new(QuickVideo::open(path, bs_opts.track, 0)?),
            FastOpen::Ffms2 => Box::new(FfmsVideo::open_with_progress(path, cancel, on_progress)?),
        };
        if bs_opts.threads <= 0 {
            bs_opts.threads = background_decode_threads();
        }
        let (tx, rx) = mpsc::channel();
        let worker = {
            let path = path.to_path_buf();
            let priority = opts.priority;
            let store = opts.store.clone();
            let bg_cancel = Arc::clone(&bg_cancel);
            let progress = Arc::clone(&progress);
            std::thread::Builder::new()
                .name("bestsource-index".to_owned())
                .spawn(move || {
                    let _ = set_current_thread_priority(priority);
                    let res = BestSourceProvider::open_with_progress(
                        &path,
                        &bs_opts,
                        &bg_cancel,
                        |cur, tot| {
                            if cur != i64::MAX {
                                progress.cur.store(cur, Ordering::Relaxed);
                                progress.total.store(tot, Ordering::Relaxed);
                            }
                        },
                    );
                    if res.is_ok() {
                        if let Some(store) = store {
                            let _ = store.enforce_limit();
                        }
                    }
                    let _ = tx.send(res);
                })
                .map_err(|e| MediaError::BestSource(e.to_string()))?
        };
        Ok(assemble(active, Some(rx), Some(worker)))
    }

    /// Whether the background exact index is still being built.
    #[must_use]
    pub fn is_indexing(&self) -> bool {
        self.pending.is_some()
    }

    /// Progress + ETA of the background index (`None` when not indexing).
    #[must_use]
    pub fn index_progress(&self) -> Option<IndexProgress> {
        self.pending.as_ref()?;
        let cur = self.progress.cur.load(Ordering::Relaxed);
        let tot = self.progress.total.load(Ordering::Relaxed);
        let fraction = if tot > 0 {
            (cur as f64 / tot as f64).clamp(0.0, 1.0)
        } else {
            0.0
        };
        let elapsed = self.started.elapsed();
        let eta = (fraction >= 0.02 && elapsed >= Duration::from_secs(1))
            .then(|| elapsed.mul_f64((1.0 - fraction) / fraction));
        #[allow(clippy::cast_possible_truncation)]
        let fraction = fraction as f32;
        Some(IndexProgress {
            fraction,
            elapsed,
            eta,
        })
    }

    /// Non-blocking: switches to `BestSource` once its background open has finished.
    pub fn poll_upgrade(&mut self) -> UpgradeStatus {
        let Some(rx) = &self.pending else {
            return UpgradeStatus::Done;
        };
        let status = match rx.try_recv() {
            Err(TryRecvError::Empty) => return UpgradeStatus::Pending,
            Err(TryRecvError::Disconnected) => {
                UpgradeStatus::Failed(MediaError::BestSource("indexing thread died".to_owned()))
            }
            Ok(Ok(bs)) => {
                self.active = Box::new(bs);
                if let Some(store) = &self.store {
                    let _ = store.touch(&self.path);
                }
                UpgradeStatus::Switched
            }
            Ok(Err(e)) => UpgradeStatus::Failed(e),
        };
        self.pending = None;
        if let Some(w) = self.worker.take() {
            let _ = w.join();
        }
        status
    }

    /// Blocks until the background open finishes, then switches like [`Self::poll_upgrade`].
    pub fn wait_upgrade(&mut self) -> UpgradeStatus {
        if let Some(w) = self.worker.take() {
            let _ = w.join();
        }
        self.poll_upgrade()
    }
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

    fn info(&self) -> &VideoInfo {
        self.active.info()
    }

    fn decode_frame_into(
        &mut self,
        frame_index: usize,
        out_rgba: &mut Vec<u8>,
    ) -> Result<i64, MediaError> {
        self.active.decode_frame_into(frame_index, out_rgba)
    }
}
