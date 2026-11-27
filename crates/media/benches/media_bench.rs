use criterion::{black_box, criterion_group, criterion_main, Criterion};
use media::{copy_rgba_plane, BestSourceOptions, BestSourceProvider, FfmsVideo, VideoProvider};
use std::path::Path;
use std::sync::atomic::AtomicBool;
use std::time::Duration;

fn bench_media(c: &mut Criterion) {
    let mut group = c.benchmark_group("media_hot_paths");
    group
        .warm_up_time(Duration::from_millis(500))
        .measurement_time(Duration::from_secs(3))
        .sample_size(40);

    let width = 1920u32;
    let height = 1080u32;
    let padded_stride = (width as usize * 4) + 64;
    let src_padded = vec![128u8; padded_stride * height as usize];
    let mut dst = Vec::with_capacity(width as usize * height as usize * 4);
    group.bench_function("rgba_upload_prep_1080p", |b| {
        b.iter(|| {
            let res = copy_rgba_plane(&src_padded, padded_stride, width, height, &mut dst);
            black_box((res.is_ok(), dst.len()));
        });
    });

    if let Ok(path_str) = std::env::var("HLOUIS_TEST_VIDEO") {
        let cancel = AtomicBool::new(false);
        if let Ok(mut video) =
            FfmsVideo::open_with_progress(Path::new(&path_str), &cancel, |_, _| {})
        {
            let frame_count = video.info().num_frames.clamp(1, 240);
            let mut rgba = Vec::new();
            let _ = video.decode_frame_into(0, &mut rgba);
            let mut idx = 0usize;
            group.sample_size(20);
            group.bench_function("media_decode_1080p", |b| {
                b.iter(|| {
                    idx = (idx + 1) % frame_count;
                    let res = video.decode_frame_into(idx, &mut rgba);
                    black_box((res.is_ok(), rgba.len()));
                });
            });
        }
        if let Ok(mut video) = BestSourceProvider::open_with_progress(
            Path::new(&path_str),
            &BestSourceOptions::default(),
            &cancel,
            |_, _| {},
        ) {
            let frame_count = video.info().num_frames.clamp(1, 240);
            let mut rgba = Vec::new();
            let _ = video.decode_frame_into(0, &mut rgba);
            let mut idx = 0usize;
            group.bench_function("bestsource_decode_1080p", |b| {
                b.iter(|| {
                    idx = (idx + 1) % frame_count;
                    let res = video.decode_frame_into(idx, &mut rgba);
                    black_box((res.is_ok(), rgba.len()));
                });
            });
        }
    }

    group.finish();
}

criterion_group!(benches, bench_media);
criterion_main!(benches);
