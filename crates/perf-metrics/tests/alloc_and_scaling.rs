//! Zero-allocation record/query & scaling tests for `perf-metrics`.

use perf_metrics::{HdrHistogram, PerfMetrics};
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

fn hot_path_record_and_percentile_zero_alloc() {
    let mut pm = PerfMetrics::default();
    let mut table_buf = String::with_capacity(1024);
    let mut json_buf = String::with_capacity(1024);

    let before = allocs();
    for i in 1..=2_000u64 {
        pm.keystroke_to_present.begin(i * 10_000_000);
        pm.record_frame_us((i * 37) % 50_000, i * 10_000_000 + 4_000_000);
        let _ = std::hint::black_box(pm.frame_time.p50());
        let _ = std::hint::black_box(pm.frame_time.p95());
        let _ = std::hint::black_box(pm.frame_time.p99());
        let _ = std::hint::black_box(pm.frame_time.worst());
    }
    pm.format_text_table(&mut table_buf);
    pm.format_json(&mut json_buf);
    let after = allocs();
    assert_ne!(table_buf.len(), 0);
    assert_ne!(json_buf.len(), 0);
    assert_eq!(
        after - before,
        0,
        "perf-metrics hot path allocated {} times",
        after - before
    );
}

fn scaling_n_vs_2n_record() {
    let mut h1 = HdrHistogram::new();
    let mut h2 = HdrHistogram::new();

    let t1 = Instant::now();
    for i in 0..50_000u64 {
        h1.record(i % 20_000);
    }
    let d1 = t1.elapsed().as_nanos().max(1);

    let t2 = Instant::now();
    for i in 0..100_000u64 {
        h2.record(i % 20_000);
    }
    let d2 = t2.elapsed().as_nanos().max(1);

    assert!(d2 <= d1 * 4, "expected O(n) scaling: n={d1}ns, 2n={d2}ns");
}

#[test]
fn zero_alloc_hot_path_and_scaling() {
    hot_path_record_and_percentile_zero_alloc();
    scaling_n_vs_2n_record();
}
