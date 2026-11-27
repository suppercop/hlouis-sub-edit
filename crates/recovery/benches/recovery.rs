use criterion::{black_box, criterion_group, criterion_main, Criterion};
use recovery::count_diff_lines;

fn bench_recovery(c: &mut Criterion) {
    let a = "Dialogue: 0,0:00:01.00,0:00:02.00,Default,,0,0,0,,Line A\n".repeat(1_000);
    let mut b = a.clone();
    b.push_str("Dialogue: 0,0:00:02.00,0:00:03.00,Default,,0,0,0,,Line B\n");
    c.bench_function("recovery_diff_1000_lines", |bench| {
        bench.iter(|| {
            black_box(count_diff_lines(black_box(&a), black_box(&b)));
        });
    });
}

criterion_group!(benches, bench_recovery);
criterion_main!(benches);
