//! Safe `FFMS2` video indexing and frame-accurate decoding wrapper.

#![cfg_attr(not(test), deny(clippy::unwrap_used, clippy::expect_used))]

use std::path::Path;
use std::sync::atomic::AtomicBool;
use thiserror::Error;

/// Errors returned by the FFMS2 video source.
#[derive(Debug, Error)]
pub enum MediaError {
    #[error("video path is not valid UTF-8")]
    InvalidPath,
    #[error("video path contains an interior NUL byte")]
    InteriorNul(#[from] std::ffi::NulError),
    #[error("video indexing was cancelled")]
    Cancelled,
    #[error("FFMS2 error: {0}")]
    Ffms(String),
    #[error("video file has no usable video stream or invalid dimensions")]
    InvalidMetadata,
    #[error("requested frame {frame} out of range (total {total})")]
    FrameOutOfRange { frame: usize, total: usize },
}

/// Static metadata and frame index extracted after opening a video file.
#[derive(Debug, Clone)]
pub struct VideoInfo {
    pub width: u32,
    pub height: u32,
    pub num_frames: usize,
    pub fps_num: i64,
    pub fps_den: i64,
    pub fps: f64,
    pub duration_ms: f64,
    pub keyframes: Vec<usize>,
    pub frame_pts_ms: Vec<i64>,
}

impl VideoInfo {
    /// Finds the next keyframe strictly after `current`.
    #[must_use]
    pub fn next_keyframe(&self, current: usize) -> Option<usize> {
        let pos = self.keyframes.partition_point(|&kf| kf <= current);
        self.keyframes.get(pos).copied()
    }

    /// Finds the previous keyframe strictly before `current`.
    #[must_use]
    pub fn prev_keyframe(&self, current: usize) -> Option<usize> {
        let pos = self.keyframes.partition_point(|&kf| kf < current);
        pos.checked_sub(1)
            .and_then(|idx| self.keyframes.get(idx).copied())
    }

    /// Returns `true` if `frame` is a keyframe.
    #[must_use]
    pub fn is_keyframe(&self, frame: usize) -> bool {
        self.keyframes.binary_search(&frame).is_ok()
    }
}

mod ffi {
    use super::{MediaError, VideoInfo};
    use ffms2_sys::{
        FFMS_CreateIndexer, FFMS_CreateVideoSource, FFMS_DestroyIndex, FFMS_DestroyVideoSource,
        FFMS_DoIndexing2, FFMS_ErrorInfo, FFMS_GetFirstTrackOfType, FFMS_GetFrame,
        FFMS_GetFrameInfo, FFMS_GetNumFrames, FFMS_GetPixFmt, FFMS_GetTimeBase,
        FFMS_GetTrackFromVideo, FFMS_GetVersion, FFMS_GetVideoProperties, FFMS_Index, FFMS_Init,
        FFMS_SetLogLevel, FFMS_SetOutputFormatV2, FFMS_SetProgressCallback,
        FFMS_TrackTypeIndexSettings, FFMS_VideoProperties, FFMS_VideoSource, FFMS_IEH_ABORT,
        FFMS_LOG_WARNING, FFMS_RESIZER_FAST_BILINEAR, FFMS_SEEK_NORMAL, FFMS_TYPE_AUDIO,
        FFMS_TYPE_VIDEO,
    };
    use std::ffi::{CStr, CString};
    use std::os::raw::{c_char, c_int, c_void};
    use std::path::Path;
    use std::ptr::NonNull;
    use std::sync::atomic::{AtomicBool, Ordering};
    use std::sync::Once;

    static FFMS_INIT: Once = Once::new();

    fn ensure_init() {
        FFMS_INIT.call_once(|| {
            // SAFETY: `FFMS_Init(0, 0)` initializes FFMS2/FFmpeg globals once at startup.
            unsafe {
                FFMS_Init(0, 0);
                FFMS_SetLogLevel(FFMS_LOG_WARNING);
            }
        });
    }

    struct ErrorBuf {
        buf: [c_char; 1024],
    }

    impl ErrorBuf {
        fn new() -> Self {
            Self { buf: [0; 1024] }
        }

        fn as_info(&mut self) -> FFMS_ErrorInfo {
            FFMS_ErrorInfo {
                ErrorType: 0,
                SubType: 0,
                BufferSize: c_int::try_from(self.buf.len()).unwrap_or(1024),
                Buffer: self.buf.as_mut_ptr(),
            }
        }

        fn message(&self) -> String {
            // SAFETY: `self.buf` is always NUL-terminated by FFMS2 or initialized to 0.
            let cstr = unsafe { CStr::from_ptr(self.buf.as_ptr()) };
            cstr.to_string_lossy().into_owned()
        }
    }

    struct CallbackState<'a> {
        cancel: &'a AtomicBool,
        on_progress: &'a mut dyn FnMut(i64, i64),
    }

    // SAFETY: Invoked synchronously by `FFMS_DoIndexing2` while `CallbackState` lives on stack.
    unsafe extern "C" fn progress_trampoline(
        current: i64,
        total: i64,
        ic_private: *mut c_void,
    ) -> c_int {
        if ic_private.is_null() {
            return 0;
        }
        // SAFETY: `ic_private` points to the stack-pinned `CallbackState` during `FFMS_DoIndexing2`.
        let state = unsafe { &mut *ic_private.cast::<CallbackState<'_>>() };
        let panicked = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
            (state.on_progress)(current, total);
        }))
        .is_err();
        if panicked {
            return 1;
        }
        i32::from(state.cancel.load(Ordering::Relaxed))
    }

    struct IndexGuard(NonNull<FFMS_Index>);

    impl Drop for IndexGuard {
        fn drop(&mut self) {
            // SAFETY: `IndexGuard` uniquely owns the `FFMS_Index` returned by `FFMS_DoIndexing2`.
            unsafe { FFMS_DestroyIndex(self.0.as_ptr()) };
        }
    }

    struct VideoGuard(NonNull<FFMS_VideoSource>);

    impl Drop for VideoGuard {
        fn drop(&mut self) {
            // SAFETY: `VideoGuard` uniquely owns the `FFMS_VideoSource` handle.
            unsafe { FFMS_DestroyVideoSource(self.0.as_ptr()) };
        }
    }

    pub(crate) struct RawVideoSource {
        source: VideoGuard,
        info: VideoInfo,
    }

    // SAFETY: `RawVideoSource` owns the underlying `FFMS_VideoSource` exclusively and is only
    // mutated through `&mut self` on a single owning thread.
    unsafe impl Send for RawVideoSource {}

    impl RawVideoSource {
        pub(crate) fn version() -> i32 {
            ensure_init();
            // SAFETY: `FFMS_GetVersion` is safe to call once `FFMS_Init` has run.
            unsafe { FFMS_GetVersion() }
        }

        pub(crate) fn open_with_progress(
            path: &Path,
            cancel: &AtomicBool,
            mut on_progress: impl FnMut(i64, i64),
        ) -> Result<Self, MediaError> {
            ensure_init();
            let path_str = path.to_str().ok_or(MediaError::InvalidPath)?;
            let c_path = CString::new(path_str)?;

            let index = run_indexer(&c_path, cancel, &mut on_progress)?;
            let mut err = ErrorBuf::new();
            let mut err_info = err.as_info();

            // SAFETY: `index.0` is valid.
            let track_idx = unsafe {
                FFMS_GetFirstTrackOfType(index.0.as_ptr(), FFMS_TYPE_VIDEO, &raw mut err_info)
            };
            if track_idx < 0 {
                return Err(MediaError::Ffms(err.message()));
            }

            let threads = std::thread::available_parallelism()
                .map_or(4, |n| c_int::try_from(n.get().min(4)).unwrap_or(4));

            // SAFETY: `c_path` and `index.0` are valid; `FFMS_CreateVideoSource` copies needed
            // track data so `index` can be dropped afterward.
            let src_ptr = unsafe {
                FFMS_CreateVideoSource(
                    c_path.as_ptr(),
                    track_idx,
                    index.0.as_ptr(),
                    threads,
                    FFMS_SEEK_NORMAL,
                    &raw mut err_info,
                )
            };
            let source =
                VideoGuard(NonNull::new(src_ptr).ok_or_else(|| MediaError::Ffms(err.message()))?);
            drop(index);

            let (width, height) = configure_rgba_output(source.0)?;
            let info = extract_video_info(source.0, width, height)?;

            Ok(Self { source, info })
        }

        pub(crate) fn info(&self) -> &VideoInfo {
            &self.info
        }

        pub(crate) fn decode_frame_into(
            &mut self,
            frame_index: usize,
            out_rgba: &mut Vec<u8>,
        ) -> Result<i64, MediaError> {
            if frame_index >= self.info.num_frames {
                return Err(MediaError::FrameOutOfRange {
                    frame: frame_index,
                    total: self.info.num_frames,
                });
            }

            let n = c_int::try_from(frame_index).map_err(|_| MediaError::FrameOutOfRange {
                frame: frame_index,
                total: self.info.num_frames,
            })?;
            let mut err = ErrorBuf::new();
            let mut err_info = err.as_info();

            // SAFETY: `self.source` is valid and `n` is within `0..num_frames`.
            let frame_ptr = unsafe { FFMS_GetFrame(self.source.0.as_ptr(), n, &raw mut err_info) };
            if frame_ptr.is_null() {
                return Err(MediaError::Ffms(err.message()));
            }
            // SAFETY: `frame_ptr` is valid until the next `FFMS_GetFrame` or `FFMS_DestroyVideoSource`.
            let frame = unsafe { &*frame_ptr };
            let src_ptr = frame.Data[0];
            let linesize = usize::try_from(frame.Linesize[0].max(0)).unwrap_or(0);
            let width = self.info.width;
            let height = self.info.height;
            let row_bytes = (width as usize)
                .checked_mul(4)
                .ok_or(MediaError::InvalidMetadata)?;
            let height_usize = height as usize;
            if src_ptr.is_null() || height_usize == 0 || linesize < row_bytes {
                return Err(MediaError::InvalidMetadata);
            }

            let total_bytes = row_bytes
                .checked_mul(height_usize)
                .ok_or(MediaError::InvalidMetadata)?;
            let readable_bytes = (height_usize - 1)
                .checked_mul(linesize)
                .and_then(|n| n.checked_add(row_bytes))
                .ok_or(MediaError::InvalidMetadata)?;
            let max_slice = usize::try_from(isize::MAX).unwrap_or(usize::MAX);
            if readable_bytes > max_slice || total_bytes > max_slice {
                return Err(MediaError::InvalidMetadata);
            }

            // SAFETY: `src_ptr` is non-null and has at least `(height - 1) * linesize + row_bytes <= isize::MAX` readable bytes.
            let src_slice = unsafe { std::slice::from_raw_parts(src_ptr, readable_bytes) };
            super::copy_rgba_plane(src_slice, linesize, width, height, out_rgba)?;

            let pts_ms = self
                .info
                .frame_pts_ms
                .get(frame_index)
                .copied()
                .unwrap_or(0);
            Ok(pts_ms)
        }
    }

    fn run_indexer(
        c_path: &CStr,
        cancel: &AtomicBool,
        on_progress: &mut dyn FnMut(i64, i64),
    ) -> Result<IndexGuard, MediaError> {
        let mut err = ErrorBuf::new();
        let mut err_info = err.as_info();

        // SAFETY: `c_path` and `err_info` are valid pointers for this call.
        let indexer = unsafe { FFMS_CreateIndexer(c_path.as_ptr(), &raw mut err_info) };
        if indexer.is_null() {
            return Err(MediaError::Ffms(err.message()));
        }

        let mut cb_state = CallbackState {
            cancel,
            on_progress,
        };

        // SAFETY: `indexer` is valid; `cb_state` stays live for the synchronous `FFMS_DoIndexing2` call.
        // Note: `FFMS_DoIndexing2` unconditionally deletes `indexer` before returning.
        let index_ptr = unsafe {
            FFMS_TrackTypeIndexSettings(indexer, FFMS_TYPE_AUDIO, 0, 0);
            FFMS_SetProgressCallback(
                indexer,
                Some(progress_trampoline),
                (&raw mut cb_state).cast::<c_void>(),
            );
            FFMS_DoIndexing2(indexer, FFMS_IEH_ABORT, &raw mut err_info)
        };

        if cancel.load(Ordering::Relaxed) {
            if let Some(non_null) = NonNull::new(index_ptr) {
                drop(IndexGuard(non_null));
            }
            return Err(MediaError::Cancelled);
        }

        NonNull::new(index_ptr)
            .map(IndexGuard)
            .ok_or_else(|| MediaError::Ffms(err.message()))
    }

    fn configure_rgba_output(source: NonNull<FFMS_VideoSource>) -> Result<(u32, u32), MediaError> {
        let mut err = ErrorBuf::new();
        let mut err_info = err.as_info();

        // SAFETY: `source` is valid.
        let props_ptr = unsafe { FFMS_GetVideoProperties(source.as_ptr()) };
        let props_nn = NonNull::new(props_ptr.cast_mut()).ok_or(MediaError::InvalidMetadata)?;
        // SAFETY: `props_nn` is non-null and owned by `source`.
        let props: &FFMS_VideoProperties = unsafe { props_nn.as_ref() };
        if props.NumFrames <= 0 {
            return Err(MediaError::InvalidMetadata);
        }

        // SAFETY: `source` is valid and `NumFrames > 0`.
        let probe_ptr = unsafe { FFMS_GetFrame(source.as_ptr(), 0, &raw mut err_info) };
        if probe_ptr.is_null() {
            return Err(MediaError::Ffms(err.message()));
        }
        // SAFETY: `probe_ptr` is non-null and valid until the next `FFMS_GetFrame` call.
        let probe = unsafe { &*probe_ptr };
        let width = u32::try_from(probe.EncodedWidth).map_err(|_| MediaError::InvalidMetadata)?;
        let height = u32::try_from(probe.EncodedHeight).map_err(|_| MediaError::InvalidMetadata)?;
        if width == 0 || height == 0 {
            return Err(MediaError::InvalidMetadata);
        }

        // SAFETY: `c"rgba"` is a static NUL-terminated C string.
        let rgba_fmt = unsafe { FFMS_GetPixFmt(c"rgba".as_ptr()) };
        if rgba_fmt < 0 {
            return Err(MediaError::Ffms(
                "FFMS_GetPixFmt(\"rgba\") failed".to_owned(),
            ));
        }

        let target_formats = [rgba_fmt, -1];
        // SAFETY: `source` is valid and `target_formats` is `-1`-terminated.
        let fmt_res = unsafe {
            FFMS_SetOutputFormatV2(
                source.as_ptr(),
                target_formats.as_ptr(),
                probe.EncodedWidth,
                probe.EncodedHeight,
                FFMS_RESIZER_FAST_BILINEAR,
                &raw mut err_info,
            )
        };
        if fmt_res != 0 {
            return Err(MediaError::Ffms(err.message()));
        }

        Ok((width, height))
    }

    fn extract_video_info(
        source: NonNull<FFMS_VideoSource>,
        width: u32,
        height: u32,
    ) -> Result<VideoInfo, MediaError> {
        // SAFETY: `source` is valid.
        let props_ptr = unsafe { FFMS_GetVideoProperties(source.as_ptr()) };
        let props_nn = NonNull::new(props_ptr.cast_mut()).ok_or(MediaError::InvalidMetadata)?;
        // SAFETY: `props_nn` is non-null and owned by `source`.
        let props = unsafe { props_nn.as_ref() };
        // SAFETY: `source` is valid.
        let track = unsafe { FFMS_GetTrackFromVideo(source.as_ptr()) };
        if track.is_null() {
            return Err(MediaError::InvalidMetadata);
        }
        // SAFETY: `track` is non-null and owned by `source`.
        let track_frames = unsafe { FFMS_GetNumFrames(track) };
        let num_frames = usize::try_from(track_frames.max(0)).unwrap_or(0);
        // SAFETY: `track` is valid.
        let tb_ptr = unsafe { FFMS_GetTimeBase(track) };
        let tb_nn = NonNull::new(tb_ptr.cast_mut()).ok_or(MediaError::InvalidMetadata)?;
        // SAFETY: `tb_nn` is non-null and owned by `track`.
        let tb = unsafe { *tb_nn.as_ref() };

        let mut keyframes = Vec::new();
        let mut frame_pts_ms = Vec::with_capacity(num_frames);
        for idx in 0..num_frames {
            let frame_c = c_int::try_from(idx).unwrap_or(c_int::MAX);
            // SAFETY: `track` is valid and `idx < num_frames`.
            let finfo_ptr = unsafe { FFMS_GetFrameInfo(track, frame_c) };
            if finfo_ptr.is_null() {
                frame_pts_ms.push(0);
            } else {
                // SAFETY: `finfo_ptr` is non-null and owned by `track`.
                let finfo = unsafe { *finfo_ptr };
                if finfo.KeyFrame != 0 {
                    keyframes.push(idx);
                }
                let pts_ms = super::pts_to_ms_rounded(finfo.PTS, tb.Num, tb.Den);
                frame_pts_ms.push(pts_ms);
            }
        }

        let fps_num = i64::from(props.FPSNumerator);
        let fps_den = i64::from(props.FPSDenominator);
        let fps = if fps_den > 0 {
            fps_num as f64 / fps_den as f64
        } else {
            23.976
        };
        let duration_ms = if props.LastEndTime > 0.0 {
            props.LastEndTime * 1000.0
        } else if let Some(&last_pts) = frame_pts_ms.last() {
            last_pts as f64 + (1000.0 / fps.max(1.0))
        } else {
            0.0
        };

        Ok(VideoInfo {
            width,
            height,
            num_frames,
            fps_num,
            fps_den,
            fps,
            duration_ms,
            keyframes,
            frame_pts_ms,
        })
    }
}

/// Converts an FFMS2 track PTS (with millisecond-scaled timebase `tb_num / tb_den`)
/// to the nearest millisecond, returning `0` for `AV_NOPTS_VALUE` (`i64::MIN`) or invalid timebases.
#[must_use]
pub(crate) fn pts_to_ms_rounded(pts: i64, tb_num: i64, tb_den: i64) -> i64 {
    if pts == i64::MIN || tb_den <= 0 {
        return 0;
    }
    let num = i128::from(pts) * i128::from(tb_num);
    let den = i128::from(tb_den);
    let half = den / 2;
    let rounded = if num >= 0 {
        (num + half) / den
    } else {
        (num - half) / den
    };
    i64::try_from(rounded).unwrap_or(0)
}

/// Safe FFMS2 video decoder producing tightly-packed RGBA8 frames.
pub struct FfmsVideo {
    inner: ffi::RawVideoSource,
}

impl FfmsVideo {
    /// Returns the runtime FFMS2 version integer.
    #[must_use]
    pub fn ffms_version() -> i32 {
        ffi::RawVideoSource::version()
    }

    /// Opens and indexes a video file, reporting `(current_bytes, total_bytes)` progress.
    pub fn open_with_progress(
        path: &Path,
        cancel: &AtomicBool,
        on_progress: impl FnMut(i64, i64),
    ) -> Result<Self, MediaError> {
        let inner = ffi::RawVideoSource::open_with_progress(path, cancel, on_progress)?;
        Ok(Self { inner })
    }

    /// Returns static video metadata, timestamps, and keyframe indices.
    #[must_use]
    pub fn info(&self) -> &VideoInfo {
        self.inner.info()
    }

    /// Decodes frame `frame_index` to tightly-packed RGBA8 (`width * height * 4`) in `out_rgba`.
    /// Returns the frame's presentation timestamp in milliseconds.
    pub fn decode_frame_into(
        &mut self,
        frame_index: usize,
        out_rgba: &mut Vec<u8>,
    ) -> Result<i64, MediaError> {
        self.inner.decode_frame_into(frame_index, out_rgba)
    }

    /// Finds the next keyframe strictly after `current`.
    #[must_use]
    pub fn next_keyframe(&self, current: usize) -> Option<usize> {
        self.inner.info().next_keyframe(current)
    }

    /// Finds the previous keyframe strictly before `current`.
    #[must_use]
    pub fn prev_keyframe(&self, current: usize) -> Option<usize> {
        self.inner.info().prev_keyframe(current)
    }
}

/// Copies a decoded RGBA8 plane (`src` with byte stride `linesize`) into a tightly packed
/// `width * height * 4` buffer `out_rgba`, reusing `out_rgba`'s existing allocation when sized.
pub fn copy_rgba_plane(
    src: &[u8],
    linesize: usize,
    width: u32,
    height: u32,
    out_rgba: &mut Vec<u8>,
) -> Result<(), MediaError> {
    let row_bytes = (width as usize)
        .checked_mul(4)
        .ok_or(MediaError::InvalidMetadata)?;
    let height_usize = height as usize;
    if row_bytes == 0 || height_usize == 0 || linesize < row_bytes {
        return Err(MediaError::InvalidMetadata);
    }
    let total_bytes = row_bytes
        .checked_mul(height_usize)
        .ok_or(MediaError::InvalidMetadata)?;
    let required_src = (height_usize - 1)
        .checked_mul(linesize)
        .and_then(|n| n.checked_add(row_bytes))
        .ok_or(MediaError::InvalidMetadata)?;
    if src.len() < required_src {
        return Err(MediaError::InvalidMetadata);
    }

    out_rgba.resize(total_bytes, 0);
    if linesize == row_bytes {
        out_rgba.copy_from_slice(&src[..total_bytes]);
    } else {
        for y in 0..height_usize {
            let src_row = &src[y * linesize..y * linesize + row_bytes];
            let dst_row = &mut out_rgba[y * row_bytes..(y + 1) * row_bytes];
            dst_row.copy_from_slice(src_row);
        }
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn ffms_version_is_nonzero() {
        assert!(FfmsVideo::ffms_version() > 0);
    }

    #[test]
    fn keyframe_navigation_works() {
        let info = VideoInfo {
            width: 1920,
            height: 1080,
            num_frames: 200,
            fps_num: 24000,
            fps_den: 1001,
            fps: 23.976,
            duration_ms: 8341.0,
            keyframes: vec![0, 48, 96, 150],
            frame_pts_ms: vec![0; 200],
        };

        assert!(info.is_keyframe(48));
        assert!(!info.is_keyframe(49));
        assert_eq!(info.next_keyframe(0), Some(48));
        assert_eq!(info.next_keyframe(47), Some(48));
        assert_eq!(info.next_keyframe(48), Some(96));
        assert_eq!(info.next_keyframe(150), None);
        assert_eq!(info.prev_keyframe(0), None);
        assert_eq!(info.prev_keyframe(48), Some(0));
        assert_eq!(info.prev_keyframe(49), Some(48));
        assert_eq!(info.prev_keyframe(199), Some(150));
    }

    #[test]
    fn pts_to_ms_rounds_at_24000_1001_and_handles_nopts() {
        // FFMS2 timebase for 24000/1001 fps: 1001/24 ms per frame tick.
        assert_eq!(pts_to_ms_rounded(0, 1001, 24), 0);
        // 41.7083... ms -> rounds to 42 ms (not truncated 41 ms).
        assert_eq!(pts_to_ms_rounded(1, 1001, 24), 42);
        // 83.4166... ms -> rounds to 83 ms.
        assert_eq!(pts_to_ms_rounded(2, 1001, 24), 83);
        // 125.125 ms -> rounds to 125 ms.
        assert_eq!(pts_to_ms_rounded(3, 1001, 24), 125);
        // 24 frames = exact 1001 ms.
        assert_eq!(pts_to_ms_rounded(24, 1001, 24), 1001);
        // 999.99 ms -> rounds to 1000 ms instead of missing a 0:00:01.00 subtitle.
        assert_eq!(pts_to_ms_rounded(99_999, 1, 100), 1000);
        // Negative timestamps round half away from zero.
        assert_eq!(pts_to_ms_rounded(-1, 1001, 24), -42);
        // AV_NOPTS_VALUE (i64::MIN) and invalid timebase return 0 safely.
        assert_eq!(pts_to_ms_rounded(i64::MIN, 1001, 24), 0);
        assert_eq!(pts_to_ms_rounded(10, 1001, 0), 0);
        assert_eq!(pts_to_ms_rounded(10, 1001, -24), 0);
    }

    #[test]
    fn optional_env_video_opens_and_decodes_frame_zero() {
        let Ok(path_str) = std::env::var("HLOUIS_TEST_VIDEO") else {
            return;
        };
        let cancel = AtomicBool::new(false);
        let mut saw_progress = false;
        let mut video = FfmsVideo::open_with_progress(Path::new(&path_str), &cancel, |cur, tot| {
            if tot > 0 && cur >= 0 {
                saw_progress = true;
            }
        })
        .expect("open HLOUIS_TEST_VIDEO");
        assert!(saw_progress);
        let info = video.info().clone();
        assert!(info.width > 0 && info.height > 0 && info.num_frames > 0);
        let mut rgba = Vec::new();
        let _pts = video
            .decode_frame_into(0, &mut rgba)
            .expect("decode frame 0");
        assert_eq!(
            rgba.len(),
            (info.width as usize) * (info.height as usize) * 4
        );
    }
}
