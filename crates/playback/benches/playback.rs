use criterion::{Criterion, black_box, criterion_group, criterion_main};
use keyframes::Timecodes;
use playback::{Clock, PlaybackConfig, PlaybackController};
use std::cell::Cell;

struct BenchClock(Cell<u64>);
impl Clock for BenchClock {
    fn now_ns(&self) -> u64 {
        self.0.get()
    }
}

fn bench_playback(c: &mut Criterion) {
    let tc = Timecodes::from_cfr(24_000, 1_001).expect("cfr");
    let kfs: Vec<u32> = (0..1_000).map(|i| i * 24).collect();

    c.bench_function("playback_tick_scheduling_decision", |b| {
        let mut pb =
            PlaybackController::new(tc.clone(), 36_000, kfs.clone(), PlaybackConfig::default());
        let clk = BenchClock(Cell::new(0));
        pb.set_looping(true);
        pb.play(&clk);
        let mut queue = [0u32; 4];
        let mut t = 0u64;
        b.iter(|| {
            t = t.wrapping_add(16_666_666);
            clk.0.set(t);
            let d = pb.tick(&clk, &mut queue);
            black_box(d);
        });
    });

    c.bench_function("playback_scrub_coalescing_1000_events", |b| {
        let mut pb =
            PlaybackController::new(tc.clone(), 36_000, kfs.clone(), PlaybackConfig::default());
        let clk = BenchClock(Cell::new(0));
        let mut queue = [0u32; 4];
        b.iter(|| {
            for i in 0..1_000u32 {
                pb.scrub_to_frame(black_box(i * 30));
            }
            let d = pb.tick(&clk, &mut queue);
            black_box(d);
        });
    });
}

criterion_group!(benches, bench_playback);
criterion_main!(benches);
