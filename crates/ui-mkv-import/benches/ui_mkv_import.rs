use criterion::{Criterion, black_box, criterion_group, criterion_main};
use egui::{Pos2, RawInput, Rect, Vec2};
use i18n_catalog::{Catalog, Locale};
use mkv_import::TrackPickerModel;
use mkv_subs::{SubtitleCodec, TrackInfo, TrackType};
use ui_mkv_import::{MkvImportDialogState, show_mkv_import_dialog};

fn bench_ui_mkv_import(c: &mut Criterion) {
    let tracks: Vec<TrackInfo> = (1u64..=68u64)
        .map(|n| TrackInfo {
            number: n,
            track_type: TrackType::Subtitle,
            codec_id: if n <= 64 {
                "S_TEXT/UTF8".to_owned()
            } else {
                "S_HDMV/PGS".to_owned()
            },
            subtitle_codec: if n <= 64 {
                SubtitleCodec::Srt
            } else {
                SubtitleCodec::UnsupportedImage("S_HDMV/PGS".to_owned())
            },
            language: Some("vie".to_owned()),
            language_bcp47: None,
            name: Some(format!("Track {n}")),
            is_default: n == 1,
            is_forced: false,
            codec_private: None,
            encodings: Vec::new(),
        })
        .collect();
    let mut state = MkvImportDialogState::from_picker(TrackPickerModel::from_tracks(&tracks));
    let cat = Catalog::for_locale(Locale::En);
    let ctx = egui::Context::default();
    let raw = RawInput {
        screen_rect: Some(Rect::from_min_size(Pos2::ZERO, Vec2::new(760.0, 480.0))),
        ..Default::default()
    };

    c.bench_function("ui_mkv_import_68_tracks_frame", |b| {
        b.iter(|| {
            let out = ctx.run_ui(raw.clone(), |ui| {
                let _ = show_mkv_import_dialog(ui, &mut state, &cat);
            });
            black_box(out.shapes.len());
        });
    });
}

criterion_group!(benches, bench_ui_mkv_import);
criterion_main!(benches);
