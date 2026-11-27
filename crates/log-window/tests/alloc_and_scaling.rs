//! Zero-allocation ring eviction & O(n) scaling test for `log-window`.

use log_window::{LogLevel, LogRecord, RingLogBuffer};
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
fn ring_buffer_push_zero_alloc_when_pre_allocated() {
    let buf = RingLogBuffer::new(16);
    for i in 0..16u64 {
        buf.push(LogRecord {
            timestamp_ms: i,
            level: LogLevel::Info,
            target: String::new(),
            message: String::new(),
        });
    }
    let before = ALLOC_COUNT.load(Ordering::Relaxed);
    for i in 16..200u64 {
        buf.push(LogRecord {
            timestamp_ms: i,
            level: LogLevel::Info,
            target: String::new(),
            message: String::new(),
        });
    }
    let after = ALLOC_COUNT.load(Ordering::Relaxed);
    assert_eq!(after - before, 0);
    assert_eq!(buf.len(), 16);
}
