//! Lossless, fault-tolerant ASS parser.

use crate::charset::{decode_bytes, DetectedEncoding};
use crate::model::{
    AssEvent, AssStyle, EventEntry, EventKind, EventsSection, KeyValueEntry, KeyValueSection,
    LineEnding, RawSection, ScriptFile, Section, StyleEntry, StylesSection, DEFAULT_EVENT_FORMAT,
    DEFAULT_STYLE_FORMAT,
};
use crate::time::AssTime;
use crate::{ParseOutcome, ParseWarning};

/// Decodes raw bytes with charset detection and parses them as an ASS script.
#[must_use]
pub fn parse_ass_bytes(bytes: &[u8]) -> (ParseOutcome<ScriptFile>, DetectedEncoding) {
    let (text, encoding) = decode_bytes(bytes);
    let mut outcome = parse_ass(&text);
    outcome.value.has_bom = encoding.has_bom;
    if encoding.had_errors {
        outcome.warnings.push(ParseWarning {
            line: 1,
            message: format!(
                "encoding `{}` contained invalid byte sequences replaced with U+FFFD",
                encoding.name
            ),
        });
    }
    (outcome, encoding)
}

/// Parses an ASS script string losslessly while tolerating malformed lines and collecting warnings.
#[must_use]
pub fn parse_ass(input: &str) -> ParseOutcome<ScriptFile> {
    let (has_bom, body) = match input.strip_prefix('\u{FEFF}') {
        Some(rest) => (true, rest),
        None => (false, input),
    };

    let line_ending = detect_line_ending(body);
    let trailing_newline = body.ends_with('\n');

    let mut warnings = Vec::new();
    let mut sections: Vec<Section> = Vec::new();
    let mut current: Option<Section> = None;

    for (zero_idx, raw_line) in split_lines_lossless(body).enumerate() {
        let line_no = zero_idx + 1;
        let trimmed = raw_line.trim();

        if trimmed.starts_with('[') && trimmed.ends_with(']') && trimmed.len() >= 2 {
            if let Some(sec) = current.take() {
                sections.push(sec);
            }
            current = Some(start_section(raw_line, trimmed));
            continue;
        }

        if current.is_none() {
            current = Some(Section::Preamble(Vec::new()));
        }

        if let Some(sec) = current.as_mut() {
            push_section_line(sec, raw_line, line_no, &mut warnings);
        }
    }

    if let Some(sec) = current {
        sections.push(sec);
    }

    // Warn if standard sections are missing (tolerant: still returns a valid ScriptFile).
    if !sections.iter().any(|s| matches!(s, Section::ScriptInfo(_))) {
        warnings.push(ParseWarning {
            line: 1,
            message: "missing [Script Info] section".to_owned(),
        });
    }
    if !sections.iter().any(|s| matches!(s, Section::Events(_))) {
        warnings.push(ParseWarning {
            line: 1,
            message: "missing [Events] section".to_owned(),
        });
    }

    ParseOutcome {
        value: ScriptFile {
            has_bom,
            line_ending,
            trailing_newline,
            sections,
        },
        warnings,
    }
}

fn detect_line_ending(text: &str) -> LineEnding {
    // Inspect the first newline; default to CRLF if single-line.
    if let Some(pos) = text.find('\n') {
        if pos > 0 && text.as_bytes()[pos - 1] == b'\r' {
            return LineEnding::CrLf;
        }
        return LineEnding::Lf;
    }
    LineEnding::CrLf
}

fn split_lines_lossless(text: &str) -> impl Iterator<Item = &str> {
    let trimmed = text
        .strip_suffix("\r\n")
        .or_else(|| text.strip_suffix('\n'))
        .unwrap_or(text);
    let empty_input = text.is_empty();
    let iter = if empty_input {
        None
    } else {
        Some(
            trimmed
                .split('\n')
                .map(|l| l.strip_suffix('\r').unwrap_or(l)),
        )
    };
    iter.into_iter().flatten()
}

fn start_section(raw_header: &str, trimmed: &str) -> Section {
    if trimmed.eq_ignore_ascii_case("[Script Info]") {
        Section::ScriptInfo(KeyValueSection {
            header: raw_header.to_owned(),
            entries: Vec::new(),
        })
    } else if trimmed.eq_ignore_ascii_case("[Aegisub Project Garbage]") {
        Section::AegisubGarbage(KeyValueSection {
            header: raw_header.to_owned(),
            entries: Vec::new(),
        })
    } else if trimmed.eq_ignore_ascii_case("[V4+ Styles]")
        || trimmed.eq_ignore_ascii_case("[V4 Styles]")
    {
        Section::Styles(StylesSection {
            header: raw_header.to_owned(),
            format: DEFAULT_STYLE_FORMAT
                .iter()
                .map(|s| (*s).to_owned())
                .collect(),
            raw_format_line: None,
            entries: Vec::new(),
        })
    } else if trimmed.eq_ignore_ascii_case("[Events]") {
        Section::Events(EventsSection {
            header: raw_header.to_owned(),
            format: DEFAULT_EVENT_FORMAT
                .iter()
                .map(|s| (*s).to_owned())
                .collect(),
            raw_format_line: None,
            entries: Vec::new(),
        })
    } else if trimmed.eq_ignore_ascii_case("[Fonts]") {
        Section::Fonts(RawSection {
            header: raw_header.to_owned(),
            lines: Vec::new(),
        })
    } else if trimmed.eq_ignore_ascii_case("[Graphics]") {
        Section::Graphics(RawSection {
            header: raw_header.to_owned(),
            lines: Vec::new(),
        })
    } else {
        Section::Unknown(RawSection {
            header: raw_header.to_owned(),
            lines: Vec::new(),
        })
    }
}

fn push_section_line(
    section: &mut Section,
    raw_line: &str,
    line_no: usize,
    warnings: &mut Vec<ParseWarning>,
) {
    match section {
        Section::Preamble(lines) => lines.push(raw_line.to_owned()),
        Section::ScriptInfo(kv) | Section::AegisubGarbage(kv) => {
            kv.entries.push(parse_kv_entry(raw_line));
        }
        Section::Styles(sec) => parse_style_line(sec, raw_line, line_no, warnings),
        Section::Events(sec) => parse_event_line(sec, raw_line, line_no, warnings),
        Section::Fonts(raw) | Section::Graphics(raw) | Section::Unknown(raw) => {
            raw.lines.push(raw_line.to_owned());
        }
    }
}

fn parse_kv_entry(raw_line: &str) -> KeyValueEntry {
    if raw_line.is_empty() {
        return KeyValueEntry::Blank;
    }
    if raw_line.starts_with(';') || raw_line.starts_with("!:") {
        return KeyValueEntry::Comment(raw_line.to_owned());
    }
    if let Some(colon_idx) = raw_line.find(':') {
        let key = &raw_line[..colon_idx];
        let after_colon = &raw_line[colon_idx + 1..];
        let ws_len = after_colon
            .bytes()
            .take_while(|&b| b == b' ' || b == b'\t')
            .count();
        let separator = &raw_line[colon_idx..=colon_idx + ws_len];
        let value = &after_colon[ws_len..];
        return KeyValueEntry::KeyValue {
            key: key.to_owned(),
            separator: separator.to_owned(),
            value: value.to_owned(),
        };
    }
    KeyValueEntry::Raw(raw_line.to_owned())
}

fn parse_format_columns(raw_line: &str) -> Option<Vec<String>> {
    let rest = raw_line
        .strip_prefix("Format:")
        .or_else(|| raw_line.strip_prefix("format:"))?;
    let cols: Vec<String> = rest
        .split(',')
        .map(|c| c.trim().to_owned())
        .filter(|c| !c.is_empty())
        .collect();
    if cols.is_empty() {
        None
    } else {
        Some(cols)
    }
}

fn parse_style_line(
    sec: &mut StylesSection,
    raw_line: &str,
    line_no: usize,
    warnings: &mut Vec<ParseWarning>,
) {
    if raw_line.is_empty() {
        sec.entries.push(StyleEntry::Blank);
        return;
    }
    if raw_line.starts_with(';') {
        sec.entries.push(StyleEntry::Comment(raw_line.to_owned()));
        return;
    }
    if sec.raw_format_line.is_none() {
        if let Some(cols) = parse_format_columns(raw_line) {
            sec.format = cols;
            sec.raw_format_line = Some(raw_line.to_owned());
            return;
        }
    }
    let Some(rest) = raw_line
        .strip_prefix("Style:")
        .or_else(|| raw_line.strip_prefix("style:"))
    else {
        sec.entries.push(StyleEntry::Raw(raw_line.to_owned()));
        return;
    };

    if sec.raw_format_line.is_none() {
        warnings.push(ParseWarning {
            line: line_no,
            message: "Style line appeared before Format: line; using default V4+ style columns"
                .to_owned(),
        });
    }

    let payload = rest.strip_prefix(' ').unwrap_or(rest);
    let col_count = sec.format.len().max(1);
    let fields: Vec<&str> = payload.splitn(col_count, ',').collect();
    if fields.len() < sec.format.len() {
        warnings.push(ParseWarning {
            line: line_no,
            message: format!(
                "Style line has {} fields, expected {}",
                fields.len(),
                sec.format.len()
            ),
        });
    }

    let mut style = AssStyle {
        raw_line: Some(raw_line.to_owned()),
        ..AssStyle::default()
    };

    for (col_name, &raw_val) in sec.format.iter().zip(fields.iter()) {
        let val = raw_val.trim();
        apply_style_field(&mut style, col_name, raw_val, val, line_no, warnings);
    }

    sec.entries.push(StyleEntry::Style(Box::new(style)));
}

fn apply_style_field(
    style: &mut AssStyle,
    col_name: &str,
    raw_val: &str,
    val: &str,
    line_no: usize,
    warnings: &mut Vec<ParseWarning>,
) {
    match col_name.to_ascii_lowercase().as_str() {
        "name" => raw_val.clone_into(&mut style.name),
        "fontname" => raw_val.clone_into(&mut style.fontname),
        "fontsize" => parse_f64_field(val, &mut style.fontsize, "Fontsize", line_no, warnings),
        "primarycolour" => val.clone_into(&mut style.primary_colour),
        "secondarycolour" => val.clone_into(&mut style.secondary_colour),
        "outlinecolour" => val.clone_into(&mut style.outline_colour),
        "backcolour" => val.clone_into(&mut style.back_colour),
        "bold" => parse_i32_field(val, &mut style.bold, "Bold", line_no, warnings),
        "italic" => parse_i32_field(val, &mut style.italic, "Italic", line_no, warnings),
        "underline" => parse_i32_field(val, &mut style.underline, "Underline", line_no, warnings),
        "strikeout" => parse_i32_field(val, &mut style.strike_out, "StrikeOut", line_no, warnings),
        "scalex" => parse_f64_field(val, &mut style.scale_x, "ScaleX", line_no, warnings),
        "scaley" => parse_f64_field(val, &mut style.scale_y, "ScaleY", line_no, warnings),
        "spacing" => parse_f64_field(val, &mut style.spacing, "Spacing", line_no, warnings),
        "angle" => parse_f64_field(val, &mut style.angle, "Angle", line_no, warnings),
        "borderstyle" => {
            parse_i32_field(
                val,
                &mut style.border_style,
                "BorderStyle",
                line_no,
                warnings,
            );
        }
        "outline" => parse_f64_field(val, &mut style.outline, "Outline", line_no, warnings),
        "shadow" => parse_f64_field(val, &mut style.shadow, "Shadow", line_no, warnings),
        "alignment" => parse_i32_field(val, &mut style.alignment, "Alignment", line_no, warnings),
        "marginl" => parse_i32_field(val, &mut style.margin_l, "MarginL", line_no, warnings),
        "marginr" => parse_i32_field(val, &mut style.margin_r, "MarginR", line_no, warnings),
        "marginv" => parse_i32_field(val, &mut style.margin_v, "MarginV", line_no, warnings),
        "encoding" => parse_i32_field(val, &mut style.encoding, "Encoding", line_no, warnings),
        _ => {}
    }
}

fn parse_event_line(
    sec: &mut EventsSection,
    raw_line: &str,
    line_no: usize,
    warnings: &mut Vec<ParseWarning>,
) {
    if raw_line.is_empty() {
        sec.entries.push(EventEntry::Blank);
        return;
    }
    if raw_line.starts_with(';') {
        sec.entries.push(EventEntry::Comment(raw_line.to_owned()));
        return;
    }
    if sec.raw_format_line.is_none() {
        if let Some(cols) = parse_format_columns(raw_line) {
            sec.format = cols;
            sec.raw_format_line = Some(raw_line.to_owned());
            return;
        }
    }

    let (kind, rest) = if let Some(r) = raw_line.strip_prefix("Dialogue:") {
        (EventKind::Dialogue, r)
    } else if let Some(r) = raw_line.strip_prefix("Comment:") {
        (EventKind::Comment, r)
    } else {
        sec.entries.push(EventEntry::Raw(raw_line.to_owned()));
        return;
    };

    if sec.raw_format_line.is_none() {
        warnings.push(ParseWarning {
            line: line_no,
            message: "Event line appeared before Format: line; using default event columns"
                .to_owned(),
        });
    }

    let has_canonical_space = rest.starts_with(' ');
    let payload = rest.strip_prefix(' ').unwrap_or(rest);
    let col_count = sec.format.len().max(1);

    let mut event = AssEvent {
        kind,
        layer: 0,
        start: AssTime::ZERO,
        end: AssTime::ZERO,
        style: String::new(),
        name: String::new(),
        margin_l: 0,
        margin_r: 0,
        margin_v: 0,
        effect: String::new(),
        text: String::new(),
        raw_line: None,
    };

    let mut is_canonical = has_canonical_space;
    let mut parsed_fields = 0usize;

    for (col_name, raw_val) in sec.format.iter().zip(payload.splitn(col_count, ',')) {
        parsed_fields += 1;
        apply_event_field(
            &mut event,
            col_name,
            raw_val,
            line_no,
            warnings,
            &mut is_canonical,
        );
    }

    if parsed_fields < sec.format.len() {
        is_canonical = false;
        warnings.push(ParseWarning {
            line: line_no,
            message: format!(
                "{} line has {} fields, expected {}",
                kind.as_str(),
                parsed_fields,
                sec.format.len()
            ),
        });
    }

    // Only allocate a backup raw_line when the line has non-canonical formatting (e.g. "0000"
    // margins, "00:00:01.00" timestamps, missing space after ":", or unknown columns).
    if !is_canonical {
        event.raw_line = Some(raw_line.to_owned());
    }

    sec.entries.push(EventEntry::Event(event));
}

fn apply_event_field(
    event: &mut AssEvent,
    col_name: &str,
    raw_val: &str,
    line_no: usize,
    warnings: &mut Vec<ParseWarning>,
    is_canonical: &mut bool,
) {
    match col_name {
        "Layer" | "layer" => {
            if let Ok(v) = raw_val.parse::<i32>() {
                event.layer = v;
                if !is_canonical_int(raw_val, v) {
                    *is_canonical = false;
                }
            } else if let Ok(v) = raw_val.trim().parse::<i32>() {
                event.layer = v;
                *is_canonical = false;
            } else {
                *is_canonical = false;
                warnings.push(ParseWarning {
                    line: line_no,
                    message: format!("invalid Layer `{raw_val}`; defaulting to 0"),
                });
            }
        }
        "Start" | "start" => {
            if let Some(t) = AssTime::parse_ass(raw_val) {
                event.start = t;
                if !is_canonical_time(raw_val, t) {
                    *is_canonical = false;
                }
            } else {
                *is_canonical = false;
                warnings.push(ParseWarning {
                    line: line_no,
                    message: format!(
                        "invalid Start timestamp `{raw_val}`; defaulting to 0:00:00.00"
                    ),
                });
            }
        }
        "End" | "end" => {
            if let Some(t) = AssTime::parse_ass(raw_val) {
                event.end = t;
                if !is_canonical_time(raw_val, t) {
                    *is_canonical = false;
                }
            } else {
                *is_canonical = false;
                warnings.push(ParseWarning {
                    line: line_no,
                    message: format!("invalid End timestamp `{raw_val}`; defaulting to 0:00:00.00"),
                });
            }
        }
        "Style" | "style" => raw_val.clone_into(&mut event.style),
        "Name" | "name" | "Actor" | "actor" => raw_val.clone_into(&mut event.name),
        "MarginL" | "marginl" => {
            parse_event_int(
                raw_val,
                &mut event.margin_l,
                "MarginL",
                line_no,
                warnings,
                is_canonical,
            );
        }
        "MarginR" | "marginr" => {
            parse_event_int(
                raw_val,
                &mut event.margin_r,
                "MarginR",
                line_no,
                warnings,
                is_canonical,
            );
        }
        "MarginV" | "marginv" => {
            parse_event_int(
                raw_val,
                &mut event.margin_v,
                "MarginV",
                line_no,
                warnings,
                is_canonical,
            );
        }
        "Effect" | "effect" => raw_val.clone_into(&mut event.effect),
        "Text" | "text" => raw_val.clone_into(&mut event.text),
        _ => {
            *is_canonical = false;
        }
    }
}

fn parse_event_int(
    raw_val: &str,
    dest: &mut i32,
    field: &str,
    line_no: usize,
    warnings: &mut Vec<ParseWarning>,
    is_canonical: &mut bool,
) {
    if let Ok(v) = raw_val.parse::<i32>() {
        *dest = v;
        if !is_canonical_int(raw_val, v) {
            *is_canonical = false;
        }
    } else if let Ok(v) = raw_val.trim().parse::<i32>() {
        *dest = v;
        *is_canonical = false;
    } else {
        *is_canonical = false;
        warnings.push(ParseWarning {
            line: line_no,
            message: format!("invalid {field} `{raw_val}`; defaulting to 0"),
        });
    }
}

fn is_canonical_int(raw: &str, value: i32) -> bool {
    let bytes = raw.as_bytes();
    if bytes.is_empty() {
        return false;
    }
    if value == 0 {
        return bytes == b"0";
    }
    let digits = if value < 0 {
        if bytes[0] != b'-' {
            return false;
        }
        &bytes[1..]
    } else {
        bytes
    };
    !digits.is_empty() && digits[0] != b'0' && digits.iter().all(u8::is_ascii_digit)
}

fn is_canonical_time(raw: &str, time: AssTime) -> bool {
    // Fast allocation-free check for canonical `H:MM:SS.cc` (1-digit hour when < 10).
    let (h, m, s, cs) = time.to_hms_cs();
    let bytes = raw.as_bytes();
    if h < 10 {
        if bytes.len() != 10 {
            return false;
        }
        #[allow(clippy::cast_possible_truncation)]
        let expected = [
            b'0' + (h as u8),
            b':',
            b'0' + ((m / 10) as u8),
            b'0' + ((m % 10) as u8),
            b':',
            b'0' + ((s / 10) as u8),
            b'0' + ((s % 10) as u8),
            b'.',
            b'0' + ((cs / 10) as u8),
            b'0' + ((cs % 10) as u8),
        ];
        bytes == expected
    } else {
        raw == time.to_ass_string()
    }
}

fn parse_i32_field(
    val: &str,
    dest: &mut i32,
    name: &str,
    line_no: usize,
    warnings: &mut Vec<ParseWarning>,
) {
    match val.parse::<i32>() {
        Ok(v) => *dest = v,
        Err(_) => warnings.push(ParseWarning {
            line: line_no,
            message: format!("invalid integer for style `{name}`: `{val}`"),
        }),
    }
}

fn parse_f64_field(
    val: &str,
    dest: &mut f64,
    name: &str,
    line_no: usize,
    warnings: &mut Vec<ParseWarning>,
) {
    match val.parse::<f64>() {
        Ok(v) if v.is_finite() => *dest = v,
        _ => warnings.push(ParseWarning {
            line: line_no,
            message: format!("invalid number for style `{name}`: `{val}`"),
        }),
    }
}
