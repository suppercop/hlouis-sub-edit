//! Pure-Rust data preprocessing for Aegisub 3.5.1 `karaskel.lua` & `kara-templater.lua`
//! (`automation/include/karaskel.lua`, `automation/autoload/kara-templater.lua`).
//!
//! Computes script metadata, style tables, line geometry (`left`/`center`/`right`/`x`/`y`),
//! karaoke syllable metrics (`start_time`/`end_time`/`duration`/`inline_fx`/`highlights`),
//! furigana splits (`base|reading`), and full `kara-templater` template/code line classification
//! ready for a Lua runtime to consume.

#![forbid(unsafe_code)]
#![cfg_attr(not(test), deny(clippy::unwrap_used, clippy::expect_used))]

use ass_core::{AssEvent, AssStyle, EventKind, ScriptFile};
use std::collections::HashMap;

/// Font measurement output matching Aegisub `aegisub.text_extents`.
#[derive(Debug, Clone, Copy, Default, PartialEq)]
pub struct TextMetric {
    pub width: f64,
    pub height: f64,
    pub descent: f64,
    pub ext_lead: f64,
}

/// Font measurement provider (implemented by DirectWrite/FreeType in the app, mocked in tests).
pub trait TextExtents {
    /// Measures `text` rendered in `style`.
    fn extents(&self, style: &AssStyle, text: &str) -> TextMetric;
}

/// Script header metadata (`karaskel.collect_head`).
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct ScriptMeta {
    pub res_x: f64,
    pub res_y: f64,
    pub video_x_correct_factor: f64,
}

/// One multi-highlight timing slice within a syllable (`syl.highlights`).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Highlight {
    pub start_time: i64,
    pub end_time: i64,
    pub duration: i64,
}

/// Furigana entry parsed from `kanji|reading` (`karaskel` `generate_furigana`).
#[derive(Debug, Clone, PartialEq)]
pub struct FuriganaEntry {
    pub text: String,
    pub start_time: i64,
    pub end_time: i64,
    pub duration: i64,
    pub width: f64,
    pub height: f64,
    pub left: f64,
    pub center: f64,
    pub right: f64,
}

/// Preprocessed karaoke syllable (`syl` table in `karaskel.lua`).
#[derive(Debug, Clone, PartialEq)]
pub struct KaraskelSyllable {
    /// 1-based syllable index (matching Lua `syl.i`).
    pub i: usize,
    /// Relative start time in ms from line start.
    pub start_time: i64,
    /// Relative end time in ms from line start.
    pub end_time: i64,
    /// Duration in ms.
    pub duration: i64,
    /// Karaoke tag kind (`"k"`, `"kf"`, `"ko"`, `"K"`, or `""`).
    pub tag: &'static str,
    /// Raw syllable text (stripped of `{...}`).
    pub text_stripped: String,
    /// Inline effect name from `{\-fxname}`.
    pub inline_fx: String,
    pub width: f64,
    pub height: f64,
    pub left: f64,
    pub center: f64,
    pub right: f64,
    pub highlights: Vec<Highlight>,
    pub furigana: Vec<FuriganaEntry>,
}

/// Preprocessed karaoke line (`line` table in `karaskel.lua`).
#[derive(Debug, Clone, PartialEq)]
pub struct PreprocessedLine {
    pub line_index: usize,
    pub start_ms: i64,
    pub end_ms: i64,
    pub duration_ms: i64,
    pub style_name: String,
    pub text_stripped: String,
    pub width: f64,
    pub height: f64,
    pub descent: f64,
    pub ext_lead: f64,
    pub left: f64,
    pub center: f64,
    pub right: f64,
    pub top: f64,
    pub middle: f64,
    pub bottom: f64,
    pub x: f64,
    pub y: f64,
    pub syllables: Vec<KaraskelSyllable>,
}

/// `kara-templater.lua` template/code kind.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum TemplateKind {
    Code,
    Template,
}

/// `kara-templater.lua` execution class.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum TemplateClass {
    Once,
    Line,
    Syl,
    Char,
    Furi,
    PreLine,
}

/// Modifiers parsed from a `kara-templater` `Effect` field (`parse_templates` in
/// `automation/autoload/kara-templater.lua`).
#[derive(Debug, Clone, Default, PartialEq, Eq)]
#[allow(clippy::struct_excessive_bools)]
pub struct TemplateModifiers {
    pub all: bool,
    pub keeptags: bool,
    pub multi: bool,
    pub noblank: bool,
    pub notext: bool,
    pub char_mode: bool,
    pub furi_mode: bool,
    pub loop_count: Option<u32>,
    pub repeat_count: Option<u32>,
    pub fx: Option<String>,
    pub fxgroup: Option<String>,
}

/// Classified template or code line for `kara-templater`.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct TemplateLine {
    pub line_index: usize,
    pub kind: TemplateKind,
    pub class: TemplateClass,
    pub style: String,
    pub modifiers: TemplateModifiers,
    pub body: String,
}

/// Complete output of karaskel preprocessing for a script.
#[derive(Debug, Clone)]
pub struct KaraskelOutput {
    pub meta: ScriptMeta,
    pub lines: Vec<PreprocessedLine>,
    pub templates: Vec<TemplateLine>,
}

/// Extracts [`ScriptMeta`] (`karaskel.collect_head`).
#[must_use]
pub fn collect_head(script: &ScriptFile, video_aspect_override: Option<f64>) -> ScriptMeta {
    let mut res_x = 384.0_f64;
    let mut res_y = 288.0_f64;
    if let Some(info) = script.script_info() {
        if let Some(x) = info
            .get("PlayResX")
            .and_then(|s| s.trim().parse::<f64>().ok())
        {
            if x > 0.0 {
                res_x = x;
            }
        }
        if let Some(y) = info
            .get("PlayResY")
            .and_then(|s| s.trim().parse::<f64>().ok())
        {
            if y > 0.0 {
                res_y = y;
            }
        }
    }
    let script_aspect = res_x / res_y.max(1.0);
    let video_x_correct_factor =
        video_aspect_override.map_or(1.0, |va| (va / script_aspect).max(0.1));
    ScriptMeta {
        res_x,
        res_y,
        video_x_correct_factor,
    }
}

/// Classifies a `Comment` line as a `kara-templater.lua` `template` or `code` line.
#[must_use]
pub fn classify_template_line(line_index: usize, event: &AssEvent) -> Option<TemplateLine> {
    if event.kind() != EventKind::Comment {
        return None;
    }
    let mut tokens = event.effect().split_whitespace();
    let first = tokens.next()?;
    let kind = if first.eq_ignore_ascii_case("template") {
        TemplateKind::Template
    } else if first.eq_ignore_ascii_case("code") {
        TemplateKind::Code
    } else {
        return None;
    };

    let mut class = match kind {
        TemplateKind::Code => TemplateClass::Once,
        TemplateKind::Template => TemplateClass::Line,
    };
    let mut mods = TemplateModifiers::default();

    while let Some(tok) = tokens.next() {
        match tok.to_ascii_lowercase().as_str() {
            "once" => class = TemplateClass::Once,
            "line" => class = TemplateClass::Line,
            "syl" => class = TemplateClass::Syl,
            "char" => {
                mods.char_mode = true;
                if class == TemplateClass::Line {
                    class = TemplateClass::Char;
                }
            }
            "furi" => {
                mods.furi_mode = true;
                if class == TemplateClass::Line {
                    class = TemplateClass::Furi;
                }
            }
            "pre-line" => class = TemplateClass::PreLine,
            "all" => mods.all = true,
            "keeptags" => mods.keeptags = true,
            "multi" => mods.multi = true,
            "noblank" => mods.noblank = true,
            "notext" => mods.notext = true,
            "loop" => {
                mods.loop_count = tokens.next().and_then(|n| n.parse::<u32>().ok());
            }
            "repeat" => {
                mods.repeat_count = tokens.next().and_then(|n| n.parse::<u32>().ok());
            }
            "fx" => {
                mods.fx = tokens.next().map(ToOwned::to_owned);
            }
            "fxgroup" => {
                mods.fxgroup = tokens.next().map(ToOwned::to_owned);
            }
            _ => {}
        }
    }

    Some(TemplateLine {
        line_index,
        kind,
        class,
        style: event.style().to_owned(),
        modifiers: mods,
        body: event.text().to_owned(),
    })
}

/// Preprocesses an entire [`ScriptFile`] in one pass (collects head, classifies templates, and
/// computes karaskel line/syllable/furigana geometry for all dialogue lines).
#[must_use]
pub fn preprocess_script(
    script: &ScriptFile,
    extents: &dyn TextExtents,
    generate_furigana: bool,
) -> KaraskelOutput {
    let meta = collect_head(script, None);
    let styles: Vec<&AssStyle> = script.styles().collect();
    let mut style_map: HashMap<&str, &AssStyle> = HashMap::with_capacity(styles.len());
    for st in &styles {
        style_map.insert(st.name(), *st);
    }
    let fallback_style = styles.first().copied();

    let mut lines = Vec::new();
    let mut templates = Vec::new();

    for (idx, ev) in script.events().enumerate() {
        if let Some(tpl) = classify_template_line(idx, ev) {
            templates.push(tpl);
            continue;
        }
        if ev.kind() != EventKind::Dialogue {
            continue;
        }
        if let Some(st) = style_map.get(ev.style()).copied().or(fallback_style) {
            lines.push(preprocess_line(
                idx,
                ev,
                st,
                meta,
                extents,
                generate_furigana,
            ));
        }
    }

    KaraskelOutput {
        meta,
        lines,
        templates,
    }
}

/// Preprocesses a single dialogue line into a [`PreprocessedLine`].
#[must_use]
pub fn preprocess_line(
    line_index: usize,
    event: &AssEvent,
    style: &AssStyle,
    meta: ScriptMeta,
    extents: &dyn TextExtents,
    generate_furigana: bool,
) -> PreprocessedLine {
    let start_ms = event.start().as_ms().cast_signed();
    let end_ms = event.end().as_ms().cast_signed();
    let duration_ms = (end_ms - start_ms).max(0);

    let mut raw_syls = parse_karaskel_syllables(event.text(), generate_furigana);
    let mut text_stripped = String::with_capacity(event.text().len());
    for s in &raw_syls {
        text_stripped.push_str(&s.text_stripped);
    }

    let lm = extents.extents(style, &text_stripped);
    let margin_left = f64::from(if event.margin_l() > 0 {
        event.margin_l()
    } else {
        style.margin_l()
    });
    let margin_right = f64::from(if event.margin_r() > 0 {
        event.margin_r()
    } else {
        style.margin_r()
    });
    let margin_vert = f64::from(if event.margin_v() > 0 {
        event.margin_v()
    } else {
        style.margin_v()
    });
    let an = style.alignment().clamp(1, 9);

    let (left, center, right, x) = match an {
        1 | 4 | 7 => (
            margin_left,
            margin_left + lm.width * 0.5,
            margin_left + lm.width,
            margin_left,
        ),
        3 | 6 | 9 => {
            let r = meta.res_x - margin_right;
            (r - lm.width, r - lm.width * 0.5, r, r)
        }
        _ => {
            let c = f64::midpoint(margin_left, meta.res_x - margin_right);
            (c - lm.width * 0.5, c, c + lm.width * 0.5, c)
        }
    };
    let (top, middle, bottom, y) = match an {
        7..=9 => (
            margin_vert,
            margin_vert + lm.height * 0.5,
            margin_vert + lm.height,
            margin_vert,
        ),
        4..=6 => {
            let m = meta.res_y * 0.5;
            (m - lm.height * 0.5, m, m + lm.height * 0.5, m)
        }
        _ => {
            let b = meta.res_y - margin_vert;
            (b - lm.height, b - lm.height * 0.5, b, b)
        }
    };

    // Lay out syllable X coordinates horizontally from `left`.
    let mut cursor_x = left;
    for syl in &mut raw_syls {
        let sm = extents.extents(style, &syl.text_stripped);
        syl.width = sm.width;
        syl.height = sm.height;
        syl.left = cursor_x;
        syl.center = cursor_x + sm.width * 0.5;
        syl.right = cursor_x + sm.width;
        for f in &mut syl.furigana {
            let fm = extents.extents(style, &f.text);
            f.width = fm.width * 0.5;
            f.height = fm.height * 0.5;
            f.center = syl.center;
            f.left = f.center - f.width * 0.5;
            f.right = f.center + f.width * 0.5;
        }
        cursor_x += sm.width;
    }

    PreprocessedLine {
        line_index,
        start_ms,
        end_ms,
        duration_ms,
        style_name: event.style().to_owned(),
        text_stripped,
        width: lm.width,
        height: lm.height,
        descent: lm.descent,
        ext_lead: lm.ext_lead,
        left,
        center,
        right,
        top,
        middle,
        bottom,
        x,
        y,
        syllables: raw_syls,
    }
}

/// Fast single-pass syllable scanner extracting `\k`/`\kf`/`\ko`/`\K`, `{\-inline_fx}`,
/// multi-highlights, and `kanji|furi` splits.
#[allow(clippy::too_many_lines)]
fn parse_karaskel_syllables(text: &str, generate_furigana: bool) -> Vec<KaraskelSyllable> {
    let mut out = Vec::with_capacity(24);
    let bytes = text.as_bytes();
    let len = bytes.len();
    let mut pos = 0usize;
    let mut cur_ms = 0i64;
    let mut cur_fx = String::new();
    let mut cur_tag: &'static str = "";
    let mut cur_dur_ms = 0i64;
    let mut cur_highlights: Vec<Highlight> = Vec::new();
    let mut cur_text = String::new();
    let mut have_started = false;

    let flush_syl = |out: &mut Vec<KaraskelSyllable>,
                     cur_ms: &mut i64,
                     cur_dur_ms: &mut i64,
                     cur_tag: &'static str,
                     cur_fx: &str,
                     cur_highlights: &mut Vec<Highlight>,
                     cur_text: &mut String| {
        let start_time = *cur_ms;
        let end_time = start_time + *cur_dur_ms;
        let mut furi = Vec::new();
        let display_text = if generate_furigana {
            if let Some((base, reading)) = cur_text.split_once('|') {
                if !reading.is_empty() {
                    furi.push(FuriganaEntry {
                        text: reading.to_owned(),
                        start_time,
                        end_time,
                        duration: *cur_dur_ms,
                        width: 0.0,
                        height: 0.0,
                        left: 0.0,
                        center: 0.0,
                        right: 0.0,
                    });
                }
                base.to_owned()
            } else {
                std::mem::take(cur_text)
            }
        } else {
            std::mem::take(cur_text)
        };
        if cur_highlights.is_empty() && *cur_dur_ms > 0 {
            cur_highlights.push(Highlight {
                start_time,
                end_time,
                duration: *cur_dur_ms,
            });
        }
        let idx = out.len() + 1;
        out.push(KaraskelSyllable {
            i: idx,
            start_time,
            end_time,
            duration: *cur_dur_ms,
            tag: cur_tag,
            text_stripped: display_text,
            inline_fx: cur_fx.to_owned(),
            width: 0.0,
            height: 0.0,
            left: 0.0,
            center: 0.0,
            right: 0.0,
            highlights: std::mem::take(cur_highlights),
            furigana: furi,
        });
        *cur_ms = end_time;
        *cur_dur_ms = 0;
        cur_text.clear();
    };

    while pos < len {
        if bytes[pos] == b'{' {
            if let Some(rel_close) = bytes[pos + 1..].iter().position(|&c| c == b'}') {
                let block = &text[pos + 1..pos + 1 + rel_close];
                for part in block.split('\\').skip(1) {
                    if let Some(fx_name) = part.strip_prefix('-') {
                        fx_name.clone_into(&mut cur_fx);
                        continue;
                    }
                    if let Some((tag, cs)) = parse_k_tag(part) {
                        if have_started && !cur_text.is_empty() {
                            flush_syl(
                                &mut out,
                                &mut cur_ms,
                                &mut cur_dur_ms,
                                cur_tag,
                                &cur_fx,
                                &mut cur_highlights,
                                &mut cur_text,
                            );
                        }
                        let dur = i64::from(cs) * 10;
                        let hl_start = cur_ms + cur_dur_ms;
                        cur_highlights.push(Highlight {
                            start_time: hl_start,
                            end_time: hl_start + dur,
                            duration: dur,
                        });
                        cur_dur_ms += dur;
                        cur_tag = tag;
                        have_started = true;
                    }
                }
                pos += rel_close + 2;
                continue;
            }
        }
        let next_open = bytes[pos..]
            .iter()
            .position(|&c| c == b'{')
            .map_or(len, |r| pos + r);
        cur_text.push_str(&text[pos..next_open]);
        have_started = true;
        pos = next_open;
    }

    if have_started && (!cur_text.is_empty() || cur_dur_ms > 0) {
        flush_syl(
            &mut out,
            &mut cur_ms,
            &mut cur_dur_ms,
            cur_tag,
            &cur_fx,
            &mut cur_highlights,
            &mut cur_text,
        );
    }

    out
}

#[inline]
fn parse_k_tag(part: &str) -> Option<(&'static str, u32)> {
    for (prefix, canon) in [("kf", "kf"), ("ko", "ko"), ("k", "k"), ("K", "K")] {
        if let Some(rest) = part.strip_prefix(prefix) {
            let digits_end = rest
                .as_bytes()
                .iter()
                .take_while(|b| b.is_ascii_digit())
                .count();
            if digits_end > 0 {
                let cs = rest[..digits_end].parse::<u32>().ok()?;
                return Some((canon, cs));
            }
        }
    }
    None
}

#[cfg(test)]
mod tests {
    use super::*;

    struct MonoExtents;
    impl TextExtents for MonoExtents {
        fn extents(&self, _style: &AssStyle, text: &str) -> TextMetric {
            #[allow(clippy::cast_precision_loss)]
            let w = (text.chars().count() as f64) * 10.0;
            TextMetric {
                width: w,
                height: 24.0,
                descent: 4.0,
                ext_lead: 0.0,
            }
        }
    }

    #[test]
    fn preprocesses_syllables_inline_fx_furigana_and_templates() {
        let raw = "\
[Script Info]
PlayResX: 1920
PlayResY: 1080

[V4+ Styles]
Format: Name, Fontname, Fontsize, PrimaryColour, SecondaryColour, OutlineColour, BackColour, Bold, Italic, Underline, StrikeOut, ScaleX, ScaleY, Spacing, Angle, BorderStyle, Outline, Shadow, Alignment, MarginL, MarginR, MarginV, Encoding
Style: Romaji,Arial,48,&H00FFFFFF,&H000000FF,&H00000000,&H00000000,0,0,0,0,100,100,0,0,1,2,2,2,20,20,30,1

[Events]
Format: Layer, Start, End, Style, Name, MarginL, MarginR, MarginV, Effect, Text
Comment: 0,0:00:00.00,0:00:00.00,Romaji,,0,0,0,template syl noblank fx glow loop 3,{\\pos($center,$middle)}
Dialogue: 0,0:00:01.00,0:00:03.00,Romaji,,0,0,0,,{\\k50\\-glow}夢|ゆめ{\\kf50}{\\kf50}wo
";
        let script = ass_core::parse_ass(raw).value;
        let out = preprocess_script(&script, &MonoExtents, true);

        assert_eq!(out.meta.res_x, 1920.0);
        assert_eq!(out.templates.len(), 1);
        let tpl = &out.templates[0];
        assert_eq!(tpl.kind, TemplateKind::Template);
        assert_eq!(tpl.class, TemplateClass::Syl);
        assert!(tpl.modifiers.noblank);
        assert_eq!(tpl.modifiers.fx.as_deref(), Some("glow"));
        assert_eq!(tpl.modifiers.loop_count, Some(3));

        assert_eq!(out.lines.len(), 1);
        let line = &out.lines[0];
        assert_eq!(line.syllables.len(), 2);
        assert_eq!(line.syllables[0].text_stripped, "夢");
        assert_eq!(line.syllables[0].inline_fx, "glow");
        assert_eq!(line.syllables[0].furigana[0].text, "ゆめ");
        assert_eq!(line.syllables[1].highlights.len(), 2);
        assert_eq!(line.syllables[1].duration, 1000);
    }
}
