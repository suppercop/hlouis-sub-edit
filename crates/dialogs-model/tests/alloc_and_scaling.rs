//! Zero-allocation field update + validation & scaling tests for `dialogs-model`.

use dialogs_model::{DialogsState, StyleManagerAction};
use std::alloc::{GlobalAlloc, Layout, System};
use std::sync::atomic::{AtomicUsize, Ordering};
use std::time::Instant;

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

fn hot_path_field_updates_and_validation_zero_alloc() {
    let mut state = DialogsState::default();
    state.find_replace.query.push_str("hello");
    state.jump_to.total_frames = 50_000;

    let before = allocs();
    for i in 1..=500u32 {
        assert!(state.shift_times.set_time_cs(i * 10).is_ok());
        assert!(state.tpp.set_adjacent_bias(0.5).is_ok());
        assert!(state.find_replace.validate().is_ok());
        assert!(state.select_lines.validate().is_ok());
        assert!(state.resample.set_dest_resolution(1920, 1080).is_ok());
        let _ = state.style_manager.can_execute(StyleManagerAction::New);
        assert!(state.fonts_check.validate().is_ok());
        assert!(state.jump_to.set_frame(i).is_ok());
        assert!(state.properties.validate().is_ok());
    }
    let after = allocs();
    assert_eq!(
        after - before,
        0,
        "dialogs-model validation hot path allocated {} times",
        after - before
    );
}

fn scaling_n_vs_2n_validation() -> bool {
    let mut state = DialogsState::default();
    state.jump_to.total_frames = 100_000;

    let t1 = Instant::now();
    for i in 1..=10_000u32 {
        let _ = std::hint::black_box(state.resample.set_dest_resolution(1280 + (i % 640), 720));
        let _ = std::hint::black_box(state.jump_to.set_frame(i));
    }
    let d1 = t1.elapsed().as_nanos().max(1);

    let t2 = Instant::now();
    for i in 1..=20_000u32 {
        let _ = std::hint::black_box(state.resample.set_dest_resolution(1280 + (i % 640), 720));
        let _ = std::hint::black_box(state.jump_to.set_frame(i));
    }
    let d2 = t2.elapsed().as_nanos().max(1);

    d2 <= d1 * 4
}

#[test]
fn zero_alloc_hot_path_and_scaling() {
    hot_path_field_updates_and_validation_zero_alloc();
    // Wall-clock ratios are noisy on a loaded machine: accept the first of a few attempts.
    assert!(
        (0..5).any(|_| scaling_n_vs_2n_validation()),
        "expected O(n) scaling in 5 consecutive attempts"
    );
}
