//! Zero-allocation hot-path & linear scaling (`n` vs `2n`) tests for `grid-model`.

use grid_model::{FontMetrics, GridModel, OverlapIndex, RowFlags, RowTiming};

struct FixedFont;
impl FontMetrics for FixedFont {
    fn row_height(&self) -> f32 {
        22.0
    }
}
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

fn make_rows(n: usize) -> Vec<RowTiming> {
    (0..n)
        .map(|i| {
            #[allow(clippy::cast_possible_truncation)]
            let start = (i as u32) * 100;
            RowTiming {
                start_cs: start,
                end_cs: start + 120,
                is_comment: i % 11 == 0,
            }
        })
        .collect()
}

fn hot_path_visible_rows_and_overlap_update_zero_alloc() {
    let rows = make_rows(2_000);
    let mut model = GridModel::new();
    model.load_rows(&rows);
    let mut visible: Vec<RowFlags> = Vec::with_capacity(64);
    // Warm up internal scratch buffer.
    model.update_row_timing(
        50,
        RowTiming {
            start_cs: 5000,
            end_cs: 5150,
            is_comment: false,
        },
    );

    let before = allocs();
    for frame in 0..100u32 {
        model.visible_rows_into(600.0, &FixedFont, Some(frame * 100), &mut visible);
        model.update_row_timing(
            50,
            RowTiming {
                start_cs: 5000 + frame,
                end_cs: 5150 + frame,
                is_comment: false,
            },
        );
    }
    let after = allocs();
    assert_ne!(visible.len(), 0);
    assert_eq!(
        after - before,
        0,
        "grid-model hot path allocated {} times",
        after - before
    );
}

fn scaling_n_vs_2n_overlap_rebuild() -> bool {
    let r1 = make_rows(20_000);
    let r2 = make_rows(40_000);
    let mut idx1 = OverlapIndex::default();
    let mut idx2 = OverlapIndex::default();
    idx1.rebuild(&r1);
    idx2.rebuild(&r2);

    let t1 = Instant::now();
    idx1.rebuild(&r1);
    let d1 = t1.elapsed().as_nanos().max(1);

    let t2 = Instant::now();
    idx2.rebuild(&r2);
    let d2 = t2.elapsed().as_nanos().max(1);

    d2 <= d1 * 4
}

#[test]
fn zero_alloc_hot_path_and_scaling() {
    hot_path_visible_rows_and_overlap_update_zero_alloc();
    // Wall-clock ratios are noisy on a loaded machine: accept the first of a few attempts.
    assert!(
        (0..5).any(|_| scaling_n_vs_2n_overlap_rebuild()),
        "expected O(n log n) scaling in 5 consecutive attempts"
    );
}
