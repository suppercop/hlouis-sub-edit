use align_to_video::{align_to_video, AlignOptions, AlignScratch};
use criterion::{black_box, criterion_group, criterion_main, Criterion};

fn bench_align_to_video(c: &mut Criterion) {
    let width = 1920usize;
    let height = 1080usize;
    let stride = width * 4;
    let mut frame = vec![20u8; width * height * 4];
    // 300x200 target sign box inside 1080p frame
    for y in 400..600 {
        for x in 800..1100 {
            let off = y * stride + x * 4;
            frame[off] = 230;
            frame[off + 1] = 232;
            frame[off + 2] = 228;
            frame[off + 3] = 255;
        }
    }

    let opts = AlignOptions::default();
    let mut scratch = AlignScratch::new();

    c.bench_function("align_to_video_1080p_300x200_region", |b| {
        b.iter(|| {
            let res = align_to_video(
                black_box(&frame),
                width,
                height,
                stride,
                950,
                500,
                opts,
                &mut scratch,
            )
            .expect("align");
            black_box(res);
        });
    });
}

criterion_group!(benches, bench_align_to_video);
criterion_main!(benches);
