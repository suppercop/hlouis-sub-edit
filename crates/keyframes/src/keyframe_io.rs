//! Keyframe file reader/writer supporting Aegisub v1, `XviD` pass-1 logs, x264/x265 `--stats`,
//! and plain frame-number lists (Lavf / WWXD-style) with automatic format detection.

use crate::KeyframeError;
use std::fmt::Write as _;

/// Detected keyframe file format.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum KeyframeFormat {
    /// `# keyframe format v1` (Aegisub).
    AegisubV1,
    /// `XviD` 2-pass `.pass` log (`i`/`p`/`b` frame lines).
    XvidPassLog,
    /// x264 / x265 `--stats` log (`in:N ... type:I`).
    X264Stats,
    /// Plain one-frame-number-per-line list (Lavf / WWXD).
    PlainFrameList,
}

/// Auto-detects the format of `input` and parses sorted, deduplicated 0-based keyframe indices.
pub fn parse_keyframes(input: &str) -> Result<(Vec<u32>, KeyframeFormat), KeyframeError> {
    let body = input.strip_prefix('\u{FEFF}').unwrap_or(input);
    let lines: Vec<&str> = body.lines().collect();

    let format = detect_format(&lines)?;
    let mut keyframes = match format {
        KeyframeFormat::AegisubV1 => parse_aegisub_v1(&lines)?,
        KeyframeFormat::XvidPassLog => parse_xvid(&lines),
        KeyframeFormat::X264Stats => parse_x264_stats(&lines),
        KeyframeFormat::PlainFrameList => parse_plain_list(&lines)?,
    };

    keyframes.sort_unstable();
    keyframes.dedup();
    Ok((keyframes, format))
}

/// Serializes keyframes in Aegisub `# keyframe format v1` format.
#[must_use]
pub fn write_aegisub_keyframes(keyframes: &[u32], fps: Option<f64>) -> String {
    let mut out = String::with_capacity(64 + keyframes.len() * 8);
    out.push_str("# keyframe format v1\n");
    let fps_val = fps.unwrap_or(0.0);
    let _ = writeln!(out, "fps {fps_val}");
    for &kf in keyframes {
        let _ = writeln!(out, "{kf}");
    }
    out
}

/// Serializes keyframes as a plain one-frame-per-line list (Lavf / WWXD-compatible).
#[must_use]
pub fn write_plain_keyframes(keyframes: &[u32]) -> String {
    let mut out = String::with_capacity(keyframes.len() * 8);
    for &kf in keyframes {
        let _ = writeln!(out, "{kf}");
    }
    out
}

fn detect_format(lines: &[&str]) -> Result<KeyframeFormat, KeyframeError> {
    for &raw in lines {
        let t = raw.trim();
        if t.is_empty() {
            continue;
        }
        if t.eq_ignore_ascii_case("# keyframe format v1") {
            return Ok(KeyframeFormat::AegisubV1);
        }
        if t.starts_with("# `XviD` 2pass stat file") {
            return Ok(KeyframeFormat::XvidPassLog);
        }
        if t.starts_with("#options:") || (t.contains("in:") && t.contains("type:")) {
            return Ok(KeyframeFormat::X264Stats);
        }
        if t.starts_with('#') || t.starts_with(';') {
            continue;
        }
        if is_xvid_frame_line(t) {
            return Ok(KeyframeFormat::XvidPassLog);
        }
        if t.parse::<u32>().is_ok() {
            return Ok(KeyframeFormat::PlainFrameList);
        }
        return Err(KeyframeError::UnrecognizedFormat);
    }
    Err(KeyframeError::UnrecognizedFormat)
}

fn is_xvid_frame_line(trimmed: &str) -> bool {
    let mut parts = trimmed.split_whitespace();
    let Some(first) = parts.next() else {
        return false;
    };
    matches!(first, "i" | "p" | "b" | "I" | "P" | "B") && parts.next().is_some()
}

fn parse_aegisub_v1(lines: &[&str]) -> Result<Vec<u32>, KeyframeError> {
    let mut out = Vec::new();
    for (idx, &raw) in lines.iter().enumerate() {
        let t = raw.trim();
        if t.is_empty() || t.starts_with('#') || t.starts_with(';') || t.starts_with("fps ") {
            continue;
        }
        let frame = t
            .parse::<u32>()
            .map_err(|_| KeyframeError::InvalidKeyframeLine {
                line: idx + 1,
                message: format!("expected integer frame index, got `{t}`"),
            })?;
        out.push(frame);
    }
    Ok(out)
}

fn parse_xvid(lines: &[&str]) -> Vec<u32> {
    let mut out = Vec::new();
    let mut frame_idx = 0u32;
    for &raw in lines {
        let t = raw.trim();
        if t.is_empty() || t.starts_with('#') || t.starts_with(';') {
            continue;
        }
        if is_xvid_frame_line(t) {
            if t.starts_with('i') || t.starts_with('I') {
                out.push(frame_idx);
            }
            frame_idx = frame_idx.saturating_add(1);
        }
    }
    out
}

fn parse_x264_stats(lines: &[&str]) -> Vec<u32> {
    let mut out = Vec::new();
    let mut fallback_idx = 0u32;
    for &raw in lines {
        let t = raw.trim();
        if t.is_empty() || t.starts_with('#') || t.starts_with(';') {
            continue;
        }
        let mut frame_in: Option<u32> = None;
        let mut is_key = false;
        let mut saw_type = false;

        for tok in t.split_whitespace() {
            if let Some(num) = tok.strip_prefix("in:") {
                if let Ok(n) = num.parse::<u32>() {
                    frame_in = Some(n);
                }
            } else if let Some(kind) = tok.strip_prefix("type:") {
                saw_type = true;
                // x264 uses `type:I` (IDR) and `type:i` (non-IDR I); x265 uses `type:I` / `type:K`.
                if matches!(kind, "I" | "i" | "K") {
                    is_key = true;
                }
            }
        }

        if saw_type {
            if is_key {
                out.push(frame_in.unwrap_or(fallback_idx));
            }
            fallback_idx = fallback_idx.saturating_add(1);
        }
    }
    out
}

fn parse_plain_list(lines: &[&str]) -> Result<Vec<u32>, KeyframeError> {
    let mut out = Vec::new();
    for (idx, &raw) in lines.iter().enumerate() {
        let t = raw.trim();
        if t.is_empty() || t.starts_with('#') || t.starts_with(';') {
            continue;
        }
        // Tolerate inline comments after the frame number (e.g. "48 # scene cut").
        let token = t.split_whitespace().next().unwrap_or("");
        let frame = token
            .parse::<u32>()
            .map_err(|_| KeyframeError::InvalidKeyframeLine {
                line: idx + 1,
                message: format!("expected frame number, got `{t}`"),
            })?;
        out.push(frame);
    }
    Ok(out)
}
