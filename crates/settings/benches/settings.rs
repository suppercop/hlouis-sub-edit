use criterion::{black_box, criterion_group, criterion_main, Criterion};
use settings::AppSettings;

fn bench_settings(c: &mut Criterion) {
    let default_settings = AppSettings::default();
    let json = default_settings.save_json().expect("save");

    c.bench_function("settings_load_and_validate", |b| {
        b.iter(|| {
            let s = AppSettings::load_json(black_box(&json)).expect("load");
            black_box(s);
        });
    });

    c.bench_function("settings_save_json", |b| {
        b.iter(|| {
            let out = default_settings.save_json().expect("save");
            black_box(out);
        });
    });
}

criterion_group!(benches, bench_settings);
criterion_main!(benches);
