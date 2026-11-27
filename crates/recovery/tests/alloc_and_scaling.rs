//! Zero-allocation diff-counting hot path test for `recovery`.

use recovery::count_diff_lines;
use std::alloc::{GlobalAlloc, Layout, System};
use std::sync::atomic::{AtomicUsize, Ordering};

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
fn count_diff_lines_zero_alloc() {
    let a = "line1\nline2\nline3\n";
    let b = "line1\nline2_mod\nline3\n";
    let before = ALLOC_COUNT.load(Ordering::Relaxed);
    for _ in 0..1_000 {
        assert_eq!(std::hint::black_box(count_diff_lines(a, b)), 1);
    }
    let after = ALLOC_COUNT.load(Ordering::Relaxed);
    assert_eq!(after - before, 0);
}
