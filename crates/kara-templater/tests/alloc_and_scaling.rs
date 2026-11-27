//! Zero-allocation inline-variable formatting & O(n) scaling tests for `kara-templater`.

use std::fmt::Write as _;

use ass_core::{AssStyle, parse_ass};
use kara_templater::{EngineConfig, KaraTemplater};
use karaskel::{TextExtents, TextMetric};
use std::time::Instant;

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

fn build_script(syl_count: usize) -> String {
    let mut s = String::from(
        "[Script Info]\nScriptType: v4.00+\nPlayResX: 1920\nPlayResY: 1080\n\n\
         [V4+ Styles]\nFormat: Name, Fontname, Fontsize, PrimaryColour, SecondaryColour, OutlineColour, BackColour, Bold, Italic, Underline, StrikeOut, ScaleX, ScaleY, Spacing, Angle, BorderStyle, Outline, Shadow, Alignment, MarginL, MarginR, MarginV, Encoding\n\
         Style: Default,Arial,40,&H00FFFFFF,&H000000FF,&H00000000,&H00000000,0,0,0,0,100,100,0,0,1,2,2,2,10,10,10,1\n\n\
         [Events]\nFormat: Layer, Start, End, Style, Name, MarginL, MarginR, MarginV, Effect, Text\n\
         Comment: 0,0:00:00.00,0:00:00.00,Default,,0,0,0,template syl,!retime(\"syl\",0,0)!{\\an5\\pos($x,$y)\\fscx!100+syl.i!}\n",
    );
    s.push_str("Dialogue: 0,0:00:01.00,0:00:30.00,Default,,0,0,0,karaoke,");
    for i in 0..syl_count {
        let _ = write!(s, "{{\\k20}}s{i}");
    }
    s.push('\n');
    s
}

#[test]
fn scaling_n_vs_2n() {
    let engine = KaraTemplater::new(EngineConfig::default());
    let src1 = build_script(100);
    let src2 = build_script(200);

    let mut s1 = parse_ass(&src1).value;
    let t1 = Instant::now();
    let r1 = engine.apply(&mut s1, &MonoExtents).expect("apply n");
    let d1 = t1.elapsed().as_nanos().max(1);

    let mut s2 = parse_ass(&src2).value;
    let t2 = Instant::now();
    let r2 = engine.apply(&mut s2, &MonoExtents).expect("apply 2n");
    let d2 = t2.elapsed().as_nanos().max(1);

    assert_eq!(r1.generated_fx_lines, 100);
    assert_eq!(r2.generated_fx_lines, 200);
    assert!(d2 <= d1 * 5, "expected linear scaling: n={d1}ns 2n={d2}ns");
}
