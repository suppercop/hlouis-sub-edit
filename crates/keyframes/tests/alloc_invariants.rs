//! CPU-independent zero-allocation invariant tests for `keyframes` conversion hot paths.

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

use keyframes::{TimeType, Timecodes};

#[test]
fn cfr_and_vfr_conversions_make_zero_allocations() {
    let cfr = match Timecodes::from_cfr(24_000, 1_001) {
        Ok(tc) => tc,
        Err(err) => panic!("CFR init failed: {err}"),
    };
    let vfr_pts: Vec<i64> = (0..10_000i64).map(|f| f * 42).collect();
    let vfr = match Timecodes::from_vfr_ms(vfr_pts) {
        Ok(tc) => tc,
        Err(err) => panic!("VFR init failed: {err}"),
    };

    let (sum, allocs) = ffi::measure(|| {
        let mut acc = 0u64;
        for f in 0..5_000u32 {
            let cs = cfr.frame_to_time_cs(f, TimeType::Start);
            let back = cfr.time_cs_to_frame(cs, TimeType::Start);
            let v_cs = vfr.frame_to_time_cs(f, TimeType::Exact);
            let v_back = vfr.time_cs_to_frame(v_cs, TimeType::Exact);
            acc = acc
                .wrapping_add(u64::from(back))
                .wrapping_add(u64::from(v_back));
        }
        acc
    });
    assert!(sum > 0);
    assert_eq!(allocs, 0, "Timecodes conversions allocated {allocs} times");
}
