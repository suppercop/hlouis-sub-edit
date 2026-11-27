//! Criterion benchmark for parsing, serializing, and undo/redo on a 20,000-line ASS script.

use ass_core::{parse_ass, serialize_ass, AssTime, EditCommand, UndoHistory};
use criterion::{black_box, criterion_group, criterion_main, Criterion};
use std::fmt::Write as _;

fn generate_20k_ass_script() -> String {
    let mut out = String::with_capacity(2_400_000);
    out.push_str("\u{FEFF}[Script Info]\r\nTitle: Bench 20k\r\nScriptType: v4.00+\r\nPlayResX: 1920\r\nPlayResY: 1080\r\n\r\n");
    out.push_str("[V4+ Styles]\r\nFormat: Name, Fontname, Fontsize, PrimaryColour, SecondaryColour, OutlineColour, BackColour, Bold, Italic, Underline, StrikeOut, ScaleX, ScaleY, Spacing, Angle, BorderStyle, Outline, Shadow, Alignment, MarginL, MarginR, MarginV, Encoding\r\n");
    out.push_str("Style: Default,Segoe UI,54,&H00FFFFFF,&H0000FFFF,&H00181820,&H90000000,-1,0,0,0,100,100,0,0,1,3,1.5,2,40,40,36,1\r\n\r\n");
    out.push_str("[Events]\r\nFormat: Layer, Start, End, Style, Name, MarginL, MarginR, MarginV, Effect, Text\r\n");

    let templates = [
        r"{\an8\fs44\c&H00FFFF&}Xin chào Tiếng Việt\N{\k30}Ka{\kf45}raoke — 日本語テスト",
        r"{\pos(960,980)\fad(150,150)}日本語の字幕テスト：漢字・ひらがな・カタカナ",
        r"{\an5\frz-12\t(0,400,\fscx115\fscy115)}Styled sign\hwith\hnon-breaking spaces",
        "Plain dialogue line with comma, punctuation, and normal text.",
    ];

    for i in 0..20_000u32 {
        let start = AssTime::from_cs(i * 150);
        let end = start.saturating_add_cs(140);
        let text = templates[(i as usize) % templates.len()];
        let _ = writeln!(
            out,
            "Dialogue: 0,{start},{end},Default,Speaker,0,0,0,,{text} [#{i}]\r"
        );
    }
    out
}

fn bench_ass_core(c: &mut Criterion) {
    let raw_20k = generate_20k_ass_script();
    let parsed_20k = parse_ass(&raw_20k).value;

    c.bench_function("parse_20k_lines", |b| {
        b.iter(|| {
            let res = parse_ass(black_box(&raw_20k));
            black_box(res.value.events().count())
        });
    });

    c.bench_function("serialize_20k_lines", |b| {
        b.iter(|| {
            let s = serialize_ass(black_box(&parsed_20k));
            black_box(s.len())
        });
    });

    c.bench_function("undo_redo_step_20k_lines", |b| {
        let mut script = parsed_20k.clone();
        let mut history = UndoHistory::default();
        history
            .execute(
                &mut script,
                EditCommand::ModifyEventText {
                    index: 19_999,
                    before: "orig".to_owned(),
                    after: "edited".to_owned(),
                },
            )
            .expect("setup edit");
        b.iter(|| {
            let _ = history.undo(black_box(&mut script));
            let _ = history.redo(black_box(&mut script));
        });
    });
}

criterion_group!(benches, bench_ass_core);
criterion_main!(benches);
