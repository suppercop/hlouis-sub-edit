use criterion::{black_box, criterion_group, criterion_main, Criterion};
use edit_box_ops::{insert_hard_newline, set_alignment, set_color, toggle_bold, TextSelection};

fn build_2000_char_line() -> String {
    let chunk = r"{\an8\pos(640,360)\fs48\bord2\c&H00FFFFFF&}Sample typeset text\N";
    let mut s = String::with_capacity(2100);
    while s.len() < 2000 {
        s.push_str(chunk);
    }
    s.truncate(2000);
    if let Some(open) = s.rfind('{') {
        if s[open..].find('}').is_none() {
            s.truncate(open);
        }
    }
    while s.len() < 2000 {
        s.push('x');
    }
    s
}

fn bench_edit_box_ops(c: &mut Criterion) {
    let line = build_2000_char_line();
    let mid = line.len() / 2;
    let sel = TextSelection {
        start: mid,
        end: (mid + 12).min(line.len()),
    };

    c.bench_function("edit_box_ops_toggle_bold_2000_char", |b| {
        b.iter(|| {
            let res = toggle_bold(black_box(&line), sel, "Default", &[]);
            black_box(res);
        });
    });

    c.bench_function("edit_box_ops_set_color_2000_char", |b| {
        b.iter(|| {
            let res = set_color(black_box(&line), sel, 1, 255, 128, 64).expect("color");
            black_box(res);
        });
    });

    c.bench_function("edit_box_ops_set_alignment_2000_char", |b| {
        b.iter(|| {
            let res = set_alignment(black_box(&line), sel, 8).expect("an");
            black_box(res);
        });
    });

    c.bench_function("edit_box_ops_insert_newline_2000_char", |b| {
        b.iter(|| {
            let res = insert_hard_newline(black_box(&line), sel);
            black_box(res);
        });
    });
}

criterion_group!(benches, bench_edit_box_ops);
criterion_main!(benches);
