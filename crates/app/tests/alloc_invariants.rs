//! CPU-independent zero-allocation invariant tests for `hlouis-sub-edit` (`syntax`, `grid`,
//! and cached layout paths).

mod ffi {
    use std::alloc::{GlobalAlloc, Layout, System};
    use std::cell::Cell;

    pub struct CountingAlloc;

    std::thread_local! {
        static COUNTING: Cell<bool> = const { Cell::new(false) };
        static ALLOCS: Cell<usize> = const { Cell::new(0) };
    }

    pub fn measure<T>(f: impl FnOnce() -> T) -> (T, usize) {
        let _ = ALLOCS.try_with(|c| c.set(0));
        let _ = COUNTING.try_with(|flag| flag.set(true));
        let out = f();
        let _ = COUNTING.try_with(|flag| flag.set(false));
        let count = ALLOCS.try_with(Cell::get).unwrap_or(0);
        (out, count)
    }

    #[inline]
    fn record_if_active() {
        let _ = COUNTING.try_with(|flag| {
            if flag.get() {
                let _ = ALLOCS.try_with(|c| c.set(c.get().saturating_add(1)));
            }
        });
    }

    // SAFETY: Delegates allocations to `std::alloc::System` and counts allocations inside `ffi::measure`.
    unsafe impl GlobalAlloc for CountingAlloc {
        unsafe fn alloc(&self, layout: Layout) -> *mut u8 {
            record_if_active();
            // SAFETY: Forwards `GlobalAlloc::alloc` to `System`.
            unsafe { System.alloc(layout) }
        }

        unsafe fn alloc_zeroed(&self, layout: Layout) -> *mut u8 {
            record_if_active();
            // SAFETY: Forwards `GlobalAlloc::alloc_zeroed` to `System`.
            unsafe { System.alloc_zeroed(layout) }
        }

        unsafe fn dealloc(&self, ptr: *mut u8, layout: Layout) {
            // SAFETY: Forwards `GlobalAlloc::dealloc` to `System`.
            unsafe { System.dealloc(ptr, layout) }
        }

        unsafe fn realloc(&self, ptr: *mut u8, layout: Layout, new_size: usize) -> *mut u8 {
            record_if_active();
            // SAFETY: Forwards `GlobalAlloc::realloc` to `System`.
            unsafe { System.realloc(ptr, layout, new_size) }
        }
    }
}

#[global_allocator]
static GLOBAL: ffi::CountingAlloc = ffi::CountingAlloc;

use hlouis_sub_edit::grid::{format_ass_time_inline, GridState, SelectionSet};
use hlouis_sub_edit::syntax::{self, HighlightSpan};

#[test]
fn syntax_tokenize_and_grid_selection_make_zero_allocations() {
    let mut spans: Vec<HighlightSpan> = Vec::with_capacity(64);
    let sample =
        r"{\an8\fs44\c&H00FFFF&\t(0,400,\fscx120)}Xin chào Tiếng Việt\N{\k30}Ka{\kf45}raoke";
    syntax::tokenize_ass_line_into(sample, &mut spans);

    let (span_count, syn_allocs) = ffi::measure(|| {
        let mut total = 0usize;
        for _ in 0..200 {
            syntax::tokenize_ass_line_into(sample, &mut spans);
            total += spans.len();
        }
        total
    });
    assert!(span_count > 0);
    assert_eq!(
        syn_allocs, 0,
        "tokenize_ass_line_into allocated {syn_allocs} times"
    );

    let mut sel = SelectionSet::with_row_capacity(200_000);
    sel.insert(0);

    let (count_after, sel_allocs) = ffi::measure(|| {
        sel.select_all_rows(200_000);
        assert_eq!(sel.len(), 200_000);
        sel.clear();
        sel.extend(100..=5_000);
        let _ = sel.contains(&2_500);
        let _ = sel.remove(&2_500);
        let _ = sel.first();
        sel.select_all_rows(200_000);
        sel.len()
    });
    assert_eq!(count_after, 200_000);
    assert_eq!(sel_allocs, 0, "SelectionSet hot ops allocated {sel_allocs}");

    let mut grid = GridState::generate(1_000);
    // Pre-size active row text capacity as happens after first user edits.
    if let Some(line) = grid.active_line_mut() {
        line.text.reserve(64);
    }

    let ((), grid_allocs) = ffi::measure(|| {
        for step in 0..50 {
            if let Some(line) = grid.active_line_mut() {
                line.text.push('a');
                line.recompute_cps();
            }
            let _ = format_ass_time_inline(step * 150);
            grid.move_active(1, step % 2 == 0);
        }
        grid.select_all();
        grid.click_row(10, false, true);
        grid.click_row(10, false, true);
    });
    assert_eq!(
        grid_allocs, 0,
        "keystroke recompute_cps + grid nav + select_all allocated {grid_allocs} times"
    );
}
