//! Zero-allocation CPS/time/token-color helpers & scaling tests for `ui-editbox`.

use ass_core::AssTime;
use std::alloc::{GlobalAlloc, Layout, System};
use std::sync::atomic::{AtomicUsize, Ordering};
use std::time::Instant;
use syntax_highlight::{TokenKind, tokenize_into};
use theme::ThemeSpec;
use ui_editbox::{count_visible_chars_and_cps, format_time_field, token_color};

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

fn hot_path_zero_alloc() {
    let pal = theme::palette(ThemeSpec::Dark);
    let mut time_buf = String::with_capacity(32);
    let mut spans = Vec::with_capacity(128);
    let line = r"{\an5\pos(960,540)\fad(100,150)\blur2\c&H00FFFF&}Xin chào thế giới\NDòng thứ hai";

    let before = allocs();
    for _ in 0..500 {
        tokenize_into(line, &mut spans);
        let (ch, cps) = count_visible_chars_and_cps(line, 250);
        format_time_field(AssTime::from_cs(12_345), false, None, &mut time_buf);
        let col = token_color(TokenKind::TagName, &pal);
        let _ = std::hint::black_box((ch, cps, col));
    }
    let after = allocs();
    assert_ne!(spans.len(), 0);
    assert_eq!(
        after - before,
        0,
        "ui-editbox hot path allocated {} times",
        after - before
    );
}

fn scaling_n_vs_2n() {
    let chunk = r"{\pos(100,200)\blur2\c&HFFFFFF&}Hello ";
    let s1 = chunk.repeat(30);
    let s2 = chunk.repeat(60);
    let mut spans = Vec::with_capacity(1024);

    let t1 = Instant::now();
    for _ in 0..500 {
        tokenize_into(&s1, &mut spans);
    }
    let d1 = t1.elapsed().as_nanos().max(1);

    let t2 = Instant::now();
    for _ in 0..500 {
        tokenize_into(&s2, &mut spans);
    }
    let d2 = t2.elapsed().as_nanos().max(1);

    assert!(d2 <= d1 * 4, "expected O(n) scaling: n={d1}ns, 2n={d2}ns");
}

#[test]
fn zero_alloc_hot_path_and_scaling() {
    hot_path_zero_alloc();
    scaling_n_vs_2n();
}
