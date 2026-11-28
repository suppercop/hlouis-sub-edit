//! Zero-allocation lookup/formatting & scaling tests for `i18n-catalog`.

use i18n_catalog::{Catalog, Locale};
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

fn hot_path_lookup_and_format_zero_alloc() {
    let vi = Catalog::for_locale(Locale::Vi);
    let mut out = String::with_capacity(256);

    let before = allocs();
    for _ in 0..1_000 {
        let m = vi.get("grid-col-start");
        assert!(m.is_some());
        assert!(vi.format_into(
            "status-selected-count",
            &[("selected", "12"), ("total", "500")],
            &mut out
        ));
    }
    let after = allocs();
    assert_ne!(out.len(), 0);
    assert_eq!(
        after - before,
        0,
        "i18n-catalog lookup/format allocated {} times",
        after - before
    );
}

fn scaling_n_vs_2n_lookups() -> bool {
    let en = Catalog::for_locale(Locale::En);
    let keys = [
        "menu-file",
        "grid-col-cps",
        "dlg-shift-title",
        "prefs-eye-comfort",
        "err-mkv-zero-events",
    ];

    let t1 = Instant::now();
    for i in 0..20_000usize {
        let _ = std::hint::black_box(en.get(keys[i % keys.len()]));
    }
    let d1 = t1.elapsed().as_nanos().max(1);

    let t2 = Instant::now();
    for i in 0..40_000usize {
        let _ = std::hint::black_box(en.get(keys[i % keys.len()]));
    }
    let d2 = t2.elapsed().as_nanos().max(1);

    d2 <= d1 * 4
}

#[test]
fn zero_alloc_hot_path_and_scaling() {
    hot_path_lookup_and_format_zero_alloc();
    // Wall-clock ratios are noisy on a loaded machine: accept the first of a few attempts.
    assert!(
        (0..5).any(|_| scaling_n_vs_2n_lookups()),
        "expected O(n) scaling in 5 consecutive attempts"
    );
}
