use criterion::{black_box, criterion_group, criterion_main, Criterion};
use std::fmt::Write as _;
use visual_tools::{Vec2, VectorClipPath};

fn build_500_point_vector_clip() -> VectorClipPath {
    let mut raw = String::with_capacity(500 * 10);
    raw.push_str("m 0 0 l");
    for i in 1..500u32 {
        let _ = write!(raw, " {} {}", i * 3, (i * 7) % 1000);
    }
    VectorClipPath::parse(&raw, false)
}

fn bench_visual_tools(c: &mut Criterion) {
    let mut path = build_500_point_vector_clip();
    assert_eq!(path.vertices.len(), 500);
    let mut tag_buf = String::with_capacity(4096);

    c.bench_function("visual_tools_hit_test_and_drag_500pt_vector_clip", |b| {
        let mut step = 0.0_f64;
        b.iter(|| {
            step = (step + 1.0) % 50.0;
            let hit = path.hit_test_vertex(black_box(Vec2::new(750.0, 750.0)), 12.0);
            let idx = hit.unwrap_or(250);
            path.drag_vertex(idx, Vec2::new(750.0 + step, 750.0 - step));
            black_box(idx);
        });
    });

    c.bench_function("visual_tools_serialize_500pt_vector_clip", |b| {
        b.iter(|| {
            path.write_tag_into(&mut tag_buf);
            black_box(tag_buf.len());
        });
    });
}

criterion_group!(benches, bench_visual_tools);
criterion_main!(benches);
