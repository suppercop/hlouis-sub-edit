//! Fast lossless ASS serializer preserving BOM, CRLF/LF, trailing newline, section order,
//! custom `Format:` column orders, and verbatim `[Fonts]`/`[Graphics]`/unknown sections.

use crate::model::{
    AssEvent, AssStyle, EventEntry, EventsSection, KeyValueEntry, KeyValueSection, RawSection,
    ScriptFile, Section, StyleEntry, StylesSection,
};
use std::fmt::Write as _;

/// Serializes `script` to a UTF-8 `String` (including leading `\u{FEFF}` if `script.has_bom`).
#[must_use]
pub fn serialize_ass(script: &ScriptFile) -> String {
    let estimated = estimate_capacity(script);
    let mut out = String::with_capacity(estimated);
    if script.has_bom {
        out.push('\u{FEFF}');
    }

    let nl = script.line_ending.as_str();
    let mut first_line = true;

    for section in &script.sections {
        match section {
            Section::Preamble(lines) => {
                for line in lines {
                    start_line(&mut out, nl, &mut first_line);
                    out.push_str(line);
                }
            }
            Section::ScriptInfo(kv) | Section::AegisubGarbage(kv) => {
                write_kv_section(&mut out, kv, nl, &mut first_line);
            }
            Section::Styles(sec) => {
                write_styles_section(&mut out, sec, nl, &mut first_line);
            }
            Section::Events(sec) => {
                write_events_section(&mut out, sec, nl, &mut first_line);
            }
            Section::Fonts(raw) | Section::Graphics(raw) | Section::Unknown(raw) => {
                write_raw_section(&mut out, raw, nl, &mut first_line);
            }
        }
    }

    if script.trailing_newline && !first_line {
        out.push_str(nl);
    }

    out
}

/// Serializes `script` to UTF-8 bytes (including UTF-8 BOM `EF BB BF` if `script.has_bom`).
#[must_use]
pub fn serialize_ass_bytes(script: &ScriptFile) -> Vec<u8> {
    serialize_ass(script).into_bytes()
}

fn estimate_capacity(script: &ScriptFile) -> usize {
    let mut cap = 512usize;
    for sec in &script.sections {
        match sec {
            Section::Events(ev) => cap = cap.saturating_add(ev.entries.len().saturating_mul(96)),
            Section::Styles(st) => cap = cap.saturating_add(st.entries.len().saturating_mul(128)),
            Section::Fonts(r) | Section::Graphics(r) | Section::Unknown(r) => {
                cap = cap.saturating_add(r.lines.len().saturating_mul(84));
            }
            Section::ScriptInfo(kv) | Section::AegisubGarbage(kv) => {
                cap = cap.saturating_add(kv.entries.len().saturating_mul(40));
            }
            Section::Preamble(lines) => {
                cap = cap.saturating_add(lines.len().saturating_mul(40));
            }
        }
    }
    cap
}

fn start_line(out: &mut String, nl: &str, first_line: &mut bool) {
    if !*first_line {
        out.push_str(nl);
    }
    *first_line = false;
}

fn write_kv_section(out: &mut String, sec: &KeyValueSection, nl: &str, first_line: &mut bool) {
    start_line(out, nl, first_line);
    out.push_str(&sec.header);

    for entry in &sec.entries {
        start_line(out, nl, first_line);
        match entry {
            KeyValueEntry::KeyValue {
                key,
                separator,
                value,
            } => {
                out.push_str(key);
                out.push_str(separator);
                out.push_str(value);
            }
            KeyValueEntry::Comment(line) | KeyValueEntry::Raw(line) => out.push_str(line),
            KeyValueEntry::Blank => {}
        }
    }
}

fn write_raw_section(out: &mut String, sec: &RawSection, nl: &str, first_line: &mut bool) {
    start_line(out, nl, first_line);
    out.push_str(&sec.header);
    for line in &sec.lines {
        start_line(out, nl, first_line);
        out.push_str(line);
    }
}

fn write_styles_section(out: &mut String, sec: &StylesSection, nl: &str, first_line: &mut bool) {
    start_line(out, nl, first_line);
    out.push_str(&sec.header);

    if let Some(raw_fmt) = &sec.raw_format_line {
        start_line(out, nl, first_line);
        out.push_str(raw_fmt);
    } else if !sec.format.is_empty() {
        start_line(out, nl, first_line);
        out.push_str("Format: ");
        for (i, col) in sec.format.iter().enumerate() {
            if i > 0 {
                out.push_str(", ");
            }
            out.push_str(col);
        }
    }

    for entry in &sec.entries {
        start_line(out, nl, first_line);
        match entry {
            StyleEntry::Style(style) => write_style_line(out, style, &sec.format),
            StyleEntry::Comment(line) | StyleEntry::Raw(line) => out.push_str(line),
            StyleEntry::Blank => {}
        }
    }
}

fn write_style_line(out: &mut String, style: &AssStyle, format: &[String]) {
    if let Some(raw) = &style.raw_line {
        out.push_str(raw);
        return;
    }
    out.push_str("Style: ");
    for (i, col) in format.iter().enumerate() {
        if i > 0 {
            out.push(',');
        }
        match col.to_ascii_lowercase().as_str() {
            "name" => out.push_str(&style.name),
            "fontname" => out.push_str(&style.fontname),
            "fontsize" => write_ass_number(out, style.fontsize),
            "primarycolour" => out.push_str(&style.primary_colour),
            "secondarycolour" => out.push_str(&style.secondary_colour),
            "outlinecolour" => out.push_str(&style.outline_colour),
            "backcolour" => out.push_str(&style.back_colour),
            "bold" => {
                let _ = write!(out, "{}", style.bold);
            }
            "italic" => {
                let _ = write!(out, "{}", style.italic);
            }
            "underline" => {
                let _ = write!(out, "{}", style.underline);
            }
            "strikeout" => {
                let _ = write!(out, "{}", style.strike_out);
            }
            "scalex" => write_ass_number(out, style.scale_x),
            "scaley" => write_ass_number(out, style.scale_y),
            "spacing" => write_ass_number(out, style.spacing),
            "angle" => write_ass_number(out, style.angle),
            "borderstyle" => {
                let _ = write!(out, "{}", style.border_style);
            }
            "outline" => write_ass_number(out, style.outline),
            "shadow" => write_ass_number(out, style.shadow),
            "alignment" => {
                let _ = write!(out, "{}", style.alignment);
            }
            "marginl" => {
                let _ = write!(out, "{}", style.margin_l);
            }
            "marginr" => {
                let _ = write!(out, "{}", style.margin_r);
            }
            "marginv" => {
                let _ = write!(out, "{}", style.margin_v);
            }
            "encoding" => {
                let _ = write!(out, "{}", style.encoding);
            }
            _ => {}
        }
    }
}

fn write_ass_number(out: &mut String, val: f64) {
    if val.fract() == 0.0 && val.abs() < 1e15 {
        #[allow(clippy::cast_possible_truncation)]
        let int_val = val as i64;
        let _ = write!(out, "{int_val}");
    } else {
        let _ = write!(out, "{val}");
    }
}

fn write_events_section(out: &mut String, sec: &EventsSection, nl: &str, first_line: &mut bool) {
    start_line(out, nl, first_line);
    out.push_str(&sec.header);

    if let Some(raw_fmt) = &sec.raw_format_line {
        start_line(out, nl, first_line);
        out.push_str(raw_fmt);
    } else if !sec.format.is_empty() {
        start_line(out, nl, first_line);
        out.push_str("Format: ");
        for (i, col) in sec.format.iter().enumerate() {
            if i > 0 {
                out.push_str(", ");
            }
            out.push_str(col);
        }
    }

    let is_standard_order = is_default_event_format(&sec.format);

    for entry in &sec.entries {
        start_line(out, nl, first_line);
        match entry {
            EventEntry::Event(ev) => {
                if let Some(raw) = &ev.raw_line {
                    out.push_str(raw);
                } else if is_standard_order {
                    write_standard_event(out, ev);
                } else {
                    write_custom_event(out, ev, &sec.format);
                }
            }
            EventEntry::Comment(line) | EventEntry::Raw(line) => out.push_str(line),
            EventEntry::Blank => {}
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

fn write_standard_event(out: &mut String, ev: &AssEvent) {
    out.push_str(ev.kind.as_str());
    out.push_str(": ");
    let _ = write!(out, "{},", ev.layer);
    ev.start.write_ass(out);
    out.push(',');
    ev.end.write_ass(out);
    out.push(',');
    out.push_str(&ev.style);
    out.push(',');
    out.push_str(&ev.name);
    let _ = write!(out, ",{},{},{},", ev.margin_l, ev.margin_r, ev.margin_v);
    out.push_str(&ev.effect);
    out.push(',');
    out.push_str(&ev.text);
}

fn write_custom_event(out: &mut String, ev: &AssEvent, format: &[String]) {
    out.push_str(ev.kind.as_str());
    out.push_str(": ");
    for (i, col) in format.iter().enumerate() {
        if i > 0 {
            out.push(',');
        }
        match col.to_ascii_lowercase().as_str() {
            "layer" => {
                let _ = write!(out, "{}", ev.layer);
            }
            "start" => ev.start.write_ass(out),
            "end" => ev.end.write_ass(out),
            "style" => out.push_str(&ev.style),
            "name" | "actor" => out.push_str(&ev.name),
            "marginl" => {
                let _ = write!(out, "{}", ev.margin_l);
            }
            "marginr" => {
                let _ = write!(out, "{}", ev.margin_r);
            }
            "marginv" => {
                let _ = write!(out, "{}", ev.margin_v);
            }
            "effect" => out.push_str(&ev.effect),
            "text" => out.push_str(&ev.text),
            _ => {}
        }
    }
}
