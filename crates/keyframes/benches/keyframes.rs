//! Criterion benchmark converting 200,000 frames (CFR and VFR, target < 10 ms).

use criterion::{black_box, criterion_group, criterion_main, Criterion};
use keyframes::{TimeType, Timecodes};

fn bench_keyframes(c: &mut Criterion) {
    let cfr = Timecodes::from_cfr(24_000, 1_001).expect("cfr");
    let vfr_ts: Vec<i64> = (0..200_000i64).map(|f| (f * 1001 + 12) / 24).collect();
    let vfr = Timecodes::from_vfr_ms(vfr_ts).expect("vfr");

    c.bench_function("cfr_convert_200k_frames", |b| {
        b.iter(|| {
            let mut acc = 0u64;
            for f in 0..200_000u32 {
                let cs = cfr.frame_to_time_cs(black_box(f), TimeType::Start);
                acc = acc.wrapping_add(u64::from(cfr.time_cs_to_frame(cs, TimeType::Start)));
            }
            black_box(acc)
        });
    });

    c.bench_function("vfr_convert_200k_frames", |b| {
        b.iter(|| {
            let mut acc = 0u64;
            for f in 0..200_000u32 {
                let cs = vfr.frame_to_time_cs(black_box(f), TimeType::Start);
                acc = acc.wrapping_add(u64::from(vfr.time_cs_to_frame(cs, TimeType::Start)));
            }
            black_box(acc)
        });
    });
}

criterion_group!(benches, bench_keyframes);
criterion_main!(benches);
