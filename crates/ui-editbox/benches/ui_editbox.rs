use criterion::{Criterion, black_box, criterion_group, criterion_main};
use egui::FontId;
use syntax_highlight::IncrementalTokenizer;
use theme::ThemeSpec;
use ui_editbox::build_highlighted_layout_job;

fn bench_ui_editbox(c: &mut Criterion) {
    let unit = r"{\pos(960,540)\t(0,250,\fscx120\fscy120\blur2)\c&H00FFFF&}Typeset ";
    let mut heavy_line = String::with_capacity(2_200);
    while heavy_line.len() < 2_000 {
        heavy_line.push_str(unit);
    }

    let pal = theme::palette(ThemeSpec::Dark);
    let font = FontId::monospace(14.0);
    let ctx = egui::Context::default();
    ctx.set_fonts(ui_theme::build_font_definitions(
        &ui_theme::FontConfig::default(),
    ));
    let _ = ctx.run_ui(egui::RawInput::default(), |_| {});
    let mut tok = IncrementalTokenizer::new();
    tok.set_text(&heavy_line);

    c.bench_function("ui_editbox_keystroke_relayout_2000_char_typeset", |b| {
        b.iter(|| {
            tok.re_tokenize_edit(black_box(&heavy_line), 1_000, 1_000, 0);
            let job = build_highlighted_layout_job(&heavy_line, tok.tokens(), &pal, &font, 640.0);
            let galley = ctx.fonts_mut(|f| f.layout_job(job));
            black_box(galley.rows.len());
        });
    });
}

criterion_group!(benches, bench_ui_editbox);
criterion_main!(benches);
