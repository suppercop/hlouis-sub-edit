//! CPU-independent zero-allocation invariant tests for `render` hot paths.

mod ffi {
    use std::alloc::{GlobalAlloc, Layout, System};
    use std::cell::Cell;

    pub struct CountingAlloc;

    std::thread_local! {
        static COUNTING: Cell<bool> = const { Cell::new(false) };
        static ALLOCS: Cell<usize> = const { Cell::new(0) };
    }

    pub fn measure<T>(f: impl FnOnce() -> T) -> (T, usize) {
        let _ = ALLOCS.try_with(|c| c.set(0));
        let _ = COUNTING.try_with(|flag| flag.set(true));
        let out = f();
        let _ = COUNTING.try_with(|flag| flag.set(false));
        let count = ALLOCS.try_with(Cell::get).unwrap_or(0);
        (out, count)
    }

    #[inline]
    fn record_if_active() {
        let _ = COUNTING.try_with(|flag| {
            if flag.get() {
                let _ = ALLOCS.try_with(|c| c.set(c.get().saturating_add(1)));
            }
        });
    }

    // SAFETY: Delegates all allocations directly to `std::alloc::System` while counting
    // allocations occurring on threads inside `ffi::measure`.
    unsafe impl GlobalAlloc for CountingAlloc {
        unsafe fn alloc(&self, layout: Layout) -> *mut u8 {
            record_if_active();
            // SAFETY: Preconditions of `GlobalAlloc::alloc` forwarded directly to `System`.
            unsafe { System.alloc(layout) }
        }

        unsafe fn alloc_zeroed(&self, layout: Layout) -> *mut u8 {
            record_if_active();
            // SAFETY: Preconditions of `GlobalAlloc::alloc_zeroed` forwarded directly to `System`.
            unsafe { System.alloc_zeroed(layout) }
        }

        unsafe fn dealloc(&self, ptr: *mut u8, layout: Layout) {
            // SAFETY: Preconditions of `GlobalAlloc::dealloc` forwarded directly to `System`.
            unsafe { System.dealloc(ptr, layout) }
        }

        unsafe fn realloc(&self, ptr: *mut u8, layout: Layout, new_size: usize) -> *mut u8 {
            record_if_active();
            // SAFETY: Preconditions of `GlobalAlloc::realloc` forwarded directly to `System`.
            unsafe { System.realloc(ptr, layout, new_size) }
        }
    }
}

#[global_allocator]
static GLOBAL: ffi::CountingAlloc = ffi::CountingAlloc;

use render::AssSubRenderer;

#[test]
fn render_and_blend_zero_rust_allocations_per_frame() {
    let mut renderer = match AssSubRenderer::new(1920, 1080) {
        Ok(r) => r,
        Err(err) => panic!("AssSubRenderer init failed: {err}"),
    };
    let script = "[Script Info]\nScriptType: v4.00+\nPlayResX: 1920\nPlayResY: 1080\n\n\
                  [V4+ Styles]\nFormat: Name, Fontname, Fontsize, PrimaryColour, SecondaryColour, OutlineColour, BackColour, Bold, Italic, Underline, StrikeOut, ScaleX, ScaleY, Spacing, Angle, BorderStyle, Outline, Shadow, Alignment, MarginL, MarginR, MarginV, Encoding\n\
                  Style: Default,Arial,48,&H00FFFFFF,&H0000FFFF,&H00000000,&H80000000,-1,0,0,0,100,100,0,0,1,2,1,2,20,20,20,1\n\n\
                  [Events]\nFormat: Layer, Start, End, Style, Name, MarginL, MarginR, MarginV, Effect, Text\n\
                  Dialogue: 0,0:00:00.00,0:00:05.00,Default,,0,0,0,,Zero allocation hot path check\n";
    if let Err(err) = renderer.load_ass_memory(script) {
        panic!("load_ass_memory failed: {err}");
    }
    let mut frame = vec![32u8; 1920 * 1080 * 4];
    // Warm up libass font provider and bitmap pipeline before measuring.
    if let Err(err) = renderer.render_and_blend_rgba(500, &mut frame, 1920, 1080) {
        panic!("warmup render failed: {err}");
    }

    let ((), allocs_active) = ffi::measure(|| {
        for frame_idx in 0..10i64 {
            if let Err(err) =
                renderer.render_and_blend_rgba(500 + frame_idx * 40, &mut frame, 1920, 1080)
            {
                panic!("render failed: {err}");
            }
        }
    });
    assert_eq!(
        allocs_active, 0,
        "render_and_blend_rgba allocated {allocs_active} times on active frames"
    );

    let ((), allocs_empty) = ffi::measure(|| {
        for frame_idx in 0..10i64 {
            if let Err(err) =
                renderer.render_and_blend_rgba(10_000 + frame_idx * 40, &mut frame, 1920, 1080)
            {
                panic!("empty render failed: {err}");
            }
        }
    });
    assert_eq!(
        allocs_empty, 0,
        "render_and_blend_rgba allocated {allocs_empty} times on empty frames"
    );
}
