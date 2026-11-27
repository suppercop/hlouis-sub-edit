//! Zero-allocation dialog-state checks & scaling tests for `ui-dialogs`.

use dialogs_model::DialogsState;
use std::alloc::{GlobalAlloc, Layout, System};
use std::sync::atomic::{AtomicUsize, Ordering};
use std::time::Instant;
use ui_dialogs::{ActiveDialog, PrefsPage};

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
    let mut models = DialogsState::default();
    let before = allocs();
    for i in 1..=1_000u32 {
        assert!(models.shift_times.set_time_cs(i).is_ok());
        assert!(models.resample.set_dest_resolution(1920, 1080).is_ok());
        let _ = std::hint::black_box((ActiveDialog::Preferences, PrefsPage::Appearance));
    }
    let after = allocs();
    assert_eq!(
        after - before,
        0,
        "ui-dialogs hot path allocated {} times",
        after - before
    );
}

fn scaling_n_vs_2n() {
    let mut models = DialogsState::default();
    let t1 = Instant::now();
    for i in 1..=10_000u32 {
        let _ = std::hint::black_box(models.shift_times.set_time_cs(i));
    }
    let d1 = t1.elapsed().as_nanos().max(1);

    let t2 = Instant::now();
    for i in 1..=20_000u32 {
        let _ = std::hint::black_box(models.shift_times.set_time_cs(i));
    }
    let d2 = t2.elapsed().as_nanos().max(1);

    assert!(d2 <= d1 * 4, "expected O(n) scaling: n={d1}ns, 2n={d2}ns");
}

#[test]
fn zero_alloc_hot_path_and_scaling() {
    hot_path_zero_alloc();
    scaling_n_vs_2n();
}
