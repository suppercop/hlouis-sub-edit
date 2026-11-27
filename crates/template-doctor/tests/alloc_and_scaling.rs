//! Zero-allocation clean-script linting & scaling tests for `template-doctor`.

use ass_core::parse_ass;
use std::alloc::{GlobalAlloc, Layout, System};
use std::fmt::Write as _;
use std::sync::atomic::{AtomicUsize, Ordering};
use std::time::Instant;
use template_doctor::{Diagnostic, is_template_or_code_line, lint_script_into};

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

fn make_templates(n: usize) -> ass_core::ScriptFile {
    let mut s = String::from(
        "[Script Info]\nPlayResX: 1920\nPlayResY: 1080\n\n\
         [Events]\n\
         Format: Layer, Start, End, Style, Name, MarginL, MarginR, MarginV, Effect, Text\n",
    );
    for i in 0..n {
        if i % 4 == 0 {
            let _ = writeln!(
                s,
                "Comment: 0,0:00:00.00,0:00:05.00,Default,,0,0,0,code syl,x = math.sin({i})"
            );
        } else {
            let _ = writeln!(
                s,
                "Comment: 0,0:00:00.00,0:00:05.00,Default,,0,0,0,template syl noblank,{{\\an5\\pos($scenter,$smiddle)\\fad(80,120)\\blur2}}"
            );
        }
    }
    parse_ass(&s).value
}

fn hot_path_classify_zero_alloc() {
    let script = make_templates(500);
    let before = allocs();
    let mut count = 0usize;
    for ev in script.events() {
        if is_template_or_code_line(ev) {
            count += 1;
        }
    }
    let after = allocs();
    assert_eq!(count, 500);
    assert_eq!(
        after - before,
        0,
        "template-doctor classifier allocated {} times",
        after - before
    );
}

fn scaling_n_vs_2n_lint() {
    let s1 = make_templates(500);
    let s2 = make_templates(1_000);
    let mut out = Vec::<Diagnostic>::with_capacity(64);

    let t1 = Instant::now();
    lint_script_into(&s1, &mut out);
    let d1 = t1.elapsed().as_nanos().max(1);
    assert_eq!(out.len(), 0);

    let t2 = Instant::now();
    lint_script_into(&s2, &mut out);
    let d2 = t2.elapsed().as_nanos().max(1);
    assert_eq!(out.len(), 0);

    assert!(d2 <= d1 * 4, "expected O(n) scaling: n={d1}ns, 2n={d2}ns");
}

#[test]
fn zero_alloc_hot_path_and_scaling() {
    hot_path_classify_zero_alloc();
    scaling_n_vs_2n_lint();
}
