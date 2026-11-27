use ass_core::ScriptFile;
use criterion::{black_box, criterion_group, criterion_main, Criterion};
use std::fmt::Write as _;
use style_editor::{apply_style_edit, StyleDraft};

fn build_20k_script() -> ScriptFile {
    let mut raw = String::with_capacity(20_000 * 80);
    raw.push_str(
        "[Script Info]\nScriptType: v4.00+\n\n[V4+ Styles]\nFormat: Name, Fontname, Fontsize, PrimaryColour, SecondaryColour, OutlineColour, BackColour, Bold, Italic, Underline, StrikeOut, ScaleX, ScaleY, Spacing, Angle, BorderStyle, Outline, Shadow, Alignment, MarginL, MarginR, MarginV, Encoding\nStyle: Default,Arial,48,&H00FFFFFF,&H000000FF,&H00000000,&H00000000,0,0,0,0,100,100,0,0,1,2,2,2,10,10,10,1\nStyle: Sign,Arial,36,&H00FFFFFF,&H000000FF,&H00000000,&H00000000,0,0,0,0,100,100,0,0,1,2,2,8,10,10,10,1\n\n[Events]\nFormat: Layer, Start, End, Style, Name, MarginL, MarginR, MarginV, Effect, Text\n",
    );
    for i in 0..20_000u32 {
        if i % 20 == 0 {
            let _ = writeln!(
                raw,
                "Dialogue: 0,0:00:01.00,0:00:03.00,Sign,,0,0,0,,{{\\rSign}}Sign {i}"
            );
        } else {
            let _ = writeln!(
                raw,
                "Dialogue: 0,0:00:01.00,0:00:03.00,Default,,0,0,0,,Dialogue {i}"
            );
        }
    }
    ass_core::parse_ass(&raw).value
}

fn bench_style_editor(c: &mut Criterion) {
    let script = build_20k_script();
    let sign = script.styles().find(|s| s.name() == "Sign").expect("Sign");
    let base_draft = StyleDraft::from_style(sign);

    c.bench_function("style_editor_apply_with_rename_20k", |b| {
        b.iter(|| {
            let mut draft = base_draft.clone();
            "SignTop".clone_into(&mut draft.name);
            draft.fontsize = 42.0;
            let cmd =
                apply_style_edit(black_box(&script), "Sign", &mut draft, true).expect("apply");
            black_box(cmd);
        });
    });
}

criterion_group!(benches, bench_style_editor);
criterion_main!(benches);
