use ass_core::{AssStyle, ScriptFile};
use criterion::{black_box, criterion_group, criterion_main, Criterion};
use karaskel::{preprocess_script, TextExtents, TextMetric};
use std::fmt::Write as _;

struct FastMockExtents;
impl TextExtents for FastMockExtents {
    #[inline]
    fn extents(&self, _style: &AssStyle, text: &str) -> TextMetric {
        #[allow(clippy::cast_precision_loss)]
        let w = (text.len() as f64) * 10.0;
        TextMetric {
            width: w,
            height: 24.0,
            descent: 4.0,
            ext_lead: 0.0,
        }
    }
}

fn build_2000_karaoke_lines() -> ScriptFile {
    let mut raw = String::with_capacity(2000 * 220);
    raw.push_str(
        "[Script Info]\nPlayResX: 1920\nPlayResY: 1080\n\n[V4+ Styles]\nFormat: Name, Fontname, Fontsize, PrimaryColour, SecondaryColour, OutlineColour, BackColour, Bold, Italic, Underline, StrikeOut, ScaleX, ScaleY, Spacing, Angle, BorderStyle, Outline, Shadow, Alignment, MarginL, MarginR, MarginV, Encoding\nStyle: Default,Arial,48,&H00FFFFFF,&H000000FF,&H00000000,&H00000000,0,0,0,0,100,100,0,0,1,2,2,2,20,20,30,1\n\n[Events]\nFormat: Layer, Start, End, Style, Name, MarginL, MarginR, MarginV, Effect, Text\n",
    );
    for line_i in 0..2000u32 {
        let _ = write!(raw, "Dialogue: 0,0:00:01.00,0:00:05.00,Default,,0,0,0,,");
        for syl_i in 0..20u32 {
            let _ = write!(raw, "{{\\k20}}s{syl_i}");
        }
        let _ = writeln!(raw, "_{line_i}");
    }
    ass_core::parse_ass(&raw).value
}

fn bench_karaskel(c: &mut Criterion) {
    let script = build_2000_karaoke_lines();

    c.bench_function("karaskel_preprocess_2000_lines_20_syls", |b| {
        b.iter(|| {
            let out = preprocess_script(black_box(&script), &FastMockExtents, false);
            black_box(out.lines.len());
        });
    });
}

criterion_group!(benches, bench_karaskel);
criterion_main!(benches);
