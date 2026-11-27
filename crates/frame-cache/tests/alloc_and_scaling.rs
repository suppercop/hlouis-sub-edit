//! Zero-allocation steady-state get/insert & scaling tests for `frame-cache`.

use frame_cache::{FrameCache, StepDirection};
use std::alloc::{GlobalAlloc, Layout, System};
use std::sync::Arc;
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

fn hot_path_steady_state_zero_alloc() {
    let frame_bytes = 4096;
    let mut cache = FrameCache::new(16 * frame_bytes, frame_bytes);
    // Warm up cache + buffer pool completely (16 slots + 1 evicted into pool).
    for f in 0..18u32 {
        let mut buf = cache.acquire_buffer(frame_bytes);
        if let Some(inner) = Arc::get_mut(&mut buf) {
            inner.frame_number = f;
            inner.is_keyframe = f % 8 == 0;
        }
        assert!(cache.insert(buf));
    }

    let mut plan = [0u32; 8];
    let kfs = [0u32, 8, 16, 24, 32, 40, 48];

    let before = allocs();
    for f in 18..500u32 {
        // Acquire pooled buffer, populate metadata, insert (evicts LRU into pool), then get.
        let mut buf = cache.acquire_buffer(frame_bytes);
        if let Some(inner) = Arc::get_mut(&mut buf) {
            inner.frame_number = f;
            inner.is_keyframe = f % 24 == 0;
        }
        assert!(cache.insert(buf));
        let got = cache.get(f);
        assert!(got.is_some());
        drop(got); // Drop UI ref before next eviction recycles it.
        let _ = cache.plan_prefetch(f, 1000, StepDirection::Forward, &kfs, &mut plan);
        assert!(cache.current_bytes() <= cache.budget_bytes());
    }
    let after = allocs();
    assert_eq!(
        after - before,
        0,
        "frame-cache steady-state get/insert/prefetch allocated {} times",
        after - before
    );
}

fn scaling_n_vs_2n_get_insert() {
    let frame_bytes = 1024;
    let mut c1 = FrameCache::new(64 * frame_bytes, frame_bytes);
    let mut c2 = FrameCache::new(128 * frame_bytes, frame_bytes);
    for f in 0..64u32 {
        let mut b = c1.acquire_buffer(frame_bytes);
        Arc::get_mut(&mut b).unwrap().frame_number = f;
        c1.insert(b);
    }
    for f in 0..128u32 {
        let mut b = c2.acquire_buffer(frame_bytes);
        Arc::get_mut(&mut b).unwrap().frame_number = f;
        c2.insert(b);
    }

    // Use min-of-3 to filter scheduler noise: O(1) per op means 2n ops take ≤ 2×
    // wall time of n ops.  Allow ×8 tolerance to cover loaded CI runners.
    let mut d1_min = u128::MAX;
    let mut d2_min = u128::MAX;
    for _ in 0..3 {
        let t1 = Instant::now();
        for i in 0..5_000u32 {
            let _ = std::hint::black_box(c1.get(i % 64));
        }
        d1_min = d1_min.min(t1.elapsed().as_nanos().max(1));

        let t2 = Instant::now();
        for i in 0..10_000u32 {
            let _ = std::hint::black_box(c2.get(i % 128));
        }
        d2_min = d2_min.min(t2.elapsed().as_nanos().max(1));
    }

    assert!(
        d2_min <= d1_min * 8,
        "expected O(1) per op (min-of-3): n={d1_min}ns, 2n={d2_min}ns"
    );
}

#[test]
fn zero_alloc_hot_path_and_scaling() {
    hot_path_steady_state_zero_alloc();
    scaling_n_vs_2n_get_insert();
}
