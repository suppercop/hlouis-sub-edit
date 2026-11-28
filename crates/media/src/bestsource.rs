//! `BestSource` video/audio providers (safe layer over `ffi::bestsource`).

use crate::ffi::bestsource::{self as raw, OpenArgs, PartialIndex, RawBsAudio, RawBsVideo};
use crate::provider::{AudioInfo, AudioProvider, ProviderKind, VideoProvider};
use crate::{pts_to_ms_rounded, MediaError, VideoInfo};
use std::path::{Path, PathBuf};
use std::sync::atomic::AtomicBool;
use std::sync::Arc;

/// Where `BestSource` keeps its on-disk index.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub enum IndexCache {
    /// Never read or write an index file (index in memory every time).
    Disabled,
    /// `BestSource`'s per-user cache tree (`%LOCALAPPDATA%\bsindex`), written for >= 100 frames.
    #[default]
    Default,
    /// Always write, under this directory (mirrors the source path below it).
    Dir(PathBuf),
    /// Always write, to `<this path>.<track>.bsindex`.
    File(PathBuf),
}

impl IndexCache {
    fn mode_and_path(&self) -> (i32, Option<&Path>) {
        match self {
            Self::Disabled => (bestsource_sys::BS_CACHE_DISABLE, None),
            Self::Default => (bestsource_sys::BS_CACHE_AUTO_SUBTREE, None),
            Self::Dir(p) => (bestsource_sys::BS_CACHE_ALWAYS_SUBTREE, Some(p.as_path())),
            Self::File(p) => (bestsource_sys::BS_CACHE_ALWAYS_ABSOLUTE, Some(p.as_path())),
        }
    }
}

/// Options for opening a `BestSource` track.
#[derive(Debug, Clone)]
pub struct BestSourceOptions {
    /// Absolute stream index, or negative for the n-th track of the kind (-1 = first).
    pub track: i32,
    /// Decoder threads; 0 = automatic.
    pub threads: i32,
    pub cache: IndexCache,
}

impl Default for BestSourceOptions {
    fn default() -> Self {
        Self {
            track: -1,
            threads: 0,
            cache: IndexCache::Default,
        }
    }
}

/// Pixel layout of [`BestSourceProvider::decode_planar_into`] output.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct PlanarFormat {
    /// 1 gray, 2 RGB, 3 YUV.
    pub color_family: i32,
    pub bits: i32,
    pub bytes_per_sample: usize,
    pub sub_sampling_w: u32,
    pub sub_sampling_h: u32,
    /// Width/height of each of the three planes in samples.
    pub plane_dims: [(u32, u32); 3],
}

/// One decoded frame as three planes in their native sample format.
#[derive(Debug, Default)]
pub struct PlanarFrame {
    pub planes: [Vec<u8>; 3],
    /// Byte stride of each plane (tightly packed).
    pub strides: [usize; 3],
}

/// Frame-accurate `BestSource` video provider producing RGBA8 or native planar frames.
pub struct BestSourceProvider {
    raw: RawBsVideo,
    info: VideoInfo,
    planar: PlanarFormat,
    track: i32,
    cancel: Arc<AtomicBool>,
}

impl BestSourceProvider {
    /// `BestSource` release number times 100 (R23 -> 2300).
    #[must_use]
    pub fn version() -> i32 {
        RawBsVideo::version()
    }

    /// Sets the `FFmpeg` log level used by `BestSource` (`AV_LOG_*`, e.g. 16 = error).
    pub fn set_ffmpeg_log_level(level: i32) {
        raw::set_ffmpeg_log_level(level);
    }

    /// Indexes (or loads the cached index of) the selected video track, reporting
    /// `(current_bytes, total_bytes)` progress. Setting `cancel` aborts indexing.
    pub fn open_with_progress(
        path: &Path,
        opts: &BestSourceOptions,
        cancel: &AtomicBool,
        on_progress: impl FnMut(i64, i64),
    ) -> Result<Self, MediaError> {
        Self::open_with_partial_and_progress(path, opts, None, cancel, on_progress)
    }

    /// Indexes (or loads the cached index of) the selected video track while streaming frame PTS
    /// and keyframe flags into `partial` as frames are indexed.
    pub fn open_with_partial_and_progress(
        path: &Path,
        opts: &BestSourceOptions,
        partial: Option<&PartialIndex>,
        cancel: &AtomicBool,
        mut on_progress: impl FnMut(i64, i64),
    ) -> Result<Self, MediaError> {
        let (cache_mode, cache_path) = opts.cache.mode_and_path();
        let args = OpenArgs {
            path,
            track: opts.track,
            threads: opts.threads,
            cache_mode,
            cache_path,
        };
        let raw = RawBsVideo::open(&args, partial, cancel, &mut on_progress)?;
        Self::from_raw(raw)
    }

    /// Opens the selected video track strictly from an existing valid on-disk index cache.
    /// Fails immediately without indexing if the cache is missing, stale, or corrupted.
    pub fn open_cached_only(path: &Path, opts: &BestSourceOptions) -> Result<Self, MediaError> {
        let (cache_mode, cache_path) = opts.cache.mode_and_path();
        let args = OpenArgs {
            path,
            track: opts.track,
            threads: opts.threads,
            cache_mode,
            cache_path,
        };
        let raw = RawBsVideo::open_cached(&args)?;
        Self::from_raw(raw)
    }

    /// Caps `BestSource`'s internal decoded-frame cache and open decoder instance count.
    pub fn set_limits(&mut self, max_cache_bytes: usize, max_decoders: i32) {
        self.raw.set_limits(max_cache_bytes, max_decoders);
    }

    fn from_raw(mut raw: RawBsVideo) -> Result<Self, MediaError> {
        let flag = Arc::new(AtomicBool::new(false));
        raw.set_cancel(Arc::clone(&flag));
        let p = raw.props();
        let width = u32::try_from(p.width).map_err(|_| MediaError::InvalidMetadata)?;
        let height = u32::try_from(p.height).map_err(|_| MediaError::InvalidMetadata)?;
        let num_frames = usize::try_from(p.num_frames).map_err(|_| MediaError::InvalidMetadata)?;
        if width == 0 || height == 0 || num_frames == 0 {
            return Err(MediaError::InvalidMetadata);
        }
        let (pts, key) = raw.frame_table(num_frames)?;
        let tb_num = i64::from(p.time_base_num) * 1000;
        let tb_den = i64::from(p.time_base_den);
        let frame_pts_ms: Vec<i64> = pts
            .iter()
            .map(|&t| pts_to_ms_rounded(t, tb_num, tb_den))
            .collect();
        let keyframes: Vec<usize> = key
            .iter()
            .enumerate()
            .filter_map(|(i, &k)| (k != 0).then_some(i))
            .collect();
        let fps_num = i64::from(p.fps_num);
        let fps_den = i64::from(p.fps_den);
        let fps = if fps_den > 0 && fps_num > 0 {
            fps_num as f64 / fps_den as f64
        } else {
            23.976
        };
        let first = frame_pts_ms.first().copied().unwrap_or(0);
        let duration_ms = if p.duration > 0 && tb_den > 0 {
            first as f64 + (p.duration as f64 * tb_num as f64 / tb_den as f64)
        } else {
            frame_pts_ms.last().copied().unwrap_or(0) as f64 + 1000.0 / fps.max(1.0)
        };
        let planar = planar_format(&p, width, height);
        Ok(Self {
            raw,
            info: VideoInfo {
                width,
                height,
                num_frames,
                fps_num,
                fps_den,
                fps,
                duration_ms,
                keyframes,
                frame_pts_ms,
                sar_num: u32::try_from(p.sar_num).unwrap_or(0),
                sar_den: u32::try_from(p.sar_den).unwrap_or(0),
            },
            planar,
            track: p.track,
            cancel: flag,
        })
    }

    /// The absolute stream index that was opened.
    #[must_use]
    pub fn track(&self) -> i32 {
        self.track
    }

    /// Flag that abandons an in-flight seek/decode when set from another thread
    /// (the call then returns [`MediaError::Cancelled`]). Reset it before the next call.
    #[must_use]
    pub fn cancel_flag(&self) -> Arc<AtomicBool> {
        Arc::clone(&self.cancel)
    }

    /// Layout of [`Self::decode_planar_into`] output.
    #[must_use]
    pub fn planar_format(&self) -> PlanarFormat {
        self.planar
    }

    /// Decodes frame `n` as planar data in its native format (no colour conversion).
    pub fn decode_planar_into(
        &mut self,
        frame_index: usize,
        out: &mut PlanarFrame,
    ) -> Result<(), MediaError> {
        self.check_range(frame_index)?;
        let mut min_lens = [0usize; 3];
        for i in 0..3 {
            let (w, h) = self.planar.plane_dims[i];
            out.strides[i] = w as usize * self.planar.bytes_per_sample;
            min_lens[i] = out.strides[i] * h as usize;
            out.planes[i].resize(min_lens[i].max(1), 0);
        }
        let [a, b, c] = &mut out.planes;
        self.raw.frame_planar(
            i64::try_from(frame_index).map_err(|_| MediaError::InvalidMetadata)?,
            [a.as_mut_slice(), b.as_mut_slice(), c.as_mut_slice()],
            out.strides,
            min_lens,
        )
    }

    fn check_range(&self, frame_index: usize) -> Result<(), MediaError> {
        if frame_index >= self.info.num_frames {
            return Err(MediaError::FrameOutOfRange {
                frame: frame_index,
                total: self.info.num_frames,
            });
        }
        Ok(())
    }
}

fn planar_format(p: &bestsource_sys::bs_video_props, width: u32, height: u32) -> PlanarFormat {
    let ssw = u32::try_from(p.sub_sampling_w.clamp(0, 4)).unwrap_or(0);
    let ssh = u32::try_from(p.sub_sampling_h.clamp(0, 4)).unwrap_or(0);
    // ExportAsPlanar writes SSMod dimensions (largest multiple of the subsampling).
    let w = width - width % (1 << ssw);
    let h = height - height % (1 << ssh);
    let chroma = if p.color_family == 1 {
        (0, 0)
    } else {
        (w >> ssw, h >> ssh)
    };
    PlanarFormat {
        color_family: p.color_family,
        bits: p.bits,
        bytes_per_sample: if p.bits > 8 { 2 } else { 1 },
        sub_sampling_w: ssw,
        sub_sampling_h: ssh,
        plane_dims: [(w, h), chroma, chroma],
    }
}

impl VideoProvider for BestSourceProvider {
    fn kind(&self) -> ProviderKind {
        ProviderKind::BestSource
    }

    fn info(&self) -> &VideoInfo {
        &self.info
    }

    fn decode_frame_into(
        &mut self,
        frame_index: usize,
        out_rgba: &mut Vec<u8>,
    ) -> Result<i64, MediaError> {
        self.check_range(frame_index)?;
        let total = (self.info.width as usize)
            .checked_mul(self.info.height as usize)
            .and_then(|n| n.checked_mul(4))
            .ok_or(MediaError::InvalidMetadata)?;
        out_rgba.resize(total, 0);
        let n = i64::try_from(frame_index).map_err(|_| MediaError::InvalidMetadata)?;
        self.raw
            .frame_rgba(n, out_rgba, self.info.width, self.info.height)?;
        Ok(self.info.frame_pts_ms[frame_index])
    }
}

/// Sample-accurate `BestSource` audio provider.
pub struct BestSourceAudio {
    raw: RawBsAudio,
    info: AudioInfo,
    cancel: Arc<AtomicBool>,
}

impl BestSourceAudio {
    /// Indexes (or loads the cached index of) the selected audio track.
    pub fn open_with_progress(
        path: &Path,
        opts: &BestSourceOptions,
        cancel: &AtomicBool,
        mut on_progress: impl FnMut(i64, i64),
    ) -> Result<Self, MediaError> {
        let (cache_mode, cache_path) = opts.cache.mode_and_path();
        let args = OpenArgs {
            path,
            track: opts.track,
            threads: opts.threads,
            cache_mode,
            cache_path,
        };
        let raw = RawBsAudio::open(&args, cancel, &mut on_progress)?;
        Self::from_raw(raw)
    }

    /// Opens the selected audio track strictly from an existing valid on-disk index cache.
    pub fn open_cached_only(path: &Path, opts: &BestSourceOptions) -> Result<Self, MediaError> {
        let (cache_mode, cache_path) = opts.cache.mode_and_path();
        let args = OpenArgs {
            path,
            track: opts.track,
            threads: opts.threads,
            cache_mode,
            cache_path,
        };
        let raw = RawBsAudio::open_cached(&args)?;
        Self::from_raw(raw)
    }

    fn from_raw(mut raw: RawBsAudio) -> Result<Self, MediaError> {
        let flag = Arc::new(AtomicBool::new(false));
        raw.set_cancel(Arc::clone(&flag));
        let p = raw.props();
        let info = AudioInfo {
            track: p.track,
            sample_rate: u32::try_from(p.sample_rate).map_err(|_| MediaError::InvalidMetadata)?,
            channels: u32::try_from(p.channels).map_err(|_| MediaError::InvalidMetadata)?,
            bits_per_sample: u32::try_from(p.bits_per_sample)
                .map_err(|_| MediaError::InvalidMetadata)?,
            bytes_per_sample: u32::try_from(p.bytes_per_sample)
                .map_err(|_| MediaError::InvalidMetadata)?,
            is_float: p.is_float != 0,
            num_samples: u64::try_from(p.num_samples).map_err(|_| MediaError::InvalidMetadata)?,
            start_time_s: p.start_time,
        };
        if info.sample_rate == 0 || info.channels == 0 || info.bytes_per_sample == 0 {
            return Err(MediaError::InvalidMetadata);
        }
        Ok(Self {
            raw,
            info,
            cancel: flag,
        })
    }

    /// Flag that abandons an in-flight decode when set from another thread.
    #[must_use]
    pub fn cancel_flag(&self) -> Arc<AtomicBool> {
        Arc::clone(&self.cancel)
    }
}

impl AudioProvider for BestSourceAudio {
    fn kind(&self) -> ProviderKind {
        ProviderKind::BestSource
    }

    fn audio_info(&self) -> &AudioInfo {
        &self.info
    }

    fn samples_into(
        &mut self,
        start: u64,
        count: usize,
        out: &mut Vec<u8>,
    ) -> Result<(), MediaError> {
        let frame_bytes = self.info.frame_bytes();
        let bytes = count
            .checked_mul(frame_bytes)
            .ok_or(MediaError::InvalidMetadata)?;
        out.resize(bytes, 0);
        if count == 0 {
            return Ok(());
        }
        let s = i64::try_from(start).map_err(|_| MediaError::InvalidMetadata)?;
        let c = i64::try_from(count).map_err(|_| MediaError::InvalidMetadata)?;
        self.raw.packed(out, s, c, frame_bytes)
    }
}

/// Media type of every stream in `path` (`0` video, `1` audio, `3` subtitle, ... as in `FFmpeg`).
pub fn track_types(path: &Path) -> Result<Vec<i32>, MediaError> {
    raw::track_types(path)
}
