//! Zero-allocation hot-path & linear scaling (`n` vs `2n`) tests for `karaskel`.

use ass_core::{parse_ass, AssStyle};
use karaskel::{collect_head, preprocess_script_into, KaraskelOutput, TextExtents, TextMetric};
use std::alloc::{GlobalAlloc, Layout, System};
use std::fmt::Write as _;
use std::sync::atomic::{AtomicUsize, Ordering};
use std::time::Instant;

struct CountingAlloc;

static ALLOC_COUNT: AtomicUsize = AtomicUsize::new(0);

#[global_allocator]
static GLOBAL: CountingAlloc = CountingAlloc;

#[allow(unsafe_code)]
unsafe impl GlobalAlloc for CountingAlloc {
    unsafe fn alloc(&self, layout: Layout) -> *mut u8 {
        ALLOC_COUNT.fetch_add(1, Ordering::Relaxed);
        unsafe { System.alloc(layout) }
    }

    unsafe fn dealloc(&self, ptr: *mut u8, layout: Layout) {
        unsafe { System.dealloc(ptr, layout) }
    }
}

fn allocs() -> usize {
    ALLOC_COUNT.load(Ordering::Relaxed)
}

struct MockExtents;

impl TextExtents for MockExtents {
    fn extents(&self, style: &AssStyle, text: &str) -> TextMetric {
        let fs = style.fontsize();
        #[allow(clippy::cast_precision_loss)]
        let width = (text.chars().count() as f64) * fs * 0.55;
        TextMetric {
            width,
            height: fs,
            descent: fs * 0.2,
            ext_lead: fs * 0.1,
        }
    }
}

fn make_karaoke_script(lines: usize) -> ass_core::ScriptFile {
    let mut s = String::from(
        "[Script Info]\nPlayResX: 1920\nPlayResY: 1080\n\n\
         [V4+ Styles]\n\
         Format: Name, Fontname, Fontsize, PrimaryColour, SecondaryColour, OutlineColour, BackColour, Bold, Italic, Underline, StrikeOut, ScaleX, ScaleY, Spacing, Angle, BorderStyle, Outline, Shadow, Alignment, MarginL, MarginR, MarginV, Encoding\n\
         Style: Default,Arial,48,&H00FFFFFF,&H000000FF,&H00000000,&H00000000,0,0,0,0,100,100,0,0,1,2,2,2,10,10,10,1\n\n\
         [Events]\n\
         Format: Layer, Start, End, Style, Name, MarginL, MarginR, MarginV, Effect, Text\n",
    );
    for i in 0..lines {
        let _ = writeln!(
            s,
            "Dialogue: 0,0:00:{:02}.00,0:00:{:02}.00,Default,,0,0,0,,{{\\k10}}a{{\\k10}}b{{\\k10}}c{{\\k10}}d{{\\k10}}e{{\\k10}}f{{\\k10}}g{{\\k10}}h{{\\k10}}i{{\\k10}}j",
            i % 55,
            (i % 55) + 3
        );
    }
    parse_ass(&s).value
}

fn hot_path_accessors_and_collect_head_zero_alloc() {
    let script = make_karaoke_script(64);
    let mut out = KaraskelOutput::default();
    preprocess_script_into(&script, &MockExtents, true, &mut out);
    assert_eq!(out.lines.len(), 64);

    let before = allocs();
    let meta = collect_head(&script, Some(16.0 / 9.0));
    assert!(meta.res_x > 0.0);
    let mut total_bytes = 0usize;
    for line in &out.lines {
        total_bytes += out.line_text(line).len();
        for syl in out.line_syllables(line) {
            total_bytes += out.syllable_text(syl).len();
            total_bytes += out.syllable_fx(syl).len();
            total_bytes += out.syllable_highlights(syl).len();
            for f in out.syllable_furigana(syl) {
                total_bytes += out.furigana_text(f).len();
            }
        }
    }
    let after = allocs();
    assert!(total_bytes > 0);
    assert_eq!(
        after - before,
        0,
        "karaskel hot-path accessors allocated {} times",
        after - before
    );
}

fn scaling_n_vs_2n_is_linear() {
    let s1 = make_karaoke_script(500);
    let s2 = make_karaoke_script(1000);
    let mut out1 = KaraskelOutput::default();
    let mut out2 = KaraskelOutput::default();
    // Warm up arenas.
    preprocess_script_into(&s1, &MockExtents, false, &mut out1);
    preprocess_script_into(&s2, &MockExtents, false, &mut out2);

    let t1 = Instant::now();
    for _ in 0..5 {
        preprocess_script_into(&s1, &MockExtents, false, &mut out1);
    }
    let d1 = t1.elapsed().as_nanos().max(1);

    let t2 = Instant::now();
    for _ in 0..5 {
        preprocess_script_into(&s2, &MockExtents, false, &mut out2);
    }
    let d2 = t2.elapsed().as_nanos().max(1);

    // Linear scaling: 2n should take well under 4x of n (allow generous jitter for debug builds).
    assert!(
        d2 <= d1 * 4,
        "expected near-linear scaling: n={d1}ns, 2n={d2}ns"
    );
}

#[test]
fn zero_alloc_hot_path_and_scaling() {
    hot_path_accessors_and_collect_head_zero_alloc();
    scaling_n_vs_2n_is_linear();
}
