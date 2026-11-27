//! CPU-independent scaling invariant tests for `edit-ops` across `10k -> 20k -> 40k`.

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

    // SAFETY: Delegates allocations to `std::alloc::System` and counts allocations inside `ffi::measure`.
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

use ass_core::{AssEvent, AssTime, EventEntry, ScriptFile};
use edit_ops::{delete_selected, stable_sort, SortKey};
use std::time::{Duration, Instant};

fn build_script(n: usize) -> ScriptFile {
    let mut script = ScriptFile::new_default();
    script.ensure_events_section_mut().entries = (0..n)
        .map(|i| {
            let t0 = ((i.wrapping_mul(7919)) % n.max(1)) * 10;
            let start = AssTime::from_cs(u32::try_from(t0).unwrap_or(u32::MAX));
            let end = start.saturating_add_cs(150);
            EventEntry::Event(AssEvent::dialogue(start, end, format!("Line #{i}")))
        })
        .collect();
    script
}

#[test]
fn sort_and_delete_scale_o_n_log_n_and_o_n() {
    let s10 = build_script(10_000);
    let s20 = build_script(20_000);
    let s40 = build_script(40_000);

    let t1 = Instant::now();
    let (_, a_sort10) = ffi::measure(|| stable_sort(&s10, SortKey::Start, false, None));
    let d10 = t1.elapsed();
    let (_, a_sort20) = ffi::measure(|| stable_sort(&s20, SortKey::Start, false, None));
    let t4 = Instant::now();
    let (_, a_sort40) = ffi::measure(|| stable_sort(&s40, SortKey::Start, false, None));
    let d40 = t4.elapsed();

    assert!(a_sort20 * 10 <= a_sort10 * 25 + 64);
    assert!(a_sort40 * 10 <= a_sort10 * 48 + 64);
    assert!(
        d40 <= d10.max(Duration::from_millis(2)) * 9,
        "sort 40k ({d40:?}) exceeded O(N log N) bound vs 10k ({d10:?})"
    );

    let del10: Vec<usize> = (0..10_000).step_by(2).collect();
    let del40: Vec<usize> = (0..40_000).step_by(2).collect();
    let (_, a_del10) = ffi::measure(|| delete_selected(&s10, &del10));
    let (_, a_del40) = ffi::measure(|| delete_selected(&s40, &del40));
    assert!(a_del40 * 10 <= a_del10 * 44 + 64);
}
