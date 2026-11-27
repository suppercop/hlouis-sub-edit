//! Safe `libass` subtitle rendering and RGBA compositing.

#![cfg_attr(not(test), deny(clippy::unwrap_used, clippy::expect_used))]

use std::path::Path;
use thiserror::Error;

/// Errors returned by the subtitle renderer.
#[derive(Debug, Error)]
pub enum RenderError {
    #[error("failed to initialize libass library")]
    LibraryInitFailed,
    #[error("failed to initialize libass renderer")]
    RendererInitFailed,
    #[error("failed to parse ASS subtitle track")]
    TrackLoadFailed,
    #[error("subtitle text contains an interior NUL byte")]
    InteriorNul(#[from] std::ffi::NulError),
    #[error("failed to read subtitle file: {0}")]
    Io(#[from] std::io::Error),
    #[error("RGBA frame buffer length mismatch: expected {expected} bytes, got {actual}")]
    FrameSizeMismatch { expected: usize, actual: usize },
}

/// Statistics for a single subtitle render/blend pass.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub struct RenderStats {
    /// Number of non-empty `ASS_Image` glyphs/bitmaps blended.
    pub bitmap_count: usize,
    /// `libass` change flag (`detect_change != 0`).
    pub changed: bool,
}

/// Borrowed descriptor for a single rasterized `ASS_Image` alpha mask.
pub struct AssBitmap<'a> {
    pub dst_x: i32,
    pub dst_y: i32,
    pub width: u32,
    pub height: u32,
    pub stride: usize,
    /// Packed `0xRRGGBBAA` where `AA` is inverted transparency (`0x00` = opaque).
    pub color: u32,
    pub data: &'a [u8],
}

mod ffi {
    use super::{AssBitmap, RenderError, RenderStats};
    use libass_sys::{
        ass_free_track, ass_library_done, ass_library_init, ass_library_version, ass_read_memory,
        ass_render_frame, ass_renderer_done, ass_renderer_init, ass_set_cache_limits,
        ass_set_extract_fonts, ass_set_fonts, ass_set_frame_size, ass_set_hinting,
        ass_set_message_cb, ass_set_shaper, ass_set_storage_size, ASS_Library, ASS_Renderer,
        ASS_Track, ASS_FONTPROVIDER_AUTODETECT, ASS_HINTING_NONE, ASS_SHAPING_COMPLEX,
    };
    use std::ffi::CString;
    use std::os::raw::{c_char, c_int, c_longlong, c_void};
    use std::ptr::{self, NonNull};

    // SAFETY: Invoked only by `libass` with valid C format pointers (unused here).
    unsafe extern "C" fn silent_log_cb(
        _level: c_int,
        _fmt: *const c_char,
        _args: *mut c_void,
        _data: *mut c_void,
    ) {
    }

    pub(crate) struct RawAssContext {
        library: NonNull<ASS_Library>,
        renderer: NonNull<ASS_Renderer>,
        track: Option<NonNull<ASS_Track>>,
    }

    // SAFETY: `RawAssContext` owns its `ASS_Library`, `ASS_Renderer`, and `ASS_Track`
    // pointers exclusively and only accesses them through `&mut self`.
    unsafe impl Send for RawAssContext {}

    impl RawAssContext {
        pub(crate) fn new(width: u32, height: u32) -> Result<Self, RenderError> {
            // SAFETY: `ass_library_init` has no preconditions and returns either NULL or a valid handle.
            let lib_ptr = unsafe { ass_library_init() };
            let library = NonNull::new(lib_ptr).ok_or(RenderError::LibraryInitFailed)?;

            // SAFETY: `library` is a valid, newly initialized `ASS_Library` handle.
            unsafe {
                ass_set_message_cb(library.as_ptr(), Some(silent_log_cb), ptr::null_mut());
                ass_set_extract_fonts(library.as_ptr(), 1);
            }

            // SAFETY: `library` is valid; `ass_renderer_init` returns NULL on allocation failure.
            let ren_ptr = unsafe { ass_renderer_init(library.as_ptr()) };
            let Some(renderer) = NonNull::new(ren_ptr) else {
                // SAFETY: `library` is valid and must be freed on early error return.
                unsafe { ass_library_done(library.as_ptr()) };
                return Err(RenderError::RendererInitFailed);
            };

            let w = c_int::try_from(width.max(1)).unwrap_or(c_int::MAX);
            let h = c_int::try_from(height.max(1)).unwrap_or(c_int::MAX);
            let default_family = CString::new("sans-serif")?;

            // SAFETY: `renderer` is valid and `default_family` is a valid NUL-terminated C string.
            unsafe {
                ass_set_frame_size(renderer.as_ptr(), w, h);
                ass_set_storage_size(renderer.as_ptr(), w, h);
                ass_set_hinting(renderer.as_ptr(), ASS_HINTING_NONE);
                ass_set_shaper(renderer.as_ptr(), ASS_SHAPING_COMPLEX);
                ass_set_cache_limits(renderer.as_ptr(), 4096, 32);
                ass_set_fonts(
                    renderer.as_ptr(),
                    ptr::null(),
                    default_family.as_ptr(),
                    ASS_FONTPROVIDER_AUTODETECT,
                    ptr::null(),
                    1,
                );
            }

            Ok(Self {
                library,
                renderer,
                track: None,
            })
        }

        pub(crate) fn version() -> i32 {
            // SAFETY: `ass_library_version` is a stateless C function returning a version integer.
            unsafe { ass_library_version() }
        }

        pub(crate) fn resize(&mut self, width: u32, height: u32) {
            let w = c_int::try_from(width.max(1)).unwrap_or(c_int::MAX);
            let h = c_int::try_from(height.max(1)).unwrap_or(c_int::MAX);
            // SAFETY: `self.renderer` is valid for the lifetime of `self`.
            unsafe {
                ass_set_frame_size(self.renderer.as_ptr(), w, h);
                ass_set_storage_size(self.renderer.as_ptr(), w, h);
            }
        }

        pub(crate) fn load_ass_memory(&mut self, content: &str) -> Result<(), RenderError> {
            let mut bytes = content.as_bytes().to_vec();
            let buf_len = bytes.len();
            let codepage = CString::new("UTF-8")?;

            // SAFETY: `self.library` is valid; `bytes` is a mutable buffer of length `buf_len`
            // (libass allows temporary modification during parsing and does not retain the pointer).
            let track_ptr = unsafe {
                ass_read_memory(
                    self.library.as_ptr(),
                    bytes.as_mut_ptr().cast::<c_char>(),
                    buf_len,
                    codepage.as_ptr(),
                )
            };
            let track = NonNull::new(track_ptr).ok_or(RenderError::TrackLoadFailed)?;
            if let Some(old_track) = self.track.replace(track) {
                // SAFETY: `old_track` was owned by `self` and is no longer referenced.
                unsafe { ass_free_track(old_track.as_ptr()) };
            }
            Ok(())
        }

        pub(crate) fn render_images(
            &mut self,
            time_ms: i64,
            mut on_bitmap: impl FnMut(AssBitmap<'_>),
        ) -> RenderStats {
            let Some(track) = self.track else {
                return RenderStats::default();
            };

            let mut detect_change: c_int = 0;
            let now = c_longlong::from(time_ms.max(0));
            // SAFETY: `self.renderer` and `track` are valid and initialized with frame size and fonts.
            let mut img = unsafe {
                ass_render_frame(
                    self.renderer.as_ptr(),
                    track.as_ptr(),
                    now,
                    &raw mut detect_change,
                )
            };

            let mut bitmap_count = 0usize;
            while !img.is_null() {
                // SAFETY: `img` is a valid non-null node in the linked list returned by `ass_render_frame`,
                // valid until the next `ass_render_frame` or teardown call.
                let node = unsafe { &*img };
                if node.w > 0 && node.h > 0 && node.stride >= node.w && !node.bitmap.is_null() {
                    let w = node.w.cast_unsigned();
                    let h = node.h.cast_unsigned();
                    let stride = usize::try_from(node.stride).unwrap_or(0);
                    let len = stride * (h as usize - 1) + w as usize;
                    // SAFETY: Per libass `ass.h`, `bitmap` has at least `stride * (h - 1) + w` readable bytes.
                    let data = unsafe { std::slice::from_raw_parts(node.bitmap, len) };
                    on_bitmap(AssBitmap {
                        dst_x: node.dst_x,
                        dst_y: node.dst_y,
                        width: w,
                        height: h,
                        stride,
                        color: node.color,
                        data,
                    });
                    bitmap_count += 1;
                }
                img = node.next;
            }

            RenderStats {
                bitmap_count,
                changed: detect_change != 0,
            }
        }
    }

    impl Drop for RawAssContext {
        fn drop(&mut self) {
            if let Some(track) = self.track.take() {
                // SAFETY: `track` is uniquely owned by `self` and freed before library/renderer.
                unsafe { ass_free_track(track.as_ptr()) };
            }
            // SAFETY: `self.renderer` is uniquely owned and freed before `self.library`.
            unsafe { ass_renderer_done(self.renderer.as_ptr()) };
            // SAFETY: `self.library` is uniquely owned and freed last.
            unsafe { ass_library_done(self.library.as_ptr()) };
        }
    }
}

/// High-level safe ASS subtitle renderer backed by `libass`.
pub struct AssSubRenderer {
    ctx: ffi::RawAssContext,
    width: u32,
    height: u32,
}

impl AssSubRenderer {
    /// Returns the runtime `libass` version integer (`LIBASS_VERSION`).
    #[must_use]
    pub fn libass_version() -> i32 {
        ffi::RawAssContext::version()
    }

    /// Creates a new `libass` renderer configured for the given video dimensions.
    pub fn new(width: u32, height: u32) -> Result<Self, RenderError> {
        let w = width.max(1);
        let h = height.max(1);
        let ctx = ffi::RawAssContext::new(w, h)?;
        Ok(Self {
            ctx,
            width: w,
            height: h,
        })
    }

    /// Updates the target video frame dimensions.
    pub fn resize(&mut self, width: u32, height: u32) {
        let w = width.max(1);
        let h = height.max(1);
        if self.width != w || self.height != h {
            self.width = w;
            self.height = h;
            self.ctx.resize(w, h);
        }
    }

    /// Loads an ASS script from UTF-8 text in memory.
    pub fn load_ass_memory(&mut self, script: &str) -> Result<(), RenderError> {
        let normalized = script.strip_prefix('\u{FEFF}').unwrap_or(script);
        self.ctx.load_ass_memory(normalized)
    }

    /// Loads an ASS script file from disk (UTF-8 or UTF-8 with BOM).
    pub fn load_ass_file(&mut self, path: &Path) -> Result<(), RenderError> {
        let bytes = std::fs::read(path)?;
        let text = String::from_utf8_lossy(&bytes);
        self.load_ass_memory(&text)
    }

    /// Renders subtitles at `time_ms` and alpha-blends them onto `frame_rgba` (`width * height * 4`).
    pub fn render_and_blend_rgba(
        &mut self,
        time_ms: i64,
        frame_rgba: &mut [u8],
        width: u32,
        height: u32,
    ) -> Result<RenderStats, RenderError> {
        let expected = (width as usize)
            .saturating_mul(height as usize)
            .saturating_mul(4);
        if frame_rgba.len() != expected {
            return Err(RenderError::FrameSizeMismatch {
                expected,
                actual: frame_rgba.len(),
            });
        }
        self.resize(width, height);
        let stats = self.ctx.render_images(time_ms, |bitmap| {
            blend_bitmap_into_rgba(&bitmap, frame_rgba, width, height);
        });
        Ok(stats)
    }
}

/// Alpha-blends a single 8-bit coverage `bitmap` onto tightly packed `frame_rgba`.
pub fn blend_rgba_mask(
    bitmap: &AssBitmap<'_>,
    frame_rgba: &mut [u8],
    frame_width: u32,
    frame_height: u32,
) -> Result<(), RenderError> {
    let expected = (frame_width as usize)
        .saturating_mul(frame_height as usize)
        .saturating_mul(4);
    if frame_rgba.len() != expected {
        return Err(RenderError::FrameSizeMismatch {
            expected,
            actual: frame_rgba.len(),
        });
    }
    let needed_mask = bitmap.stride.saturating_mul(bitmap.height as usize);
    if bitmap.width == 0
        || bitmap.height == 0
        || bitmap.stride < bitmap.width as usize
        || bitmap.data.len() < needed_mask
    {
        return Ok(());
    }
    blend_bitmap_into_rgba(bitmap, frame_rgba, frame_width, frame_height);
    Ok(())
}

pub(crate) fn blend_bitmap_into_rgba(
    bitmap: &AssBitmap<'_>,
    frame_rgba: &mut [u8],
    frame_width: u32,
    frame_height: u32,
) {
    let fw = i64::from(frame_width);
    let fh = i64::from(frame_height);

    let x_start = i64::from(bitmap.dst_x);
    let y_start = i64::from(bitmap.dst_y);
    let x_end = x_start + i64::from(bitmap.width);
    let y_end = y_start + i64::from(bitmap.height);

    let left_clip = x_start.max(0);
    let top_clip = y_start.max(0);
    let right_clip = x_end.min(fw);
    let bottom_clip = y_end.min(fh);
    if left_clip >= right_clip || top_clip >= bottom_clip {
        return;
    }

    let src_r = (bitmap.color >> 24) & 0xFF;
    let src_g = (bitmap.color >> 16) & 0xFF;
    let src_b = (bitmap.color >> 8) & 0xFF;
    let src_a = 255 - (bitmap.color & 0xFF);
    if src_a == 0 {
        return;
    }
    #[allow(clippy::cast_possible_truncation)]
    let red = src_r as u8;
    #[allow(clippy::cast_possible_truncation)]
    let green = src_g as u8;
    #[allow(clippy::cast_possible_truncation)]
    let blue = src_b as u8;

    let frame_stride = (frame_width as usize) * 4;
    let start_src_x = usize::try_from(left_clip - x_start).unwrap_or(0);
    let cols = usize::try_from(right_clip - left_clip).unwrap_or(0);
    let dst_x_start = usize::try_from(left_clip).unwrap_or(0) * 4;

    for dst_y in top_clip..bottom_clip {
        let src_y = usize::try_from(dst_y - y_start).unwrap_or(0);
        let src_row_start = src_y * bitmap.stride + start_src_x;
        let src_row = &bitmap.data[src_row_start..src_row_start + cols];

        let dst_row_start = usize::try_from(dst_y).unwrap_or(0) * frame_stride + dst_x_start;
        let dst_row = &mut frame_rgba[dst_row_start..dst_row_start + cols * 4];

        for (i, &cov) in src_row.iter().enumerate() {
            if cov == 0 {
                continue;
            }
            let alpha = if src_a == 255 {
                u32::from(cov)
            } else {
                fast_div_255(src_a * u32::from(cov))
            };
            if alpha == 0 {
                continue;
            }
            let px = &mut dst_row[i * 4..i * 4 + 4];
            if alpha == 255 {
                px[0] = red;
                px[1] = green;
                px[2] = blue;
                px[3] = 255;
            } else {
                let inv = 255 - alpha;
                #[allow(clippy::cast_possible_truncation)]
                {
                    px[0] = fast_div_255(src_r * alpha + u32::from(px[0]) * inv) as u8;
                    px[1] = fast_div_255(src_g * alpha + u32::from(px[1]) * inv) as u8;
                    px[2] = fast_div_255(src_b * alpha + u32::from(px[2]) * inv) as u8;
                }
                px[3] = 255;
            }
        }
    }
}

/// Exact rounded division by 255 (`(x + 127) / 255`) for all `0 <= x <= 65025` without hardware div.
#[inline]
const fn fast_div_255(x: u32) -> u32 {
    let t = x + 128;
    (t + (t >> 8)) >> 8
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn alpha_blend_handles_opaque_semi_and_clipping() {
        let mut frame = vec![0u8; 4 * 4 * 4];
        // Opaque red (`0xFF000000`: AA=0x00 means fully opaque in libass)
        let data = [255u8, 128, 0, 255];
        let bmp = AssBitmap {
            dst_x: -1,
            dst_y: 1,
            width: 2,
            height: 2,
            stride: 2,
            color: 0xFF00_0000,
            data: &data,
        };
        blend_bitmap_into_rgba(&bmp, &mut frame, 4, 4);

        // Pixel (0, 1) corresponds to src (1, 0) with cov=128 -> ~50% red
        let p01 = &frame[4 * 4..4 * 4 + 4];
        assert!(p01[0] > 120 && p01[0] < 135, "red={}", p01[0]);
        assert_eq!(p01[1], 0);
        assert_eq!(p01[3], 255);

        // Pixel (0, 2) corresponds to src (1, 1) with cov=255 -> full red
        let p02 = &frame[8 * 4..8 * 4 + 4];
        assert_eq!(p02, &[255, 0, 0, 255]);
    }

    #[test]
    fn libass_renders_sample_dialogue() {
        assert!(AssSubRenderer::libass_version() > 0);
        let mut renderer = AssSubRenderer::new(640, 360).expect("create libass renderer");
        let ass = "\
[Script Info]
ScriptType: v4.00+
PlayResX: 640
PlayResY: 360

[V4+ Styles]
Format: Name, Fontname, Fontsize, PrimaryColour, SecondaryColour, OutlineColour, BackColour, Bold, Italic, Underline, StrikeOut, ScaleX, ScaleY, Spacing, Angle, BorderStyle, Outline, Shadow, Alignment, MarginL, MarginR, MarginV, Encoding
Style: Default,Segoe UI,40,&H00FFFFFF,&H000000FF,&H00000000,&H80000000,0,0,0,0,100,100,0,0,1,2,1,2,10,10,20,1

[Events]
Format: Layer, Start, End, Style, Name, MarginL, MarginR, MarginV, Effect, Text
Dialogue: 0,0:00:00.50,0:00:02.00,Default,,0,0,0,,Hello Tiếng Việt
";
        renderer.load_ass_memory(ass).expect("load ASS");

        let mut frame = vec![0u8; 640 * 360 * 4];
        let stats_active = renderer
            .render_and_blend_rgba(1000, &mut frame, 640, 360)
            .expect("render active frame");
        assert!(
            stats_active.bitmap_count > 0,
            "expected subtitle bitmaps at t=1000ms"
        );
        assert!(frame
            .as_chunks::<4>()
            .0
            .iter()
            .any(|px| px[0] > 0 || px[1] > 0 || px[2] > 0));

        let mut empty_frame = vec![0u8; 640 * 360 * 4];
        let stats_inactive = renderer
            .render_and_blend_rgba(5000, &mut empty_frame, 640, 360)
            .expect("render inactive frame");
        assert_eq!(stats_inactive.bitmap_count, 0);
    }
}
