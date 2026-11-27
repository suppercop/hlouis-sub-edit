//! Lossless ASS/SSA document data model preserving section order, unknown sections,
//! `[Fonts]`/`[Graphics]` verbatim payloads, custom `Format:` orders, and numeric formatting.

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
    /// Dominant line-ending convention detected during parsing.
    pub line_ending: LineEnding,
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
/// Numeric fields are stored as typed values for editing, while `raw_line` (when present)
/// preserves exact original numeric formatting (e.g. `100` vs `100.0`, `1.50`) until modified.
#[derive(Debug, Clone, PartialEq)]
pub struct AssStyle {
    pub name: String,
    pub fontname: String,
    pub fontsize: f64,
    pub primary_colour: String,
    pub secondary_colour: String,
    pub outline_colour: String,
    pub back_colour: String,
    pub bold: i32,
    pub italic: i32,
    pub underline: i32,
    pub strike_out: i32,
    pub scale_x: f64,
    pub scale_y: f64,
    pub spacing: f64,
    pub angle: f64,
    pub border_style: i32,
    pub outline: f64,
    pub shadow: f64,
    pub alignment: i32,
    pub margin_l: i32,
    pub margin_r: i32,
    pub margin_v: i32,
    pub encoding: i32,
    /// Original raw `Style: ...` line for byte-identical round-trip when unmodified.
    /// Call [`AssStyle::mark_dirty`] after mutating fields directly.
    pub raw_line: Option<String>,
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

impl AssStyle {
    /// Clears cached raw formatting so serialization reflects edited typed fields.
    pub fn mark_dirty(&mut self) {
        self.raw_line = None;
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
    /// Fast-path O(1) when all entries up to `idx` are `EventEntry::Event` (typical Aegisub files).
    #[must_use]
    pub fn entry_index_of_event(&self, idx: usize) -> Option<usize> {
        if let Some(EventEntry::Event(_)) = self.entries.get(idx) {
            // Verify there are no non-event entries before `idx`? For O(1) on standard files
            // without comments/blanks in [Events], we still need exactness if comments exist.
            // Check if all entries are events (common case: scan only if needed).
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
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct AssEvent {
    pub kind: EventKind,
    pub layer: i32,
    pub start: AssTime,
    pub end: AssTime,
    pub style: String,
    pub name: String,
    pub margin_l: i32,
    pub margin_r: i32,
    pub margin_v: i32,
    pub effect: String,
    pub text: String,
    /// Optional raw line stored only when the input had non-canonical formatting (e.g. extra
    /// leading zeros in margins/timestamps or custom unknown columns) for byte-identical output.
    /// Cleared automatically on edits via [`AssEvent::set_text`] or [`AssEvent::mark_dirty`].
    pub raw_line: Option<String>,
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

impl AssEvent {
    /// Updates `text` and clears `raw_line` so serialization emits the updated text.
    pub fn set_text(&mut self, text: String) {
        self.text = text;
        self.raw_line = None;
    }

    /// Clears cached raw formatting so serialization reflects edited typed fields.
    pub fn mark_dirty(&mut self) {
        self.raw_line = None;
    }
}
