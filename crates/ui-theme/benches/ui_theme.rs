use criterion::{Criterion, black_box, criterion_group, criterion_main};
use theme::{BaseTheme, ThemeSpec};
use ui_theme::{FontConfig, build_style, is_dark_spec};

fn bench_ui_theme(c: &mut Criterion) {
    let spec = ThemeSpec::EyeComfort {
        base: BaseTheme::Dark,
        strength: 65,
    };
    let pal = theme::palette(spec);
    let fonts = FontConfig::default();

    c.bench_function("ui_theme_build_style_eye_comfort", |b| {
        b.iter(|| {
            let st = build_style(black_box(&pal), is_dark_spec(spec), black_box(&fonts));
            black_box(st.animation_time);
        });
    });
}

criterion_group!(benches, bench_ui_theme);
criterion_main!(benches);
