//! Zero-allocation unchanged-line cleaning & quick-select + scaling tests for `hlouis-tools`.

use ass_core::{AssEvent, AssTime, ScriptFile};
use hlouis_tools::{CleanOptions, clean_line_into, quick_select_by_style_into};
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

fn make_script(n: usize) -> ScriptFile {
    let mut script = ScriptFile::new_default();
    let sec = script.ensure_events_section_mut();
    for i in 0..n {
        #[allow(clippy::cast_possible_truncation)]
        let t = (i as u32) * 10;
        let text = if i % 4 == 0 {
            r"{\i1}  Hello world  \N  Line 2  {\i0}"
        } else {
            "Already clean subtitle text"
        };
        sec.insert_event(
            i,
            AssEvent::dialogue(AssTime::from_cs(t), AssTime::from_cs(t + 8), text),
        );
    }
    script
}

fn hot_path_zero_alloc() {
    let script = make_script(2_000);
    let mut sel_buf = Vec::with_capacity(2_000);
    let mut clean_buf = String::with_capacity(256);

    let before = allocs();
    let count = quick_select_by_style_into(&script, 0, false, &mut sel_buf);
    assert_eq!(count, Some(2_000));
    let changed = clean_line_into(
        "Already clean subtitle text",
        CleanOptions::default(),
        &mut clean_buf,
    );
    assert!(!changed);
    let after = allocs();
    assert_eq!(
        after - before,
        0,
        "hlouis-tools hot path allocated {} times",
        after - before
    );
}

fn scaling_n_vs_2n() {
    let s1 = make_script(20_000);
    let s2 = make_script(40_000);
    let empty = std::collections::BTreeSet::new();

    let t1 = Instant::now();
    let r1 = hlouis_tools::clean_whitespace(&s1, &empty, CleanOptions::default());
    let d1 = t1.elapsed().as_nanos().max(1);

    let t2 = Instant::now();
    let r2 = hlouis_tools::clean_whitespace(&s2, &empty, CleanOptions::default());
    let d2 = t2.elapsed().as_nanos().max(1);

    assert_eq!(r1.changed_count * 2, r2.changed_count);
    assert!(d2 <= d1 * 4, "expected O(n) scaling: n={d1}ns, 2n={d2}ns");
}

#[test]
fn zero_alloc_hot_path_and_scaling() {
    hot_path_zero_alloc();
    scaling_n_vs_2n();
}
