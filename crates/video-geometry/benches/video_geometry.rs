use criterion::{black_box, criterion_group, criterion_main, Criterion};
use video_geometry::{render_dummy_frame_rgba, DummyVideoConfig, VideoDisplayState};
use visual_tools::Vec2;

fn bench_video_geometry(c: &mut Criterion) {
    let mut state = VideoDisplayState::new(1920.0, 1080.0, 1280.0, 800.0);
    state.fit_to_viewport();

    c.bench_function("video_geometry_transform_per_frame", |b| {
        b.iter(|| {
            let lay = state.layout();
            let tf = state.to_visual_coord_transform(1920.0, 1080.0);
            let pt = tf.screen_to_script(black_box(Vec2::new(640.0, 400.0)));
            black_box((lay.video_rect.w, pt));
        });
    });

    let cfg = DummyVideoConfig {
        fps: 24000.0 / 1001.0,
        frames: 1000,
        width: 1920,
        height: 1080,
        red: 47,
        green: 163,
        blue: 254,
        checkerboard: true,
    };
    let mut rgba = vec![0u8; cfg.rgba_byte_len()];

    c.bench_function("video_geometry_dummy_1080p_checkerboard_fill", |b| {
        b.iter(|| {
            render_dummy_frame_rgba(black_box(&cfg), 0, black_box(&mut rgba)).expect("render");
            black_box(rgba[0]);
        });
    });
}

criterion_group!(benches, bench_video_geometry);
criterion_main!(benches);
