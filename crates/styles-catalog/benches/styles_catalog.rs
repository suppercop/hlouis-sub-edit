use ass_core::{AssStyle, ScriptFile};
use criterion::{black_box, criterion_group, criterion_main, Criterion};
use std::fmt::Write as _;
use styles_catalog::{import_styles, rename_style, ConflictPolicy};

fn build_20k_script_for_rename() -> ScriptFile {
    let mut raw = String::with_capacity(20_000 * 80);
    raw.push_str(
        "[Script Info]\nScriptType: v4.00+\n\n[V4+ Styles]\nFormat: Name, Fontname, Fontsize, PrimaryColour, SecondaryColour, OutlineColour, BackColour, Bold, Italic, Underline, StrikeOut, ScaleX, ScaleY, Spacing, Angle, BorderStyle, Outline, Shadow, Alignment, MarginL, MarginR, MarginV, Encoding\nStyle: Default,Arial,48,&H00FFFFFF,&H000000FF,&H00000000,&H00000000,0,0,0,0,100,100,0,0,1,2,2,2,10,10,10,1\nStyle: Sign,Arial,36,&H00FFFFFF,&H000000FF,&H00000000,&H00000000,0,0,0,0,100,100,0,0,1,2,2,8,10,10,10,1\n\n[Events]\nFormat: Layer, Start, End, Style, Name, MarginL, MarginR, MarginV, Effect, Text\n",
    );
    for i in 0..20_000u32 {
        if i % 20 == 0 {
            let _ = writeln!(
                raw,
                "Dialogue: 0,0:00:01.00,0:00:03.00,Sign,,0,0,0,,{{\\rSign}}Sign text {i}"
            );
        } else {
            let _ = writeln!(
                raw,
                "Dialogue: 0,0:00:01.00,0:00:03.00,Default,,0,0,0,,Normal dialogue line {i}"
            );
        }
    }
    ass_core::parse_ass(&raw).value
}

fn build_200_styles() -> Vec<AssStyle> {
    let mut raw = String::with_capacity(200 * 120);
    raw.push_str(
        "[V4+ Styles]\nFormat: Name, Fontname, Fontsize, PrimaryColour, SecondaryColour, OutlineColour, BackColour, Bold, Italic, Underline, StrikeOut, ScaleX, ScaleY, Spacing, Angle, BorderStyle, Outline, Shadow, Alignment, MarginL, MarginR, MarginV, Encoding\n",
    );
    for i in 0..200u32 {
        let _ = writeln!(
            raw,
            "Style: ImportedStyle_{i},Arial,48,&H00FFFFFF,&H000000FF,&H00000000,&H00000000,0,0,0,0,100,100,0,0,1,2,2,2,10,10,10,1"
        );
    }
    ass_core::parse_ass(&raw).value.styles().cloned().collect()
}

fn bench_styles_catalog(c: &mut Criterion) {
    let script = build_20k_script_for_rename();
    let incoming_200 = build_200_styles();

    c.bench_function("styles_catalog_rename_propagate_20k", |b| {
        b.iter(|| {
            let cmd =
                rename_style(black_box(&script), "Sign", "SignRenamed", true).expect("rename");
            black_box(cmd);
        });
    });

    c.bench_function("styles_catalog_import_200_styles", |b| {
        b.iter(|| {
            let cmd = import_styles(
                black_box(&script),
                black_box(&incoming_200),
                ConflictPolicy::RenameWithSuffix,
            )
            .expect("import");
            black_box(cmd);
        });
    });
}

criterion_group!(benches, bench_styles_catalog);
criterion_main!(benches);
