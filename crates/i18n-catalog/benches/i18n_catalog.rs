use criterion::{Criterion, black_box, criterion_group, criterion_main};
use i18n_catalog::{Catalog, Locale};

fn bench_i18n_catalog(c: &mut Criterion) {
    let vi = Catalog::for_locale(Locale::Vi);
    c.bench_function("i18n_catalog_lookup_simple", |b| {
        b.iter(|| {
            let msg = vi.get(black_box("dlg-tpp-title"));
            black_box(msg);
        });
    });

    c.bench_function("i18n_catalog_format_placeables", |b| {
        let mut out = String::with_capacity(256);
        b.iter(|| {
            let ok = vi.format_into(
                black_box("status-selected-count"),
                &[("selected", "12"), ("total", "500")],
                &mut out,
            );
            black_box((ok, out.len()));
        });
    });
}

criterion_group!(benches, bench_i18n_catalog);
criterion_main!(benches);
