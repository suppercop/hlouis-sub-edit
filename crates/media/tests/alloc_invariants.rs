//! CPU-independent zero-allocation invariant tests for `media` upload preparation.

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

    // SAFETY: Delegates allocations to `std::alloc::System` and counts allocations made inside
    // `ffi::measure` on the calling thread.
    unsafe impl GlobalAlloc for CountingAlloc {
        unsafe fn alloc(&self, layout: Layout) -> *mut u8 {
            record_if_active();
            // SAFETY: Forwards `GlobalAlloc::alloc` to `System`.
            unsafe { System.alloc(layout) }
        }

        unsafe fn alloc_zeroed(&self, layout: Layout) -> *mut u8 {
            record_if_active();
            // SAFETY: Forwards `GlobalAlloc::alloc_zeroed` to `System`.
            unsafe { System.alloc_zeroed(layout) }
        }

        unsafe fn dealloc(&self, ptr: *mut u8, layout: Layout) {
            // SAFETY: Forwards `GlobalAlloc::dealloc` to `System`.
            unsafe { System.dealloc(ptr, layout) }
        }

        unsafe fn realloc(&self, ptr: *mut u8, layout: Layout, new_size: usize) -> *mut u8 {
            record_if_active();
            // SAFETY: Forwards `GlobalAlloc::realloc` to `System`.
            unsafe { System.realloc(ptr, layout, new_size) }
        }
    }
}

#[global_allocator]
static GLOBAL: ffi::CountingAlloc = ffi::CountingAlloc;

#[test]
fn copy_rgba_plane_zero_allocations_with_reused_buffer() {
    let w = 1920u32;
    let h = 1080u32;
    let tight_stride = (w as usize) * 4;
    let padded_stride = tight_stride + 128;

    let src_tight = vec![127u8; tight_stride * (h as usize)];
    let src_padded = vec![200u8; padded_stride * (h as usize)];
    let mut dst = Vec::with_capacity(tight_stride * (h as usize));

    // Warm once so `dst` has full capacity before measuring steady state.
    if let Err(err) = media::copy_rgba_plane(&src_tight, tight_stride, w, h, &mut dst) {
        panic!("warmup tight copy failed: {err}");
    }

    let ((), tight_allocs) = ffi::measure(|| {
        for _ in 0..5 {
            if let Err(err) = media::copy_rgba_plane(&src_tight, tight_stride, w, h, &mut dst) {
                panic!("tight copy failed: {err}");
            }
        }
    });
    assert_eq!(tight_allocs, 0, "tight copy allocated {tight_allocs} times");

    let ((), padded_allocs) = ffi::measure(|| {
        for _ in 0..5 {
            if let Err(err) = media::copy_rgba_plane(&src_padded, padded_stride, w, h, &mut dst) {
                panic!("padded copy failed: {err}");
            }
        }
    });
    assert_eq!(
        padded_allocs, 0,
        "padded copy allocated {padded_allocs} times"
    );
}
