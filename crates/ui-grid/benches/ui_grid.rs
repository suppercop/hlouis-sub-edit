use ass_core::{AssEvent, AssTime, ScriptFile};
use criterion::{Criterion, black_box, criterion_group, criterion_main};
use i18n_catalog::{Catalog, Locale};
use theme::ThemeSpec;
use ui_grid::{SubtitleGridState, show_subtitle_grid};

fn make_200k_script() -> ScriptFile {
    let mut script = ScriptFile::new_default();
    let sec = script.ensure_events_section_mut();
    for i in 0..200_000u32 {
        sec.insert_event(
            i as usize,
            AssEvent::dialogue(
                AssTime::from_cs(i * 5),
                AssTime::from_cs(i * 5 + 120),
                r"{\an5\blur1}Sample subtitle dialogue line",
            ),
        );
    }
    script
}

fn bench_ui_grid(c: &mut Criterion) {
    let script = make_200k_script();
    let mut state = SubtitleGridState::default();
    state.sync_from_script(&script);
    state.model.click(100_000);
    state.scroll_to_active();

    let pal = theme::palette(ThemeSpec::Dark);
    let cat = Catalog::for_locale(Locale::En);
    let ctx = egui::Context::default();
    ctx.set_fonts(ui_theme::build_font_definitions(
        &ui_theme::FontConfig::default(),
    ));
    let raw_input = egui::RawInput {
        screen_rect: Some(egui::Rect::from_min_size(
            egui::Pos2::ZERO,
            egui::Vec2::new(1280.0, 420.0),
        )),
        ..Default::default()
    };

    // Warm up one frame so scroll offset settles at row 100,000.
    let _ = ctx.run_ui(raw_input.clone(), |ui| {
        egui::CentralPanel::default().show(ui, |ui| {
            let _ = show_subtitle_grid(ui, &script, &mut state, Some(500_000), &pal, &cat);
        });
    });

    c.bench_function("ui_grid_frame_200k_scrolled_middle", |b| {
        b.iter(|| {
            let out = ctx.run_ui(raw_input.clone(), |ui| {
                egui::CentralPanel::default().show(ui, |ui| {
                    let act =
                        show_subtitle_grid(ui, &script, &mut state, Some(500_000), &pal, &cat);
                    let _ = black_box(act);
                });
            });
            black_box(out.shapes.len());
        });
    });
}

criterion_group!(benches, bench_ui_grid);
criterion_main!(benches);
