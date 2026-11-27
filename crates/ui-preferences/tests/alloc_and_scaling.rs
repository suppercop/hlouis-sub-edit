//! Zero-allocation page-metadata lookup test for `ui-preferences`.

use std::alloc::{GlobalAlloc, Layout, System};
use std::sync::atomic::{AtomicUsize, Ordering};
use ui_preferences::PrefsPage;

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
fn zero_alloc_page_key_lookup() {
    let before = ALLOC_COUNT.load(Ordering::Relaxed);
    for _ in 0..1_000 {
        for p in PrefsPage::ALL {
            let _ = std::hint::black_box(p.ftl_key());
        }
    }
    let after = ALLOC_COUNT.load(Ordering::Relaxed);
    assert_eq!(after - before, 0);
}
