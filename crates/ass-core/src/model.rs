//! Lossless ASS/SSA document data model preserving section order, unknown sections,
//! `[Fonts]`/`[Graphics]` verbatim payloads, custom `Format:` orders, mixed line endings,
//! and original numeric formatting via private `raw_line` caches invalidated by setters.

use crate::time::AssTime;

/// Default V4+ Styles `Format:` column order as written by Aegisub.
pub const DEFAULT_STYLE_FORMAT: &[&str] = &[
    "Name",
    "Fontname",
    "Fontsize",
    "PrimaryColour",
    "SecondaryColour",
    "OutlineColour",
    "BackColour",
    "Bold",
    "Italic",
    "Underline",
    "StrikeOut",
    "ScaleX",
    "ScaleY",
    "Spacing",
    "Angle",
    "BorderStyle",
    "Outline",
    "Shadow",
    "Alignment",
    "MarginL",
    "MarginR",
    "MarginV",
    "Encoding",
];

/// Default Events `Format:` column order as written by Aegisub.
pub const DEFAULT_EVENT_FORMAT: &[&str] = &[
    "Layer", "Start", "End", "Style", "Name", "MarginL", "MarginR", "MarginV", "Effect", "Text",
];

/// Line-ending convention preserved across parse/serialize round-trips.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum LineEnding {
    /// Windows `\r\n`.
    #[default]
    CrLf,
    /// Unix `\n`.
    Lf,
}

impl LineEnding {
    /// Returns the string representation (`"\r\n"` or `"\n"`).
    #[must_use]
    pub const fn as_str(self) -> &'static str {
        match self {
            Self::CrLf => "\r\n",
            Self::Lf => "\n",
        }
    }
}

/// Complete ASS subtitle file model.
#[derive(Debug, Clone, PartialEq)]
pub struct ScriptFile {
    /// Whether the file began with a UTF-8 BOM (`\u{FEFF}`).
    pub has_bom: bool,
    /// Dominant line-ending convention detected during parsing (used for uniform files and new lines).
    pub line_ending: LineEnding,
    /// Per-line terminators populated *only* when the input mixes `\r\n` and `\n` (stays `None`
    /// on uniform files for zero-overhead fast-path parsing and serialization).
    pub line_endings: Option<Vec<LineEnding>>,
    /// Whether the original file ended with a trailing newline.
    pub trailing_newline: bool,
    /// Ordered list of sections (including preamble, known sections, and unknown sections).
    pub sections: Vec<Section>,
}

impl Default for ScriptFile {
    fn default() -> Self {
        Self::new_default()
    }
}

impl ScriptFile {
    /// Creates a minimal valid V4+ ASS script with `[Script Info]`, `[V4+ Styles]`, and `[Events]`.
    #[must_use]
    pub fn new_default() -> Self {
        let script_info = KeyValueSection {
            header: "[Script Info]".to_owned(),
            entries: vec![
                KeyValueEntry::KeyValue {
                    key: "Title".to_owned(),
                    separator: ": ".to_owned(),
                    value: "Default Aegisub file".to_owned(),
                },
                KeyValueEntry::KeyValue {
                    key: "ScriptType".to_owned(),
                    separator: ": ".to_owned(),
                    value: "v4.00+".to_owned(),
                },
                KeyValueEntry::KeyValue {
                    key: "PlayResX".to_owned(),
                    separator: ": ".to_owned(),
                    value: "1920".to_owned(),
                },
                KeyValueEntry::KeyValue {
                    key: "PlayResY".to_owned(),
                    separator: ": ".to_owned(),
                    value: "1080".to_owned(),
                },
                KeyValueEntry::Blank,
            ],
        };
        let styles = StylesSection {
            header: "[V4+ Styles]".to_owned(),
            format: DEFAULT_STYLE_FORMAT
                .iter()
                .map(|s| (*s).to_owned())
                .collect(),
            raw_format_line: None,
            entries: vec![StyleEntry::Style(Box::default()), StyleEntry::Blank],
        };
        let events = EventsSection {
            header: "[Events]".to_owned(),
            format: DEFAULT_EVENT_FORMAT
                .iter()
                .map(|s| (*s).to_owned())
                .collect(),
            raw_format_line: None,
            entries: Vec::new(),
        };

        Self {
            has_bom: true,
            line_ending: LineEnding::CrLf,
            line_endings: None,
            trailing_newline: true,
            sections: vec![
                Section::ScriptInfo(script_info),
                Section::Styles(styles),
                Section::Events(events),
            ],
        }
    }

    /// Returns a reference to the `[Script Info]` section, if present.
    #[must_use]
    pub fn script_info(&self) -> Option<&KeyValueSection> {
        self.sections.iter().find_map(|s| match s {
            Section::ScriptInfo(sec) => Some(sec),
            _ => None,
        })
    }

    /// Returns a mutable reference to the `[Script Info]` section, if present.
    pub fn script_info_mut(&mut self) -> Option<&mut KeyValueSection> {
        self.sections.iter_mut().find_map(|s| match s {
            Section::ScriptInfo(sec) => Some(sec),
            _ => None,
        })
    }

    /// Returns a reference to the `[Aegisub Project Garbage]` section, if present.
    #[must_use]
    pub fn aegisub_garbage(&self) -> Option<&KeyValueSection> {
        self.sections.iter().find_map(|s| match s {
            Section::AegisubGarbage(sec) => Some(sec),
            _ => None,
        })
    }

    /// Returns a reference to the `[V4+ Styles]` section, if present.
    #[must_use]
    pub fn styles_section(&self) -> Option<&StylesSection> {
        self.sections.iter().find_map(|s| match s {
            Section::Styles(sec) => Some(sec),
            _ => None,
        })
    }

    /// Returns a mutable reference to the `[V4+ Styles]` section, if present.
    pub fn styles_section_mut(&mut self) -> Option<&mut StylesSection> {
        self.sections.iter_mut().find_map(|s| match s {
            Section::Styles(sec) => Some(sec),
            _ => None,
        })
    }

    /// Returns a reference to the `[Events]` section, if present.
    #[must_use]
    pub fn events_section(&self) -> Option<&EventsSection> {
        self.sections.iter().find_map(|s| match s {
            Section::Events(sec) => Some(sec),
            _ => None,
        })
    }

    /// Returns a mutable reference to the `[Events]` section, if present.
    pub fn events_section_mut(&mut self) -> Option<&mut EventsSection> {
        self.sections.iter_mut().find_map(|s| match s {
            Section::Events(sec) => Some(sec),
            _ => None,
        })
    }

    /// Ensures an `[Events]` section exists and returns a mutable reference to it.
    pub fn ensure_events_section_mut(&mut self) -> &mut EventsSection {
        let pos = self
            .sections
            .iter()
            .position(|s| matches!(s, Section::Events(_)));
        let idx = if let Some(i) = pos {
            i
        } else {
            self.sections.push(Section::Events(EventsSection {
                header: "[Events]".to_owned(),
                format: DEFAULT_EVENT_FORMAT
                    .iter()
                    .map(|s| (*s).to_owned())
                    .collect(),
                raw_format_line: None,
                entries: Vec::new(),
            }));
            self.sections.len() - 1
        };
        match &mut self.sections[idx] {
            Section::Events(sec) => sec,
            _ => unreachable!(),
        }
    }

    /// Ensures a `[V4+ Styles]` section exists and returns a mutable reference to it.
    pub fn ensure_styles_section_mut(&mut self) -> &mut StylesSection {
        let pos = self
            .sections
            .iter()
            .position(|s| matches!(s, Section::Styles(_)));
        let idx = if let Some(i) = pos {
            i
        } else {
            self.sections.push(Section::Styles(StylesSection {
                header: "[V4+ Styles]".to_owned(),
                format: DEFAULT_STYLE_FORMAT
                    .iter()
                    .map(|s| (*s).to_owned())
                    .collect(),
                raw_format_line: None,
                entries: Vec::new(),
            }));
            self.sections.len() - 1
        };
        match &mut self.sections[idx] {
            Section::Styles(sec) => sec,
            _ => unreachable!(),
        }
    }

    /// Iterates over all parsed `AssEvent`s in document order.
    pub fn events(&self) -> impl Iterator<Item = &AssEvent> {
        self.events_section()
            .into_iter()
            .flat_map(EventsSection::events)
    }

    /// Iterates over all parsed `AssStyle`s in document order.
    pub fn styles(&self) -> impl Iterator<Item = &AssStyle> {
        self.styles_section()
            .into_iter()
            .flat_map(StylesSection::styles)
    }
}

/// Top-level sections in an ASS file, kept in exact original order.
#[derive(Debug, Clone, PartialEq)]
pub enum Section {
    /// Any lines appearing before the first `[Section]` header.
    Preamble(Vec<String>),
    /// `[Script Info]` section.
    ScriptInfo(KeyValueSection),
    /// `[Aegisub Project Garbage]` section.
    AegisubGarbage(KeyValueSection),
    /// `[V4+ Styles]` section.
    Styles(StylesSection),
    /// `[Events]` section.
    Events(EventsSection),
    /// `[Fonts]` uuencoded attachment section kept verbatim.
    Fonts(RawSection),
    /// `[Graphics]` uuencoded attachment section kept verbatim.
    Graphics(RawSection),
    /// Any unrecognized `[Section]` kept verbatim.
    Unknown(RawSection),
}

/// Ordered key/value section (`[Script Info]` or `[Aegisub Project Garbage]`).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct KeyValueSection {
    /// Exact header line (e.g. `"[Script Info]"`).
    pub header: String,
    /// Ordered entries including unknown keys, comments, and blank lines.
    pub entries: Vec<KeyValueEntry>,
}

impl KeyValueSection {
    /// Looks up the first value for `key` (case-insensitive).
    #[must_use]
    pub fn get(&self, key: &str) -> Option<&str> {
        self.entries.iter().find_map(|e| match e {
            KeyValueEntry::KeyValue { key: k, value, .. } if k.eq_ignore_ascii_case(key) => {
                Some(value.as_str())
            }
            _ => None,
        })
    }

    /// Sets `key` to `value` (updating the first matching key in-place, or appending before trailing blanks).
    pub fn set(&mut self, key: impl Into<String>, value: impl Into<String>) {
        let key_s = key.into();
        let val_s = value.into();
        for entry in &mut self.entries {
            if let KeyValueEntry::KeyValue {
                key: k,
                value: existing,
                ..
            } = entry
            {
                if k.eq_ignore_ascii_case(&key_s) {
                    *existing = val_s;
                    return;
                }
            }
        }
        let insert_pos = self
            .entries
            .iter()
            .rposition(|e| !matches!(e, KeyValueEntry::Blank))
            .map_or(0, |i| i + 1);
        self.entries.insert(
            insert_pos,
            KeyValueEntry::KeyValue {
                key: key_s,
                separator: ": ".to_owned(),
                value: val_s,
            },
        );
    }
}

/// Single line inside a key/value section.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum KeyValueEntry {
    /// Standard `Key: Value` pair preserving exact separator (e.g. `": "` or `":"`).
    KeyValue {
        key: String,
        separator: String,
        value: String,
    },
    /// Comment line starting with `;` or `!:`.
    Comment(String),
    /// Empty line.
    Blank,
    /// Unrecognized raw line kept verbatim.
    Raw(String),
}

/// Verbatim section used for `[Fonts]`, `[Graphics]`, and unknown `[Section]` blocks.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct RawSection {
    /// Exact header line (e.g. `"[Fonts]"`).
    pub header: String,
    /// Raw body lines kept verbatim.
    pub lines: Vec<String>,
}

/// `[V4+ Styles]` section preserving custom `Format:` order and raw non-style lines.
#[derive(Debug, Clone, PartialEq)]
pub struct StylesSection {
    pub header: String,
    pub format: Vec<String>,
    /// Original `Format: ...` line if present, for byte-identical serialization.
    pub raw_format_line: Option<String>,
    pub entries: Vec<StyleEntry>,
}

impl StylesSection {
    /// Iterates over parsed `AssStyle` items.
    pub fn styles(&self) -> impl Iterator<Item = &AssStyle> {
        self.entries.iter().filter_map(|e| match e {
            StyleEntry::Style(s) => Some(s.as_ref()),
            _ => None,
        })
    }

    /// Returns the number of `AssStyle` items.
    #[must_use]
    pub fn style_count(&self) -> usize {
        self.entries
            .iter()
            .filter(|e| matches!(e, StyleEntry::Style(_)))
            .count()
    }

    /// Maps logical style index `idx` to its entry index inside `self.entries`.
    #[must_use]
    pub fn entry_index_of_style(&self, idx: usize) -> Option<usize> {
        self.entries
            .iter()
            .enumerate()
            .filter(|(_, e)| matches!(e, StyleEntry::Style(_)))
            .nth(idx)
            .map(|(i, _)| i)
    }

    /// Gets a style by logical style index.
    #[must_use]
    pub fn get_style(&self, idx: usize) -> Option<&AssStyle> {
        self.entry_index_of_style(idx)
            .and_then(|i| match &self.entries[i] {
                StyleEntry::Style(s) => Some(s.as_ref()),
                _ => None,
            })
    }

    /// Gets a mutable style by logical style index.
    pub fn get_style_mut(&mut self, idx: usize) -> Option<&mut AssStyle> {
        self.entry_index_of_style(idx)
            .and_then(|i| match &mut self.entries[i] {
                StyleEntry::Style(s) => Some(s.as_mut()),
                _ => None,
            })
    }

    /// Inserts a style at logical index `idx` (clamped to `style_count()`).
    pub fn insert_style(&mut self, idx: usize, style: AssStyle) {
        let entry_pos = self.entry_index_of_style(idx).unwrap_or_else(|| {
            self.entries
                .iter()
                .rposition(|e| !matches!(e, StyleEntry::Blank))
                .map_or(0, |i| i + 1)
        });
        self.entries
            .insert(entry_pos, StyleEntry::Style(Box::new(style)));
    }

    /// Removes and returns the style at logical index `idx`.
    pub fn remove_style(&mut self, idx: usize) -> Option<AssStyle> {
        let entry_pos = self.entry_index_of_style(idx)?;
        match self.entries.remove(entry_pos) {
            StyleEntry::Style(s) => Some(*s),
            _ => None,
        }
    }
}

/// Single line inside `[V4+ Styles]`.
#[derive(Debug, Clone, PartialEq)]
pub enum StyleEntry {
    Style(Box<AssStyle>),
    Comment(String),
    Blank,
    Raw(String),
}

/// Complete V4+ Style definition.
///
/// All fields are mutated exclusively through setters (or `EditCommand`), which automatically
/// invalidate the private `raw_line` cache so serialization always reflects edits while
/// preserving exact original numeric formatting on untouched styles.
#[derive(Debug, Clone, PartialEq)]
pub struct AssStyle {
    pub(crate) name: String,
    pub(crate) fontname: String,
    pub(crate) fontsize: f64,
    pub(crate) primary_colour: String,
    pub(crate) secondary_colour: String,
    pub(crate) outline_colour: String,
    pub(crate) back_colour: String,
    pub(crate) bold: i32,
    pub(crate) italic: i32,
    pub(crate) underline: i32,
    pub(crate) strike_out: i32,
    pub(crate) scale_x: f64,
    pub(crate) scale_y: f64,
    pub(crate) spacing: f64,
    pub(crate) angle: f64,
    pub(crate) border_style: i32,
    pub(crate) outline: f64,
    pub(crate) shadow: f64,
    pub(crate) alignment: i32,
    pub(crate) margin_l: i32,
    pub(crate) margin_r: i32,
    pub(crate) margin_v: i32,
    pub(crate) encoding: i32,
    pub(crate) raw_line: Option<String>,
}

impl Default for AssStyle {
    fn default() -> Self {
        Self {
            name: "Default".to_owned(),
            fontname: "Arial".to_owned(),
            fontsize: 48.0,
            primary_colour: "&H00FFFFFF".to_owned(),
            secondary_colour: "&H000000FF".to_owned(),
            outline_colour: "&H00000000".to_owned(),
            back_colour: "&H00000000".to_owned(),
            bold: 0,
            italic: 0,
            underline: 0,
            strike_out: 0,
            scale_x: 100.0,
            scale_y: 100.0,
            spacing: 0.0,
            angle: 0.0,
            border_style: 1,
            outline: 2.0,
            shadow: 2.0,
            alignment: 2,
            margin_l: 10,
            margin_r: 10,
            margin_v: 10,
            encoding: 1,
            raw_line: None,
        }
    }
}

#[allow(missing_docs)]
impl AssStyle {
    #[inline]
    #[must_use]
    pub fn name(&self) -> &str {
        &self.name
    }
    #[inline]
    pub fn set_name(&mut self, value: impl Into<String>) {
        self.name = value.into();
        self.raw_line = None;
    }

    #[inline]
    #[must_use]
    pub fn fontname(&self) -> &str {
        &self.fontname
    }
    #[inline]
    pub fn set_fontname(&mut self, value: impl Into<String>) {
        self.fontname = value.into();
        self.raw_line = None;
    }

    #[inline]
    #[must_use]
    pub const fn fontsize(&self) -> f64 {
        self.fontsize
    }
    #[inline]
    pub fn set_fontsize(&mut self, value: f64) {
        self.fontsize = value;
        self.raw_line = None;
    }

    #[inline]
    #[must_use]
    pub fn primary_colour(&self) -> &str {
        &self.primary_colour
    }
    #[inline]
    pub fn set_primary_colour(&mut self, value: impl Into<String>) {
        self.primary_colour = value.into();
        self.raw_line = None;
    }

    #[inline]
    #[must_use]
    pub fn secondary_colour(&self) -> &str {
        &self.secondary_colour
    }
    #[inline]
    pub fn set_secondary_colour(&mut self, value: impl Into<String>) {
        self.secondary_colour = value.into();
        self.raw_line = None;
    }

    #[inline]
    #[must_use]
    pub fn outline_colour(&self) -> &str {
        &self.outline_colour
    }
    #[inline]
    pub fn set_outline_colour(&mut self, value: impl Into<String>) {
        self.outline_colour = value.into();
        self.raw_line = None;
    }

    #[inline]
    #[must_use]
    pub fn back_colour(&self) -> &str {
        &self.back_colour
    }
    #[inline]
    pub fn set_back_colour(&mut self, value: impl Into<String>) {
        self.back_colour = value.into();
        self.raw_line = None;
    }

    #[inline]
    #[must_use]
    pub const fn bold(&self) -> i32 {
        self.bold
    }
    #[inline]
    pub fn set_bold(&mut self, value: i32) {
        self.bold = value;
        self.raw_line = None;
    }

    #[inline]
    #[must_use]
    pub const fn italic(&self) -> i32 {
        self.italic
    }
    #[inline]
    pub fn set_italic(&mut self, value: i32) {
        self.italic = value;
        self.raw_line = None;
    }

    #[inline]
    #[must_use]
    pub const fn underline(&self) -> i32 {
        self.underline
    }
    #[inline]
    pub fn set_underline(&mut self, value: i32) {
        self.underline = value;
        self.raw_line = None;
    }

    #[inline]
    #[must_use]
    pub const fn strike_out(&self) -> i32 {
        self.strike_out
    }
    #[inline]
    pub fn set_strike_out(&mut self, value: i32) {
        self.strike_out = value;
        self.raw_line = None;
    }

    #[inline]
    #[must_use]
    pub const fn scale_x(&self) -> f64 {
        self.scale_x
    }
    #[inline]
    pub fn set_scale_x(&mut self, value: f64) {
        self.scale_x = value;
        self.raw_line = None;
    }

    #[inline]
    #[must_use]
    pub const fn scale_y(&self) -> f64 {
        self.scale_y
    }
    #[inline]
    pub fn set_scale_y(&mut self, value: f64) {
        self.scale_y = value;
        self.raw_line = None;
    }

    #[inline]
    #[must_use]
    pub const fn spacing(&self) -> f64 {
        self.spacing
    }
    #[inline]
    pub fn set_spacing(&mut self, value: f64) {
        self.spacing = value;
        self.raw_line = None;
    }

    #[inline]
    #[must_use]
    pub const fn angle(&self) -> f64 {
        self.angle
    }
    #[inline]
    pub fn set_angle(&mut self, value: f64) {
        self.angle = value;
        self.raw_line = None;
    }

    #[inline]
    #[must_use]
    pub const fn border_style(&self) -> i32 {
        self.border_style
    }
    #[inline]
    pub fn set_border_style(&mut self, value: i32) {
        self.border_style = value;
        self.raw_line = None;
    }

    #[inline]
    #[must_use]
    pub const fn outline(&self) -> f64 {
        self.outline
    }
    #[inline]
    pub fn set_outline(&mut self, value: f64) {
        self.outline = value;
        self.raw_line = None;
    }

    #[inline]
    #[must_use]
    pub const fn shadow(&self) -> f64 {
        self.shadow
    }
    #[inline]
    pub fn set_shadow(&mut self, value: f64) {
        self.shadow = value;
        self.raw_line = None;
    }

    #[inline]
    #[must_use]
    pub const fn alignment(&self) -> i32 {
        self.alignment
    }
    #[inline]
    pub fn set_alignment(&mut self, value: i32) {
        self.alignment = value;
        self.raw_line = None;
    }

    #[inline]
    #[must_use]
    pub const fn margin_l(&self) -> i32 {
        self.margin_l
    }
    #[inline]
    pub fn set_margin_l(&mut self, value: i32) {
        self.margin_l = value;
        self.raw_line = None;
    }

    #[inline]
    #[must_use]
    pub const fn margin_r(&self) -> i32 {
        self.margin_r
    }
    #[inline]
    pub fn set_margin_r(&mut self, value: i32) {
        self.margin_r = value;
        self.raw_line = None;
    }

    #[inline]
    #[must_use]
    pub const fn margin_v(&self) -> i32 {
        self.margin_v
    }
    #[inline]
    pub fn set_margin_v(&mut self, value: i32) {
        self.margin_v = value;
        self.raw_line = None;
    }

    #[inline]
    #[must_use]
    pub const fn encoding(&self) -> i32 {
        self.encoding
    }
    #[inline]
    pub fn set_encoding(&mut self, value: i32) {
        self.encoding = value;
        self.raw_line = None;
    }

    /// Returns `true` if this style still holds an uninvalidated verbatim source line.
    #[inline]
    #[must_use]
    pub const fn has_cached_raw_line(&self) -> bool {
        self.raw_line.is_some()
    }
}

/// `[Events]` section preserving custom `Format:` order and raw non-event lines.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct EventsSection {
    pub header: String,
    pub format: Vec<String>,
    /// Original `Format: ...` line if present, for byte-identical serialization.
    pub raw_format_line: Option<String>,
    pub entries: Vec<EventEntry>,
}

impl EventsSection {
    /// Iterates over parsed `AssEvent` items.
    pub fn events(&self) -> impl Iterator<Item = &AssEvent> {
        self.entries.iter().filter_map(|e| match e {
            EventEntry::Event(ev) => Some(ev),
            _ => None,
        })
    }

    /// Returns the number of `AssEvent` items.
    #[must_use]
    pub fn event_count(&self) -> usize {
        self.entries
            .iter()
            .filter(|e| matches!(e, EventEntry::Event(_)))
            .count()
    }

    /// Maps logical event index `idx` to its position in `self.entries`.
    #[must_use]
    pub fn entry_index_of_event(&self, idx: usize) -> Option<usize> {
        if let Some(EventEntry::Event(_)) = self.entries.get(idx) {
            if self
                .entries
                .iter()
                .take(idx)
                .all(|e| matches!(e, EventEntry::Event(_)))
            {
                return Some(idx);
            }
        }
        self.entries
            .iter()
            .enumerate()
            .filter(|(_, e)| matches!(e, EventEntry::Event(_)))
            .nth(idx)
            .map(|(i, _)| i)
    }

    /// Gets an event by logical event index.
    #[must_use]
    pub fn get_event(&self, idx: usize) -> Option<&AssEvent> {
        self.entry_index_of_event(idx)
            .and_then(|i| match &self.entries[i] {
                EventEntry::Event(ev) => Some(ev),
                _ => None,
            })
    }

    /// Gets a mutable event by logical event index.
    pub fn get_event_mut(&mut self, idx: usize) -> Option<&mut AssEvent> {
        self.entry_index_of_event(idx)
            .and_then(|i| match &mut self.entries[i] {
                EventEntry::Event(ev) => Some(ev),
                _ => None,
            })
    }

    /// Inserts an event at logical index `idx` (clamped to `event_count()`).
    pub fn insert_event(&mut self, idx: usize, event: AssEvent) {
        let entry_pos = self.entry_index_of_event(idx).unwrap_or_else(|| {
            self.entries
                .iter()
                .rposition(|e| !matches!(e, EventEntry::Blank))
                .map_or(0, |i| i + 1)
        });
        self.entries.insert(entry_pos, EventEntry::Event(event));
    }

    /// Removes and returns the event at logical index `idx`.
    pub fn remove_event(&mut self, idx: usize) -> Option<AssEvent> {
        let entry_pos = self.entry_index_of_event(idx)?;
        match self.entries.remove(entry_pos) {
            EventEntry::Event(ev) => Some(ev),
            _ => None,
        }
    }
}

/// Single line inside `[Events]`.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum EventEntry {
    Event(AssEvent),
    Comment(String),
    Blank,
    Raw(String),
}

/// Event line kind (`Dialogue` or `Comment`).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum EventKind {
    #[default]
    Dialogue,
    Comment,
}

impl EventKind {
    /// Returns the ASS line prefix (`"Dialogue"` or `"Comment"`).
    #[must_use]
    pub const fn as_str(self) -> &'static str {
        match self {
            Self::Dialogue => "Dialogue",
            Self::Comment => "Comment",
        }
    }
}

/// Single ASS event line (`Dialogue:` or `Comment:`).
///
/// All fields are mutated exclusively through setters (or `EditCommand`), which automatically
/// invalidate the private `raw_line` cache.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct AssEvent {
    pub(crate) kind: EventKind,
    pub(crate) layer: i32,
    pub(crate) start: AssTime,
    pub(crate) end: AssTime,
    pub(crate) style: String,
    pub(crate) name: String,
    pub(crate) margin_l: i32,
    pub(crate) margin_r: i32,
    pub(crate) margin_v: i32,
    pub(crate) effect: String,
    pub(crate) text: String,
    pub(crate) raw_line: Option<String>,
}

impl Default for AssEvent {
    fn default() -> Self {
        Self {
            kind: EventKind::Dialogue,
            layer: 0,
            start: AssTime::ZERO,
            end: AssTime::from_cs(500),
            style: "Default".to_owned(),
            name: String::new(),
            margin_l: 0,
            margin_r: 0,
            margin_v: 0,
            effect: String::new(),
            text: String::new(),
            raw_line: None,
        }
    }
}

#[allow(missing_docs)]
impl AssEvent {
    /// Creates a new `Dialogue` event with the given start, end, and text.
    #[must_use]
    pub fn dialogue(start: AssTime, end: AssTime, text: impl Into<String>) -> Self {
        Self {
            start,
            end,
            text: text.into(),
            ..Self::default()
        }
    }

    #[inline]
    #[must_use]
    pub const fn kind(&self) -> EventKind {
        self.kind
    }
    #[inline]
    pub fn set_kind(&mut self, value: EventKind) {
        self.kind = value;
        self.raw_line = None;
    }

    #[inline]
    #[must_use]
    pub const fn layer(&self) -> i32 {
        self.layer
    }
    #[inline]
    pub fn set_layer(&mut self, value: i32) {
        self.layer = value;
        self.raw_line = None;
    }

    #[inline]
    #[must_use]
    pub const fn start(&self) -> AssTime {
        self.start
    }
    #[inline]
    pub fn set_start(&mut self, value: AssTime) {
        self.start = value;
        self.raw_line = None;
    }

    #[inline]
    #[must_use]
    pub const fn end(&self) -> AssTime {
        self.end
    }
    #[inline]
    pub fn set_end(&mut self, value: AssTime) {
        self.end = value;
        self.raw_line = None;
    }

    #[inline]
    #[must_use]
    pub fn style(&self) -> &str {
        &self.style
    }
    #[inline]
    pub fn set_style(&mut self, value: impl Into<String>) {
        self.style = value.into();
        self.raw_line = None;
    }

    #[inline]
    #[must_use]
    pub fn name(&self) -> &str {
        &self.name
    }
    #[inline]
    pub fn set_name(&mut self, value: impl Into<String>) {
        self.name = value.into();
        self.raw_line = None;
    }

    #[inline]
    #[must_use]
    pub const fn margin_l(&self) -> i32 {
        self.margin_l
    }
    #[inline]
    pub fn set_margin_l(&mut self, value: i32) {
        self.margin_l = value;
        self.raw_line = None;
    }

    #[inline]
    #[must_use]
    pub const fn margin_r(&self) -> i32 {
        self.margin_r
    }
    #[inline]
    pub fn set_margin_r(&mut self, value: i32) {
        self.margin_r = value;
        self.raw_line = None;
    }

    #[inline]
    #[must_use]
    pub const fn margin_v(&self) -> i32 {
        self.margin_v
    }
    #[inline]
    pub fn set_margin_v(&mut self, value: i32) {
        self.margin_v = value;
        self.raw_line = None;
    }

    #[inline]
    #[must_use]
    pub fn effect(&self) -> &str {
        &self.effect
    }
    #[inline]
    pub fn set_effect(&mut self, value: impl Into<String>) {
        self.effect = value.into();
        self.raw_line = None;
    }

    #[inline]
    #[must_use]
    pub fn text(&self) -> &str {
        &self.text
    }
    #[inline]
    pub fn set_text(&mut self, value: impl Into<String>) {
        self.text = value.into();
        self.raw_line = None;
    }

    /// Returns `true` if this event still holds an uninvalidated verbatim source line.
    #[inline]
    #[must_use]
    pub const fn has_cached_raw_line(&self) -> bool {
        self.raw_line.is_some()
    }
}
