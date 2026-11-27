use commands::CommandRegistry;
use criterion::{black_box, criterion_group, criterion_main, Criterion};
use egui::{Pos2, RawInput, Rect, Vec2};
use i18n_catalog::{Catalog, Locale};
use settings::AppSettings;
use theme::ThemeSpec;
use ui_preferences::{show_preferences_dialog, PreferencesDialogState};
use ui_theme::{FontConfig, ThemeController};

fn bench_ui_preferences(c: &mut Criterion) {
    let mut state = PreferencesDialogState::default();
    let mut settings = AppSettings::default();
    let mut theme_ctl = ThemeController::new(ThemeSpec::Dark, FontConfig::default());
    let commands = CommandRegistry::new_default();
    let cat = Catalog::for_locale(Locale::En);
    let ctx = egui::Context::default();
    let raw = RawInput {
        screen_rect: Some(Rect::from_min_size(Pos2::ZERO, Vec2::new(720.0, 520.0))),
        ..Default::default()
    };

    c.bench_function("ui_preferences_frame", |b| {
        b.iter(|| {
            let out = ctx.run_ui(raw.clone(), |ui| {
                let _ = show_preferences_dialog(
                    ui,
                    &mut state,
                    &mut settings,
                    &mut theme_ctl,
                    &commands,
                    &cat,
                );
            });
            black_box(out.shapes.len());
        });
    });
}

criterion_group!(benches, bench_ui_preferences);
criterion_main!(benches);
