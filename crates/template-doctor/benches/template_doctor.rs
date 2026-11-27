use ass_core::parse_ass;
use criterion::{Criterion, black_box, criterion_group, criterion_main};
use std::fmt::Write as _;
use template_doctor::lint_script;

fn make_2000_templates() -> ass_core::ScriptFile {
    let mut s = String::from(
        "[Script Info]\nPlayResX: 1920\nPlayResY: 1080\n\n\
         [Events]\n\
         Format: Layer, Start, End, Style, Name, MarginL, MarginR, MarginV, Effect, Text\n",
    );
    for i in 0..2_000usize {
        if i % 10 == 0 {
            let _ = writeln!(
                s,
                "Comment: 0,0:00:00.00,0:00:05.00,Default,,0,0,0,code syl,x = math.floor({i} / 2)"
            );
        } else {
            let _ = writeln!(
                s,
                "Comment: 0,0:00:00.00,0:00:05.00,Default,,0,0,0,template syl noblank,{{\\an5\\pos($scenter,$smiddle)\\fad(100,150)\\t($start,$end,\\blur2)}}"
            );
        }
    }
    parse_ass(&s).value
}

fn bench_template_doctor(c: &mut Criterion) {
    let script = make_2000_templates();
    c.bench_function("template_doctor_lint_2000_lines", |b| {
        b.iter(|| {
            let diags = lint_script(black_box(&script));
            black_box(diags.len());
        });
    });
}

criterion_group!(benches, bench_template_doctor);
criterion_main!(benches);
