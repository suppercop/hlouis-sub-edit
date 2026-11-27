use criterion::{black_box, criterion_group, criterion_main, Criterion};
use syntax_highlight::{
    format_grid_text_into, tokenize_into, GridDisplayConfig, IncrementalTokenizer, TagDisplayMode,
};

fn build_2000_char_typeset_line() -> String {
    let chunk =
        r"{\an8\pos(640,360)\fs48\bord2\shad1\c&H00FFFFFF&\t(0,500,\fscx120\fscy120)}Typeset\N";
    let mut s = String::with_capacity(2100);
    while s.len() < 2000 {
        s.push_str(chunk);
    }
    s.truncate(2000);
    // Ensure valid UTF-8 boundary and close any partial brace
    if let Some(last_open) = s.rfind('{') {
        if s[last_open..].find('}').is_none() {
            s.truncate(last_open);
        }
    }
    while s.len() < 2000 {
        s.push('a');
    }
    s
}

fn bench_syntax_highlight(c: &mut Criterion) {
    let heavy = build_2000_char_typeset_line();
    let mut buf = Vec::with_capacity(256);

    c.bench_function("syntax_highlight_tokenize_2000_char", |b| {
        b.iter(|| {
            tokenize_into(black_box(&heavy), &mut buf);
            black_box(buf.len());
        });
    });

    // Incremental 1-char edit in the middle of the 2000-char line.
    let edit_pos = heavy.len() / 2;
    let mut edited = heavy.clone();
    edited.insert(edit_pos, 'Z');

    c.bench_function("syntax_highlight_retokenize_1_char_edit", |b| {
        let mut inc = IncrementalTokenizer::new();
        inc.set_text(&heavy);
        b.iter(|| {
            inc.re_tokenize_edit(black_box(&edited), edit_pos, edit_pos, edit_pos + 1);
            inc.re_tokenize_edit(black_box(&heavy), edit_pos, edit_pos + 1, edit_pos);
            black_box(inc.tokens().len());
        });
    });

    // 200 visible grid rows
    let rows: Vec<String> = (0..200)
        .map(|i| {
            format!(
                r"{{\an8\pos({i},200)\c&H00FFFFFF&}}Visible grid line {i} with {{\i1}}italics{{\i0}}."
            )
        })
        .collect();
    let cfg = GridDisplayConfig {
        mode: TagDisplayMode::Simplify,
        ..GridDisplayConfig::default()
    };
    let mut out = String::with_capacity(256);

    c.bench_function("syntax_highlight_grid_display_200_rows", |b| {
        b.iter(|| {
            let mut total = 0usize;
            for row in &rows {
                format_grid_text_into(black_box(row), &cfg, &mut out);
                total += out.len();
            }
            black_box(total);
        });
    });
}

criterion_group!(benches, bench_syntax_highlight);
criterion_main!(benches);
