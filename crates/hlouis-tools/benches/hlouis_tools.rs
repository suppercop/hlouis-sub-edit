use ass_core::{AssEvent, AssTime, ScriptFile};
use criterion::{Criterion, black_box, criterion_group, criterion_main};
use hlouis_tools::{CleanOptions, build_style_counts, clean_whitespace, quick_select_by_style};
use std::collections::BTreeSet;

fn make_200k_script() -> ScriptFile {
    let mut script = ScriptFile::new_default();
    let sec = script.ensure_events_section_mut();
    for i in 0..200_000u32 {
        let text = if i % 10 == 0 {
            r"{\i1}  Xin chào  \N  thế giới  {\i0}"
        } else {
            "Clean subtitle line without extra spaces"
        };
        let mut ev = AssEvent::dialogue(AssTime::from_cs(i * 5), AssTime::from_cs(i * 5 + 4), text);
        if i % 3 == 0 {
            ev.set_style("Sign");
        }
        sec.insert_event(i as usize, ev);
    }
    script
}

fn bench_hlouis_tools(c: &mut Criterion) {
    let script = make_200k_script();
    let empty = BTreeSet::new();

    c.bench_function("hlouis_quick_select_by_style_200k", |b| {
        b.iter(|| {
            let r = quick_select_by_style(black_box(&script), 1, false);
            black_box(r.selection.len());
        });
    });

    c.bench_function("hlouis_build_style_counts_200k", |b| {
        b.iter(|| {
            let list = build_style_counts(black_box(&script));
            black_box(list.len());
        });
    });

    c.bench_function("hlouis_clean_whitespace_200k", |b| {
        b.iter(|| {
            let r = clean_whitespace(black_box(&script), &empty, CleanOptions::default());
            black_box(r.changed_count);
        });
    });
}

criterion_group!(benches, bench_hlouis_tools);
criterion_main!(benches);
