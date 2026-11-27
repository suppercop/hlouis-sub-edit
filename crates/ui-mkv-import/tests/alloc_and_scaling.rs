//! Zero-allocation empty-filter check test for `ui-mkv-import`.

use mkv_import::TrackPickerRow;
use mkv_subs::SubtitleCodec;
use std::alloc::{GlobalAlloc, Layout, System};
use std::sync::atomic::{AtomicUsize, Ordering};
use ui_mkv_import::row_matches_filter;

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
fn empty_filter_zero_alloc() {
    let row = TrackPickerRow {
        display_index: 0,
        track_number: 1,
        codec_id: "S_TEXT/UTF8".to_owned(),
        codec: SubtitleCodec::Srt,
        language: "vie".to_owned(),
        name: "Vietnamese".to_owned(),
        is_default: true,
        is_forced: false,
        event_count: Some(420),
        preview_lines: Vec::new(),
        selectable: true,
        unselectable_reason: None,
        selected: true,
    };
    let before = ALLOC_COUNT.load(Ordering::Relaxed);
    for _ in 0..10_000 {
        assert!(std::hint::black_box(row_matches_filter(&row, "")));
    }
    let after = ALLOC_COUNT.load(Ordering::Relaxed);
    assert_eq!(after - before, 0);
}
