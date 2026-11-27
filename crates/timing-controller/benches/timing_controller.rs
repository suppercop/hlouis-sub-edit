use criterion::{black_box, criterion_group, criterion_main, Criterion};
use timing_controller::{DialogueTimingController, DragTarget, SnapIndex, TimingControllerConfig};

fn bench_timing_controller(c: &mut Criterion) {
    // 20,000 lines (40,000 boundaries) + 3,000 keyframes over 24 minutes (1,440,000 ms).
    let keyframes: Vec<i64> = (0..3_000).map(|i| i * 480).collect();
    let mut bounds = Vec::with_capacity(40_000);
    for i in 0..20_000i64 {
        bounds.push(i * 72);
        bounds.push(i * 72 + 55);
    }
    let snap_index = SnapIndex::from_raw(keyframes, bounds);

    let mut ctrl = DialogueTimingController::new(TimingControllerConfig::default(), 1_440_000);
    ctrl.begin_drag(DragTarget::Start);

    let mut cursor = 100_000i64;
    c.bench_function("timing_controller_drag_snap_20k_lines_3k_kf", |b| {
        b.iter(|| {
            cursor = (cursor + 37) % 1_400_000;
            let res = ctrl.update_drag(black_box(cursor), 40, false, black_box(&snap_index));
            black_box(res);
        });
    });
}

criterion_group!(benches, bench_timing_controller);
criterion_main!(benches);
