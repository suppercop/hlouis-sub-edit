//! Pure-logic missing-font and missing-glyph checker for ASS subtitle scripts.
//!
//! Collects every `(family, weight, italic)` font actually rendered by active `Dialogue` lines
//! (honoring styles in use, `\fn`, `\b`, `\i`, `\r`, and `\rStyleName`, while ignoring `Comment`
//! lines, unused styles, and `\p>0` vector drawings) along with the lines and glyphs per font.
//! Platform font queries (e.g. Windows DirectWrite) implement [`FontProvider`].

#![forbid(unsafe_code)]
#![cfg_attr(not(test), deny(clippy::unwrap_used, clippy::expect_used))]

use ass_core::{
    parse_text_blocks, AssStyle, EscapeKind, EventKind, OverrideItem, ScriptFile, TextBlock,
};
use std::collections::{BTreeMap, BTreeSet, HashMap};

/// Normal OpenType / CSS font weight.
pub const WEIGHT_NORMAL: u16 = 400;
/// Bold OpenType / CSS font weight.
pub const WEIGHT_BOLD: u16 = 700;

/// Canonical font identity (family + OpenType weight + italic flag).
#[derive(Debug, Clone, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub struct FontSpec {
    /// Font family name with any leading ASS `@` vertical-writing marker stripped.
    pub family: String,
    /// OpenType weight (`400` normal, `700` bold, or explicit `\b100..=900`).
    pub weight: u16,
    pub italic: bool,
}

impl FontSpec {
    /// Creates a normalized [`FontSpec`].
    #[must_use]
    pub fn new(family: &str, weight: u16, italic: bool) -> Self {
        let cleaned = family.trim().strip_prefix('@').unwrap_or(family.trim());
        Self {
            family: cleaned.to_owned(),
            weight,
            italic,
        }
    }

    /// Builds a [`FontSpec`] from an [`AssStyle`].
    #[must_use]
    pub fn from_style(style: &AssStyle) -> Self {
        Self::new(
            style.fontname(),
            ass_bold_to_weight(style.bold()),
            style.italic() != 0,
        )
    }
}

/// Aggregated usage of a single [`FontSpec`] across a script.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct FontUsage {
    pub spec: FontSpec,
    /// 0-based logical event indices where this font renders visible text.
    pub lines: BTreeSet<usize>,
    /// Distinct non-control characters rendered with this font.
    pub glyphs: BTreeSet<char>,
}

/// Missing-glyph diagnostic for an installed font.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct MissingGlyphReport {
    pub spec: FontSpec,
    pub missing: Vec<char>,
    pub lines: BTreeSet<usize>,
}

/// Explicit diagnostics so a font check never "succeeds" silently with nothing checked.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum FontCheckWarning {
    /// The script has no `[Events]` section, so no font could be checked.
    NoEventsSection,
    /// No active `Dialogue` line rendered any text, so no font could be checked.
    NoRenderedText,
    /// A dialogue line references a style that is not defined; the default style's font was assumed.
    UnknownStyle {
        /// 0-based logical event index.
        line: usize,
        style: String,
    },
}

impl std::fmt::Display for FontCheckWarning {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::NoEventsSection => {
                write!(
                    f,
                    "the script has no [Events] section; no fonts were checked"
                )
            }
            Self::NoRenderedText => write!(
                f,
                "no dialogue line contains visible text; no fonts were checked"
            ),
            Self::UnknownStyle { line, style } => write!(
                f,
                "line {} uses undefined style `{style}`; the default style's font was assumed",
                line + 1
            ),
        }
    }
}

/// Result of checking a script's used fonts against a [`FontProvider`].
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct FontCheckReport {
    /// All fonts used by the script.
    pub used_fonts: Vec<FontUsage>,
    /// Fonts not installed according to the provider.
    pub missing_fonts: Vec<FontUsage>,
    /// Installed fonts that are missing one or more glyphs required by the script.
    pub missing_glyphs: Vec<MissingGlyphReport>,
    /// Diagnostics the UI must show (empty script, undefined styles, ...).
    pub warnings: Vec<FontCheckWarning>,
}

impl FontCheckReport {
    /// `false` when nothing could be checked (see [`FontCheckReport::warnings`]); a report that
    /// is "clean" but not conclusive must not be shown as "all fonts OK".
    #[must_use]
    pub fn is_conclusive(&self) -> bool {
        !self.used_fonts.is_empty()
    }

    /// Returns `true` if every used font is installed and covers all required glyphs.
    #[must_use]
    pub const fn is_clean(&self) -> bool {
        self.missing_fonts.is_empty() && self.missing_glyphs.is_empty()
    }
}

/// Platform-agnostic font availability and glyph-coverage provider (implemented by DirectWrite on Windows).
pub trait FontProvider {
    /// Returns `true` if a font matching `spec` is installed.
    fn has_font(&self, spec: &FontSpec) -> bool;

    /// Returns any characters from `glyphs` not covered by the installed `spec`.
    fn missing_glyphs(&self, spec: &FontSpec, glyphs: &BTreeSet<char>) -> Vec<char>;
}

/// Collects every font actually used by active `Dialogue` lines in `script`, along with the
/// logical line indices and distinct glyphs rendered with each font.
#[must_use]
pub fn collect_used_fonts(script: &ScriptFile) -> Vec<FontUsage> {
    collect_used_fonts_with_warnings(script).0
}

/// Like [`collect_used_fonts`] but also returns the diagnostics (undefined styles, nothing to check).
#[must_use]
pub fn collect_used_fonts_with_warnings(
    script: &ScriptFile,
) -> (Vec<FontUsage>, Vec<FontCheckWarning>) {
    let mut warnings = Vec::new();
    let mut style_map: HashMap<&str, FontSpec> = HashMap::new();
    let mut default_spec = FontSpec::new("Arial", WEIGHT_NORMAL, false);

    if let Some(styles_sec) = script.styles_section() {
        for (i, st) in styles_sec.styles().enumerate() {
            let spec = FontSpec::from_style(st);
            if i == 0 || st.name().eq_ignore_ascii_case("Default") {
                default_spec = spec.clone();
            }
            style_map.insert(st.name(), spec);
        }
    }

    let Some(events_sec) = script.events_section() else {
        warnings.push(FontCheckWarning::NoEventsSection);
        return (Vec::new(), warnings);
    };

    let mut usage_map: BTreeMap<FontSpec, (BTreeSet<usize>, BTreeSet<char>)> = BTreeMap::new();

    for (line_idx, ev) in events_sec.events().enumerate() {
        if ev.kind() != EventKind::Dialogue {
            continue;
        }
        let text = ev.text();
        if text.is_empty() {
            continue;
        }

        let style_name = ev.style().strip_prefix('*').unwrap_or(ev.style());
        let base_spec = if let Some(spec) = style_map.get(style_name) {
            spec
        } else {
            warnings.push(FontCheckWarning::UnknownStyle {
                line: line_idx,
                style: style_name.to_owned(),
            });
            &default_spec
        };

        // Fast path: lines without `{` override blocks use the base style directly.
        if !text.contains('{') {
            record_plain_text(text, base_spec, line_idx, &mut usage_map);
            continue;
        }

        let mut active_spec = base_spec.clone();
        let blocks = parse_text_blocks(text);
        for block in blocks {
            match block {
                TextBlock::Override { items, .. } => {
                    for item in items {
                        let OverrideItem::Tag(tag) = item else {
                            continue;
                        };
                        apply_font_override(
                            &mut active_spec,
                            base_spec,
                            &style_map,
                            &tag.name,
                            &tag.raw_args,
                        );
                    }
                }
                TextBlock::Plain(s) => {
                    record_chars(s.chars(), &active_spec, line_idx, &mut usage_map);
                }
                TextBlock::Escape(EscapeKind::HardSpace) => {
                    record_chars(
                        std::iter::once('\u{00A0}'),
                        &active_spec,
                        line_idx,
                        &mut usage_map,
                    );
                }
                TextBlock::Escape(EscapeKind::HardNewline | EscapeKind::SoftNewline)
                | TextBlock::Drawing { .. } => {}
            }
        }
    }

    if usage_map.is_empty() {
        warnings.push(FontCheckWarning::NoRenderedText);
    }
    let usages = usage_map
        .into_iter()
        .map(|(spec, (lines, glyphs))| FontUsage {
            spec,
            lines,
            glyphs,
        })
        .collect();
    (usages, warnings)
}

/// Collects used fonts in `script` and checks availability and glyph coverage against `provider`.
#[must_use]
pub fn check_fonts(script: &ScriptFile, provider: &dyn FontProvider) -> FontCheckReport {
    let (used_fonts, warnings) = collect_used_fonts_with_warnings(script);
    let mut missing_fonts = Vec::new();
    let mut missing_glyphs = Vec::new();

    for usage in &used_fonts {
        if provider.has_font(&usage.spec) {
            let missing = provider.missing_glyphs(&usage.spec, &usage.glyphs);
            if !missing.is_empty() {
                missing_glyphs.push(MissingGlyphReport {
                    spec: usage.spec.clone(),
                    missing,
                    lines: usage.lines.clone(),
                });
            }
        } else {
            missing_fonts.push(usage.clone());
        }
    }

    FontCheckReport {
        used_fonts,
        missing_fonts,
        missing_glyphs,
        warnings,
    }
}

fn record_plain_text(
    text: &str,
    spec: &FontSpec,
    line_idx: usize,
    usage_map: &mut BTreeMap<FontSpec, (BTreeSet<usize>, BTreeSet<char>)>,
) {
    let mut chars = text.chars().peekable();
    let mut any_visible = false;
    let entry = usage_map.entry(spec.clone()).or_default();

    while let Some(ch) = chars.next() {
        if ch == '\\' {
            match chars.peek() {
                Some('N' | 'n') => {
                    let _ = chars.next();
                    continue;
                }
                Some('h') => {
                    let _ = chars.next();
                    entry.1.insert('\u{00A0}');
                    any_visible = true;
                    continue;
                }
                _ => {}
            }
        }
        if !ch.is_control() {
            entry.1.insert(ch);
            any_visible = true;
        }
    }

    if any_visible {
        entry.0.insert(line_idx);
    } else if entry.0.is_empty() && entry.1.is_empty() {
        usage_map.remove(spec);
    }
}

fn record_chars(
    chars: impl Iterator<Item = char>,
    spec: &FontSpec,
    line_idx: usize,
    usage_map: &mut BTreeMap<FontSpec, (BTreeSet<usize>, BTreeSet<char>)>,
) {
    let mut any = false;
    for ch in chars {
        if ch.is_control() {
            continue;
        }
        if !any {
            any = true;
            let entry = usage_map.entry(spec.clone()).or_default();
            entry.0.insert(line_idx);
            entry.1.insert(ch);
        } else if let Some(entry) = usage_map.get_mut(spec) {
            entry.1.insert(ch);
        }
    }
}

fn apply_font_override(
    active: &mut FontSpec,
    base: &FontSpec,
    styles: &HashMap<&str, FontSpec>,
    tag_name: &str,
    raw_args: &str,
) {
    let val = raw_args.trim();
    match tag_name {
        "fn" => {
            if val.is_empty() {
                active.family.clone_from(&base.family);
            } else {
                let cleaned = val.strip_prefix('@').unwrap_or(val);
                cleaned.clone_into(&mut active.family);
            }
        }
        "b" => {
            if val.is_empty() {
                active.weight = base.weight;
            } else if let Ok(v) = val.parse::<i32>() {
                active.weight = ass_bold_to_weight(v);
            }
        }
        "i" => {
            if val.is_empty() {
                active.italic = base.italic;
            } else if let Ok(v) = val.parse::<i32>() {
                active.italic = v != 0;
            }
        }
        "r" => {
            if val.is_empty() {
                active.clone_from(base);
            } else if let Some(target) = styles.get(val) {
                active.clone_from(target);
            } else {
                active.clone_from(base);
            }
        }
        _ => {}
    }
}

const fn ass_bold_to_weight(raw: i32) -> u16 {
    if raw == 0 {
        WEIGHT_NORMAL
    } else if raw == 1 || raw == -1 {
        WEIGHT_BOLD
    } else if raw >= 100 && raw <= 900 {
        #[allow(clippy::cast_possible_truncation, clippy::cast_sign_loss)]
        {
            raw as u16
        }
    } else {
        WEIGHT_BOLD
    }
}

/// In-memory mock [`FontProvider`] for unit and integration tests.
#[derive(Debug, Default, Clone)]
pub struct MockFontProvider {
    /// Map of installed fonts -> optional glyph coverage set (`None` = supports all glyphs).
    installed: HashMap<FontSpec, Option<BTreeSet<char>>>,
}

impl MockFontProvider {
    /// Registers an installed font that covers all glyphs.
    pub fn add_full_font(&mut self, spec: FontSpec) {
        self.installed.insert(spec, None);
    }

    /// Registers an installed font that only covers the characters in `supported_chars`.
    pub fn add_partial_font(&mut self, spec: FontSpec, supported_chars: &str) {
        self.installed
            .insert(spec, Some(supported_chars.chars().collect()));
    }
}

impl FontProvider for MockFontProvider {
    fn has_font(&self, spec: &FontSpec) -> bool {
        self.installed.contains_key(spec)
    }

    fn missing_glyphs(&self, spec: &FontSpec, glyphs: &BTreeSet<char>) -> Vec<char> {
        let Some(coverage) = self.installed.get(spec) else {
            return glyphs.iter().copied().collect();
        };
        let Some(supported) = coverage else {
            return Vec::new();
        };
        glyphs
            .iter()
            .copied()
            .filter(|ch| !ch.is_whitespace() && !supported.contains(ch))
            .collect()
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use ass_core::parse_ass;

    #[test]
    fn collects_styles_overrides_resets_and_ignores_comments_unused_styles_and_drawings() {
        let raw = "\
[Script Info]
ScriptType: v4.00+

[V4+ Styles]
Format: Name, Fontname, Fontsize, PrimaryColour, SecondaryColour, OutlineColour, BackColour, Bold, Italic, Underline, StrikeOut, ScaleX, ScaleY, Spacing, Angle, BorderStyle, Outline, Shadow, Alignment, MarginL, MarginR, MarginV, Encoding
Style: Default,Segoe UI,48,&H00FFFFFF,&H0000FFFF,&H00000000,&H80000000,0,0,0,0,100,100,0,0,1,2,1,2,10,10,20,1
Style: Sign,Yu Gothic,60,&H00FFFFFF,&H0000FFFF,&H00000000,&H80000000,-1,1,0,0,100,100,0,0,1,2,1,2,10,10,20,1
Style: UnusedStyle,Comic Sans MS,40,&H00FFFFFF,&H0000FFFF,&H00000000,&H80000000,0,0,0,0,100,100,0,0,1,2,1,2,10,10,20,1

[Events]
Format: Layer, Start, End, Style, Name, MarginL, MarginR, MarginV, Effect, Text
Comment: 0,0:00:00.00,0:00:01.00,UnusedStyle,,0,0,0,,{\\fnIgnoredCommentFont}Should be ignored
Dialogue: 0,0:00:01.00,0:00:03.00,Default,,0,0,0,,Hello {\\b1}Bold{\\b} {\\fnRoboto\\i1}Ital{\\rSign}日本{\\r}Back
Dialogue: 0,0:00:03.00,0:00:05.00,Default,,0,0,0,,{\\fnEmptySwitch}{\\fn@Meiryo\\p1}m 0 0 l 10 10{\\p0}語
";
        let script = parse_ass(raw).value;
        let used = collect_used_fonts(&script);
        let specs: Vec<&FontSpec> = used.iter().map(|u| &u.spec).collect();

        // UnusedStyle, IgnoredCommentFont, and EmptySwitch must NOT appear.
        assert!(!specs.iter().any(|s| s.family == "Comic Sans MS"));
        assert!(!specs.iter().any(|s| s.family == "IgnoredCommentFont"));
        assert!(!specs.iter().any(|s| s.family == "EmptySwitch"));

        let segoe_reg = FontSpec::new("Segoe UI", WEIGHT_NORMAL, false);
        let segoe_bold = FontSpec::new("Segoe UI", WEIGHT_BOLD, false);
        let roboto_ital = FontSpec::new("Roboto", WEIGHT_NORMAL, true);
        let yugoth_bi = FontSpec::new("Yu Gothic", WEIGHT_BOLD, true);
        let meiryo = FontSpec::new("Meiryo", WEIGHT_NORMAL, false);

        assert!(specs.contains(&&segoe_reg));
        assert!(specs.contains(&&segoe_bold));
        assert!(specs.contains(&&roboto_ital));
        assert!(specs.contains(&&yugoth_bi));
        assert!(specs.contains(&&meiryo));

        // Drawing coordinates ("m 0 0 l 10 10") must NOT pollute Meiryo's glyph set.
        let meiryo_usage = used.iter().find(|u| u.spec == meiryo).unwrap();
        assert_eq!(meiryo_usage.glyphs, BTreeSet::from(['語']));
        assert_eq!(meiryo_usage.lines, BTreeSet::from([2]));

        // Check against MockFontProvider.
        let mut provider = MockFontProvider::default();
        provider.add_full_font(segoe_reg);
        provider.add_full_font(segoe_bold);
        provider.add_partial_font(yugoth_bi.clone(), "日"); // missing '本'
        provider.add_full_font(meiryo);

        let report = check_fonts(&script, &provider);
        assert!(!report.is_clean());
        assert_eq!(report.missing_fonts.len(), 1);
        assert_eq!(report.missing_fonts[0].spec, roboto_ital);
        assert_eq!(report.missing_glyphs.len(), 1);
        assert_eq!(report.missing_glyphs[0].spec, yugoth_bi);
        assert_eq!(report.missing_glyphs[0].missing, vec!['本']);
    }

    #[test]
    fn empty_script_reports_that_nothing_was_checked() {
        let script = parse_ass(
            "[Script Info]
ScriptType: v4.00+
",
        )
        .value;
        let report = check_fonts(&script, &MockFontProvider::default());
        assert!(report.is_clean());
        assert!(!report.is_conclusive());
        assert!(report.warnings.contains(&FontCheckWarning::NoEventsSection));
    }

    #[test]
    fn comment_only_script_reports_no_rendered_text() {
        let raw = "[Events]
Format: Layer, Start, End, Style, Name, MarginL, MarginR, MarginV, Effect, Text
Comment: 0,0:00:00.00,0:00:01.00,Default,,0,0,0,,note
";
        let script = parse_ass(raw).value;
        let report = check_fonts(&script, &MockFontProvider::default());
        assert!(!report.is_conclusive());
        assert!(report.warnings.contains(&FontCheckWarning::NoRenderedText));
        assert!(report.warnings[0]
            .to_string()
            .contains("no fonts were checked"));
    }

    #[test]
    fn undefined_style_is_reported() {
        let raw = "[Events]
Format: Layer, Start, End, Style, Name, MarginL, MarginR, MarginV, Effect, Text
Dialogue: 0,0:00:00.00,0:00:01.00,Ghost,,0,0,0,,hi
";
        let script = parse_ass(raw).value;
        let report = check_fonts(&script, &MockFontProvider::default());
        assert!(report.warnings.iter().any(|w| matches!(
            w,
            FontCheckWarning::UnknownStyle { line: 0, style } if style == "Ghost"
        )));
    }
}
