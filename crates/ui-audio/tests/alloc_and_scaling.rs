//! Zero-allocation steady-state rasterization & scaling tests for `ui-audio`.

use audio_peaks::PeakBucket;
use std::alloc::{GlobalAlloc, Layout, System};
use std::sync::atomic::{AtomicUsize, Ordering};
use std::time::Instant;
use theme::ThemeSpec;
use ui_audio::{AudioColumnProvider, AudioPanelState};

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

struct MockProvider;
impl AudioColumnProvider for MockProvider {
    fn duration_ms(&self) -> i64 {
        60_000
    }
    fn query_columns_into(
        &self,
        _start_ms: i64,
        _end_ms: i64,
        width_px: usize,
        out: &mut Vec<PeakBucket>,
    ) {
        out.clear();
        for i in 0..width_px {
            #[allow(clippy::cast_possible_truncation, clippy::cast_possible_wrap)]
            let v = ((i % 32) * 800) as i16;
            out.push(PeakBucket {
                min: -v,
                max: v,
                rms: v.unsigned_abs(),
            });
        }
    }
}

fn hot_path_zero_alloc() {
    let mut state = AudioPanelState::new(60_000);
    let pal = theme::palette(ThemeSpec::Dark);
    let prov = MockProvider;
    // Warm up internal pixel_buf to 800x120.
    state.rasterize_columns(800, 120, &pal);
    let _ = prov.duration_ms();

    let before = allocs();
    for _ in 0..50 {
        state.rasterize_columns(800, 120, &pal);
    }
    let after = allocs();
    assert_eq!(
        after - before,
        0,
        "ui-audio steady-state rasterize allocated {} times",
        after - before
    );
}

fn scaling_n_vs_2n() {
    let mut s1 = AudioPanelState::new(60_000);
    let mut s2 = AudioPanelState::new(60_000);
    let pal = theme::palette(ThemeSpec::Dark);
    s1.rasterize_columns(500, 100, &pal);
    s2.rasterize_columns(1000, 100, &pal);

    let t1 = Instant::now();
    for _ in 0..100 {
        s1.rasterize_columns(500, 100, &pal);
    }
    let d1 = t1.elapsed().as_nanos().max(1);

    let t2 = Instant::now();
    for _ in 0..100 {
        s2.rasterize_columns(1000, 100, &pal);
    }
    let d2 = t2.elapsed().as_nanos().max(1);

    assert!(d2 <= d1 * 4, "expected O(n) scaling: n={d1}ns, 2n={d2}ns");
}

#[test]
fn zero_alloc_hot_path_and_scaling() {
    hot_path_zero_alloc();
    scaling_n_vs_2n();
}
