//! CPU-independent zero-allocation invariant tests for `audio-peaks::query_columns`.

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

use audio_peaks::{PeakBucket, WaveformPyramid};

#[test]
fn query_columns_and_prereserved_push_make_zero_allocations() {
    let mut pyr = match WaveformPyramid::new(48_000, 2) {
        Ok(p) => p,
        Err(err) => panic!("pyramid init failed: {err}"),
    };
    pyr.reserve_frames(480_000);
    let chunk = vec![1024i16; 48_000 * 2];

    let ((), push_allocs) = ffi::measure(|| {
        for _ in 0..5 {
            pyr.push_interleaved_i16(&chunk);
        }
    });
    pyr.finish();
    assert_eq!(
        push_allocs, 0,
        "pre-reserved push_interleaved_i16 allocated {push_allocs} times"
    );

    let mut cols: Vec<PeakBucket> = Vec::with_capacity(2000);
    pyr.query_columns(0, pyr.total_frames(), 2000, &mut cols);

    let (_, query_allocs) = ffi::measure(|| {
        for px in [256usize, 800, 2000] {
            pyr.query_columns(0, pyr.total_frames(), px, &mut cols);
        }
        cols.len()
    });
    assert_eq!(
        query_allocs, 0,
        "query_columns allocated {query_allocs} times"
    );
}
