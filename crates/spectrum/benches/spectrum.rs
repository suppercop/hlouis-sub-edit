use criterion::{black_box, criterion_group, criterion_main, Criterion};
use spectrum::{ColorLut, SpectrumCache, SpectrumColorScheme, SpectrumQuality};
use theme::ThemeSpec;

fn bench_spectrum(c: &mut Criterion) {
    // 24 minutes of 48 kHz mono = 69,120,000 samples (~264 MB f32).
    // Benchmark with 10 samples to stay fast while measuring the full 24-min build.
    let samples_24m = vec![0.25f32; 24 * 60 * 48_000];
    let mut g = c.benchmark_group("spectrum_24min");
    g.sample_size(10);
    g.bench_function("spectrum_build_24min_48khz_mono", |b| {
        b.iter(|| {
            let cache = SpectrumCache::build_from_mono(
                black_box(&samples_24m),
                48_000,
                SpectrumQuality::Regular,
                1.0,
            );
            black_box(cache.tile_count());
        });
    });
    g.finish();

    let cache = SpectrumCache::build_from_mono(
        &samples_24m[..48_000 * 60],
        48_000,
        SpectrumQuality::Regular,
        1.0,
    );
    let pal = theme::palette(ThemeSpec::Dark);
    let lut = ColorLut::build(SpectrumColorScheme::IcyBlue, &pal);
    let mut rgba = vec![0u8; 256 * 128 * 4];
    c.bench_function("spectrum_scroll_tile_256x128", |b| {
        let mut col = 0usize;
        b.iter(|| {
            col = (col + 7) % 1_000;
            cache.render_viewport_into(0, col, 256, 128, Some((50, 120)), &lut, &mut rgba);
            black_box(rgba[0]);
        });
    });
}

criterion_group!(benches, bench_spectrum);
criterion_main!(benches);
