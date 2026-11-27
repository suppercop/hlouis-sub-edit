use criterion::{black_box, criterion_group, criterion_main, Criterion};
use script_props::{check_resolution_mismatch, ResCheckPolicy, ScriptProperties};

fn bench_script_props(c: &mut Criterion) {
    let mut script = ass_core::parse_ass(
        "[Script Info]\nTitle: Episode 01\nPlayResX: 1280\nPlayResY: 720\nWrapStyle: 0\nScaledBorderAndShadow: yes\nYCbCr Matrix: TV.709\n",
    )
    .value;

    c.bench_function("script_props_read_edit_apply", |b| {
        b.iter(|| {
            let mut props = ScriptProperties::from_script(black_box(&script));
            let dec = check_resolution_mismatch(&props, 1920, 1080, true, ResCheckPolicy::Ask);
            props.set_from_video(1920, 1080);
            let edit = props.build_edit(&script).expect("edit");
            edit.apply(&mut script).expect("apply");
            edit.revert(&mut script).expect("revert");
            black_box(dec);
        });
    });
}

criterion_group!(benches, bench_script_props);
criterion_main!(benches);
