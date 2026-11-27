//! Zero-allocation per-tick & scrub-coalescing + scaling tests for `playback`.

use keyframes::Timecodes;
use playback::{Clock, PlaybackConfig, PlaybackController};
use std::alloc::{GlobalAlloc, Layout, System};
use std::cell::Cell;
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

struct MockClock(Cell<u64>);
impl Clock for MockClock {
    fn now_ns(&self) -> u64 {
        self.0.get()
    }
}

fn hot_path_tick_and_scrub_zero_alloc() {
    let tc = Timecodes::from_cfr(24_000, 1_001).unwrap();
    let kfs: Vec<u32> = (0..1_000).map(|i| i * 24).collect();
    let mut pb = PlaybackController::new(tc, 24_000, kfs, PlaybackConfig::default());
    let clk = MockClock(Cell::new(0));
    pb.play(&clk);
    let mut queue = [0u32; 8];

    let before = allocs();
    for i in 0..1_000u64 {
        clk.0.set(i * 16_666_666);
        let d = pb.tick(&clk, &mut queue);
        assert!(d.decode_request_count <= 8);
    }
    for f in 0..1_000u32 {
        pb.scrub_to_frame(f * 10);
    }
    let _ = pb.tick(&clk, &mut queue);
    let after = allocs();
    assert_eq!(
        after - before,
        0,
        "playback tick/scrub hot path allocated {} times",
        after - before
    );
}

fn scaling_n_vs_2n_scrub_and_keyframe_seek() -> bool {
    let tc1 = Timecodes::from_cfr(24_000, 1_001).unwrap();
    let tc2 = Timecodes::from_cfr(24_000, 1_001).unwrap();
    let kfs1: Vec<u32> = (0..5_000).map(|i| i * 24).collect();
    let kfs2: Vec<u32> = (0..10_000).map(|i| i * 24).collect();
    let mut pb1 = PlaybackController::new(tc1, 120_000, kfs1, PlaybackConfig::default());
    let mut pb2 = PlaybackController::new(tc2, 240_000, kfs2, PlaybackConfig::default());

    let t1 = Instant::now();
    for i in 0..1_000u32 {
        pb1.scrub_to_frame(i * 10);
        let _ = pb1.seek_keyframe(true);
    }
    let d1 = t1.elapsed().as_nanos().max(1);

    let t2 = Instant::now();
    for i in 0..2_000u32 {
        pb2.scrub_to_frame(i * 10);
        let _ = pb2.seek_keyframe(true);
    }
    let d2 = t2.elapsed().as_nanos().max(1);

    d2 <= d1 * 5
}

#[test]
fn zero_alloc_hot_path_and_scaling() {
    hot_path_tick_and_scrub_zero_alloc();
    // Wall-clock ratios are noisy on a loaded machine: accept the first of a few attempts.
    assert!(
        (0..5).any(|_| scaling_n_vs_2n_scrub_and_keyframe_seek()),
        "expected O(n log K) scaling in 5 consecutive attempts"
    );
}
