//! The only `unsafe` in this crate: Win32 cursor / DPI / GDI screen capture calls.

use crate::{Point, Rect};
use windows::Win32::Foundation::{HWND, POINT};
use windows::Win32::Graphics::Gdi::{
    BitBlt, CreateCompatibleDC, CreateDIBSection, DeleteDC, DeleteObject, GetDC, MonitorFromPoint,
    ReleaseDC, SelectObject, BITMAPINFO, BITMAPINFOHEADER, BI_RGB, CAPTUREBLT, DIB_RGB_COLORS,
    HBITMAP, HDC, HGDIOBJ, MONITOR_DEFAULTTONEAREST, SRCCOPY,
};
use windows::Win32::UI::HiDpi::{
    GetDpiForMonitor, SetThreadDpiAwarenessContext, DPI_AWARENESS_CONTEXT,
    DPI_AWARENESS_CONTEXT_PER_MONITOR_AWARE_V2, MDT_EFFECTIVE_DPI,
};
use windows::Win32::UI::WindowsAndMessaging::{
    GetCursorPos, GetSystemMetrics, SM_CXVIRTUALSCREEN, SM_CYVIRTUALSCREEN, SM_XVIRTUALSCREEN,
    SM_YVIRTUALSCREEN,
};

/// Makes the calling thread per-monitor-DPI-aware (v2) until dropped, so every coordinate and
/// screen DC below is in physical pixels regardless of the process manifest.
pub struct PhysicalPixels {
    previous: DPI_AWARENESS_CONTEXT,
}

impl PhysicalPixels {
    pub fn enter() -> Self {
        // SAFETY: changes only the calling thread's DPI awareness; the previous context is
        // restored on drop. A null return (unsupported) is handled by restoring nothing.
        let previous =
            unsafe { SetThreadDpiAwarenessContext(DPI_AWARENESS_CONTEXT_PER_MONITOR_AWARE_V2) };
        Self { previous }
    }
}

impl Drop for PhysicalPixels {
    fn drop(&mut self) {
        if !self.previous.0.is_null() {
            // SAFETY: restores the context returned by `SetThreadDpiAwarenessContext` above on
            // the same thread.
            unsafe {
                SetThreadDpiAwarenessContext(self.previous);
            }
        }
    }
}

/// Cursor position in the current thread's DPI awareness.
pub fn cursor_pos() -> windows::core::Result<Point> {
    let mut p = POINT::default();
    // SAFETY: `p` is a live, writable POINT.
    unsafe { GetCursorPos(&raw mut p)? };
    Ok(Point { x: p.x, y: p.y })
}

/// Virtual-screen rectangle (all monitors) in the current thread's DPI awareness.
pub fn virtual_screen() -> Rect {
    // SAFETY: `GetSystemMetrics` has no preconditions.
    unsafe {
        Rect {
            x: GetSystemMetrics(SM_XVIRTUALSCREEN),
            y: GetSystemMetrics(SM_YVIRTUALSCREEN),
            width: GetSystemMetrics(SM_CXVIRTUALSCREEN),
            height: GetSystemMetrics(SM_CYVIRTUALSCREEN),
        }
    }
}

/// Effective DPI of the monitor nearest to `p` (96 = 100 %).
pub fn dpi_at(p: Point) -> Option<u32> {
    // SAFETY: `MonitorFromPoint` with DEFAULTTONEAREST always returns a monitor handle; the
    // out-pointers reference live locals.
    unsafe {
        let mon = MonitorFromPoint(POINT { x: p.x, y: p.y }, MONITOR_DEFAULTTONEAREST);
        let (mut dx, mut dy) = (0u32, 0u32);
        GetDpiForMonitor(mon, MDT_EFFECTIVE_DPI, &raw mut dx, &raw mut dy).ok()?;
        Some(dx)
    }
}

/// GDI resources released in reverse order on drop.
struct Capture {
    screen: HDC,
    mem: HDC,
    bitmap: HBITMAP,
    old: HGDIOBJ,
}

impl Drop for Capture {
    fn drop(&mut self) {
        // SAFETY: every handle was created / obtained in `capture_bgra` and is released exactly
        // once here; null handles are skipped.
        unsafe {
            if !self.old.0.is_null() {
                SelectObject(self.mem, self.old);
            }
            if !self.bitmap.0.is_null() {
                let _ = DeleteObject(self.bitmap.into());
            }
            if !self.mem.0.is_null() {
                let _ = DeleteDC(self.mem);
            }
            if !self.screen.0.is_null() {
                ReleaseDC(None::<HWND>, self.screen);
            }
        }
    }
}

/// Copies the screen rectangle `r` into `out` as top-down BGRA rows (`out.len()` must be
/// `r.width * r.height * 4`). Areas outside every monitor come back black.
pub fn capture_bgra(r: Rect, out: &mut [u8]) -> Result<(), &'static str> {
    let w = usize::try_from(r.width).map_err(|_| "negative width")?;
    let h = usize::try_from(r.height).map_err(|_| "negative height")?;
    if w == 0 || h == 0 || out.len() != w * h * 4 {
        return Err("bad capture buffer");
    }
    let mut cap = Capture {
        screen: HDC::default(),
        mem: HDC::default(),
        bitmap: HBITMAP::default(),
        old: HGDIOBJ::default(),
    };
    // SAFETY: standard GDI capture sequence. `cap` owns each handle as soon as it exists and
    // releases it on every return path. The DIB section is 32-bpp top-down with exactly
    // `w * h` pixels, so `bits` points at `w * h * 4` readable bytes while `cap.bitmap` lives,
    // and the copy below stays inside that range.
    unsafe {
        cap.screen = GetDC(None);
        if cap.screen.0.is_null() {
            return Err("GetDC(screen) failed");
        }
        cap.mem = CreateCompatibleDC(Some(cap.screen));
        if cap.mem.0.is_null() {
            return Err("CreateCompatibleDC failed");
        }
        let info = BITMAPINFO {
            bmiHeader: BITMAPINFOHEADER {
                biSize: u32::try_from(std::mem::size_of::<BITMAPINFOHEADER>()).unwrap_or(40),
                biWidth: r.width,
                biHeight: -r.height,
                biPlanes: 1,
                biBitCount: 32,
                biCompression: BI_RGB.0,
                ..Default::default()
            },
            ..Default::default()
        };
        let mut bits: *mut core::ffi::c_void = core::ptr::null_mut();
        cap.bitmap = CreateDIBSection(
            Some(cap.mem),
            &raw const info,
            DIB_RGB_COLORS,
            &raw mut bits,
            None,
            0,
        )
        .map_err(|_| "CreateDIBSection failed")?;
        if bits.is_null() {
            return Err("CreateDIBSection returned no bits");
        }
        cap.old = SelectObject(cap.mem, cap.bitmap.into());
        BitBlt(
            cap.mem,
            0,
            0,
            r.width,
            r.height,
            Some(cap.screen),
            r.x,
            r.y,
            // CAPTUREBLT includes layered (e.g. translucent) windows.
            SRCCOPY | CAPTUREBLT,
        )
        .map_err(|_| "BitBlt failed")?;
        let src = std::slice::from_raw_parts(bits.cast::<u8>(), out.len());
        out.copy_from_slice(src);
    }
    Ok(())
}
