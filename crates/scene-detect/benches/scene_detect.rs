use criterion::{black_box, criterion_group, criterion_main, Criterion};
use scene_detect::{SceneDetectConfig, SceneDetector};

fn bench_scene_detect(c: &mut Criterion) {
    let width = 1920usize;
    let height = 1080usize;
    let stride = 1920usize;
    let mut frame_a = vec![0u8; width * height];
    let mut frame_b = vec![0u8; width * height];
    for (i, px) in frame_a.iter_mut().enumerate() {
        #[allow(clippy::cast_possible_truncation)]
        {
            *px = ((i.wrapping_mul(13) >> 4) & 0xFF) as u8;
        }
    }
    for (i, px) in frame_b.iter_mut().enumerate() {
        #[allow(clippy::cast_possible_truncation)]
        {
            *px = ((i.wrapping_mul(29).wrapping_add(77) >> 4) & 0xFF) as u8;
        }
    }

    let mut det = SceneDetector::new(SceneDetectConfig {
        quarter_res: true,
        ..SceneDetectConfig::default()
    });
    // Prime frame 0 so the benchmark measures steady-state per-frame cost.
    det.push_frame(&frame_a, width, height, stride)
        .expect("prime");

    let mut toggle = false;
    c.bench_function("scene_detect_1080p_quarter_res_per_frame", |b| {
        b.iter(|| {
            toggle = !toggle;
            let frame = if toggle { &frame_b } else { &frame_a };
            let metrics = det
                .push_frame(black_box(frame), width, height, stride)
                .expect("push_frame");
            black_box(metrics);
        });
    });
}

criterion_group!(benches, bench_scene_detect);
criterion_main!(benches);
