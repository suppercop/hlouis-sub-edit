//! Integration tests, fixture tests, proptest round-trip tests, and `HLOUIS_GOLDEN_DIR` test.

use ass_core::{
    parse_ass, parse_ass_bytes, parse_srt_bytes, parse_text_blocks, serialize_ass,
    serialize_ass_bytes, serialize_text_blocks, write_srt, AssEvent, AssTime,
};
use proptest::prelude::*;
use std::fmt::Write as _;
use std::path::Path;

const FIXTURES_DIR: &str = concat!(env!("CARGO_MANIFEST_DIR"), "/tests/fixtures");

#[test]
fn crlf_bom_full_fixture_round_trips_byte_identically() {
    let bytes = std::fs::read(Path::new(FIXTURES_DIR).join("crlf_bom_full.ass"))
        .expect("read crlf_bom_full.ass");
    let (outcome, enc) = parse_ass_bytes(&bytes);
    assert_eq!(enc.name, "UTF-8");
    assert!(enc.has_bom);
    assert!(
        outcome.warnings.is_empty(),
        "unexpected warnings: {:?}",
        outcome.warnings
    );

    let script = &outcome.value;
    assert_eq!(
        script.script_info().and_then(|s| s.get("X-Custom-Header")),
        Some("keep-me-verbatim")
    );
    assert_eq!(
        script.aegisub_garbage().and_then(|s| s.get("Active Line")),
        Some("2")
    );
    assert_eq!(script.styles().count(), 2);
    assert_eq!(script.events().count(), 3);

    // Trailing whitespace in Text is preserved.
    let first_ev = script.events().next().expect("first event");
    assert_eq!(first_ev.text, "Keep trailing spaces   ");

    let out_bytes = serialize_ass_bytes(script);
    assert_eq!(
        out_bytes, bytes,
        "CRLF + BOM fixture must round-trip byte-for-byte"
    );
}

#[test]
fn lf_custom_format_fixture_round_trips_byte_identically() {
    let bytes = std::fs::read(Path::new(FIXTURES_DIR).join("lf_custom_format.ass"))
        .expect("read lf_custom_format.ass");
    let (outcome, enc) = parse_ass_bytes(&bytes);
    assert_eq!(enc.name, "UTF-8");
    assert!(!enc.has_bom);
    assert!(outcome.warnings.is_empty(), "{:?}", outcome.warnings);

    let ev = outcome.value.events().next().expect("event");
    assert_eq!(ev.layer, 2);
    assert_eq!(ev.start, AssTime::from_cs(50));
    assert_eq!(ev.end, AssTime::from_cs(200));
    assert_eq!(ev.style, "Mini");
    assert_eq!(ev.text, "Custom order, with comma");

    assert_eq!(serialize_ass_bytes(&outcome.value), bytes);
}

#[test]
fn utf16le_bom_fixture_decodes_cleanly() {
    let bytes = std::fs::read(Path::new(FIXTURES_DIR).join("utf16le_bom.ass"))
        .expect("read utf16le_bom.ass");
    let (outcome, enc) = parse_ass_bytes(&bytes);
    assert_eq!(enc.name, "UTF-16LE");
    assert!(enc.has_bom);
    assert!(!enc.had_errors);
    assert!(outcome.warnings.is_empty(), "{:?}", outcome.warnings);
    let ev = outcome.value.events().next().expect("event");
    assert_eq!(ev.text, "Xin chào Tiếng Việt — 日本語");
}

#[test]
fn malformed_fixture_does_not_panic_and_collects_warnings() {
    let bytes =
        std::fs::read(Path::new(FIXTURES_DIR).join("malformed.ass")).expect("read malformed.ass");
    let (outcome, _) = parse_ass_bytes(&bytes);
    assert!(outcome.warnings.len() >= 2);
    let events: Vec<&AssEvent> = outcome.value.events().collect();
    assert_eq!(events.len(), 2);
    assert_eq!(events[0].start, AssTime::ZERO);
    assert_eq!(events[0].end, AssTime::from_cs(500));
    assert_eq!(events[0].text, "Hello, world, extra, commas");
}

#[test]
fn srt_fixture_parses_and_writes_cleanly() {
    let bytes = std::fs::read(Path::new(FIXTURES_DIR).join("sample_bom_tolerant.srt"))
        .expect("read sample_bom_tolerant.srt");
    let (outcome, enc) = parse_srt_bytes(&bytes);
    assert_eq!(enc.name, "UTF-8");
    assert!(enc.has_bom);
    assert!(outcome.warnings.is_empty(), "{:?}", outcome.warnings);
    assert_eq!(outcome.value.events().count(), 2);

    let out = write_srt(&outcome.value);
    assert!(out.contains("<i>Xin chào</i> <b>Tiếng Việt</b>"));
    assert!(out.contains("<font color=\"#00FF88\">Colored</font> <u>line</u>\r\nSecond line"));
}

/// Ignored golden round-trip test for real Aegisub 3.5.1 files.
/// Run with: `HLOUIS_GOLDEN_DIR=/path/to/ass/files cargo test -p ass-core -- --ignored golden`
#[test]
#[ignore = "requires HLOUIS_GOLDEN_DIR pointing to real Aegisub-saved .ass files"]
fn golden_dir_byte_identical_round_trip() {
    let dir = std::env::var("HLOUIS_GOLDEN_DIR")
        .expect("set HLOUIS_GOLDEN_DIR to a directory of .ass files");
    let entries = std::fs::read_dir(&dir).expect("read HLOUIS_GOLDEN_DIR");
    let mut checked = 0usize;

    for entry in entries {
        let entry = entry.expect("dir entry");
        let path = entry.path();
        if !path.is_file() {
            continue;
        }
        let is_ass = path
            .extension()
            .and_then(|e| e.to_str())
            .is_some_and(|e| e.eq_ignore_ascii_case("ass"));
        if !is_ass {
            continue;
        }
        let bytes = std::fs::read(&path).expect("read golden file");
        let (outcome, enc) = parse_ass_bytes(&bytes);
        assert_eq!(
            enc.name,
            "UTF-8",
            "golden file {} is not UTF-8 (detected {})",
            path.display(),
            enc.name
        );
        let serialized = serialize_ass_bytes(&outcome.value);
        assert_eq!(
            serialized,
            bytes,
            "round-trip mismatch for golden file {}",
            path.display()
        );
        checked += 1;
    }

    assert!(
        checked > 0,
        "no .ass files found in HLOUIS_GOLDEN_DIR={dir}"
    );
}

fn arb_single_line_text() -> impl Strategy<Value = String> {
    proptest::string::string_regex(r"[A-Za-z0-9 ,.!?\-\\\{\}\(\)&;:_]{0,80}").expect("valid regex")
}

proptest! {
    #[test]
    fn prop_tag_parser_round_trips_any_utf8_string(input in "\\PC{0,120}") {
        let blocks = parse_text_blocks(&input);
        let reserialized = serialize_text_blocks(&blocks);
        prop_assert_eq!(reserialized, input);
    }

    #[test]
    fn prop_ass_file_round_trips_byte_identically(
        has_bom in any::<bool>(),
        use_crlf in any::<bool>(),
        trailing_nl in any::<bool>(),
        title in "[A-Za-z0-9 _-]{1,24}",
        events in prop::collection::vec(
            (0i32..10, 0u32..359_999, 0u32..5_000, arb_single_line_text()),
            1..12
        )
    ) {
        let nl = if use_crlf { "\r\n" } else { "\n" };
        let mut raw = String::new();
        if has_bom {
            raw.push('\u{FEFF}');
        }
        raw.push_str("[Script Info]");
        raw.push_str(nl);
        raw.push_str("Title: ");
        raw.push_str(&title);
        raw.push_str(nl);
        raw.push_str("ScriptType: v4.00+");
        raw.push_str(nl);
        raw.push_str(nl);
        raw.push_str("[V4+ Styles]");
        raw.push_str(nl);
        raw.push_str("Format: Name, Fontname, Fontsize, PrimaryColour, SecondaryColour, OutlineColour, BackColour, Bold, Italic, Underline, StrikeOut, ScaleX, ScaleY, Spacing, Angle, BorderStyle, Outline, Shadow, Alignment, MarginL, MarginR, MarginV, Encoding");
        raw.push_str(nl);
        raw.push_str("Style: Default,Arial,48,&H00FFFFFF,&H000000FF,&H00000000,&H00000000,0,0,0,0,100,100,0,0,1,2,2,2,10,10,10,1");
        raw.push_str(nl);
        raw.push_str(nl);
        raw.push_str("[Events]");
        raw.push_str(nl);
        raw.push_str("Format: Layer, Start, End, Style, Name, MarginL, MarginR, MarginV, Effect, Text");
        for (layer, start_cs, dur_cs, text) in events {
            let start = AssTime::from_cs(start_cs);
            let end = start.saturating_add_cs(dur_cs);
            raw.push_str(nl);
            let _ = write!(raw, "Dialogue: {layer},{start},{end},Default,,0,0,0,,{text}");
        }
        if trailing_nl {
            raw.push_str(nl);
        }

        let outcome = parse_ass(&raw);
        prop_assert!(outcome.warnings.is_empty(), "warnings: {:?}", outcome.warnings);
        let serialized = serialize_ass(&outcome.value);
        prop_assert_eq!(serialized, raw);
    }
}
