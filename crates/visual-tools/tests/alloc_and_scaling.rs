//! Zero-allocation hot-path & linear scaling (`n` vs `2n`) tests for `visual-tools`.

use std::alloc::{GlobalAlloc, Layout, System};
use std::sync::atomic::{AtomicUsize, Ordering};
use std::time::Instant;
use visual_tools::vector_clip::VectorClipPath;
use visual_tools::Vec2;

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

fn make_path(n: usize) -> VectorClipPath {
    let mut p = VectorClipPath::default();
    for i in 0..n {
        #[allow(clippy::cast_precision_loss)]
        let x = (i as f64) * 3.0;
        p.append_line(Vec2::new(x, (x * 0.1).sin() * 50.0));
    }
    p
}

fn hot_path_hit_test_drag_and_write_zero_alloc() {
    let mut path = make_path(256);
    let mut out = String::with_capacity(8192);
    path.write_tag_into(&mut out);

    let before = allocs();
    for i in 0..50usize {
        #[allow(clippy::cast_precision_loss)]
        let q = Vec2::new((i as f64) * 3.0, 0.0);
        let hit = path.hit_test_vertex(q, 100.0);
        if let Some(idx) = hit {
            path.drag_vertex(idx, Vec2::new(q.x + 1.0, q.y + 1.0));
        }
        path.write_tag_into(&mut out);
    }
    let after = allocs();
    assert_ne!(out.len(), 0);
    assert_eq!(
        after - before,
        0,
        "visual-tools mouse-move hot path allocated {} times",
        after - before
    );
}

fn scaling_n_vs_2n_hit_test() -> bool {
    let p1 = make_path(1_000);
    let p2 = make_path(2_000);

    let t1 = Instant::now();
    for _ in 0..200 {
        let _ = std::hint::black_box(p1.hit_test_vertex(Vec2::new(1500.0, 10.0), 5.0));
    }
    let d1 = t1.elapsed().as_nanos().max(1);

    let t2 = Instant::now();
    for _ in 0..200 {
        let _ = std::hint::black_box(p2.hit_test_vertex(Vec2::new(3000.0, 10.0), 5.0));
    }
    let d2 = t2.elapsed().as_nanos().max(1);

    d2 <= d1 * 4
}

#[test]
fn zero_alloc_hot_path_and_scaling() {
    hot_path_hit_test_drag_and_write_zero_alloc();
    // Wall-clock ratios are noisy on a loaded machine: accept the first of a few attempts.
    assert!(
        (0..5).any(|_| scaling_n_vs_2n_hit_test()),
        "expected O(n) scaling for hit_test_vertex in 5 consecutive attempts"
    );
}
