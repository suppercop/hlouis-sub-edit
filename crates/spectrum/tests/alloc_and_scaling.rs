//! Zero-allocation viewport scroll & O(n) scaling tests for `spectrum`.

use spectrum::{ColorLut, SpectrumCache, SpectrumColorScheme, SpectrumQuality};
use std::alloc::{GlobalAlloc, Layout, System};
use std::cell::Cell;
use std::time::Instant;
use theme::ThemeSpec;

struct CountingAlloc;

thread_local! {
    // Allocations made by THIS thread only: rayon workers and the test harness keep allocating on
    // their own threads while we measure, which must not be charged to the scroll loop.
    static ALLOC_COUNT: Cell<usize> = const { Cell::new(0) };
}

#[global_allocator]
static GLOBAL: CountingAlloc = CountingAlloc;

#[allow(unsafe_code)]
unsafe impl GlobalAlloc for CountingAlloc {
    unsafe fn alloc(&self, layout: Layout) -> *mut u8 {
        let _ = ALLOC_COUNT.try_with(|c| c.set(c.get() + 1));
        unsafe { System.alloc(layout) }
    }
    unsafe fn dealloc(&self, ptr: *mut u8, layout: Layout) {
        unsafe { System.dealloc(ptr, layout) }
    }
}

fn allocs() -> usize {
    ALLOC_COUNT.with(Cell::get)
}

#[test]
fn zero_alloc_scroll_and_linear_scaling() {
    let samples = vec![0.25f32; 48_000 * 4];
    let cache = SpectrumCache::build_from_mono(&samples, 48_000, SpectrumQuality::Regular, 1.0);
    let pal = theme::palette(ThemeSpec::Dark);
    let lut = ColorLut::build(SpectrumColorScheme::IcyBlue, &pal);
    let mut rgba = vec![0u8; 256 * 64 * 4];

    let before = allocs();
    for offset in 0..32usize {
        cache.render_viewport_into(0, offset, 256, 64, Some((10, 50)), &lut, &mut rgba);
    }
    let after = allocs();
    assert_eq!(
        after - before,
        0,
        "scroll allocated {} times",
        after - before
    );

    // Scaling n vs 2n
    let s1 = vec![0.1f32; 48_000 * 10];
    let t1 = Instant::now();
    let c1 = SpectrumCache::build_from_mono(&s1, 48_000, SpectrumQuality::Regular, 1.0);
    let d1 = t1.elapsed().as_nanos().max(1);

    let s2 = vec![0.1f32; 48_000 * 20];
    let t2 = Instant::now();
    let c2 = SpectrumCache::build_from_mono(&s2, 48_000, SpectrumQuality::Regular, 1.0);
    let d2 = t2.elapsed().as_nanos().max(1);

    assert!(c2.total_columns() >= c1.total_columns());
    assert!(d2 <= d1 * 5, "expected O(n) scaling: n={d1}ns 2n={d2}ns");
}
