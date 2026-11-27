use audio_peaks::PeakBucket;
use audio_view::TimelineMarkerIndex;
use criterion::{Criterion, black_box, criterion_group, criterion_main};
use i18n_catalog::{Catalog, Locale};
use theme::ThemeSpec;
use ui_audio::{AudioColumnProvider, AudioPanelState, show_audio_panel};

struct BenchProvider;
impl AudioColumnProvider for BenchProvider {
    fn duration_ms(&self) -> i64 {
        7_200_000
    }
    fn query_columns_into(
        &self,
        _start_ms: i64,
        _end_ms: i64,
        width_px: usize,
        out: &mut Vec<PeakBucket>,
    ) {
        out.clear();
        for i in 0..width_px {
            #[allow(clippy::cast_possible_truncation, clippy::cast_possible_wrap)]
            let v = ((i & 63) * 400) as i16;
            out.push(PeakBucket {
                min: -v,
                max: v,
                rms: v.unsigned_abs(),
            });
        }
    }
}

fn bench_ui_audio(c: &mut Criterion) {
    let mut state = AudioPanelState::new(7_200_000);
    let provider = BenchProvider;
    let kfs: Vec<i64> = (0..2_000).map(|i| i * 3_500).collect();
    let bounds: Vec<i64> = (0..20_000).map(|i| i * 350).collect();
    let markers = TimelineMarkerIndex::new(kfs, bounds);
    let pal = theme::palette(ThemeSpec::Dark);
    let cat = Catalog::for_locale(Locale::En);

    let ctx = egui::Context::default();
    ctx.set_fonts(ui_theme::build_font_definitions(
        &ui_theme::FontConfig::default(),
    ));
    let raw_input = egui::RawInput {
        screen_rect: Some(egui::Rect::from_min_size(
            egui::Pos2::ZERO,
            egui::Vec2::new(2040.0, 260.0),
        )),
        ..Default::default()
    };

    // Warm up one frame so the waveform texture is uploaded once.
    let _ = ctx.run_ui(raw_input.clone(), |ui| {
        egui::CentralPanel::default().show(ui, |ui| {
            let _ = show_audio_panel(
                ui, &mut state, &provider, &markers, None, None, &pal, 1, &cat,
            );
        });
    });
    assert_eq!(state.texture_uploads(), 1);

    c.bench_function("ui_audio_frame_2000px_20000_lines", |b| {
        b.iter(|| {
            let out = ctx.run_ui(raw_input.clone(), |ui| {
                egui::CentralPanel::default().show(ui, |ui| {
                    let act = show_audio_panel(
                        ui, &mut state, &provider, &markers, None, None, &pal, 1, &cat,
                    );
                    let _ = black_box(act);
                });
            });
            black_box(out.shapes.len());
        });
    });
    assert_eq!(
        state.texture_uploads(),
        1,
        "zero texture re-upload when unchanged"
    );
}

criterion_group!(benches, bench_ui_audio);
criterion_main!(benches);
