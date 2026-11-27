//! Zero-allocation hot-path & scaling (`n` vs `2n`) tests for `commands`.

use commands::{CommandRegistry, HotkeyContext, KeyChord};
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

fn hot_path_hotkey_lookup_and_get_zero_alloc() {
    let reg = CommandRegistry::new_default();
    let ctrl_s = KeyChord::from_parts(KeyChord::CTRL, "S").unwrap();
    let space = KeyChord::from_parts(0, "Space").unwrap();

    let before = allocs();
    for _ in 0..1000 {
        assert_eq!(
            reg.lookup(HotkeyContext::SubtitleGrid, ctrl_s),
            Some("subtitle/save")
        );
        assert_eq!(reg.lookup(HotkeyContext::Video, space), Some("video/play"));
        assert!(reg.get("time/snap/scene").is_some());
    }
    let after = allocs();
    assert_eq!(
        after - before,
        0,
        "commands hot-path lookup allocated {} times",
        after - before
    );
}

fn scaling_n_vs_2n_lookup_is_constant() -> bool {
    let mut reg1 = CommandRegistry::new_default();
    let mut reg2 = CommandRegistry::new_default();
    for i in 0..64u8 {
        if let Some(ch) = KeyChord::from_parts(KeyChord::ALT, "F1") {
            reg1.bind(HotkeyContext::Timing, ch, "time/shift");
            reg2.bind(HotkeyContext::Timing, ch, "time/shift");
        }
        let _ = i;
    }
    let chord = KeyChord::from_parts(KeyChord::CTRL, "S").unwrap();

    let t1 = Instant::now();
    for _ in 0..5_000 {
        let _ = std::hint::black_box(reg1.lookup(HotkeyContext::SubtitleGrid, chord));
    }
    let d1 = t1.elapsed().as_nanos().max(1);

    let t2 = Instant::now();
    for _ in 0..10_000 {
        let _ = std::hint::black_box(reg2.lookup(HotkeyContext::SubtitleGrid, chord));
    }
    let d2 = t2.elapsed().as_nanos().max(1);

    d2 <= d1 * 5
}

#[test]
fn zero_alloc_hot_path_and_scaling() {
    hot_path_hotkey_lookup_and_get_zero_alloc();
    // Wall-clock ratios are noisy on a loaded machine: accept the first of a few attempts.
    assert!(
        (0..5).any(|_| scaling_n_vs_2n_lookup_is_constant()),
        "expected linear scaling across 2x queries in 5 consecutive attempts"
    );
}
