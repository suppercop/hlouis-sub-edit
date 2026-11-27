use audio_view::{
    build_frame_layout, AudioFrameInput, AudioFrameLayout, AudioViewport, TimelineMarkerIndex,
};
use criterion::{black_box, criterion_group, criterion_main, Criterion};
use timing_controller::{TimeRangeMs, VideoPositionMarker};

fn bench_audio_view(c: &mut Criterion) {
    // 20,000 lines (40,000 boundaries) + 3,000 keyframes over 24 minutes (1,440,000 ms).
    let keyframes: Vec<i64> = (0..3_000).map(|i| i * 480).collect();
    let mut bounds = Vec::with_capacity(40_000);
    for i in 0..20_000i64 {
        bounds.push(i * 72);
        bounds.push(i * 72 + 55);
    }
    let markers = TimelineMarkerIndex::new(keyframes, bounds);

    let mut vp = AudioViewport::new(2000.0, 220.0, 1_440_000);
    vp.karaoke_strip_height_px = 24.0;
    vp.scroll_to_ms(200_000.0, true); // 200_000..220_000 ms visible (20 seconds at 10 ms/px)

    let inactive = [
        TimeRangeMs {
            start_ms: 201_000,
            end_ms: 203_000,
        },
        TimeRangeMs {
            start_ms: 208_000,
            end_ms: 211_000,
        },
    ];
    let kara: Vec<i64> = (0..=20).map(|i| 203_500 + i * 200).collect();
    let input = AudioFrameInput {
        viewport: vp,
        selection: TimeRangeMs {
            start_ms: 203_500,
            end_ms: 207_500,
        },
        play_cursor_ms: Some(205_120),
        video_marker: Some(VideoPositionMarker {
            position_ms: 205_000,
            frame_start_ms: 204_980,
            frame_end_ms: 205_022,
        }),
        inactive_spans: &inactive,
        karaoke_boundaries_ms: &kara,
        active_karaoke_syllable: Some(4),
        markers: &markers,
    };

    let mut layout = AudioFrameLayout::with_capacity(2000);

    c.bench_function("audio_view_frame_2000px_20k_lines_3k_kf", |b| {
        b.iter(|| {
            build_frame_layout(black_box(&input), None, &mut layout);
            black_box(layout.markers.len() + layout.spans.len() + layout.karaoke_labels.len());
        });
    });
}

criterion_group!(benches, bench_audio_view);
criterion_main!(benches);
