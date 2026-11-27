//! Raw `BestSource` handles (through `bestsource-sys`). Every `unsafe` block of the
//! `BestSource` provider lives here; `crate::bestsource` is the safe layer on top.

use crate::MediaError;
use bestsource_sys as sys;
use std::ffi::{CStr, CString};
use std::os::raw::{c_char, c_int, c_void};
use std::path::Path;
use std::ptr::NonNull;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::Arc;

const ERR_LEN: usize = 1024;

fn err_string(buf: &[c_char; ERR_LEN]) -> String {
    // SAFETY: the buffer is zero-initialised and the shim always NUL-terminates what it writes
    // (snprintf), so a NUL lies within `ERR_LEN` bytes.
    let s = unsafe { CStr::from_ptr(buf.as_ptr()) };
    s.to_string_lossy().into_owned()
}

fn map_rc(rc: c_int, buf: &[c_char; ERR_LEN]) -> Result<(), MediaError> {
    match rc {
        sys::BS_OK => Ok(()),
        sys::BS_CANCELLED => Err(MediaError::Cancelled),
        _ => Err(MediaError::BestSource(err_string(buf))),
    }
}

fn c_path(path: &Path) -> Result<CString, MediaError> {
    let s = path.to_str().ok_or(MediaError::InvalidPath)?;
    Ok(CString::new(s)?)
}

struct ProgressState<'a> {
    cancel: &'a AtomicBool,
    on_progress: &'a mut dyn FnMut(i64, i64),
}

// SAFETY: called synchronously by `bs_video_open` / `bs_audio_open` on the calling thread while
// the `ProgressState` passed as `user` is alive on that thread's stack.
unsafe extern "C" fn progress_trampoline(
    user: *mut c_void,
    _track: c_int,
    current: i64,
    total: i64,
) -> c_int {
    if user.is_null() {
        return 0;
    }
    // SAFETY: `user` is the `&mut ProgressState` handed to the open call, see above.
    let state = unsafe { &mut *user.cast::<ProgressState<'_>>() };
    let panicked = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
        (state.on_progress)(current, total);
    }))
    .is_err();
    c_int::from(panicked || state.cancel.load(Ordering::Relaxed))
}

// SAFETY: `user` is the `Arc<AtomicBool>` pointer owned by the handle that registered this
// callback; the handle unregisters it before dropping the `Arc` (see `Drop` impls).
unsafe extern "C" fn cancel_trampoline(user: *mut c_void) -> c_int {
    if user.is_null() {
        return 0;
    }
    // SAFETY: see above; the pointee outlives every call.
    let flag = unsafe { &*user.cast::<AtomicBool>() };
    c_int::from(flag.load(Ordering::Relaxed))
}

/// Shared open arguments for video and audio.
pub(crate) struct OpenArgs<'a> {
    pub path: &'a Path,
    pub track: i32,
    pub threads: i32,
    pub cache_mode: i32,
    pub cache_path: Option<&'a Path>,
}

pub(crate) struct RawBsVideo {
    handle: NonNull<sys::bs_video>,
    cancel: Option<Arc<AtomicBool>>,
}

// SAFETY: the BestSource video object is only touched through `&mut self` (or `&self` for
// read-only property queries) by whichever single thread currently owns this handle; BestSource
// keeps no thread-affine state.
unsafe impl Send for RawBsVideo {}

impl Drop for RawBsVideo {
    fn drop(&mut self) {
        // SAFETY: `handle` was returned by `bs_video_open` and is uniquely owned; unregistering
        // the cancel callback first means no pointer to `self.cancel` survives it.
        unsafe {
            sys::bs_video_set_cancel(self.handle.as_ptr(), None, std::ptr::null_mut());
            sys::bs_video_close(self.handle.as_ptr());
        }
    }
}

impl RawBsVideo {
    pub(crate) fn version() -> i32 {
        // SAFETY: no preconditions.
        unsafe { sys::bs_version() }
    }

    pub(crate) fn open(
        args: &OpenArgs<'_>,
        cancel: &AtomicBool,
        on_progress: &mut dyn FnMut(i64, i64),
    ) -> Result<Self, MediaError> {
        let path = c_path(args.path)?;
        let cache = args.cache_path.map(c_path).transpose()?;
        let mut state = ProgressState {
            cancel,
            on_progress,
        };
        let mut out: *mut sys::bs_video = std::ptr::null_mut();
        let mut err = [0 as c_char; ERR_LEN];
        // SAFETY: all pointers are valid for the duration of the call; `state` outlives it and
        // is only used synchronously by the progress trampoline.
        let rc = unsafe {
            sys::bs_video_open(
                path.as_ptr(),
                args.track,
                args.threads,
                args.cache_mode,
                cache.as_ref().map_or(std::ptr::null(), |c| c.as_ptr()),
                Some(progress_trampoline),
                (&raw mut state).cast::<c_void>(),
                &raw mut out,
                err.as_mut_ptr(),
                ERR_LEN,
            )
        };
        if cancel.load(Ordering::Relaxed) {
            if let Some(h) = NonNull::new(out) {
                drop(Self {
                    handle: h,
                    cancel: None,
                });
            }
            return Err(MediaError::Cancelled);
        }
        map_rc(rc, &err)?;
        let handle =
            NonNull::new(out).ok_or_else(|| MediaError::BestSource("null handle".to_owned()))?;
        Ok(Self {
            handle,
            cancel: None,
        })
    }

    /// Lets `flag` abandon in-flight seeks/decodes on this handle.
    pub(crate) fn set_cancel(&mut self, flag: Arc<AtomicBool>) {
        let ptr = Arc::as_ptr(&flag).cast_mut().cast::<c_void>();
        // SAFETY: `ptr` stays valid while `self.cancel` holds the `Arc`; `Drop` unregisters
        // the callback before the `Arc` is released, and replacing it re-registers first.
        unsafe {
            sys::bs_video_set_cancel(self.handle.as_ptr(), Some(cancel_trampoline), ptr);
        }
        self.cancel = Some(flag);
    }

    pub(crate) fn props(&self) -> sys::bs_video_props {
        let mut p = sys::bs_video_props::default();
        // SAFETY: `handle` is valid and `p` is a valid out pointer.
        unsafe { sys::bs_video_get_props(self.handle.as_ptr(), &raw mut p) };
        p
    }

    pub(crate) fn frame_table(&self, count: usize) -> Result<(Vec<i64>, Vec<u8>), MediaError> {
        let mut pts = vec![0i64; count];
        let mut key = vec![0u8; count];
        let n = i64::try_from(count).map_err(|_| MediaError::InvalidMetadata)?;
        // SAFETY: both buffers hold `count` elements, which is what the shim writes at most.
        let rc = unsafe {
            sys::bs_video_frame_table(self.handle.as_ptr(), pts.as_mut_ptr(), key.as_mut_ptr(), n)
        };
        if rc == sys::BS_OK {
            Ok((pts, key))
        } else {
            Err(MediaError::BestSource("frame table unavailable".to_owned()))
        }
    }

    /// Decodes frame `n` into `dst` as RGBA8 (`width * height * 4` bytes, tightly packed).
    pub(crate) fn frame_rgba(
        &mut self,
        n: i64,
        dst: &mut [u8],
        width: u32,
        height: u32,
    ) -> Result<i64, MediaError> {
        let stride = (width as usize) * 4;
        if dst.len() < stride * height as usize {
            return Err(MediaError::InvalidMetadata);
        }
        let w = c_int::try_from(width).map_err(|_| MediaError::InvalidMetadata)?;
        let h = c_int::try_from(height).map_err(|_| MediaError::InvalidMetadata)?;
        let s = isize::try_from(stride).map_err(|_| MediaError::InvalidMetadata)?;
        let mut pts = 0i64;
        let mut err = [0 as c_char; ERR_LEN];
        // SAFETY: `dst` holds at least `stride * height` writable bytes (checked above), which is
        // exactly what sws_scale writes for a `width x height` RGBA image at that stride.
        let rc = unsafe {
            sys::bs_video_get_frame_rgba(
                self.handle.as_ptr(),
                n,
                dst.as_mut_ptr(),
                s,
                w,
                h,
                &raw mut pts,
                err.as_mut_ptr(),
                ERR_LEN,
            )
        };
        map_rc(rc, &err)?;
        Ok(pts)
    }

    /// Decodes frame `n` into three planes (`planes[i]` with byte stride `strides[i]`).
    /// The caller sizes the planes from [`Self::props`]; this re-checks the minimum sizes.
    pub(crate) fn frame_planar(
        &mut self,
        n: i64,
        planes: [&mut [u8]; 3],
        strides: [usize; 3],
        min_lens: [usize; 3],
    ) -> Result<(), MediaError> {
        for i in 0..3 {
            if planes[i].len() < min_lens[i] {
                return Err(MediaError::InvalidMetadata);
            }
        }
        let [p0, p1, p2] = planes;
        let dst = [p0.as_mut_ptr(), p1.as_mut_ptr(), p2.as_mut_ptr()];
        let mut st = [0isize; 3];
        for (o, s) in st.iter_mut().zip(strides) {
            *o = isize::try_from(s).map_err(|_| MediaError::InvalidMetadata)?;
        }
        let mut err = [0 as c_char; ERR_LEN];
        // SAFETY: each plane holds at least `min_lens[i]` bytes, which the caller computed as
        // `stride * plane_height` for the frame's format, i.e. what ExportAsPlanar writes.
        let rc = unsafe {
            sys::bs_video_get_frame_planar(
                self.handle.as_ptr(),
                n,
                dst.as_ptr(),
                st.as_ptr(),
                err.as_mut_ptr(),
                ERR_LEN,
            )
        };
        map_rc(rc, &err)
    }
}

pub(crate) struct RawBsAudio {
    handle: NonNull<sys::bs_audio>,
    cancel: Option<Arc<AtomicBool>>,
}

// SAFETY: as for `RawBsVideo`: exclusively owned, used by one thread at a time.
unsafe impl Send for RawBsAudio {}

impl Drop for RawBsAudio {
    fn drop(&mut self) {
        // SAFETY: `handle` came from `bs_audio_open` and is uniquely owned; the cancel callback
        // is unregistered before `self.cancel` is released.
        unsafe {
            sys::bs_audio_set_cancel(self.handle.as_ptr(), None, std::ptr::null_mut());
            sys::bs_audio_close(self.handle.as_ptr());
        }
    }
}

impl RawBsAudio {
    pub(crate) fn open(
        args: &OpenArgs<'_>,
        cancel: &AtomicBool,
        on_progress: &mut dyn FnMut(i64, i64),
    ) -> Result<Self, MediaError> {
        let path = c_path(args.path)?;
        let cache = args.cache_path.map(c_path).transpose()?;
        let mut state = ProgressState {
            cancel,
            on_progress,
        };
        let mut out: *mut sys::bs_audio = std::ptr::null_mut();
        let mut err = [0 as c_char; ERR_LEN];
        // SAFETY: as in `RawBsVideo::open`.
        let rc = unsafe {
            sys::bs_audio_open(
                path.as_ptr(),
                args.track,
                args.threads,
                args.cache_mode,
                cache.as_ref().map_or(std::ptr::null(), |c| c.as_ptr()),
                Some(progress_trampoline),
                (&raw mut state).cast::<c_void>(),
                &raw mut out,
                err.as_mut_ptr(),
                ERR_LEN,
            )
        };
        if cancel.load(Ordering::Relaxed) {
            if let Some(h) = NonNull::new(out) {
                drop(Self {
                    handle: h,
                    cancel: None,
                });
            }
            return Err(MediaError::Cancelled);
        }
        map_rc(rc, &err)?;
        let handle =
            NonNull::new(out).ok_or_else(|| MediaError::BestSource("null handle".to_owned()))?;
        Ok(Self {
            handle,
            cancel: None,
        })
    }

    pub(crate) fn set_cancel(&mut self, flag: Arc<AtomicBool>) {
        let ptr = Arc::as_ptr(&flag).cast_mut().cast::<c_void>();
        // SAFETY: as in `RawBsVideo::set_cancel`.
        unsafe {
            sys::bs_audio_set_cancel(self.handle.as_ptr(), Some(cancel_trampoline), ptr);
        }
        self.cancel = Some(flag);
    }

    pub(crate) fn props(&self) -> sys::bs_audio_props {
        let mut p = sys::bs_audio_props::default();
        // SAFETY: `handle` is valid and `p` is a valid out pointer.
        unsafe { sys::bs_audio_get_props(self.handle.as_ptr(), &raw mut p) };
        p
    }

    /// Fills `dst` (exactly `count * channels * bytes_per_sample` bytes) with packed samples.
    pub(crate) fn packed(
        &mut self,
        dst: &mut [u8],
        start: i64,
        count: i64,
        frame_bytes: usize,
    ) -> Result<(), MediaError> {
        let needed = usize::try_from(count)
            .ok()
            .and_then(|c| c.checked_mul(frame_bytes))
            .ok_or(MediaError::InvalidMetadata)?;
        if dst.len() < needed {
            return Err(MediaError::InvalidMetadata);
        }
        let mut err = [0 as c_char; ERR_LEN];
        // SAFETY: `dst` holds `count * channels * bytes_per_sample` bytes (checked above), the
        // amount GetPackedAudio writes.
        let rc = unsafe {
            sys::bs_audio_get_packed(
                self.handle.as_ptr(),
                dst.as_mut_ptr(),
                start,
                count,
                err.as_mut_ptr(),
                ERR_LEN,
            )
        };
        map_rc(rc, &err)
    }
}

/// Lists the media type (`AVMediaType`) of every track in `path`.
pub(crate) fn track_types(path: &Path) -> Result<Vec<i32>, MediaError> {
    let p = c_path(path)?;
    let mut err = [0 as c_char; ERR_LEN];
    let mut types = vec![0 as c_int; 256];
    let cap = c_int::try_from(types.len()).unwrap_or(c_int::MAX);
    // SAFETY: `types` has room for `cap` entries; `p` and `err` are valid.
    let n = unsafe {
        sys::bs_track_list(
            p.as_ptr(),
            types.as_mut_ptr(),
            cap,
            err.as_mut_ptr(),
            ERR_LEN,
        )
    };
    if n < 0 {
        return Err(MediaError::BestSource(err_string(&err)));
    }
    let n = usize::try_from(n).unwrap_or(0);
    if n > types.len() {
        types.resize(n, -1);
        let cap = c_int::try_from(n).unwrap_or(c_int::MAX);
        // SAFETY: as above with the enlarged buffer.
        let n2 = unsafe {
            sys::bs_track_list(
                p.as_ptr(),
                types.as_mut_ptr(),
                cap,
                err.as_mut_ptr(),
                ERR_LEN,
            )
        };
        if n2 < 0 {
            return Err(MediaError::BestSource(err_string(&err)));
        }
    }
    types.truncate(n);
    Ok(types)
}

/// Sets `FFmpeg`'s log level for the whole process (`AV_LOG_*`).
pub(crate) fn set_ffmpeg_log_level(level: i32) {
    // SAFETY: av_log_set_level is a plain global setter.
    unsafe { sys::bs_set_ffmpeg_log_level(level) };
}
