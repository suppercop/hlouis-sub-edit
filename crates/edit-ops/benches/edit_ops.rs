use ass_core::ScriptFile;
use criterion::{black_box, criterion_group, criterion_main, Criterion};
use edit_ops::{
    delete_selected, event_metrics, paste_lines, stable_sort, stable_sort_in_place, CpsOptions,
    SortKey,
};
use std::fmt::Write as _;

fn build_20k_script() -> ScriptFile {
    let mut raw = String::with_capacity(20_000 * 80);
    raw.push_str("[Script Info]\nScriptType: v4.00+\n\n[Events]\nFormat: Layer, Start, End, Style, Name, MarginL, MarginR, MarginV, Effect, Text\n");
    for i in 0..20_000u32 {
        // Scramble start times deterministically so sort actually moves rows.
        let t0 = ((i.wrapping_mul(7919)) % 20_000) * 10;
        let t1 = t0 + 250;
        let _ = writeln!(
            raw,
            "Dialogue: 0,0:{:02}:{:02}.{:02},0:{:02}:{:02}.{:02},Default,Actor{},0,0,0,,{{\\an8}}Line {} with \\N some subtitle text.",
            (t0 / 6000) % 60,
            (t0 / 100) % 60,
            t0 % 100,
            (t1 / 6000) % 60,
            (t1 / 100) % 60,
            t1 % 100,
            i % 12,
            i
        );
    }
    ass_core::parse_ass(&raw).value
}

fn build_5k_clipboard() -> String {
    let mut out = String::with_capacity(5_000 * 70);
    for i in 0..5_000u32 {
        let _ = writeln!(
            out,
            "Dialogue: 0,0:00:01.00,0:00:03.50,Default,Clip,0,0,0,,Clipboard line {i}"
        );
    }
    out
}

fn bench_edit_ops(c: &mut Criterion) {
    let script = build_20k_script();
    let clip_5k = build_5k_clipboard();
    let del_10k: Vec<usize> = (0..20_000).step_by(2).collect();

    c.bench_function("edit_ops_sort_20k_by_start_in_place", |b| {
        b.iter_batched(
            || script.clone(),
            |mut s| {
                let perm = stable_sort_in_place(black_box(&mut s), SortKey::Start, false, None)
                    .expect("sort");
                black_box(perm);
            },
            criterion::BatchSize::LargeInput,
        );
    });

    c.bench_function("edit_ops_sort_20k_by_start_cmd", |b| {
        b.iter(|| {
            let cmd = stable_sort(black_box(&script), SortKey::Start, false, None).expect("sort");
            black_box(cmd);
        });
    });

    c.bench_function("edit_ops_paste_5000_lines", |b| {
        b.iter(|| {
            let cmd = paste_lines(black_box(&script), 100, black_box(&clip_5k)).expect("paste");
            black_box(cmd);
        });
    });

    c.bench_function("edit_ops_delete_10000_selected", |b| {
        b.iter(|| {
            let cmd = delete_selected(black_box(&script), black_box(&del_10k)).expect("delete");
            black_box(cmd);
        });
    });

    c.bench_function("edit_ops_cps_all_20k_lines", |b| {
        let opts = CpsOptions::default();
        b.iter(|| {
            let mut sum = 0.0_f64;
            for ev in script.events() {
                sum += event_metrics(ev, opts).cps;
            }
            black_box(sum);
        });
    });
}

criterion_group!(benches, bench_edit_ops);
criterion_main!(benches);
