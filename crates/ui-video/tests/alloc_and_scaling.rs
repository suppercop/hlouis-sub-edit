//! Zero-allocation coordinate transforms & scaling tests for `ui-video`.

use egui::{Pos2, Rect, Vec2};
use std::alloc::{GlobalAlloc, Layout, System};
use std::sync::atomic::{AtomicUsize, Ordering};
use std::time::Instant;
use ui_video::VideoPanelState;

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
    let vr = Rect::from_min_size(Pos2::new(20.0, 20.0), Vec2::new(640.0, 360.0));
    let before = allocs();
    for i in 0..1_000u32 {
        let sp = VideoPanelState::video_to_screen(vr, 1920.0, 1080.0, f64::from(i), 540.0);
        let vp = VideoPanelState::screen_to_video(vr, 1920.0, 1080.0, sp);
        let _ = std::hint::black_box(vp);
    }
    let after = allocs();
    assert_eq!(
        after - before,
        0,
        "ui-video coordinate transform allocated {} times",
        after - before
    );
}

fn scaling_n_vs_2n() {
    let vr = Rect::from_min_size(Pos2::ZERO, Vec2::new(640.0, 360.0));
    let t1 = Instant::now();
    for i in 0..50_000u32 {
        let _ = std::hint::black_box(VideoPanelState::video_to_screen(
            vr,
            1920.0,
            1080.0,
            f64::from(i % 1920),
            500.0,
        ));
    }
    let d1 = t1.elapsed().as_nanos().max(1);

    let t2 = Instant::now();
    for i in 0..100_000u32 {
        let _ = std::hint::black_box(VideoPanelState::video_to_screen(
            vr,
            1920.0,
            1080.0,
            f64::from(i % 1920),
            500.0,
        ));
    }
    let d2 = t2.elapsed().as_nanos().max(1);

    assert!(d2 <= d1 * 4, "expected O(n) scaling: n={d1}ns, 2n={d2}ns");
}

#[test]
fn zero_alloc_hot_path_and_scaling() {
    hot_path_zero_alloc();
    scaling_n_vs_2n();
}
