//! Fuzzing entrypoints for every parser in `hlouis-sub-edit`.
//!
//! Each function accepts arbitrary untrusted bytes and must never panic, abort, or cause
//! unbounded work regardless of input.

#![forbid(unsafe_code)]
#![cfg_attr(not(test), deny(clippy::unwrap_used, clippy::expect_used))]

use std::io::Cursor;

use aegisub_config::{import_config_json, import_hotkey_json};
use ass_core::{
    parse_ass_bytes, parse_srt_bytes, parse_text_blocks, serialize_ass, serialize_text_blocks,
    write_srt,
};
use karaskel::{TextExtents, TextMetric, classify_template_line, collect_head, preprocess_script};
use keyframes::{parse_keyframes, parse_timecodes};
use mkv_subs::{extract_tracks, list_tracks};
use project::{read_project_links, write_project_links};
use settings::AppSettings;
use styles_catalog::StyleCatalog;
use syntax_highlight::{GridDisplayConfig, IncrementalTokenizer, format_grid_text_into, tokenize};
use template_doctor::{fix_all_safe, lint_script};
use visual_tools::{parse_move_tag, upsert_leading_tag, vector_clip::VectorClipPath};

/// Fuzzes ASS parser, charset detection, SRT parser, override tag parser, and round-trips.
pub fn fuzz_ass_core(data: &[u8]) {
    let (ass_out, _enc) = parse_ass_bytes(data);
    let script = ass_out.value;
    let serialized = serialize_ass(&script);
    let _ = parse_ass_bytes(serialized.as_bytes());
    let _ = write_srt(&script);

    let (srt_out, _) = parse_srt_bytes(data);
    let _ = serialize_ass(&srt_out.value);

    if let Ok(s) = std::str::from_utf8(data) {
        let blocks = parse_text_blocks(s);
        let _ = serialize_text_blocks(&blocks);
    }
}

/// Fuzzes EBML / Matroska track listing and single-pass subtitle extraction.
pub fn fuzz_mkv_subs(data: &[u8]) {
    let mut cursor = Cursor::new(data);
    if let Ok(tracks) = list_tracks(&mut cursor) {
        let nums: Vec<u64> = tracks.iter().take(4).map(|t| t.number).collect();
        if !nums.is_empty() {
            let _ = extract_tracks(Cursor::new(data), &nums, None, |_, _| {});
        }
    }
}

/// Fuzzes keyframe files and timecode v1/v2 parsers plus round-trip writers.
pub fn fuzz_keyframes(data: &[u8]) {
    let s = String::from_utf8_lossy(data);
    if let Ok((kfs, _fmt)) = parse_keyframes(&s) {
        let _ = keyframes::write_aegisub_keyframes(&kfs, Some(23.976));
        let _ = keyframes::write_plain_keyframes(&kfs);
    }
    if let Ok(tc) = parse_timecodes(&s) {
        let _ = tc.write_v1();
        let _ = tc.write_v2(8);
    }
}

/// Fuzzes Aegisub `config.json` and `hotkey.json` importers.
pub fn fuzz_aegisub_config(data: &[u8]) {
    let s = String::from_utf8_lossy(data);
    let _ = import_config_json(&s);
    let _ = import_hotkey_json(&s);
}

/// Fuzzes `AppSettings` JSON loading, migration, clamping, and serialization round-trip.
pub fn fuzz_settings(data: &[u8]) {
    let s = String::from_utf8_lossy(data);
    if let Ok(mut cfg) = AppSettings::load_json(&s) {
        cfg.migrate();
        cfg.validate_and_clamp();
        let _ = cfg.save_json();
    }
}

/// Fuzzes `[Aegisub Project Garbage]` link reading and writing.
pub fn fuzz_project(data: &[u8]) {
    let (out, _) = parse_ass_bytes(data);
    let mut script = out.value;
    let links = read_project_links(&script);
    write_project_links(&mut script, &links);
}

/// Fuzzes `.sty` style catalog parsing and serialization round-trip.
pub fn fuzz_styles_catalog(data: &[u8]) {
    let s = String::from_utf8_lossy(data);
    let cat = StyleCatalog::parse_sty("fuzz", &s);
    let ser = cat.serialize_sty();
    let _ = StyleCatalog::parse_sty("fuzz2", &ser);
}

struct FuzzExtents;

impl TextExtents for FuzzExtents {
    fn extents(&self, _style: &ass_core::AssStyle, text: &str) -> TextMetric {
        #[allow(clippy::cast_precision_loss)]
        let width = (text.len() as f64) * 12.0;
        TextMetric {
            width,
            height: 24.0,
            descent: 4.0,
            ext_lead: 0.0,
        }
    }
}

/// Fuzzes karaskel template classification, head collection, and script preprocessing.
pub fn fuzz_karaskel(data: &[u8]) {
    let (out, _) = parse_ass_bytes(data);
    let script = out.value;
    let _ = collect_head(&script, None);
    for (idx, ev) in script.events().enumerate() {
        let _ = classify_template_line(idx, ev);
    }
    let _ = preprocess_script(&script, &FuzzExtents, true);
}

/// Fuzzes ASS override syntax highlighter, incremental cache edit, and grid text formatter.
pub fn fuzz_syntax_highlight(data: &[u8]) {
    let s = String::from_utf8_lossy(data);
    let _ = tokenize(&s);
    let mut cache = IncrementalTokenizer::new();
    cache.set_text(&s);
    let mid = s.len() / 2;
    let mut edited = String::with_capacity(s.len() + 5);
    let split = s.floor_char_boundary(mid);
    edited.push_str(&s[..split]);
    edited.push_str(r"{\b1}");
    edited.push_str(&s[split..]);
    cache.re_tokenize_edit(&edited, split, split, split + 5);
    let mut out = String::new();
    format_grid_text_into(&s, &GridDisplayConfig::default(), &mut out);
}

/// Fuzzes vector clip parser, spline/bezier conversions, tag writer, and `\move` / tag upsert.
pub fn fuzz_visual_tools(data: &[u8]) {
    let s = String::from_utf8_lossy(data);
    let mut clip = VectorClipPath::parse(&s, false);
    let mut out = String::new();
    clip.write_tag_into(&mut out);
    if !clip.vertices.is_empty() {
        let _ = clip.convert_segment_at(0);
        let _ = clip.convert_bezier_to_spline_at(0);
        let _ = clip.convert_spline_to_bezier_at(0);
    }
    let _ = parse_move_tag(&s);
    let _ = upsert_leading_tag(&s, "pos", r"\pos(100,200)");
}

/// Fuzzes `template-doctor` ASS + Lua 5.1 linter and idempotent `fix_all_safe`.
pub fn fuzz_template_doctor(data: &[u8]) {
    let (out, _) = parse_ass_bytes(data);
    let script = out.value;
    let _ = lint_script(&script);
    let _ = fix_all_safe(&script);
}
