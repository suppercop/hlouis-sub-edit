//! mkvmerge timecodes v1/v2 reader/writer and CFR/VFR frame↔time conversion matching
//! Aegisub 3.5.1's `libaegisub/vfr.cpp` rounding and Start/End midpoint semantics.

use crate::KeyframeError;
use std::fmt::Write as _;

/// Frame↔time boundary semantics matching Aegisub's `agi::vfr::Time`.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum TimeType {
    /// Exact presentation timestamp of frame `n`.
    Exact,
    /// Subtitle Start boundary: midpoint between frame `n - 1` and frame `n` (`0` for frame `0`),
    /// ensuring centisecond rounding displays on frame `n` and not `n - 1`.
    Start,
    /// Subtitle End boundary (inclusive of frame `n`): midpoint between frame `n` and frame `n + 1`.
    End,
}

/// Constant- or variable-framerate timecode table.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Timecodes {
    /// Constant framerate (`num / den`, e.g. `24000 / 1001`).
    Cfr { num: u64, den: u64 },
    /// Variable framerate: monotonically non-decreasing frame start timestamps in milliseconds,
    /// plus a fallback CFR (`default_num / default_den`) beyond the last listed frame.
    Vfr {
        timestamps_ms: Vec<i64>,
        default_num: u64,
        default_den: u64,
    },
}

impl Timecodes {
    /// Creates a CFR timecode table (`num / den`, e.g. `24000, 1001`).
    pub fn from_cfr(num: u64, den: u64) -> Result<Self, KeyframeError> {
        if num == 0 || den == 0 {
            return Err(KeyframeError::InvalidFramerate(num, den));
        }
        Ok(Self::Cfr { num, den })
    }

    /// Creates a VFR timecode table from sorted frame timestamps in milliseconds.
    pub fn from_vfr_ms(timestamps_ms: Vec<i64>) -> Result<Self, KeyframeError> {
        if timestamps_ms.is_empty() {
            return Err(KeyframeError::InvalidTimecodes(
                "VFR timecode table cannot be empty".to_owned(),
            ));
        }
        // Aegisub 3.5.1 (libaegisub/vfr.cpp): frames past the last listed timestamp are
        // extrapolated with the average frame duration of the listed frames, i.e.
        // fps = (n - 1) * 1000 / (last - first). Fall back to 24000/1001 when that is undefined
        // (a single frame, or all timestamps equal).
        let n = timestamps_ms.len();
        let span = timestamps_ms[n - 1] - timestamps_ms[0];
        let (default_num, default_den) = match (u64::try_from(n - 1), u64::try_from(span)) {
            (Ok(intervals), Ok(span)) if intervals > 0 && span > 0 => (intervals * 1_000, span),
            _ => (24_000, 1_001),
        };
        Ok(Self::Vfr {
            timestamps_ms,
            default_num,
            default_den,
        })
    }

    /// Converts 0-based `frame` to milliseconds using Aegisub's `Exact`, `Start`, or `End` semantics.
    #[inline]
    #[must_use]
    pub fn frame_to_time_ms(&self, frame: u32, kind: TimeType) -> i64 {
        match kind {
            TimeType::Exact => self.exact_ms(frame),
            TimeType::Start => {
                if frame == 0 {
                    0
                } else {
                    let prev = self.exact_ms(frame - 1);
                    let cur = self.exact_ms(frame);
                    prev + (cur - prev) / 2
                }
            }
            TimeType::End => {
                let cur = self.exact_ms(frame);
                let next = self.exact_ms(frame.saturating_add(1));
                cur + (next - cur) / 2
            }
        }
    }

    /// Converts 0-based `frame` to ASS centiseconds (rounded half-up from ms, matching Aegisub).
    #[inline]
    #[must_use]
    pub fn frame_to_time_cs(&self, frame: u32, kind: TimeType) -> u32 {
        let ms = self.frame_to_time_ms(frame, kind).max(0);
        let cs = (ms + 5) / 10;
        u32::try_from(cs).unwrap_or(u32::MAX)
    }

    /// Converts a timestamp in milliseconds to a 0-based frame index using Aegisub's semantics:
    /// - `Exact`: frame displayed at `time_ms` (largest `f` with `exact_ms(f) <= time_ms`).
    /// - `Start`: first frame at or after `time_ms` (smallest `f` with `exact_ms(f) >= time_ms`).
    /// - `End`: last frame strictly before `time_ms` (matching Aegisub's exclusive-end subtitle model).
    #[inline]
    #[must_use]
    pub fn time_ms_to_frame(&self, time_ms: i64, kind: TimeType) -> u32 {
        let clamped = time_ms.max(0);
        match self {
            Self::Cfr { num, den } => cfr_time_ms_to_frame(clamped, *num, *den, kind),
            Self::Vfr {
                timestamps_ms,
                default_num,
                default_den,
            } => vfr_time_ms_to_frame(clamped, timestamps_ms, *default_num, *default_den, kind),
        }
    }

    /// Converts ASS centiseconds (`cs * 10` ms) to a 0-based frame index.
    #[inline]
    #[must_use]
    pub fn time_cs_to_frame(&self, time_cs: u32, kind: TimeType) -> u32 {
        self.time_ms_to_frame(i64::from(time_cs) * 10, kind)
    }

    /// Serializes as mkvmerge timecode v2 (`# timecode format v2`, one ms value per frame).
    #[must_use]
    pub fn write_v2(&self, frame_count: usize) -> String {
        let count = match self {
            Self::Cfr { .. } => frame_count,
            Self::Vfr { timestamps_ms, .. } => {
                if frame_count == 0 {
                    timestamps_ms.len()
                } else {
                    frame_count
                }
            }
        };
        let mut out = String::with_capacity(24 + count * 10);
        out.push_str("# timecode format v2\n");
        for i in 0..count {
            let f = u32::try_from(i).unwrap_or(u32::MAX);
            let _ = writeln!(out, "{}.000000", self.exact_ms(f));
        }
        out
    }

    /// Serializes as mkvmerge timecode v1 (`# timecode format v1` with `Assume <fps>`).
    #[must_use]
    pub fn write_v1(&self) -> String {
        let mut out = String::from("# timecode format v1\n");
        match self {
            Self::Cfr { num, den } => {
                let fps = (*num as f64) / (*den as f64);
                let _ = writeln!(out, "Assume {fps:.6}");
            }
            Self::Vfr {
                timestamps_ms,
                default_num,
                default_den,
            } => {
                let default_fps = (*default_num as f64) / (*default_den as f64);
                let _ = writeln!(out, "Assume {default_fps:.6}");
                if timestamps_ms.len() >= 2 {
                    let mut range_start = 0usize;
                    let mut prev_dur = (timestamps_ms[1] - timestamps_ms[0]).max(1);
                    for i in 1..(timestamps_ms.len() - 1) {
                        let dur = (timestamps_ms[i + 1] - timestamps_ms[i]).max(1);
                        if (dur - prev_dur).abs() > 1 {
                            let fps = 1000.0 / (prev_dur as f64);
                            let _ = writeln!(out, "{range_start},{i},{fps:.6}");
                            range_start = i + 1;
                            prev_dur = dur;
                        }
                    }
                    let last_idx = timestamps_ms.len() - 1;
                    if range_start < last_idx {
                        let fps = 1000.0 / (prev_dur as f64);
                        let _ = writeln!(out, "{range_start},{last_idx},{fps:.6}");
                    }
                }
            }
        }
        out
    }

    #[inline]
    fn exact_ms(&self, frame: u32) -> i64 {
        match self {
            Self::Cfr { num, den } => cfr_frame_to_ms(frame, *num, *den),
            Self::Vfr {
                timestamps_ms,
                default_num,
                default_den,
            } => {
                let idx = frame as usize;
                if let Some(&t) = timestamps_ms.get(idx) {
                    t
                } else {
                    let last_idx = timestamps_ms.len() - 1;
                    let base_ms = timestamps_ms[last_idx];
                    let extra_frames = u32::try_from(idx - last_idx).unwrap_or(u32::MAX);
                    base_ms.saturating_add(cfr_frame_to_ms(
                        extra_frames,
                        *default_num,
                        *default_den,
                    ))
                }
            }
        }
    }
}

/// Parses an mkvmerge timecode v1 or v2 file (auto-detected from header).
pub fn parse_timecodes(input: &str) -> Result<Timecodes, KeyframeError> {
    let body = input.strip_prefix('\u{FEFF}').unwrap_or(input);
    let mut lines = body.lines().map(str::trim).filter(|l| !l.is_empty());
    let header = lines
        .next()
        .ok_or_else(|| KeyframeError::InvalidTimecodes("empty timecode file".to_owned()))?;
    let lower = header.to_ascii_lowercase();

    if lower.starts_with("# timecode format v1") || lower.starts_with("# timestamp format v1") {
        parse_v1(lines)
    } else if lower.starts_with("# timecode format v2")
        || lower.starts_with("# timestamp format v2")
    {
        parse_v2(lines)
    } else {
        Err(KeyframeError::InvalidTimecodes(format!(
            "unrecognized timecode header `{header}`"
        )))
    }
}

fn parse_v1<'a>(lines: impl Iterator<Item = &'a str>) -> Result<Timecodes, KeyframeError> {
    let mut assume_fps: Option<f64> = None;
    let mut ranges: Vec<(usize, usize, f64)> = Vec::new();

    for line in lines {
        if line.starts_with('#') {
            continue;
        }
        if let Some(rest) = line
            .strip_prefix("Assume ")
            .or_else(|| line.strip_prefix("assume "))
        {
            let fps = rest
                .trim()
                .parse::<f64>()
                .map_err(|_| KeyframeError::InvalidTimecodes(format!("invalid Assume `{rest}`")))?;
            if !fps.is_finite() || fps <= 0.0 {
                return Err(KeyframeError::InvalidTimecodes(format!(
                    "non-positive Assume fps `{fps}`"
                )));
            }
            assume_fps = Some(fps);
        } else {
            let mut parts = line.split(',');
            let start = parts
                .next()
                .and_then(|s| s.trim().parse::<usize>().ok())
                .ok_or_else(|| {
                    KeyframeError::InvalidTimecodes(format!("invalid v1 range `{line}`"))
                })?;
            let end = parts
                .next()
                .and_then(|s| s.trim().parse::<usize>().ok())
                .ok_or_else(|| {
                    KeyframeError::InvalidTimecodes(format!("invalid v1 range `{line}`"))
                })?;
            let fps = parts
                .next()
                .and_then(|s| s.trim().parse::<f64>().ok())
                .filter(|f| f.is_finite() && *f > 0.0)
                .ok_or_else(|| {
                    KeyframeError::InvalidTimecodes(format!("invalid v1 fps in `{line}`"))
                })?;
            if end < start {
                return Err(KeyframeError::InvalidTimecodes(format!(
                    "v1 range end {end} < start {start}"
                )));
            }
            ranges.push((start, end, fps));
        }
    }

    let base_fps = assume_fps.ok_or_else(|| {
        KeyframeError::InvalidTimecodes("missing `Assume <fps>` in timecode v1".to_owned())
    })?;
    let (def_num, def_den) = fps_to_rational(base_fps);

    if ranges.is_empty() {
        return Ok(Timecodes::Cfr {
            num: def_num,
            den: def_den,
        });
    }

    ranges.sort_by_key(|&(s, _, _)| s);
    let max_frame = ranges.iter().map(|&(_, e, _)| e).max().unwrap_or(0);
    let mut timestamps_ms = Vec::with_capacity(max_frame + 2);
    let mut cur_ms = 0.0f64;
    let mut range_idx = 0usize;

    for frame in 0..=max_frame {
        #[allow(clippy::cast_possible_truncation)]
        timestamps_ms.push(cur_ms.round() as i64);
        while range_idx < ranges.len() && ranges[range_idx].1 < frame {
            range_idx += 1;
        }
        let fps = if range_idx < ranges.len()
            && frame >= ranges[range_idx].0
            && frame <= ranges[range_idx].1
        {
            ranges[range_idx].2
        } else {
            base_fps
        };
        cur_ms += 1000.0 / fps;
    }
    #[allow(clippy::cast_possible_truncation)]
    timestamps_ms.push(cur_ms.round() as i64);

    Ok(Timecodes::Vfr {
        timestamps_ms,
        default_num: def_num,
        default_den: def_den,
    })
}

fn parse_v2<'a>(lines: impl Iterator<Item = &'a str>) -> Result<Timecodes, KeyframeError> {
    let mut timestamps_ms = Vec::new();
    for line in lines {
        if line.starts_with('#') {
            continue;
        }
        let v = line.parse::<f64>().map_err(|_| {
            KeyframeError::InvalidTimecodes(format!("invalid v2 timestamp `{line}`"))
        })?;
        if !v.is_finite() {
            return Err(KeyframeError::InvalidTimecodes(format!(
                "non-finite v2 timestamp `{line}`"
            )));
        }
        #[allow(clippy::cast_possible_truncation)]
        timestamps_ms.push(v.round() as i64);
    }
    Timecodes::from_vfr_ms(timestamps_ms)
}

#[inline]
fn cfr_frame_to_ms(frame: u32, num: u64, den: u64) -> i64 {
    // Round half-up: (frame * den * 1000 + num / 2) / num
    let numerator = u128::from(frame)
        .saturating_mul(u128::from(den))
        .saturating_mul(1_000)
        .saturating_add(u128::from(num) / 2);
    let ms = numerator / u128::from(num);
    i64::try_from(ms).unwrap_or(i64::MAX)
}

#[inline]
fn cfr_time_ms_to_frame(time_ms: i64, num: u64, den: u64, kind: TimeType) -> u32 {
    let ms = u128::from(time_ms.cast_unsigned());
    let num128 = u128::from(num);
    let den128 = u128::from(den) * 1_000;
    let approx = (ms * num128) / den128;
    let mut frame = u32::try_from(approx).unwrap_or(u32::MAX);

    match kind {
        TimeType::Exact => {
            while cfr_frame_to_ms(frame.saturating_add(1), num, den) <= time_ms {
                frame = frame.saturating_add(1);
            }
            while frame > 0 && cfr_frame_to_ms(frame, num, den) > time_ms {
                frame -= 1;
            }
            frame
        }
        TimeType::Start => {
            while cfr_frame_to_ms(frame, num, den) < time_ms {
                frame = frame.saturating_add(1);
            }
            while frame > 0 && cfr_frame_to_ms(frame - 1, num, den) >= time_ms {
                frame -= 1;
            }
            frame
        }
        TimeType::End => {
            if time_ms <= 0 {
                return 0;
            }
            while cfr_frame_to_ms(frame.saturating_add(1), num, den) < time_ms {
                frame = frame.saturating_add(1);
            }
            while frame > 0 && cfr_frame_to_ms(frame, num, den) >= time_ms {
                frame -= 1;
            }
            frame
        }
    }
}

#[inline]
fn vfr_time_ms_to_frame(
    time_ms: i64,
    timestamps_ms: &[i64],
    default_num: u64,
    default_den: u64,
    kind: TimeType,
) -> u32 {
    let last_idx = timestamps_ms.len() - 1;
    let last_ms = timestamps_ms[last_idx];
    if time_ms > last_ms {
        let extra = cfr_time_ms_to_frame(time_ms - last_ms, default_num, default_den, kind);
        return u32::try_from(last_idx)
            .unwrap_or(u32::MAX)
            .saturating_add(extra);
    }

    let idx = match kind {
        TimeType::Exact => timestamps_ms
            .partition_point(|&t| t <= time_ms)
            .saturating_sub(1),
        TimeType::Start => timestamps_ms
            .partition_point(|&t| t < time_ms)
            .min(last_idx),
        TimeType::End => timestamps_ms
            .partition_point(|&t| t < time_ms)
            .saturating_sub(1),
    };
    u32::try_from(idx).unwrap_or(u32::MAX)
}

fn fps_to_rational(fps: f64) -> (u64, u64) {
    // Recognize standard NTSC fractional framerates cleanly.
    for (num, den) in [
        (24_000u64, 1_001u64),
        (30_000, 1_001),
        (60_000, 1_001),
        (120_000, 1_001),
    ] {
        let target = (num as f64) / (den as f64);
        if (fps - target).abs() < 1e-3 {
            return (num, den);
        }
    }
    #[allow(clippy::cast_possible_truncation, clippy::cast_sign_loss)]
    let scaled = (fps * 1_000_000.0).round() as u64;
    (scaled.max(1), 1_000_000)
}
