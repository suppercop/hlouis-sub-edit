use criterion::{black_box, criterion_group, criterion_main, Criterion};
use theme::{palette, BaseTheme, ThemeSpec};

fn bench_theme(c: &mut Criterion) {
    c.bench_function("theme_eye_comfort_full_palette_light_100", |b| {
        b.iter(|| {
            let p = palette(black_box(ThemeSpec::EyeComfort {
                base: BaseTheme::Light,
                strength: 100,
            }));
            black_box(p);
        });
    });

    c.bench_function("theme_eye_comfort_full_palette_dark_100", |b| {
        b.iter(|| {
            let p = palette(black_box(ThemeSpec::EyeComfort {
                base: BaseTheme::Dark,
                strength: 100,
            }));
            black_box(p);
        });
    });
}

criterion_group!(benches, bench_theme);
criterion_main!(benches);
