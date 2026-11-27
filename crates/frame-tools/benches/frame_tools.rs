use ass_core::ScriptFile;
use criterion::{black_box, criterion_group, criterion_main, Criterion};
use frame_tools::{snap_to_video_frames_in_place, transform_framerate_in_place};
use keyframes::Timecodes;
use std::fmt::Write as _;

fn build_20k_script() -> ScriptFile {
    let mut raw = String::with_capacity(20_000 * 96);
    raw.push_str("[Script Info]\nScriptType: v4.00+\n\n[Events]\nFormat: Layer, Start, End, Style, Name, MarginL, MarginR, MarginV, Effect, Text\n");
    for i in 0..20_000u32 {
        let s = i * 50;
        let e = s + 45;
        if i % 5 == 0 {
            let _ = writeln!(
                raw,
                "Dialogue: 0,0:{:02}:{:02}.{:02},0:{:02}:{:02}.{:02},Default,,0,0,0,,{{\\fad(100,100)\\t(0,200,\\fs50)}}Tagged line {i}",
                (s / 6000) % 60,
                (s / 100) % 60,
                s % 100,
                (e / 6000) % 60,
                (e / 100) % 60,
                e % 100
            );
        } else {
            let _ = writeln!(
                raw,
                "Dialogue: 0,0:{:02}:{:02}.{:02},0:{:02}:{:02}.{:02},Default,,0,0,0,,Plain line {i}",
                (s / 6000) % 60,
                (s / 100) % 60,
                s % 100,
                (e / 6000) % 60,
                (e / 100) % 60,
                e % 100
            );
        }
    }
    ass_core::parse_ass(&raw).value
}

fn bench_frame_tools(c: &mut Criterion) {
    let base = build_20k_script();
    let tc_in = Timecodes::from_cfr(24_000, 1001).expect("in");
    let tc_out = Timecodes::from_cfr(25, 1).expect("out");

    c.bench_function("frame_tools_transform_20k_lines_with_tags", |b| {
        b.iter_batched(
            || base.clone(),
            |mut s| {
                let n = transform_framerate_in_place(black_box(&mut s), &tc_in, &tc_out)
                    .expect("transform");
                black_box(n);
            },
            criterion::BatchSize::LargeInput,
        );
    });

    c.bench_function("frame_tools_snap_20k_lines", |b| {
        b.iter_batched(
            || base.clone(),
            |mut s| {
                let n =
                    snap_to_video_frames_in_place(black_box(&mut s), None, &tc_in).expect("snap");
                black_box(n);
            },
            criterion::BatchSize::LargeInput,
        );
    });
}

criterion_group!(benches, bench_frame_tools);
criterion_main!(benches);
