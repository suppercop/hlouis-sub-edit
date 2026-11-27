//! Lossless, fault-tolerant ASS parser.

use crate::charset::{decode_bytes, DetectedEncoding};
use crate::model::{
    AssEvent, AssStyle, EventEntry, EventKind, EventsSection, KeyValueEntry, KeyValueSection,
    LineEnding, RawSection, ScriptFile, Section, StyleEntry, StylesSection, DEFAULT_STYLE_FORMAT,
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
    let mut mixed_endings: Option<Vec<LineEnding>> = None;
    let mut ev_default_fmt = true;
    let mut ev_all_only = true;

    for (zero_idx, (raw_line, term)) in split_lines_with_endings(body).enumerate() {
        if let Some(le) = term {
            if let Some(ref mut endings) = mixed_endings {
                endings.push(le);
            } else if le != line_ending {
                let mut v = vec![line_ending; zero_idx];
                v.push(le);
                mixed_endings = Some(v);
            }
        }
        let line_no = zero_idx + 1;
        let maybe_header = raw_line
            .as_bytes()
            .first()
            .is_some_and(|&b| b == b'[' || b.is_ascii_whitespace());
        if maybe_header {
            let trimmed = raw_line.trim();
            if trimmed.starts_with('[') && trimmed.ends_with(']') && trimmed.len() >= 2 {
                if let Some(mut sec) = current.take() {
                    if let Section::Events(ref mut ev) = sec {
                        ev.set_all_events_cache(ev_all_only);
                    }
                    sections.push(sec);
                }
                let mut next_sec = start_section(raw_line, trimmed);
                if let Section::Events(ref mut ev) = next_sec {
                    // Estimate event count from input size (typically ~100-130 bytes/line) to avoid Vec regrowth.
                    let est = (body.len() / 100).saturating_add(8);
                    ev.entries.reserve(est);
                    ev_default_fmt = true;
                    ev_all_only = true;
                }
                current = Some(next_sec);
                continue;
            }
        }

        if current.is_none() {
            current = Some(Section::Preamble(Vec::new()));
        }

        match current.as_mut() {
            Some(Section::Events(sec)) => {
                parse_event_line(
                    sec,
                    raw_line,
                    line_no,
                    &mut warnings,
                    &mut ev_default_fmt,
                    &mut ev_all_only,
                );
            }
            Some(sec) => push_section_line(sec, raw_line, line_no, &mut warnings),
            None => {}
        }
    }

    if let Some(mut sec) = current {
        if let Section::Events(ref mut ev) = sec {
            ev.set_all_events_cache(ev_all_only);
        }
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
            line_endings: mixed_endings,
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

fn split_lines_with_endings(text: &str) -> impl Iterator<Item = (&str, Option<LineEnding>)> {
    struct LineIter<'a> {
        rest: &'a str,
    }
    impl<'a> Iterator for LineIter<'a> {
        type Item = (&'a str, Option<LineEnding>);
        fn next(&mut self) -> Option<Self::Item> {
            if self.rest.is_empty() {
                return None;
            }
            if let Some(pos) = self.rest.find('\n') {
                let (line, le) = if pos > 0 && self.rest.as_bytes()[pos - 1] == b'\r' {
                    (&self.rest[..pos - 1], LineEnding::CrLf)
                } else {
                    (&self.rest[..pos], LineEnding::Lf)
                };
                self.rest = &self.rest[pos + 1..];
                Some((line, Some(le)))
            } else {
                let line = self.rest;
                self.rest = "";
                Some((line, None))
            }
        }
    }
    LineIter { rest: text }
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
        Section::Events(EventsSection::new(raw_header))
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
        Section::Events(sec) => {
            let mut default_fmt = is_default_event_format(&sec.format);
            let mut all_ev = true;
            parse_event_line(
                sec,
                raw_line,
                line_no,
                warnings,
                &mut default_fmt,
                &mut all_ev,
            );
        }
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
    default_fmt: &mut bool,
    all_ev: &mut bool,
) {
    if raw_line.is_empty() {
        *all_ev = false;
        sec.entries.push(EventEntry::Blank);
        return;
    }
    if raw_line.starts_with(';') {
        *all_ev = false;
        sec.entries.push(EventEntry::Comment(raw_line.to_owned()));
        return;
    }
    if sec.raw_format_line.is_none() {
        if let Some(cols) = parse_format_columns(raw_line) {
            *default_fmt = is_default_event_format(&cols);
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
        *all_ev = false;
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
    let parsed_fields = if *default_fmt {
        parse_default_event_fields(&mut event, payload, line_no, warnings, &mut is_canonical)
    } else {
        let mut count = 0usize;
        for (col_name, raw_val) in sec.format.iter().zip(payload.splitn(col_count, ',')) {
            count += 1;
            apply_event_field(
                &mut event,
                col_name,
                raw_val,
                line_no,
                warnings,
                &mut is_canonical,
            );
        }
        count
    };

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

#[inline]
fn parse_default_event_fields(
    event: &mut AssEvent,
    payload: &str,
    line_no: usize,
    warnings: &mut Vec<ParseWarning>,
    is_canonical: &mut bool,
) -> usize {
    // Fast path for complete 10-column lines using direct `split_once(',')` (memchr).
    let Some((f_layer, r)) = payload.split_once(',') else {
        return parse_default_event_fields_partial(event, payload, line_no, warnings, is_canonical);
    };
    let Some((f_start, r)) = r.split_once(',') else {
        return parse_default_event_fields_partial(event, payload, line_no, warnings, is_canonical);
    };
    let Some((f_end, r)) = r.split_once(',') else {
        return parse_default_event_fields_partial(event, payload, line_no, warnings, is_canonical);
    };
    let Some((f_style, r)) = r.split_once(',') else {
        return parse_default_event_fields_partial(event, payload, line_no, warnings, is_canonical);
    };
    let Some((f_name, r)) = r.split_once(',') else {
        return parse_default_event_fields_partial(event, payload, line_no, warnings, is_canonical);
    };
    let Some((left_m, r)) = r.split_once(',') else {
        return parse_default_event_fields_partial(event, payload, line_no, warnings, is_canonical);
    };
    let Some((right_m, r)) = r.split_once(',') else {
        return parse_default_event_fields_partial(event, payload, line_no, warnings, is_canonical);
    };
    let Some((vert_m, r)) = r.split_once(',') else {
        return parse_default_event_fields_partial(event, payload, line_no, warnings, is_canonical);
    };
    let Some((f_eff, f_text)) = r.split_once(',') else {
        return parse_default_event_fields_partial(event, payload, line_no, warnings, is_canonical);
    };

    parse_event_int(
        f_layer,
        &mut event.layer,
        "Layer",
        line_no,
        warnings,
        is_canonical,
    );
    parse_event_time(
        f_start,
        &mut event.start,
        "Start",
        line_no,
        warnings,
        is_canonical,
    );
    parse_event_time(
        f_end,
        &mut event.end,
        "End",
        line_no,
        warnings,
        is_canonical,
    );
    f_style.clone_into(&mut event.style);
    f_name.clone_into(&mut event.name);
    parse_event_int(
        left_m,
        &mut event.margin_l,
        "MarginL",
        line_no,
        warnings,
        is_canonical,
    );
    parse_event_int(
        right_m,
        &mut event.margin_r,
        "MarginR",
        line_no,
        warnings,
        is_canonical,
    );
    parse_event_int(
        vert_m,
        &mut event.margin_v,
        "MarginV",
        line_no,
        warnings,
        is_canonical,
    );
    f_eff.clone_into(&mut event.effect);
    f_text.clone_into(&mut event.text);
    10
}

fn parse_default_event_fields_partial(
    event: &mut AssEvent,
    payload: &str,
    line_no: usize,
    warnings: &mut Vec<ParseWarning>,
    is_canonical: &mut bool,
) -> usize {
    let mut parsed_fields = 0usize;
    let mut it = payload.splitn(10, ',');
    if let Some(raw_val) = it.next() {
        parsed_fields += 1;
        parse_event_int(
            raw_val,
            &mut event.layer,
            "Layer",
            line_no,
            warnings,
            is_canonical,
        );
    }
    if let Some(raw_val) = it.next() {
        parsed_fields += 1;
        parse_event_time(
            raw_val,
            &mut event.start,
            "Start",
            line_no,
            warnings,
            is_canonical,
        );
    }
    if let Some(raw_val) = it.next() {
        parsed_fields += 1;
        parse_event_time(
            raw_val,
            &mut event.end,
            "End",
            line_no,
            warnings,
            is_canonical,
        );
    }
    if let Some(raw_val) = it.next() {
        parsed_fields += 1;
        raw_val.clone_into(&mut event.style);
    }
    if let Some(raw_val) = it.next() {
        parsed_fields += 1;
        raw_val.clone_into(&mut event.name);
    }
    if let Some(raw_val) = it.next() {
        parsed_fields += 1;
        parse_event_int(
            raw_val,
            &mut event.margin_l,
            "MarginL",
            line_no,
            warnings,
            is_canonical,
        );
    }
    if let Some(raw_val) = it.next() {
        parsed_fields += 1;
        parse_event_int(
            raw_val,
            &mut event.margin_r,
            "MarginR",
            line_no,
            warnings,
            is_canonical,
        );
    }
    if let Some(raw_val) = it.next() {
        parsed_fields += 1;
        parse_event_int(
            raw_val,
            &mut event.margin_v,
            "MarginV",
            line_no,
            warnings,
            is_canonical,
        );
    }
    if let Some(raw_val) = it.next() {
        parsed_fields += 1;
        raw_val.clone_into(&mut event.effect);
    }
    if let Some(raw_val) = it.next() {
        parsed_fields += 1;
        raw_val.clone_into(&mut event.text);
    }
    parsed_fields
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
            parse_event_time(
                raw_val,
                &mut event.start,
                "Start",
                line_no,
                warnings,
                is_canonical,
            );
        }
        "End" | "end" => {
            parse_event_time(
                raw_val,
                &mut event.end,
                "End",
                line_no,
                warnings,
                is_canonical,
            );
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

fn is_default_event_format(format: &[String]) -> bool {
    format.len() == 10
        && format[0] == "Layer"
        && format[1] == "Start"
        && format[2] == "End"
        && format[3] == "Style"
        && format[4] == "Name"
        && format[5] == "MarginL"
        && format[6] == "MarginR"
        && format[7] == "MarginV"
        && format[8] == "Effect"
        && format[9] == "Text"
}

#[inline]
fn parse_event_time(
    raw_val: &str,
    dest: &mut AssTime,
    field: &str,
    line_no: usize,
    warnings: &mut Vec<ParseWarning>,
    is_canonical: &mut bool,
) {
    if let Some((t, canonical)) = AssTime::parse_ass_canonical(raw_val) {
        *dest = t;
        if !canonical {
            *is_canonical = false;
        }
    } else {
        *is_canonical = false;
        warnings.push(ParseWarning {
            line: line_no,
            message: format!("invalid {field} timestamp `{raw_val}`; defaulting to 0:00:00.00"),
        });
    }
}

#[inline]
fn parse_event_int(
    raw_val: &str,
    dest: &mut i32,
    field: &str,
    line_no: usize,
    warnings: &mut Vec<ParseWarning>,
    is_canonical: &mut bool,
) {
    let b = raw_val.as_bytes();
    if b.len() == 1 && b[0].is_ascii_digit() {
        *dest = i32::from(b[0] - b'0');
        return;
    }
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
