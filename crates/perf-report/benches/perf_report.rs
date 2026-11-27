use criterion::{black_box, criterion_group, criterion_main, Criterion};
use perf_report::{render_markdown_report, BenchmarkRow};

fn bench_perf_report(c: &mut Criterion) {
    let rows: Vec<BenchmarkRow> = (0..50)
        .map(|i| BenchmarkRow {
            name: format!("bench_{i}"),
            measured_us: 123.4,
            ceiling_us: Some(500.0),
            aegisub_us: Some(2_000.0),
        })
        .collect();
    c.bench_function("perf_report_render_50_rows", |b| {
        b.iter(|| black_box(render_markdown_report(black_box(&rows), None)));
    });
}

criterion_group!(benches, bench_perf_report);
criterion_main!(benches);
