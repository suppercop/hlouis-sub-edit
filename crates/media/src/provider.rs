//! The common media provider traits and runtime provider selection.

use crate::bestsource::{BestSourceOptions, BestSourceProvider};
use crate::{FfmsVideo, MediaError, VideoInfo};
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicBool, AtomicI64, Ordering};
use std::sync::mpsc::{self, Receiver, TryRecvError};
use std::sync::Arc;
use std::thread::JoinHandle;

/// Which decoding backend serves a track.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Default)]
pub enum ProviderKind {
    /// `BestSource`: frame-accurate, cached index; the default.
    #[default]
    BestSource,
    /// `FFMS2`: fast-open fallback.
    Ffms2,
}

impl ProviderKind {
    /// Stable name for settings files.
    #[must_use]
    pub fn as_str(self) -> &'static str {
        match self {
            Self::BestSource => "bestsource",
            Self::Ffms2 => "ffms2",
        }
    }

    /// Parses [`Self::as_str`] (case-insensitive).
    #[must_use]
    pub fn parse(s: &str) -> Option<Self> {
        if s.eq_ignore_ascii_case("bestsource") {
            Some(Self::BestSource)
        } else if s.eq_ignore_ascii_case("ffms2") {
            Some(Self::Ffms2)
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
    })
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
    /// Background `BestSource` open failed; staying on `FFMS2`.
    Failed(MediaError),
}

/// `FFMS2` for a fast first open, `BestSource` indexed on a worker thread, switched in when ready.
///
/// `BestSource` indexing progress is readable through [`Self::background_progress`], and it is
/// cancelled when this value is dropped.
pub struct HybridVideo {
    active: Box<dyn VideoProvider>,
    pending: Option<Receiver<Result<BestSourceProvider, MediaError>>>,
    worker: Option<JoinHandle<()>>,
    cancel: Arc<AtomicBool>,
    progress: Arc<(AtomicI64, AtomicI64)>,
}

impl HybridVideo {
    /// Opens `FFMS2` now (blocking, with `on_progress`) and starts opening `BestSource` with
    /// `opts` on a worker thread; with a cached index the switch is near-immediate.
    pub fn open(
        path: &Path,
        opts: &BestSourceOptions,
        cancel: &AtomicBool,
        on_progress: impl FnMut(i64, i64),
    ) -> Result<Self, MediaError> {
        let ffms = FfmsVideo::open_with_progress(path, cancel, on_progress)?;
        let bg_cancel = Arc::new(AtomicBool::new(false));
        let progress = Arc::new((AtomicI64::new(0), AtomicI64::new(0)));
        let (tx, rx) = mpsc::channel();
        let worker = {
            let path: PathBuf = path.to_path_buf();
            let opts = opts.clone();
            let bg_cancel = Arc::clone(&bg_cancel);
            let progress = Arc::clone(&progress);
            std::thread::Builder::new()
                .name("bestsource-index".to_owned())
                .spawn(move || {
                    let res = BestSourceProvider::open_with_progress(
                        &path,
                        &opts,
                        &bg_cancel,
                        |cur, tot| {
                            progress.0.store(cur, Ordering::Relaxed);
                            progress.1.store(tot, Ordering::Relaxed);
                        },
                    );
                    let _ = tx.send(res);
                })
                .map_err(|e| MediaError::BestSource(e.to_string()))?
        };
        Ok(Self {
            active: Box::new(ffms),
            pending: Some(rx),
            worker: Some(worker),
            cancel: bg_cancel,
            progress,
        })
    }

    /// `(current_bytes, total_bytes)` of the background `BestSource` indexing.
    #[must_use]
    pub fn background_progress(&self) -> (i64, i64) {
        (
            self.progress.0.load(Ordering::Relaxed),
            self.progress.1.load(Ordering::Relaxed),
        )
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
