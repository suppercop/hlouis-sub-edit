//! CPU-independent zero-allocation invariant tests for `scene-detect` per-frame analysis.

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

use scene_detect::{SceneDetectConfig, SceneDetector};

#[test]
fn push_frame_steady_state_zero_allocations() {
    let w = 1920usize;
    let h = 1080usize;
    let y_plane = vec![96u8; w * h];
    let mut detector = SceneDetector::new(SceneDetectConfig::default());

    // Frame 0 initializes working planes.
    if let Err(err) = detector.push_frame(&y_plane, w, h, w) {
        panic!("frame 0 push failed: {err}");
    }

    let ((), allocs) = ffi::measure(|| {
        for _ in 0..8 {
            if let Err(err) = detector.push_frame(&y_plane, w, h, w) {
                panic!("steady-state push failed: {err}");
            }
        }
    });
    assert_eq!(
        allocs, 0,
        "SceneDetector::push_frame allocated {allocs} times in steady state"
    );
}
