use ass_core::{AssStyle, parse_ass};
use criterion::{Criterion, black_box, criterion_group, criterion_main};
use kara_templater::{EngineConfig, KaraTemplater};
use karaskel::{TextExtents, TextMetric};
use std::fmt::Write as _;

struct MonoExtents;
impl TextExtents for MonoExtents {
    fn extents(&self, _style: &AssStyle, text: &str) -> TextMetric {
        #[allow(clippy::cast_precision_loss)]
        let width = (text.chars().count() as f64) * 18.0;
        TextMetric {
            width,
            height: 36.0,
            descent: 6.0,
            ext_lead: 0.0,
        }
    }
}

fn bench_kara_templater(c: &mut Criterion) {
    let mut src = String::from(
        "[Script Info]\nScriptType: v4.00+\nPlayResX: 1920\nPlayResY: 1080\n\n\
         [V4+ Styles]\nFormat: Name, Fontname, Fontsize, PrimaryColour, SecondaryColour, OutlineColour, BackColour, Bold, Italic, Underline, StrikeOut, ScaleX, ScaleY, Spacing, Angle, BorderStyle, Outline, Shadow, Alignment, MarginL, MarginR, MarginV, Encoding\n\
         Style: Default,Arial,40,&H00FFFFFF,&H000000FF,&H00000000,&H00000000,0,0,0,0,100,100,0,0,1,2,2,2,10,10,10,1\n\n\
         [Events]\nFormat: Layer, Start, End, Style, Name, MarginL, MarginR, MarginV, Effect, Text\n",
    );
    for t in 0..10 {
        let _ = writeln!(
            src,
            "Comment: {t},0:00:00.00,0:00:00.00,Default,,0,0,0,template syl,!retime(\"syl\",0,0)!{{\\an5\\pos($x,$y)\\blur!1+{t}!}}"
        );
    }
    // 25 lines x 20 syllables = 500 syllables x 10 templates = 5,000 generated fx lines
    for line_idx in 0..25 {
        let _ = write!(
            src,
            "Dialogue: 0,0:00:{:02}.00,0:00:{:02}.00,Default,,0,0,0,karaoke,",
            line_idx * 2,
            line_idx * 2 + 2
        );
        for syl_idx in 0..20 {
            let _ = write!(src, "{{\\k10}}s{syl_idx}");
        }
        src.push('\n');
    }

    let base_script = parse_ass(&src).value;
    let engine = KaraTemplater::new(EngineConfig::default());

    c.bench_function("kara_templater_500_syls_x_10_templates", |b| {
        b.iter(|| {
            let mut s = base_script.clone();
            let rep = engine.apply(&mut s, &MonoExtents);
            let _ = black_box(rep);
        });
    });
}

criterion_group!(benches, bench_kara_templater);
criterion_main!(benches);
