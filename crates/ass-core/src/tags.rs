//! Lossless ASS override-tag parser and serializer.
//!
//! Splits dialogue `Text` into `{...}` override blocks, plain text, `\N`/`\n`/`\h` escapes,
//! and drawing segments (`\p` > 0). Re-serializing the parsed block stream reproduces the
//! original `Text` byte-for-byte for every valid UTF-8 input.

/// Top-level block inside an ASS event's `Text` field.
#[derive(Debug, Clone, PartialEq)]
pub enum TextBlock {
    /// Override block enclosed in `{...}` (or unclosed `{...` at end of line).
    Override {
        items: Vec<OverrideItem>,
        /// `true` if a closing `}` was present in the input.
        closed: bool,
    },
    /// Plain dialogue text (outside override blocks and drawing mode).
    Plain(String),
    /// ASS inline escape (`\N`, `\n`, `\h`).
    Escape(EscapeKind),
    /// Vector drawing commands emitted while `\p` drawing level > 0.
    Drawing { level: u32, commands: String },
}

/// ASS inline escape sequences outside override blocks.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum EscapeKind {
    /// `\N` hard line break.
    HardNewline,
    /// `\n` soft line break.
    SoftNewline,
    /// `\h` non-breaking space.
    HardSpace,
}

impl EscapeKind {
    /// Returns the exact ASS escape string.
    #[must_use]
    pub const fn as_str(self) -> &'static str {
        match self {
            Self::HardNewline => r"\N",
            Self::SoftNewline => r"\n",
            Self::HardSpace => r"\h",
        }
    }
}

/// Single item inside an `{...}` override block.
#[derive(Debug, Clone, PartialEq)]
pub enum OverrideItem {
    /// Override tag starting with `\`.
    Tag(OverrideTag),
    /// Raw comment text inside `{...}` not preceded by `\`.
    Comment(String),
}

/// Parsed override tag with both structured arguments and exact raw text for lossless serialization.
#[derive(Debug, Clone, PartialEq)]
pub struct OverrideTag {
    /// Tag name without leading `\` (e.g. `"pos"`, `"t"`, `"clip"`, `"kf"`).
    pub name: String,
    /// Parsed structured argument representation.
    pub args: TagArgs,
    /// Exact raw argument substring after `\name` for byte-identical round-trip.
    pub raw_args: String,
}

/// Karaoke timing tag variant.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum KaraokeKind {
    /// `\k`
    K,
    /// `\kf`
    Kf,
    /// `\K`
    CapitalK,
    /// `\ko`
    Ko,
    /// `\kt`
    Kt,
}

/// Structured arguments for well-known ASS override tags.
#[derive(Debug, Clone, PartialEq)]
pub enum TagArgs {
    /// `\pos(x, y)` or `\org(x, y)`.
    Point { x: f64, y: f64 },
    /// `\move(x1, y1, x2, y2 [, t1, t2])`.
    Move {
        x1: f64,
        y1: f64,
        x2: f64,
        y2: f64,
        t1: Option<i64>,
        t2: Option<i64>,
    },
    /// `\fad(t1, t2)`.
    Fad { t1: i64, t2: i64 },
    /// `\fade(a1, a2, a3, t1, t2, t3, t4)`.
    Fade {
        a1: i64,
        a2: i64,
        a3: i64,
        t1: i64,
        t2: i64,
        t3: i64,
        t4: i64,
    },
    /// Rectangular `\clip(x1, y1, x2, y2)` or `\iclip(...)`.
    ClipRect {
        inverted: bool,
        x1: f64,
        y1: f64,
        x2: f64,
        y2: f64,
    },
    /// Vector `\clip([scale,] drawing_commands)` or `\iclip(...)`.
    ClipVector {
        inverted: bool,
        scale: u32,
        commands: String,
    },
    /// `\t([t1, t2, [accel,]] \tags...)`.
    Transform {
        t1: Option<i64>,
        t2: Option<i64>,
        accel: Option<f64>,
        items: Vec<OverrideItem>,
    },
    /// Karaoke `\k`, `\kf`, `\K`, `\ko`, `\kt` duration in centiseconds.
    Karaoke { kind: KaraokeKind, duration_cs: i64 },
    /// `\p<level>` drawing mode toggle.
    DrawingMode(u32),
    /// Any other or malformed tag kept as raw argument text.
    Raw(String),
}

/// Known ASS override tag names, ordered longest-first to avoid prefix collisions.
const KNOWN_TAGS: &[&str] = &[
    "1img", "2img", "3img", "4img", "1vc", "2vc", "3vc", "4vc", "1va", "2va", "3va", "4va", "1c",
    "2c", "3c", "4c", "1a", "2a", "3a", "4a", "xbord", "ybord", "xshad", "yshad", "alpha", "fscx",
    "fscy", "iclip", "clip", "move", "fade", "blur", "bord", "shad", "fsvp", "fsc", "fsp", "frx",
    "fry", "frz", "fax", "fay", "pbo", "pos", "org", "fad", "frs", "rnd", "fn", "fs", "fe", "fr",
    "kf", "ko", "kt", "be", "an", "a", "b", "i", "u", "s", "c", "k", "K", "p", "q", "r", "t",
];

/// Parses an ASS event `Text` field into a lossless stream of [`TextBlock`]s.
#[must_use]
pub fn parse_text_blocks(text: &str) -> Vec<TextBlock> {
    let mut blocks = Vec::new();
    let bytes = text.as_bytes();
    let len = bytes.len();
    let mut i = 0usize;
    let mut drawing_level: u32 = 0;

    while i < len {
        if bytes[i] == b'{' {
            i += 1;
            let (items, next_i, closed) = parse_override_block(text, i, &mut drawing_level);
            blocks.push(TextBlock::Override { items, closed });
            i = next_i;
        } else {
            let next_brace = bytes[i..]
                .iter()
                .position(|&b| b == b'{')
                .map_or(len, |rel| i + rel);
            let segment = &text[i..next_brace];
            if drawing_level > 0 {
                blocks.push(TextBlock::Drawing {
                    level: drawing_level,
                    commands: segment.to_owned(),
                });
            } else {
                parse_plain_segment(segment, &mut blocks);
            }
            i = next_brace;
        }
    }

    blocks
}

/// Re-serializes a [`TextBlock`] slice back into exact ASS `Text`.
#[must_use]
pub fn serialize_text_blocks(blocks: &[TextBlock]) -> String {
    let mut out = String::new();
    for block in blocks {
        match block {
            TextBlock::Override { items, closed } => {
                out.push('{');
                for item in items {
                    write_override_item(&mut out, item);
                }
                if *closed {
                    out.push('}');
                }
            }
            TextBlock::Plain(s) | TextBlock::Drawing { commands: s, .. } => out.push_str(s),
            TextBlock::Escape(esc) => out.push_str(esc.as_str()),
        }
    }
    out
}

fn write_override_item(out: &mut String, item: &OverrideItem) {
    match item {
        OverrideItem::Comment(c) => out.push_str(c),
        OverrideItem::Tag(tag) => {
            out.push('\\');
            out.push_str(&tag.name);
            out.push_str(&tag.raw_args);
        }
    }
}

fn parse_plain_segment(segment: &str, blocks: &mut Vec<TextBlock>) {
    let bytes = segment.as_bytes();
    let len = bytes.len();
    let mut i = 0usize;
    let mut plain_start = 0usize;

    while i < len {
        if bytes[i] == b'\\' && i + 1 < len {
            let esc = match bytes[i + 1] {
                b'N' => Some(EscapeKind::HardNewline),
                b'n' => Some(EscapeKind::SoftNewline),
                b'h' => Some(EscapeKind::HardSpace),
                _ => None,
            };
            if let Some(kind) = esc {
                if i > plain_start {
                    blocks.push(TextBlock::Plain(segment[plain_start..i].to_owned()));
                }
                blocks.push(TextBlock::Escape(kind));
                i += 2;
                plain_start = i;
                continue;
            }
        }
        i = advance_char(segment, i);
    }

    if plain_start < len {
        blocks.push(TextBlock::Plain(segment[plain_start..len].to_owned()));
    }
}

fn parse_override_block(
    text: &str,
    i: usize,
    drawing_level: &mut u32,
) -> (Vec<OverrideItem>, usize, bool) {
    let mut items = Vec::new();
    let (next, closed) = scan_override_block(text, i, |raw| match raw {
        RawOverrideItem::Comment(c) => items.push(OverrideItem::Comment(c.to_owned())),
        RawOverrideItem::Tag { name, raw_args } => {
            let args = parse_tag_args(name, raw_args);
            if let TagArgs::DrawingMode(lvl) = args {
                *drawing_level = lvl;
            }
            items.push(OverrideItem::Tag(OverrideTag {
                name: name.to_owned(),
                args,
                raw_args: raw_args.to_owned(),
            }));
        }
    });
    (items, next, closed)
}

/// Borrowed view of one item inside an override block (no allocation, no argument parsing).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum RawOverrideItem<'a> {
    /// Free text between tags inside `{...}`.
    Comment(&'a str),
    /// A `\name` tag with its exact raw argument text.
    Tag {
        /// Tag name without the leading backslash.
        name: &'a str,
        /// Raw argument text following the name.
        raw_args: &'a str,
    },
}

/// Scans the override block whose contents begin at byte `start` (just after `{`), calling `f`
/// for every item. Returns the byte offset after the block (past `}` when closed) and whether
/// the closing brace was present. This is the zero-allocation core shared by
/// [`parse_text_blocks`].
pub fn scan_override_block<'a>(
    text: &'a str,
    start: usize,
    mut f: impl FnMut(RawOverrideItem<'a>),
) -> (usize, bool) {
    let bytes = text.as_bytes();
    let len = bytes.len();
    let mut i = start;

    while i < len && bytes[i] != b'}' {
        if bytes[i] != b'\\' {
            let cstart = i;
            while i < len && bytes[i] != b'}' && bytes[i] != b'\\' {
                i = advance_char(text, i);
            }
            f(RawOverrideItem::Comment(&text[cstart..i]));
            continue;
        }

        i += 1; // consume the backslash
        let (tag_name, after_name) = match_tag_name(bytes, i);
        i = after_name;

        let arg_start = i;
        let mut paren_depth: u32 = 0;
        while i < len && bytes[i] != b'}' {
            let b = bytes[i];
            if b == b'(' {
                paren_depth = paren_depth.saturating_add(1);
                i += 1;
            } else if b == b')' {
                paren_depth = paren_depth.saturating_sub(1);
                i += 1;
            } else if b == b'\\' && paren_depth == 0 {
                break;
            } else {
                i = advance_char(text, i);
            }
        }

        f(RawOverrideItem::Tag {
            name: tag_name,
            raw_args: &text[arg_start..i],
        });
    }

    let closed = i < len && bytes[i] == b'}';
    if closed {
        i += 1;
    }
    (i, closed)
}

/// Parses a karaoke tag (`k`, `kf`, `K`, `ko`, `kt`) with a valid integer duration.
#[must_use]
pub fn parse_karaoke_tag(name: &str, raw_args: &str) -> Option<(KaraokeKind, i64)> {
    let kind = match name {
        "k" => KaraokeKind::K,
        "kf" => KaraokeKind::Kf,
        "K" => KaraokeKind::CapitalK,
        "ko" => KaraokeKind::Ko,
        "kt" => KaraokeKind::Kt,
        _ => return None,
    };
    raw_args.trim().parse::<i64>().ok().map(|d| (kind, d))
}

/// Parses the structured arguments of tag `name` from its exact `raw_args` text.
#[must_use]
pub fn parse_tag_args(name: &str, raw_args: &str) -> TagArgs {
    match name {
        "pos" | "org" => {
            if let Some(nums) = parse_paren_f64_list(raw_args) {
                if nums.len() == 2 {
                    return TagArgs::Point {
                        x: nums[0],
                        y: nums[1],
                    };
                }
            }
        }
        "move" => {
            if let Some(nums) = parse_paren_f64_list(raw_args) {
                if nums.len() == 4 || nums.len() == 6 {
                    #[allow(clippy::cast_possible_truncation)]
                    let (t1, t2) = if nums.len() == 6 {
                        (Some(nums[4].round() as i64), Some(nums[5].round() as i64))
                    } else {
                        (None, None)
                    };
                    return TagArgs::Move {
                        x1: nums[0],
                        y1: nums[1],
                        x2: nums[2],
                        y2: nums[3],
                        t1,
                        t2,
                    };
                }
            }
        }
        "fad" => {
            if let Some(nums) = parse_paren_i64_list(raw_args) {
                if nums.len() == 2 {
                    return TagArgs::Fad {
                        t1: nums[0],
                        t2: nums[1],
                    };
                }
            }
        }
        "fade" => {
            if let Some(nums) = parse_paren_i64_list(raw_args) {
                if nums.len() == 7 {
                    return TagArgs::Fade {
                        a1: nums[0],
                        a2: nums[1],
                        a3: nums[2],
                        t1: nums[3],
                        t2: nums[4],
                        t3: nums[5],
                        t4: nums[6],
                    };
                }
            }
        }
        "clip" | "iclip" => {
            if let Some(args) = parse_clip_args(name == "iclip", raw_args) {
                return args;
            }
        }
        "t" => {
            if let Some(inner) = strip_outer_parens(raw_args) {
                return parse_transform_args(inner);
            }
        }
        "k" | "kf" | "K" | "ko" | "kt" => {
            if let Ok(dur) = raw_args.trim().parse::<i64>() {
                let kind = match name {
                    "k" => KaraokeKind::K,
                    "kf" => KaraokeKind::Kf,
                    "K" => KaraokeKind::CapitalK,
                    "ko" => KaraokeKind::Ko,
                    _ => KaraokeKind::Kt,
                };
                return TagArgs::Karaoke {
                    kind,
                    duration_cs: dur,
                };
            }
        }
        "p" => {
            if let Ok(lvl) = raw_args.trim().parse::<u32>() {
                return TagArgs::DrawingMode(lvl);
            }
        }
        _ => {}
    }
    TagArgs::Raw(raw_args.to_owned())
}

fn parse_clip_args(inverted: bool, raw_args: &str) -> Option<TagArgs> {
    let inner = strip_outer_parens(raw_args)?;
    if let Some(nums) = parse_f64_csv(inner) {
        if nums.len() == 4 {
            return Some(TagArgs::ClipRect {
                inverted,
                x1: nums[0],
                y1: nums[1],
                x2: nums[2],
                y2: nums[3],
            });
        }
    }
    if let Some((scale_str, cmds)) = inner.split_once(',') {
        if let Ok(scale) = scale_str.trim().parse::<u32>() {
            return Some(TagArgs::ClipVector {
                inverted,
                scale,
                commands: cmds.trim().to_owned(),
            });
        }
    }
    Some(TagArgs::ClipVector {
        inverted,
        scale: 1,
        commands: inner.trim().to_owned(),
    })
}

fn parse_transform_args(inner: &str) -> TagArgs {
    let mut parts = Vec::new();
    let mut depth: u32 = 0;
    let mut start = 0usize;
    for (idx, b) in inner.bytes().enumerate() {
        match b {
            b'(' => depth = depth.saturating_add(1),
            b')' => depth = depth.saturating_sub(1),
            b',' if depth == 0 => {
                parts.push(&inner[start..idx]);
                start = idx + 1;
            }
            _ => {}
        }
    }
    parts.push(&inner[start..]);

    let (t1, t2, accel, tags_str) = match parts.as_slice() {
        [tags] => (None, None, None, *tags),
        [acc, tags] => (None, None, acc.trim().parse::<f64>().ok(), *tags),
        [a, b, tags] => (
            a.trim().parse::<i64>().ok(),
            b.trim().parse::<i64>().ok(),
            None,
            *tags,
        ),
        [a, b, acc, tags, ..] => (
            a.trim().parse::<i64>().ok(),
            b.trim().parse::<i64>().ok(),
            acc.trim().parse::<f64>().ok(),
            *tags,
        ),
        [] => (None, None, None, ""),
    };

    let mut dummy_level = 0u32;
    let (items, _, _) = parse_override_block(tags_str, 0, &mut dummy_level);
    TagArgs::Transform {
        t1,
        t2,
        accel,
        items,
    }
}

fn strip_outer_parens(s: &str) -> Option<&str> {
    let trimmed = s.trim();
    trimmed.strip_prefix('(')?.strip_suffix(')')
}

fn parse_f64_csv(inner: &str) -> Option<Vec<f64>> {
    let mut out = Vec::new();
    for part in inner.split(',') {
        let v = part.trim().parse::<f64>().ok()?;
        if !v.is_finite() {
            return None;
        }
        out.push(v);
    }
    Some(out)
}

fn parse_paren_f64_list(raw: &str) -> Option<Vec<f64>> {
    let inner = strip_outer_parens(raw)?;
    parse_f64_csv(inner)
}

fn parse_paren_i64_list(raw: &str) -> Option<Vec<i64>> {
    let inner = strip_outer_parens(raw)?;
    let mut out = Vec::new();
    for part in inner.split(',') {
        out.push(part.trim().parse::<i64>().ok()?);
    }
    Some(out)
}

fn match_tag_name(bytes: &[u8], start: usize) -> (&str, usize) {
    let rest = &bytes[start..];
    for &tag in KNOWN_TAGS {
        let tb = tag.as_bytes();
        if rest.first() == Some(&tb[0]) && rest.starts_with(tb) {
            return (tag, start + tb.len());
        }
    }

    let mut end = start;
    while end < bytes.len() && bytes[end].is_ascii_alphabetic() {
        end += 1;
    }
    let slice = std::str::from_utf8(&bytes[start..end]).unwrap_or("");
    (slice, end)
}

fn advance_char(text: &str, byte_idx: usize) -> usize {
    text.get(byte_idx..)
        .and_then(|s| s.chars().next())
        .map_or(text.len(), |ch| byte_idx + ch.len_utf8())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_known_tags_and_round_trips_losslessly() {
        let line = r"{comment\pos(960,540)\org(100,200)\move(10,20,30,40,0,500)\fad(150,200)\t(0,400,1.5,\frz360\alpha&HFF&)\clip(0,0,100,50)\iclip(2,m 0 0 l 10 10)\k25\kf40\K10\ko15\unknownTag42}Hello\NWorld\nFoo\hBar{\p1}m 0 0 l 50 0 50 50{\p0}Done";
        let blocks = parse_text_blocks(line);
        assert_eq!(serialize_text_blocks(&blocks), line);

        // Verify structured tag parsing.
        let TextBlock::Override { items, closed } = &blocks[0] else {
            panic!("expected override block");
        };
        assert!(*closed);
        assert!(matches!(&items[0], OverrideItem::Comment(c) if c == "comment"));
        assert!(matches!(
            &items[1],
            OverrideItem::Tag(OverrideTag {
                args: TagArgs::Point { x, y },
                ..
            }) if (*x - 960.0).abs() < 1e-6 && (*y - 540.0).abs() < 1e-6
        ));
        assert!(matches!(
            &items[5],
            OverrideItem::Tag(OverrideTag {
                args: TagArgs::Transform {
                    t1: Some(0),
                    t2: Some(400),
                    accel: Some(_),
                    items: nested,
                },
                ..
            }) if nested.len() == 2
        ));
        assert!(matches!(
            &items[6],
            OverrideItem::Tag(OverrideTag {
                args: TagArgs::ClipRect {
                    inverted: false,
                    ..
                },
                ..
            })
        ));
        assert!(matches!(
            &items[7],
            OverrideItem::Tag(OverrideTag {
                args: TagArgs::ClipVector {
                    inverted: true,
                    scale: 2,
                    ..
                },
                ..
            })
        ));
        assert!(blocks.iter().any(|b| matches!(
            b,
            TextBlock::Drawing { level: 1, commands } if commands == "m 0 0 l 50 0 50 50"
        )));
    }

    #[test]
    fn preserves_unclosed_override_block() {
        let line = r"Hello {\pos(1,2) unclosed";
        let blocks = parse_text_blocks(line);
        assert_eq!(serialize_text_blocks(&blocks), line);
        assert!(matches!(
            blocks.last(),
            Some(TextBlock::Override { closed: false, .. })
        ));
    }
}
