use ass_core::{AssEvent, AssTime};
use criterion::{black_box, criterion_group, criterion_main, Criterion};
use karaoke_timing::KaraokeTimingController;
use std::fmt::Write as _;

fn build_200_syllable_event() -> AssEvent {
    let mut text = String::with_capacity(200 * 12);
    for i in 0..200u32 {
        let _ = write!(text, "{{\\k15}}s{i}");
    }
    AssEvent::dialogue(AssTime::from_cs(0), AssTime::from_cs(3000), text)
}

fn bench_karaoke_timing(c: &mut Criterion) {
    let ev = build_200_syllable_event();
    let keyframes: Vec<i64> = (0..1000).map(|i| i * 300).collect();

    let mut ctrl = KaraokeTimingController::from_event(0, &ev);
    ctrl.begin_drag_boundary(100).expect("begin");

    c.bench_function("karaoke_timing_drag_200_syllables", |b| {
        let mut t = 14_950i64;
        b.iter(|| {
            t = if t > 15_050 { 14_950 } else { t + 10 };
            let res = ctrl.update_drag_boundary(black_box(t), black_box(&keyframes), 30, false);
            black_box(res);
        });
    });

    c.bench_function("karaoke_timing_commit_200_syllables", |b| {
        b.iter(|| {
            let cmd = ctrl.commit(black_box(ev.text()));
            black_box(cmd);
        });
    });
}

criterion_group!(benches, bench_karaoke_timing);
criterion_main!(benches);
