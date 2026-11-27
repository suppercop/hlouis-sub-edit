use criterion::{black_box, criterion_group, criterion_main, Criterion};
use render::{blend_rgba_mask, AssBitmap, AssSubRenderer};
use std::time::Duration;

const SAMPLE_ASS: &str = "\
[Script Info]
Title: Bench Sample
ScriptType: v4.00+
PlayResX: 1920
PlayResY: 1080
ScaledBorderAndShadow: yes

[V4+ Styles]
Format: Name, Fontname, Fontsize, PrimaryColour, SecondaryColour, OutlineColour, BackColour, Bold, Italic, Underline, StrikeOut, ScaleX, ScaleY, Spacing, Angle, BorderStyle, Outline, Shadow, Alignment, MarginL, MarginR, MarginV, Encoding
Style: Default,Segoe UI,56,&H00FFFFFF,&H0000FFFF,&H00181820,&H90000000,-1,0,0,0,100,100,0,0,1,3,1.5,2,40,40,36,1

[Events]
Format: Layer, Start, End, Style, Name, MarginL, MarginR, MarginV, Effect, Text
Dialogue: 0,0:00:00.00,0:00:30.00,Default,,0,0,0,,{\\fad(200,200)\\k40}Phụ đề {\\kf60}Tiếng Việt & {\\k40}English karaoke test line!
";

fn bench_render(c: &mut Criterion) {
    let mut group = c.benchmark_group("render_hot_paths");
    group
        .warm_up_time(Duration::from_millis(500))
        .measurement_time(Duration::from_secs(3))
        .sample_size(40);

    if let Ok(mut renderer) = AssSubRenderer::new(1920, 1080) {
        if renderer.load_ass_memory(SAMPLE_ASS).is_ok() {
            let mut frame = vec![64u8; 1920 * 1080 * 4];
            let _ = renderer.render_and_blend_rgba(500, &mut frame, 1920, 1080);

            let mut step = 0i64;
            group.bench_function("libass_render_blend_1080p", |b| {
                b.iter(|| {
                    step = (step + 42) % 4_000;
                    let stats =
                        renderer.render_and_blend_rgba(1_000 + step, &mut frame, 1920, 1080);
                    black_box(stats.is_ok());
                });
            });
        }
    }

    let mask = vec![160u8; 1280 * 140];
    let bitmap = AssBitmap {
        dst_x: 320,
        dst_y: 880,
        width: 1280,
        height: 140,
        stride: 1280,
        color: 0xFFCC_8820,
        data: &mask,
    };
    let mut frame = vec![32u8; 1920 * 1080 * 4];
    group.bench_function("rgba_blend_mask_1080p", |b| {
        b.iter(|| {
            let res = blend_rgba_mask(&bitmap, &mut frame, 1920, 1080);
            black_box(res.is_ok());
        });
    });

    group.finish();
}

criterion_group!(benches, bench_render);
criterion_main!(benches);
