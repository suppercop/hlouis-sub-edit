//! Zero-allocation `SemVer` parsing test for `version-check`.

use std::alloc::{GlobalAlloc, Layout, System};
use std::sync::atomic::{AtomicUsize, Ordering};
use version_check::SemVer;

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

#[test]
fn semver_parse_zero_alloc() {
    let before = ALLOC_COUNT.load(Ordering::Relaxed);
    for _ in 0..10_000 {
        let v = std::hint::black_box(SemVer::parse("v1.23.45-rc.1"));
        assert!(v.is_some());
    }
    let after = ALLOC_COUNT.load(Ordering::Relaxed);
    assert_eq!(after - before, 0);
}
