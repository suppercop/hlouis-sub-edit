use commands::CommandRegistry;
use criterion::{Criterion, black_box, criterion_group, criterion_main};
use i18n_catalog::{Catalog, Locale};
use settings::AppSettings;
use theme::ThemeSpec;
use ui_dialogs::{ActiveDialog, DialogHostState, show_active_dialog};

fn bench_ui_dialogs(c: &mut Criterion) {
    let mut host = DialogHostState::new();
    host.open = Some(ActiveDialog::TimingPostProcessor);
    let mut settings = AppSettings::default();
    let cmds = CommandRegistry::new_default();
    let pal = theme::palette(ThemeSpec::Dark);
    let cat = Catalog::for_locale(Locale::En);
    let ctx = egui::Context::default();
    ctx.set_fonts(ui_theme::build_font_definitions(
        &ui_theme::FontConfig::default(),
    ));
    let raw = egui::RawInput::default();

    c.bench_function("ui_dialogs_tpp_frame", |b| {
        b.iter(|| {
            let out = ctx.run_ui(raw.clone(), |ui| {
                egui::CentralPanel::default().show(ui, |ui| {
                    let act =
                        show_active_dialog(ui, &mut host, &mut settings, &cmds, None, &pal, &cat);
                    let _ = black_box(act);
                });
            });
            black_box(out.shapes.len());
        });
    });
}

criterion_group!(benches, bench_ui_dialogs);
criterion_main!(benches);
