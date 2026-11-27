//! Warm glyph-coverage check: 50 font specs x 2,000 glyphs (budget < 20 ms).

use criterion::{criterion_group, criterion_main, Criterion};
use fonts::{FontProvider, FontSpec};
use fonts_dwrite::DWriteFontProvider;
use std::collections::BTreeSet;
use std::hint::black_box;

fn glyphs() -> BTreeSet<char> {
    [
        (0x21u32, 0x7E),
        (0xA1, 0x17F),
        (0x1EA0, 0x1EF9),
        (0x3041, 0x3096),
        (0x4E00, 0x9FFF),
    ]
    .iter()
    .flat_map(|&(a, b)| (a..=b).filter_map(char::from_u32))
    .take(2_000)
    .collect()
}

fn bench(c: &mut Criterion) {
    let p = DWriteFontProvider::new().expect("DirectWrite");
    let families = [
        "Segoe UI",
        "Arial",
        "Yu Gothic",
        "Times New Roman",
        "Courier New",
    ];
    let specs: Vec<FontSpec> = (0..50)
        .map(|i| {
            let weight = [400u16, 700, 300, 600, 900][(i / 5) % 5];
            FontSpec::new(families[i % 5], weight, i % 2 == 1)
        })
        .collect();
    let glyphs = glyphs();
    c.bench_function("fonts_dwrite/missing_glyphs_50x2000_warm", |b| {
        b.iter(|| {
            let mut n = 0usize;
            for s in &specs {
                n += p.missing_glyphs(black_box(s), &glyphs).len();
            }
            black_box(n)
        });
    });
}

criterion_group!(benches, bench);
criterion_main!(benches);
