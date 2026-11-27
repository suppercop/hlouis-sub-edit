//! Zero-allocation autosave-check & scaling tests for `ui-shell`.

use ass_core::{AssEvent, AssTime, ScriptFile};
use std::alloc::{GlobalAlloc, Layout, System};
use std::sync::atomic::{AtomicUsize, Ordering};
use std::time::Instant;
use ui_shell::AppShell;

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
        let s = (i as u32) * 5;
        sec.insert_event(
            i,
            AssEvent::dialogue(AssTime::from_cs(s), AssTime::from_cs(s + 100), "Line"),
        );
    }
    script
}

fn hot_path_zero_alloc() {
    let script = make_script(100);
    let shell = AppShell::new(script);

    let before = allocs();
    for i in 0..1_000u64 {
        let _ = std::hint::black_box(shell.should_autosave(i * 1_000_000));
        let _ = std::hint::black_box(shell.layout_state.compute(1280.0, 800.0, 1.0));
    }
    let after = allocs();
    assert_eq!(
        after - before,
        0,
        "ui-shell layout/autosave hot path allocated {} times",
        after - before
    );
}

fn scaling_n_vs_2n() {
    let t1 = Instant::now();
    let s1 = make_script(1_000);
    let d1 = t1.elapsed().as_nanos().max(1);

    let t2 = Instant::now();
    let s2 = make_script(2_000);
    let d2 = t2.elapsed().as_nanos().max(1);

    assert_eq!(s1.events().count() * 2, s2.events().count());
    assert!(d2 <= d1 * 5, "expected O(n) scaling: n={d1}ns, 2n={d2}ns");
}

#[test]
fn zero_alloc_hot_path_and_scaling() {
    hot_path_zero_alloc();
    scaling_n_vs_2n();
}
