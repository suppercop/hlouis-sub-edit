use criterion::{Criterion, black_box, criterion_group, criterion_main};
use perf_metrics::HdrHistogram;

fn bench_perf_metrics(c: &mut Criterion) {
    c.bench_function("perf_metrics_hdr_record", |b| {
        let mut h = HdrHistogram::new();
        let mut v = 100u64;
        b.iter(|| {
            v = v.wrapping_mul(1_664_525).wrapping_add(1_013_904_223) & 0xFFFF;
            h.record(black_box(v));
        });
    });

    c.bench_function("perf_metrics_hdr_percentile_p99", |b| {
        let mut h = HdrHistogram::new();
        for i in 1..=10_000u64 {
            h.record((i * 17) % 33_000);
        }
        b.iter(|| {
            let p50 = h.p50();
            let p95 = h.p95();
            let p99 = h.p99();
            black_box((p50, p95, p99));
        });
    });
}

criterion_group!(benches, bench_perf_metrics);
criterion_main!(benches);
