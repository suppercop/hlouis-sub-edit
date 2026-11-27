//! `SubRip` (`.srt`) reader and writer with HTML-style formatting <-> ASS override conversion.

use crate::charset::{decode_bytes, DetectedEncoding};
use crate::model::{AssEvent, EventEntry, EventKind, LineEnding, ScriptFile};
use crate::tags::{parse_text_blocks, EscapeKind, OverrideItem, TextBlock};
use crate::time::AssTime;
use crate::{ParseOutcome, ParseWarning};
use std::fmt::Write as _;

/// Decodes raw bytes with charset detection and parses them as an SRT file into a [`ScriptFile`].
#[must_use]
pub fn parse_srt_bytes(bytes: &[u8]) -> (ParseOutcome<ScriptFile>, DetectedEncoding) {
    let (text, encoding) = decode_bytes(bytes);
    let mut outcome = parse_srt(&text);
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

/// Parses an SRT subtitle string into an ASS [`ScriptFile`].
///
/// Tolerates UTF-8 BOM, CRLF/LF, missing blank lines between cues, and inline HTML formatting
/// (`<i>`, `<b>`, `<u>`, `<s>`, `<font color="...">`).
#[must_use]
pub fn parse_srt(input: &str) -> ParseOutcome<ScriptFile> {
    let (has_bom, body) = match input.strip_prefix('\u{FEFF}') {
        Some(rest) => (true, rest),
        None => (false, input),
    };

    let line_ending = if body.contains("\r\n") {
        LineEnding::CrLf
    } else {
        LineEnding::Lf
    };

    let lines: Vec<&str> = body
        .split('\n')
        .map(|l| l.strip_suffix('\r').unwrap_or(l))
        .collect();

    let mut script = ScriptFile::new_default();
    script.has_bom = has_bom;
    script.line_ending = line_ending;
    let mut warnings = Vec::new();
    let mut events = Vec::new();

    let mut i = 0usize;
    while i < lines.len() {
        let trimmed = lines[i].trim();
        if trimmed.is_empty() {
            i += 1;
            continue;
        }

        // Optional numeric cue index line followed by a timestamp line.
        let timing_idx = if parse_srt_timing_line(trimmed).is_some() {
            i
        } else if trimmed.bytes().all(|b| b.is_ascii_digit())
            && i + 1 < lines.len()
            && parse_srt_timing_line(lines[i + 1].trim()).is_some()
        {
            i + 1
        } else {
            warnings.push(ParseWarning {
                line: i + 1,
                message: format!("unexpected SRT line outside cue: `{trimmed}`"),
            });
            i += 1;
            continue;
        };

        let Some((start, end)) = parse_srt_timing_line(lines[timing_idx].trim()) else {
            i = timing_idx + 1;
            continue;
        };

        // Collect text lines until a blank line OR the start of the next cue (tolerant of missing blank lines).
        let mut text_lines = Vec::new();
        i = timing_idx + 1;
        while i < lines.len() {
            let cur = lines[i];
            let cur_trim = cur.trim();
            if cur_trim.is_empty() {
                i += 1;
                break;
            }
            if parse_srt_timing_line(cur_trim).is_some() {
                break;
            }
            if cur_trim.bytes().all(|b| b.is_ascii_digit())
                && i + 1 < lines.len()
                && parse_srt_timing_line(lines[i + 1].trim()).is_some()
            {
                break;
            }
            text_lines.push(cur);
            i += 1;
        }

        let joined = text_lines.join(r"\N");
        let ass_text = srt_html_to_ass(&joined);
        events.push(EventEntry::Event(AssEvent {
            kind: EventKind::Dialogue,
            layer: 0,
            start,
            end,
            style: "Default".to_owned(),
            name: String::new(),
            margin_l: 0,
            margin_r: 0,
            margin_v: 0,
            effect: String::new(),
            text: ass_text,
            raw_line: None,
        }));
    }

    let events_sec = script.ensure_events_section_mut();
    events_sec.entries = events;

    ParseOutcome {
        value: script,
        warnings,
    }
}

/// Serializes all `Dialogue` events in `script` to SRT format, converting basic ASS tags
/// (`\i`, `\b`, `\u`, `\s`, `\c`/`\1c`, `\N`/`\n`) back to SRT HTML formatting.
#[must_use]
pub fn write_srt(script: &ScriptFile) -> String {
    let nl = script.line_ending.as_str();
    let mut out = String::new();
    let mut cue_num = 0usize;

    for ev in script.events() {
        if ev.kind != EventKind::Dialogue {
            continue;
        }
        cue_num += 1;
        if cue_num > 1 {
            out.push_str(nl);
        }
        let _ = write!(
            out,
            "{cue_num}{nl}{} --> {}{nl}{}{nl}",
            ev.start.to_srt_string(),
            ev.end.to_srt_string(),
            ass_to_srt_html(&ev.text, nl)
        );
    }

    out
}

fn parse_srt_timing_line(line: &str) -> Option<(AssTime, AssTime)> {
    let (lhs, rhs) = line.split_once("-->")?;
    let start = AssTime::parse_srt(lhs)?;
    // Ignore optional trailing positioning coordinates (e.g. "X1:...").
    let end_token = rhs.split_whitespace().next()?;
    let end = AssTime::parse_srt(end_token)?;
    Some((start, end))
}

fn srt_html_to_ass(input: &str) -> String {
    let mut out = String::with_capacity(input.len());
    let mut rest = input;

    while let Some(lt) = rest.find('<') {
        out.push_str(&rest[..lt]);
        let after_lt = &rest[lt + 1..];
        let Some(gt) = after_lt.find('>') else {
            out.push_str(&rest[lt..]);
            return out;
        };
        let tag_content = after_lt[..gt].trim();
        let lower = tag_content.to_ascii_lowercase();

        match lower.as_str() {
            "i" => out.push_str(r"{\i1}"),
            "/i" => out.push_str(r"{\i0}"),
            "b" => out.push_str(r"{\b1}"),
            "/b" => out.push_str(r"{\b0}"),
            "u" => out.push_str(r"{\u1}"),
            "/u" => out.push_str(r"{\u0}"),
            "s" => out.push_str(r"{\s1}"),
            "/s" => out.push_str(r"{\s0}"),
            "/font" => out.push_str(r"{\c}"),
            _ if lower.starts_with("font ") || lower == "font" => {
                if let Some(bgr) = extract_font_color_bgr(tag_content) {
                    let _ = write!(out, r"{{\c&H{bgr}&}}");
                }
            }
            _ => {
                // Unknown HTML-like tag (or literal `<...>`): keep verbatim.
                out.push('<');
                out.push_str(&after_lt[..=gt]);
            }
        }
        rest = &after_lt[gt + 1..];
    }
    out.push_str(rest);
    out
}

fn extract_font_color_bgr(tag_content: &str) -> Option<String> {
    let lower = tag_content.to_ascii_lowercase();
    let idx = lower.find("color")?;
    let after = tag_content[idx + 5..].trim_start();
    let after_eq = after.strip_prefix('=')?.trim_start();
    let val = if let Some(quoted) = after_eq.strip_prefix('"') {
        quoted.split('"').next()?
    } else if let Some(quoted) = after_eq.strip_prefix('\'') {
        quoted.split('\'').next()?
    } else {
        after_eq.split_whitespace().next()?
    };

    let lowered = val.trim().to_ascii_lowercase();
    let hex = match lowered.as_str() {
        "white" => "FFFFFF",
        "black" => "000000",
        "red" => "FF0000",
        "green" => "008000",
        "blue" => "0000FF",
        "yellow" => "FFFF00",
        "cyan" | "aqua" => "00FFFF",
        "magenta" | "fuchsia" => "FF00FF",
        other => other.strip_prefix('#').unwrap_or(other),
    };

    if hex.len() == 6 && hex.bytes().all(|b| b.is_ascii_hexdigit()) {
        let r = &hex[0..2];
        let g = &hex[2..4];
        let b = &hex[4..6];
        Some(format!("{b}{g}{r}").to_ascii_uppercase())
    } else {
        None
    }
}

fn ass_to_srt_html(text: &str, nl: &str) -> String {
    let blocks = parse_text_blocks(text);
    let mut out = String::with_capacity(text.len());
    let mut italic = false;
    let mut bold = false;
    let mut underline = false;
    let mut strike = false;
    let mut font_open = false;

    for block in &blocks {
        match block {
            TextBlock::Plain(s) => out.push_str(s),
            TextBlock::Escape(EscapeKind::HardNewline | EscapeKind::SoftNewline) => {
                out.push_str(nl);
            }
            TextBlock::Escape(EscapeKind::HardSpace) => out.push(' '),
            TextBlock::Drawing { .. } => {}
            TextBlock::Override { items, .. } => {
                for item in items {
                    let OverrideItem::Tag(tag) = item else {
                        continue;
                    };
                    match tag.name.as_str() {
                        "i" => {
                            let on = tag.raw_args.trim() == "1";
                            if on && !italic {
                                out.push_str("<i>");
                                italic = true;
                            } else if !on && italic {
                                out.push_str("</i>");
                                italic = false;
                            }
                        }
                        "b" => {
                            let arg = tag.raw_args.trim();
                            let on = arg == "1" || arg.parse::<i32>().is_ok_and(|w| w >= 600);
                            if on && !bold {
                                out.push_str("<b>");
                                bold = true;
                            } else if !on && bold {
                                out.push_str("</b>");
                                bold = false;
                            }
                        }
                        "u" => {
                            let on = tag.raw_args.trim() == "1";
                            if on && !underline {
                                out.push_str("<u>");
                                underline = true;
                            } else if !on && underline {
                                out.push_str("</u>");
                                underline = false;
                            }
                        }
                        "s" => {
                            let on = tag.raw_args.trim() == "1";
                            if on && !strike {
                                out.push_str("<s>");
                                strike = true;
                            } else if !on && strike {
                                out.push_str("</s>");
                                strike = false;
                            }
                        }
                        "c" | "1c" => {
                            if let Some(rgb) = ass_bgr_to_hex_rgb(&tag.raw_args) {
                                if font_open {
                                    out.push_str("</font>");
                                }
                                let _ = write!(out, "<font color=\"#{rgb}\">");
                                font_open = true;
                            } else if font_open {
                                out.push_str("</font>");
                                font_open = false;
                            }
                        }
                        _ => {}
                    }
                }
            }
        }
    }

    if font_open {
        out.push_str("</font>");
    }
    if strike {
        out.push_str("</s>");
    }
    if underline {
        out.push_str("</u>");
    }
    if bold {
        out.push_str("</b>");
    }
    if italic {
        out.push_str("</i>");
    }

    out
}

fn ass_bgr_to_hex_rgb(raw: &str) -> Option<String> {
    let trimmed = raw
        .trim()
        .trim_start_matches("&H")
        .trim_start_matches("&h")
        .trim_end_matches('&');
    if trimmed.is_empty() || trimmed.len() > 8 || !trimmed.bytes().all(|b| b.is_ascii_hexdigit()) {
        return None;
    }
    let val = u32::from_str_radix(trimmed, 16).ok()?;
    let b = (val >> 16) & 0xFF;
    let g = (val >> 8) & 0xFF;
    let r = val & 0xFF;
    Some(format!("{r:02X}{g:02X}{b:02X}"))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_srt_with_bom_missing_blank_lines_and_html_tags() {
        let srt = "\u{FEFF}1\r\n00:00:01,000 --> 00:00:03,500\r\n<i>Hello</i> <b>World</b>\r\n2\r\n00:00:04,000 --> 00:00:06,250\r\n<font color=\"#FF8000\">Orange</font> <u>line</u>\r\nSecond row\r\n";
        let outcome = parse_srt(srt);
        assert!(outcome.warnings.is_empty(), "{:?}", outcome.warnings);
        let events: Vec<&AssEvent> = outcome.value.events().collect();
        assert_eq!(events.len(), 2);
        assert_eq!(events[0].start, AssTime::from_cs(100));
        assert_eq!(events[0].end, AssTime::from_cs(350));
        assert_eq!(events[0].text, r"{\i1}Hello{\i0} {\b1}World{\b0}");
        assert_eq!(
            events[1].text,
            r"{\c&H0080FF&}Orange{\c} {\u1}line{\u0}\NSecond row"
        );

        let written = write_srt(&outcome.value);
        assert!(written.contains("00:00:01,000 --> 00:00:03,500"));
        assert!(written.contains("<i>Hello</i> <b>World</b>"));
        assert!(written.contains("<font color=\"#FF8000\">Orange</font> <u>line</u>\r\nSecond row"));
    }
}
