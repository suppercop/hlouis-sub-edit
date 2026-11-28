//! Integration tests against fonts that ship with every Windows 10/11 (and Windows Server) install.

use fonts::{check_fonts, FontProvider, FontSpec, WEIGHT_BOLD, WEIGHT_NORMAL};
use fonts_dwrite::{DWriteFontProvider, FontSources, Slant};
use std::collections::BTreeSet;
use std::time::Instant;

const VIETNAMESE: &str =
    "Tiếng Việt có dấu: ăâđêôơư ĂÂĐÊÔƠƯ ạảấầẩẫậắằẳẵặẹẻẽếềểễệỉịọỏốồổỗộớờởỡợụủứừửữựỳỵỷỹ";
const JAPANESE: &str = "日本語のテキストです。ひらがなカタカナ漢字「」、";

fn spec(family: &str, weight: u16, italic: bool) -> FontSpec {
    FontSpec::new(family, weight, italic)
}

fn chars(s: &str) -> BTreeSet<char> {
    s.chars().collect()
}

#[test]
fn finds_always_present_fonts_and_rejects_unknown() {
    let p = DWriteFontProvider::new().unwrap();
    for family in ["Segoe UI", "Arial", "Yu Gothic", "segoe ui", "ARIAL"] {
        assert!(p.has_font(&spec(family, WEIGHT_NORMAL, false)), "{family}");
        assert!(
            p.has_font(&spec(family, WEIGHT_BOLD, true)),
            "{family} bold italic"
        );
    }
    // `@` vertical prefix is stripped by FontSpec.
    assert!(p.has_font(&spec("@Yu Gothic", WEIGHT_NORMAL, false)));
    assert!(!p.has_font(&spec(
        "Definitely Not Installed Font 7f3a",
        WEIGHT_NORMAL,
        false
    )));
    assert!(!p.has_font(&spec("", WEIGHT_NORMAL, false)));
}

#[test]
fn gdi_and_full_names_resolve_like_vsfilter() {
    let p = DWriteFontProvider::new().unwrap();
    // GDI family names that DirectWrite folds into the "Segoe UI" / "Arial" WWS families.
    let semibold = p
        .matched_face(&spec("Segoe UI Semibold", WEIGHT_NORMAL, false))
        .expect("Segoe UI Semibold");
    assert_eq!(semibold.attrs.weight, 600);
    let black = p
        .matched_face(&spec("Arial Black", WEIGHT_NORMAL, false))
        .expect("Arial Black");
    assert_eq!(black.attrs.weight, 900);
    // Full name.
    assert!(p.has_font(&spec("Arial Bold Italic", WEIGHT_NORMAL, false)));
}

#[test]
fn weight_and_italic_matching() {
    let p = DWriteFontProvider::new().unwrap();
    let bold = p.matched_face(&spec("Arial", WEIGHT_BOLD, false)).unwrap();
    assert_eq!((bold.attrs.weight, bold.attrs.slant), (700, Slant::Normal));
    let bi = p.matched_face(&spec("Arial", WEIGHT_BOLD, true)).unwrap();
    assert_eq!((bi.attrs.weight, bi.attrs.slant), (700, Slant::Italic));
    let reg = p
        .matched_face(&spec("Segoe UI", WEIGHT_NORMAL, false))
        .unwrap();
    assert_eq!((reg.attrs.weight, reg.attrs.slant), (400, Slant::Normal));
    let file = reg.file.expect("Segoe UI is a local file");
    assert!(
        file.0.to_string_lossy().to_lowercase().ends_with(".ttf"),
        "{file:?}"
    );
}

#[test]
fn vietnamese_and_japanese_coverage() {
    let p = DWriteFontProvider::new().unwrap();
    for family in ["Segoe UI", "Arial"] {
        let missing = p.missing_glyphs(&spec(family, WEIGHT_NORMAL, false), &chars(VIETNAMESE));
        assert!(missing.is_empty(), "{family} misses Vietnamese {missing:?}");
    }
    let yu = spec("Yu Gothic", WEIGHT_NORMAL, false);
    let missing = p.missing_glyphs(&yu, &chars(JAPANESE));
    assert!(missing.is_empty(), "Yu Gothic misses {missing:?}");
    // Arial has no kana / kanji.
    let missing = p.missing_glyphs(&spec("Arial", WEIGHT_NORMAL, false), &chars("日本あア"));
    assert_eq!(missing, vec!['あ', 'ア', '日', '本']);
    // Whitespace is never reported missing.
    let missing = p.missing_glyphs(&spec("Arial", WEIGHT_NORMAL, false), &chars("\u{3000} a"));
    assert!(missing.is_empty(), "{missing:?}");
}

#[test]
fn check_fonts_on_a_script() {
    let raw = "\
[Script Info]
ScriptType: v4.00+

[V4+ Styles]
Format: Name, Fontname, Fontsize, PrimaryColour, SecondaryColour, OutlineColour, BackColour, Bold, Italic, Underline, StrikeOut, ScaleX, ScaleY, Spacing, Angle, BorderStyle, Outline, Shadow, Alignment, MarginL, MarginR, MarginV, Encoding
Style: Default,Segoe UI,48,&H00FFFFFF,&H0000FFFF,&H00000000,&H80000000,0,0,0,0,100,100,0,0,1,2,1,2,10,10,20,1
Style: JP,Yu Gothic,48,&H00FFFFFF,&H0000FFFF,&H00000000,&H80000000,-1,0,0,0,100,100,0,0,1,2,1,2,10,10,20,1

[Events]
Format: Layer, Start, End, Style, Name, MarginL, MarginR, MarginV, Effect, Text
Dialogue: 0,0:00:01.00,0:00:03.00,Default,,0,0,0,,Xin chào, tiếng Việt {\\i1}nghiêng{\\i0}
Dialogue: 0,0:00:03.00,0:00:05.00,JP,,0,0,0,,日本語{\\fnArial}漢字
Dialogue: 0,0:00:05.00,0:00:06.00,Default,,0,0,0,,{\\fnNo Such Font 9e1}x
";
    let script = ass_core::parse_ass(raw).value;
    let p = DWriteFontProvider::new().unwrap();
    let report = check_fonts(&script, &p);
    assert_eq!(report.missing_fonts.len(), 1);
    assert_eq!(report.missing_fonts[0].spec.family, "No Such Font 9e1");
    assert_eq!(report.missing_glyphs.len(), 1);
    assert_eq!(
        report.missing_glyphs[0].spec,
        spec("Arial", WEIGHT_BOLD, false)
    );
    assert_eq!(report.missing_glyphs[0].missing, vec!['字', '漢']);
}

#[test]
fn gdi_family_name_with_bold_italic_request() {
    // "Segoe UI Semibold" is a GDI family; requesting it bold italic must pick its semibold
    // italic face (GetFirstMatchingFont within that GDI family).
    let p = DWriteFontProvider::new().unwrap();
    let f = p
        .matched_face(&spec("Segoe UI Semibold", WEIGHT_BOLD, true))
        .unwrap();
    assert_eq!((f.attrs.weight, f.attrs.slant), (600, Slant::Italic));
}

#[test]
fn loads_fonts_from_a_folder() {
    let dir = std::env::temp_dir().join(format!("fonts-dwrite-test-{}", std::process::id()));
    std::fs::create_dir_all(&dir).unwrap();
    let windir = std::env::var_os("WINDIR").unwrap_or_else(|| "C:\\Windows".into());
    let src = std::path::Path::new(&windir)
        .join("Fonts")
        .join("segoeui.ttf");
    std::fs::copy(&src, dir.join("copy.ttf")).unwrap();
    std::fs::write(dir.join("not-a-font.txt"), b"hello").unwrap();

    let p = DWriteFontProvider::with_sources(&FontSources {
        system: false,
        user_fonts: false,
        dirs: vec![dir.clone()],
    })
    .unwrap();
    assert_eq!(p.extra_file_count(), 1);
    assert!(p.has_font(&spec("Segoe UI", WEIGHT_NORMAL, false)));
    assert!(!p.has_font(&spec("Arial", WEIGHT_NORMAL, false)));
    let f = p
        .matched_face(&spec("Segoe UI", WEIGHT_NORMAL, false))
        .unwrap();
    let file = f.file.unwrap().0;
    assert!(
        file.to_string_lossy()
            .to_lowercase()
            .contains("fonts-dwrite-test-"),
        "{file:?} not in {dir:?}"
    );
    drop(p);
    let _ = std::fs::remove_dir_all(&dir);
}

fn glyph_set_2000() -> BTreeSet<char> {
    let mut set: BTreeSet<char> = VIETNAMESE.chars().chain(JAPANESE.chars()).collect();
    let ranges = [
        (0x21u32, 0x7E),
        (0xA1, 0x17F),
        (0x1EA0, 0x1EF9),
        (0x3041, 0x3096),
        (0x30A1, 0x30FA),
        (0x4E00, 0x9FFF),
    ];
    'outer: for (a, b) in ranges {
        for c in a..=b {
            if set.len() >= 2_000 {
                break 'outer;
            }
            if let Some(ch) = char::from_u32(c) {
                set.insert(ch);
            }
        }
    }
    set
}

#[test]
fn fifty_fonts_times_two_thousand_glyphs_under_20_ms() {
    let glyphs = glyph_set_2000();
    assert_eq!(glyphs.len(), 2_000);
    let families = [
        "Segoe UI",
        "Arial",
        "Yu Gothic",
        "Times New Roman",
        "Courier New",
        "Tahoma",
        "Verdana",
        "Georgia",
        "Consolas",
        "Segoe UI Semibold",
    ];
    let t_new = Instant::now();
    let p = DWriteFontProvider::new().unwrap();
    let construct = t_new.elapsed();
    let mut specs = Vec::new();
    'fill: for w in [400u16, 700, 300, 600, 900] {
        for f in families {
            if specs.len() == 50 {
                break 'fill;
            }
            specs.push(spec(f, w, specs.len() % 2 == 1));
        }
    }
    assert_eq!(specs.len(), 50);

    let run = || {
        let t = Instant::now();
        let mut missing = 0usize;
        for s in &specs {
            if p.has_font(s) {
                missing += p.missing_glyphs(s, &glyphs).len();
            }
        }
        (t.elapsed(), missing)
    };
    let (cold, m1) = run();
    let (warm, m2) = run();
    assert_eq!(m1, m2);
    eprintln!(
        "fonts-dwrite: provider {:.2} ms; 50 fonts x 2000 glyphs: cold {:.2} ms (resolve + cmap \
         read), warm {:.2} ms",
        construct.as_secs_f64() * 1e3,
        cold.as_secs_f64() * 1e3,
        warm.as_secs_f64() * 1e3
    );
    // The 20 ms budget is for release builds on fonts DirectWrite has already
    // opened (warm); unoptimised test builds only get a sanity bound. The cold
    // pass is dominated by the OS reading ~50 font files from disk for the
    // first time (up to ~8 s seen on a fresh CI runner with cold DirectWrite
    // cache), so it only gets a sanity bound that still catches pathological
    // regressions (> 30 s would indicate a code bug, not runner variance).
    let budget_ms = if cfg!(debug_assertions) { 500 } else { 20 };
    let cold_bound_ms = 30_000u128;
    eprintln!(
        "fonts-dwrite cold {:.2} ms (bound {} ms), warm {:.2} ms (budget {} ms)",
        cold.as_secs_f64() * 1e3,
        cold_bound_ms,
        warm.as_secs_f64() * 1e3,
        budget_ms,
    );
    assert!(cold.as_millis() < cold_bound_ms, "cold check took {cold:?}");
    assert!(warm.as_millis() < budget_ms, "warm check took {warm:?}");
}
