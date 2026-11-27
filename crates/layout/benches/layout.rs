use criterion::{black_box, criterion_group, criterion_main, Criterion};
use layout::LayoutState;

fn bench_layout(c: &mut Criterion) {
    let state = LayoutState::default();

    c.bench_function("layout_compute_full_workspace", |b| {
        b.iter(|| {
            let r = state.compute(black_box(2560.0), black_box(1440.0), black_box(1.25));
            black_box(r);
        });
    });
}

criterion_group!(benches, bench_layout);
criterion_main!(benches);
