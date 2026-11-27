//! Fast single-pass rewriter for ASS override tag timestamps during Framerate Transform
//! (mirrors Aegisub `src/export_framerate.cpp` `AssTransformFramerateFilter::TransformTimeTags`).
//!
//! Handles `\t`, `\move` (6-arg), `\fad`, `\fade`, and karaoke tags (`\k`, `\K`, `\kf`, `\ko`, `\kt`).
//! Performs zero heap allocations when a line contains no `{...}` override blocks or no time tags.

use keyframes::{TimeType, Timecodes};
use std::fmt::Write as _;

/// Transforms an absolute timestamp in ms from `input` timecodes to `output` timecodes,
/// preserving sub-frame fractional position (matches Aegisub `ConvertTime`).
#[inline]
#[allow(clippy::cast_precision_loss, clippy::cast_possible_truncation)]
#[must_use]
pub fn convert_time_ms(t_ms: i64, input: &Timecodes, output: &Timecodes) -> i64 {
    let clamped = t_ms.max(0);
    let f = input.time_ms_to_frame(clamped, TimeType::Exact);
    let in_f0 = input.frame_to_time_ms(f, TimeType::Exact);
    let in_f1 = input.frame_to_time_ms(f.saturating_add(1), TimeType::Exact);
    let out_f0 = output.frame_to_time_ms(f, TimeType::Exact);
    let out_f1 = output.frame_to_time_ms(f.saturating_add(1), TimeType::Exact);
    let in_dur = (in_f1 - in_f0).max(1);
    let frac = ((clamped - in_f0) as f64) / (in_dur as f64);
    out_f0 + (frac * ((out_f1 - out_f0) as f64)).round() as i64
}

/// Rewrites `\t`, `\move`, `\fad`, `\fade`, and `\k`/`\K`/`\kf`/`\ko`/`\kt` times inside `text`.
/// Returns `None` if no time tags were modified (zero allocation fast path).
#[must_use]
pub fn transform_text_time_tags(
    text: &str,
    old_start_ms: i64,
    old_end_ms: i64,
    new_start_ms: i64,
    new_end_ms: i64,
    input: &Timecodes,
    output: &Timecodes,
) -> Option<String> {
    let bytes = text.as_bytes();
    if !bytes.contains(&b'{') {
        return None;
    }

    let len = bytes.len();
    let mut pos = 0usize;
    let mut out: Option<String> = None;
    let mut last_copied = 0usize;
    let exact_new_start = convert_time_ms(old_start_ms, input, output);
    let exact_new_end = convert_time_ms(old_end_ms, input, output);
    let _ = (new_start_ms, new_end_ms);
    let mut kara_old_ms = old_start_ms;
    let mut kara_new_ms = exact_new_start;

    while pos < len {
        let Some(rel_open) = bytes[pos..].iter().position(|&b| b == b'{') else {
            break;
        };
        let block_start = pos + rel_open + 1;
        let Some(rel_close) = bytes[block_start..].iter().position(|&b| b == b'}') else {
            break;
        };
        let block_end = block_start + rel_close;

        let mut i = block_start;
        while i < block_end {
            if bytes[i] != b'\\' {
                i += 1;
                continue;
            }
            let tag_start = i;
            let mut j = i + 1;
            let mut parens = 0usize;
            while j < block_end {
                let c = bytes[j];
                if c == b'(' {
                    parens += 1;
                } else if c == b')' && parens > 0 {
                    parens -= 1;
                } else if parens == 0 && c == b'\\' {
                    break;
                }
                j += 1;
            }
            let raw = &text[tag_start + 1..j];

            if let Some(replacement) = rewrite_single_tag(
                raw,
                old_start_ms,
                old_end_ms,
                exact_new_start,
                exact_new_end,
                &mut kara_old_ms,
                &mut kara_new_ms,
                input,
                output,
            ) {
                let buf = out.get_or_insert_with(|| String::with_capacity(text.len() + 16));
                buf.push_str(&text[last_copied..tag_start]);
                buf.push_str(&replacement);
                last_copied = j;
            }

            i = j;
        }
        pos = block_end + 1;
    }

    if let Some(mut buf) = out {
        buf.push_str(&text[last_copied..]);
        Some(buf)
    } else {
        None
    }
}

#[allow(clippy::too_many_arguments)]
fn rewrite_single_tag(
    raw: &str,
    old_start_ms: i64,
    old_end_ms: i64,
    new_start_ms: i64,
    new_end_ms: i64,
    kara_old_ms: &mut i64,
    kara_new_ms: &mut i64,
    input: &Timecodes,
    output: &Timecodes,
) -> Option<String> {
    // 1. \fad(t1, t2)
    if let Some(inner) = raw.strip_prefix("fad(").and_then(|s| s.strip_suffix(')')) {
        let (a, b) = inner.split_once(',')?;
        let t1 = a.trim().parse::<i64>().ok()?;
        let t2 = b.trim().parse::<i64>().ok()?;
        let nt1 = (convert_time_ms(old_start_ms + t1, input, output) - new_start_ms).max(0);
        let nt2 = (new_end_ms - convert_time_ms((old_end_ms - t2).max(0), input, output)).max(0);
        return Some(format!("\\fad({nt1},{nt2})"));
    }

    // 2. \fade(a1, a2, a3, t1, t2, t3, t4)
    if let Some(inner) = raw.strip_prefix("fade(").and_then(|s| s.strip_suffix(')')) {
        let parts: Vec<&str> = inner.split(',').map(str::trim).collect();
        if parts.len() == 7 {
            let t1 = parts[3].parse::<i64>().ok()?;
            let t2 = parts[4].parse::<i64>().ok()?;
            let t3 = parts[5].parse::<i64>().ok()?;
            let t4 = parts[6].parse::<i64>().ok()?;
            let map =
                |t: i64| (convert_time_ms(old_start_ms + t, input, output) - new_start_ms).max(0);
            return Some(format!(
                "\\fade({},{},{},{},{},{},{})",
                parts[0],
                parts[1],
                parts[2],
                map(t1),
                map(t2),
                map(t3),
                map(t4)
            ));
        }
    }

    // 3. \move(x1, y1, x2, y2, t1, t2) (only 6-arg form carries timestamps)
    if let Some(inner) = raw.strip_prefix("move(").and_then(|s| s.strip_suffix(')')) {
        let parts: Vec<&str> = inner.split(',').map(str::trim).collect();
        if parts.len() == 6 {
            let t1 = parts[4].parse::<i64>().ok()?;
            let t2 = parts[5].parse::<i64>().ok()?;
            let nt1 = (convert_time_ms(old_start_ms + t1, input, output) - new_start_ms).max(0);
            let nt2 = (convert_time_ms(old_start_ms + t2, input, output) - new_start_ms).max(0);
            return Some(format!(
                "\\move({},{},{},{},{nt1},{nt2})",
                parts[0], parts[1], parts[2], parts[3]
            ));
        }
    }

    // 4. \t(t1, t2, [accel,] \tags)
    if let Some(inner) = raw.strip_prefix("t(").and_then(|s| s.strip_suffix(')')) {
        let parts: Vec<&str> = inner.splitn(4, ',').collect();
        if parts.len() >= 3 {
            if let (Ok(t1), Ok(t2)) = (
                parts[0].trim().parse::<i64>(),
                parts[1].trim().parse::<i64>(),
            ) {
                let nt1 = (convert_time_ms(old_start_ms + t1, input, output) - new_start_ms).max(0);
                let nt2 = (convert_time_ms(old_start_ms + t2, input, output) - new_start_ms).max(0);
                let mut s = String::with_capacity(raw.len() + 8);
                let _ = write!(s, "\\t({nt1},{nt2},{}", parts[2]);
                if parts.len() == 4 {
                    s.push(',');
                    s.push_str(parts[3]);
                }
                s.push(')');
                return Some(s);
            }
        }
    }

    // 5. Karaoke tags: \kf, \ko, \kt, \k, \K
    for prefix in ["kf", "ko", "kt", "k", "K"] {
        if let Some(val_str) = raw.strip_prefix(prefix) {
            if !val_str.is_empty() && val_str.as_bytes()[0].is_ascii_digit() {
                if let Ok(cs) = val_str.trim().parse::<i64>() {
                    if prefix == "kt" {
                        let nt = (convert_time_ms(old_start_ms + cs * 10, input, output)
                            - new_start_ms)
                            .max(0);
                        let ncs = (nt + 5) / 10;
                        *kara_old_ms = old_start_ms + cs * 10;
                        *kara_new_ms = new_start_ms + ncs * 10;
                        return Some(format!("\\kt{ncs}"));
                    }
                    let next_old = *kara_old_ms + cs * 10;
                    let next_new = convert_time_ms(next_old, input, output);
                    let dur_cs = ((next_new - *kara_new_ms).max(0) + 5) / 10;
                    *kara_old_ms = next_old;
                    *kara_new_ms += dur_cs * 10;
                    return Some(format!("\\{prefix}{dur_cs}"));
                }
            }
        }
    }

    None
}
