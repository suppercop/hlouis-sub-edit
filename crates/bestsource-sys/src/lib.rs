//! Raw FFI bindings to the `extern "C"` shim over `BestSource` (`shim/bs_shim.h`).
//!
//! `BestSource` (MIT) and `libp2p` (WTFPL) are compiled from pinned sources by `build.rs` and
//! linked statically; `FFmpeg` (LGPL-3.0) is linked dynamically through its import libraries.

#![allow(non_camel_case_types, clippy::missing_safety_doc)]

use std::os::raw::{c_char, c_int, c_void};

pub const BS_OK: c_int = 0;
pub const BS_ERR: c_int = -1;
pub const BS_CANCELLED: c_int = -2;

pub const BS_CACHE_DISABLE: c_int = 0;
pub const BS_CACHE_AUTO_SUBTREE: c_int = 1;
pub const BS_CACHE_ALWAYS_SUBTREE: c_int = 2;
pub const BS_CACHE_AUTO_ABSOLUTE: c_int = 3;
pub const BS_CACHE_ALWAYS_ABSOLUTE: c_int = 4;

/// `AVMEDIA_TYPE_VIDEO` / `AVMEDIA_TYPE_AUDIO` as reported by [`bs_track_list`].
pub const AVMEDIA_TYPE_VIDEO: c_int = 0;
pub const AVMEDIA_TYPE_AUDIO: c_int = 1;

pub type bs_progress_cb = Option<
    unsafe extern "C" fn(user: *mut c_void, track: c_int, current: i64, total: i64) -> c_int,
>;
pub type bs_cancel_cb = Option<unsafe extern "C" fn(user: *mut c_void) -> c_int>;

#[repr(C)]
pub struct bs_video {
    _private: [u8; 0],
}

#[repr(C)]
pub struct bs_audio {
    _private: [u8; 0],
}

#[repr(C)]
pub struct bs_partial_index {
    _private: [u8; 0],
}

#[repr(C)]
#[derive(Debug, Clone, Copy, Default)]
pub struct bs_video_props {
    pub track: c_int,
    pub width: c_int,
    pub height: c_int,
    pub num_frames: i64,
    pub fps_num: c_int,
    pub fps_den: c_int,
    pub time_base_num: c_int,
    pub time_base_den: c_int,
    pub duration: i64,
    pub start_time: f64,
    pub pix_fmt: c_int,
    pub color_family: c_int,
    pub bits: c_int,
    pub sub_sampling_w: c_int,
    pub sub_sampling_h: c_int,
    pub sar_num: c_int,
    pub sar_den: c_int,
}

#[repr(C)]
#[derive(Debug, Clone, Copy, Default)]
pub struct bs_audio_props {
    pub track: c_int,
    pub sample_rate: c_int,
    pub channels: c_int,
    pub bits_per_sample: c_int,
    pub bytes_per_sample: c_int,
    pub is_float: c_int,
    pub num_samples: i64,
    pub num_frames: i64,
    pub start_time: f64,
    pub channel_layout: u64,
}

extern "C" {
    pub fn bs_partial_index_create() -> *mut bs_partial_index;
    pub fn bs_partial_index_free(p: *mut bs_partial_index);
    pub fn bs_partial_index_len(p: *const bs_partial_index) -> i64;
    pub fn bs_partial_index_is_exact_at(p: *const bs_partial_index, n: i64) -> c_int;
    pub fn bs_partial_index_next_keyframe(p: *const bs_partial_index, current: i64) -> i64;
    pub fn bs_partial_index_prev_keyframe(p: *const bs_partial_index, current: i64) -> i64;

    pub fn bs_version() -> c_int;
    pub fn bs_set_ffmpeg_log_level(level: c_int);
    pub fn bs_track_list(
        path_utf8: *const c_char,
        media_types: *mut c_int,
        cap: c_int,
        err: *mut c_char,
        err_len: usize,
    ) -> c_int;

    pub fn bs_video_open(
        path_utf8: *const c_char,
        track: c_int,
        threads: c_int,
        cache_mode: c_int,
        cache_path_utf8: *const c_char,
        progress: bs_progress_cb,
        progress_user: *mut c_void,
        out: *mut *mut bs_video,
        err: *mut c_char,
        err_len: usize,
    ) -> c_int;
    pub fn bs_video_open_with_partial(
        path_utf8: *const c_char,
        track: c_int,
        threads: c_int,
        cache_mode: c_int,
        cache_path_utf8: *const c_char,
        partial: *mut bs_partial_index,
        progress: bs_progress_cb,
        progress_user: *mut c_void,
        out: *mut *mut bs_video,
        err: *mut c_char,
        err_len: usize,
    ) -> c_int;
    pub fn bs_video_open_cached(
        path_utf8: *const c_char,
        track: c_int,
        threads: c_int,
        cache_mode: c_int,
        cache_path_utf8: *const c_char,
        out: *mut *mut bs_video,
        err: *mut c_char,
        err_len: usize,
    ) -> c_int;
    pub fn bs_video_close(v: *mut bs_video);
    pub fn bs_video_set_cancel(v: *mut bs_video, cb: bs_cancel_cb, user: *mut c_void);
    pub fn bs_video_set_limits(v: *mut bs_video, max_cache_bytes: usize, max_decoders: c_int);
    pub fn bs_video_get_props(v: *const bs_video, out: *mut bs_video_props);
    pub fn bs_video_frame_table(
        v: *const bs_video,
        pts: *mut i64,
        keyframe: *mut u8,
        count: i64,
    ) -> c_int;
    pub fn bs_video_get_frame_rgba(
        v: *mut bs_video,
        n: i64,
        dst: *mut u8,
        stride: isize,
        width: c_int,
        height: c_int,
        pts_out: *mut i64,
        err: *mut c_char,
        err_len: usize,
    ) -> c_int;
    pub fn bs_video_get_frame_planar(
        v: *mut bs_video,
        n: i64,
        dst: *const *mut u8,
        stride: *const isize,
        err: *mut c_char,
        err_len: usize,
    ) -> c_int;

    pub fn bs_audio_open(
        path_utf8: *const c_char,
        track: c_int,
        threads: c_int,
        cache_mode: c_int,
        cache_path_utf8: *const c_char,
        progress: bs_progress_cb,
        progress_user: *mut c_void,
        out: *mut *mut bs_audio,
        err: *mut c_char,
        err_len: usize,
    ) -> c_int;
    pub fn bs_audio_open_cached(
        path_utf8: *const c_char,
        track: c_int,
        threads: c_int,
        cache_mode: c_int,
        cache_path_utf8: *const c_char,
        out: *mut *mut bs_audio,
        err: *mut c_char,
        err_len: usize,
    ) -> c_int;
    pub fn bs_audio_close(a: *mut bs_audio);
    pub fn bs_audio_set_cancel(a: *mut bs_audio, cb: bs_cancel_cb, user: *mut c_void);
    pub fn bs_audio_get_props(a: *const bs_audio, out: *mut bs_audio_props);
    pub fn bs_audio_get_packed(
        a: *mut bs_audio,
        dst: *mut u8,
        start: i64,
        count: i64,
        err: *mut c_char,
        err_len: usize,
    ) -> c_int;

    pub fn bs_quick_open(
        path_utf8: *const c_char,
        track: c_int,
        threads: c_int,
        out: *mut *mut bs_quick,
        err: *mut c_char,
        err_len: usize,
    ) -> c_int;
    pub fn bs_quick_set_partial_index(q: *mut bs_quick, partial: *mut bs_partial_index);
    pub fn bs_quick_close(q: *mut bs_quick);
    pub fn bs_quick_get_props(q: *const bs_quick, out: *mut bs_quick_props);
    pub fn bs_quick_get_frame_rgba(
        q: *mut bs_quick,
        n: i64,
        dst: *mut u8,
        stride: isize,
        width: c_int,
        height: c_int,
        pts_out: *mut i64,
        err: *mut c_char,
        err_len: usize,
    ) -> c_int;
}

/// Index-free decoder handle (see `bs_quick_*` in `shim/bs_shim.h`).
#[repr(C)]
pub struct bs_quick {
    _private: [u8; 0],
}

#[repr(C)]
#[derive(Debug, Clone, Copy, Default)]
pub struct bs_quick_props {
    pub track: c_int,
    pub width: c_int,
    pub height: c_int,
    pub num_frames_estimate: i64,
    pub fps_num: c_int,
    pub fps_den: c_int,
    pub time_base_num: c_int,
    pub time_base_den: c_int,
    pub first_pts: i64,
    pub duration: i64,
    pub sar_num: c_int,
    pub sar_den: c_int,
}

#[cfg(test)]
mod tests {
    #[test]
    fn version_is_r23() {
        // SAFETY: `bs_version` has no preconditions.
        let v = unsafe { super::bs_version() };
        assert_eq!(v, 2300);
    }
}
