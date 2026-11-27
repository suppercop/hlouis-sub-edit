//! Criterion benchmark for `collect_used_fonts` + `check_fonts` over 20,000 lines (target < 50 ms).

use ass_core::{AssEvent, AssTime, EventEntry, ScriptFile};
use criterion::{black_box, criterion_group, criterion_main, Criterion};
use fonts::{check_fonts, FontSpec, MockFontProvider, WEIGHT_BOLD, WEIGHT_NORMAL};

fn bench_fonts(c: &mut Criterion) {
    let mut script = ScriptFile::new_default();
    script.ensure_events_section_mut().entries = (0..20_000u32)
        .map(|i| {
            let start = AssTime::from_cs(i * 100);
            let end = start.saturating_add_cs(90);
            let text = if i % 4 == 0 {
                format!(r"{{\fnSegoe UI\b1}}Tiếng Việt #{i}{{\r}} plain")
            } else {
                format!("Normal dialogue line #{i}")
            };
            EventEntry::Event(AssEvent::dialogue(start, end, text))
        })
        .collect();

    let mut provider = MockFontProvider::default();
    provider.add_full_font(FontSpec::new("Arial", WEIGHT_NORMAL, false));
    provider.add_full_font(FontSpec::new("Segoe UI", WEIGHT_BOLD, false));

    c.bench_function("check_fonts_20k_lines", |b| {
        b.iter(|| {
            let report = check_fonts(black_box(&script), &provider);
            black_box(report.used_fonts.len())
        });
    });
}

criterion_group!(benches, bench_fonts);
criterion_main!(benches);
