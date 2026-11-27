//! Zero-allocation colour/DPI/schedule resolution & scaling tests for `ui-theme`.

use settings::AppearancePrefs;
use std::alloc::{GlobalAlloc, Layout, System};
use std::sync::atomic::{AtomicUsize, Ordering};
use std::time::Instant;
use theme::{BaseTheme, Rgba8, ThemeSpec};
use ui_theme::{one_px_stroke, resolve_theme_spec, rgba, snap_px};

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
    let prefs = AppearancePrefs::default();
    let c = Rgba8::rgb(220, 200, 180);

    let before = allocs();
    for m in 0..1440u16 {
        let spec = resolve_theme_spec(&prefs, true, m);
        let col = rgba(c);
        let snapped = snap_px(f32::from(m) + 0.3, 1.5);
        let stroke = one_px_stroke(c, 1.5);
        let _ = std::hint::black_box((spec, col, snapped, stroke));
    }
    let after = allocs();
    assert_eq!(
        after - before,
        0,
        "ui-theme hot-path helpers allocated {} times",
        after - before
    );
}

fn scaling_n_vs_2n() {
    let t1 = Instant::now();
    for i in 0..200u32 {
        let _ = std::hint::black_box(theme::palette(ThemeSpec::EyeComfort {
            base: BaseTheme::Dark,
            strength: u8::try_from(i % 101).unwrap(),
        }));
    }
    let d1 = t1.elapsed().as_nanos().max(1);

    let t2 = Instant::now();
    for i in 0..400u32 {
        let _ = std::hint::black_box(theme::palette(ThemeSpec::EyeComfort {
            base: BaseTheme::Dark,
            strength: u8::try_from(i % 101).unwrap(),
        }));
    }
    let d2 = t2.elapsed().as_nanos().max(1);

    assert!(d2 <= d1 * 4, "expected O(n) scaling: n={d1}ns, 2n={d2}ns");
}

#[test]
fn zero_alloc_hot_path_and_scaling() {
    hot_path_zero_alloc();
    scaling_n_vs_2n();
}
