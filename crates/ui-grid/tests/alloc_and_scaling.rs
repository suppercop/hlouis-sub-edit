//! Zero-allocation CPS/time formatting & scaling tests for `ui-grid`.

use ass_core::{AssEvent, AssTime};
use std::alloc::{GlobalAlloc, Layout, System};
use std::sync::atomic::{AtomicUsize, Ordering};
use std::time::Instant;
use ui_grid::{compute_event_cps, format_ass_time_into};

struct CountingAlloc;

static ALLOC_COUNT: AtomicUsize = AtomicUsize::new(0);

#[global_allocator]
static GLOBAL: CountingAlloc = CountingAlloc;

#[allow(unsafe_code)]
unsafe impl GlobalAlloc for CountingAlloc {
    unsafe fn alloc(&self, layout: Layout) -> *mut u8 {
        ALLOC_COUNT.fetch_add(1, Ordering::Relaxed);
        unsafe { System.alloc(layout) }
    }

    unsafe fn dealloc(&self, ptr: *mut u8, layout: Layout) {
        unsafe { System.dealloc(ptr, layout) }
    }
}

fn allocs() -> usize {
    ALLOC_COUNT.load(Ordering::Relaxed)
}

fn hot_path_zero_alloc() {
    let ev = AssEvent::dialogue(
        AssTime::from_cs(12_345),
        AssTime::from_cs(12_545),
        r"{\pos(960,540)\blur2}Hello subtitle line\NSecond line",
    );
    let mut time_buf = String::with_capacity(32);

    let before = allocs();
    for _ in 0..1_000 {
        let cps = compute_event_cps(&ev);
        format_ass_time_into(ev.start(), &mut time_buf);
        assert!(cps > 0);
    }
    let after = allocs();
    assert_ne!(time_buf.len(), 0);
    assert_eq!(
        after - before,
        0,
        "ui-grid per-row formatting allocated {} times",
        after - before
    );
}

fn scaling_n_vs_2n() {
    let ev = AssEvent::dialogue(
        AssTime::from_cs(100),
        AssTime::from_cs(300),
        r"{\i1}Sample subtitle line{\i0}",
    );
    let t1 = Instant::now();
    for _ in 0..20_000 {
        let _ = std::hint::black_box(compute_event_cps(&ev));
    }
    let d1 = t1.elapsed().as_nanos().max(1);

    let t2 = Instant::now();
    for _ in 0..40_000 {
        let _ = std::hint::black_box(compute_event_cps(&ev));
    }
    let d2 = t2.elapsed().as_nanos().max(1);

    assert!(d2 <= d1 * 4, "expected O(n) scaling: n={d1}ns, 2n={d2}ns");
}

#[test]
fn zero_alloc_hot_path_and_scaling() {
    hot_path_zero_alloc();
    scaling_n_vs_2n();
}
