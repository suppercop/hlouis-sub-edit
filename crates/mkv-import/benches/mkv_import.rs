use ass_core::AssTime;
use criterion::{Criterion, black_box, criterion_group, criterion_main};
use mkv_import::{TrackPickerModel, convert_srt_events_to_ass};
use mkv_subs::{SubtitleCodec, TrackInfo, TrackType};

fn bench_mkv_import(c: &mut Criterion) {
    let tracks: Vec<TrackInfo> = (0..64u64)
        .map(|i| TrackInfo {
            number: i + 1,
            track_type: TrackType::Subtitle,
            codec_id: if i == 0 {
                "S_HDMV/PGS".to_owned()
            } else {
                "S_TEXT/UTF8".to_owned()
            },
            subtitle_codec: if i == 0 {
                SubtitleCodec::UnsupportedImage("S_HDMV/PGS".to_owned())
            } else {
                SubtitleCodec::Srt
            },
            language: Some("vie".to_owned()),
            language_bcp47: None,
            name: Some(format!("Subtitle Track {i}")),
            is_default: i == 1,
            is_forced: false,
            codec_private: None,
            encodings: Vec::new(),
        })
        .collect();

    c.bench_function("mkv_import_build_picker_64_tracks", |b| {
        b.iter(|| {
            let model = TrackPickerModel::from_tracks(black_box(&tracks));
            black_box(model);
        });
    });

    let sample =
        "<i>Hello</i> &amp; <b>world</b>\r\n<font color=\"#FFCC00\">Sample SRT line</font>";
    let srt_events: Vec<(AssTime, AssTime, &str)> = (0..2_000u32)
        .map(|i| {
            #[allow(clippy::cast_possible_truncation)]
            let s = i * 100;
            (AssTime::from_cs(s), AssTime::from_cs(s + 90), sample)
        })
        .collect();

    c.bench_function("mkv_import_convert_2000_srt_to_ass", |b| {
        b.iter(|| {
            let script = convert_srt_events_to_ass(black_box(&srt_events), "Default");
            black_box(script);
        });
    });
}

criterion_group!(benches, bench_mkv_import);
criterion_main!(benches);
