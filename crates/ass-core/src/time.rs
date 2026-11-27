//! Centisecond-precision subtitle timestamp type (`H:MM:SS.cc`) without floating-point drift.

use std::fmt;
use std::str::FromStr;

/// Subtitle timestamp stored as integer centiseconds (`1/100` second), matching Aegisub's internal
/// ASS precision (`0:00:00.00` to `9:59:59.99`, and extended hours up to `u32::MAX` centiseconds).
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash, Default)]
pub struct AssTime(u32);

impl AssTime {
    /// Zero timestamp (`0:00:00.00`).
    pub const ZERO: Self = Self(0);

    /// Maximum representable timestamp (`u32::MAX` centiseconds).
    pub const MAX: Self = Self(u32::MAX);

    /// Creates a timestamp from integer centiseconds.
    #[must_use]
    pub const fn from_cs(cs: u32) -> Self {
        Self(cs)
    }

    /// Creates a timestamp from hours, minutes, seconds, and centiseconds using saturating math.
    #[must_use]
    pub const fn from_hms_cs(hours: u32, minutes: u32, seconds: u32, centiseconds: u32) -> Self {
        let total = hours
            .saturating_mul(360_000)
            .saturating_add(minutes.saturating_mul(6_000))
            .saturating_add(seconds.saturating_mul(100))
            .saturating_add(centiseconds);
        Self(total)
    }

    /// Creates a timestamp from milliseconds, rounding half-up to the nearest centisecond.
    #[must_use]
    pub const fn from_ms_rounded(ms: u64) -> Self {
        let cs = ms.saturating_add(5) / 10;
        if cs > u32::MAX as u64 {
            Self::MAX
        } else {
            #[allow(clippy::cast_possible_truncation)]
            Self(cs as u32)
        }
    }

    /// Returns the total centiseconds.
    #[must_use]
    pub const fn as_cs(self) -> u32 {
        self.0
    }

    /// Returns the total milliseconds (`cs * 10`).
    #[must_use]
    pub const fn as_ms(self) -> u64 {
        (self.0 as u64) * 10
    }

    /// Saturating addition of `delta_cs` centiseconds.
    #[must_use]
    pub const fn saturating_add_cs(self, delta_cs: u32) -> Self {
        Self(self.0.saturating_add(delta_cs))
    }

    /// Saturating subtraction of `delta_cs` centiseconds.
    #[must_use]
    pub const fn saturating_sub_cs(self, delta_cs: u32) -> Self {
        Self(self.0.saturating_sub(delta_cs))
    }

    /// Saturating signed offset in centiseconds (clamps at `0` and `u32::MAX`).
    #[must_use]
    pub const fn saturating_add_signed_cs(self, delta_cs: i64) -> Self {
        if delta_cs >= 0 {
            let add = if delta_cs > u32::MAX as i64 {
                u32::MAX
            } else {
                #[allow(clippy::cast_sign_loss, clippy::cast_possible_truncation)]
                {
                    delta_cs as u32
                }
            };
            Self(self.0.saturating_add(add))
        } else {
            let sub_u64 = delta_cs.unsigned_abs();
            let sub = if sub_u64 > u32::MAX as u64 {
                u32::MAX
            } else {
                #[allow(clippy::cast_possible_truncation)]
                {
                    sub_u64 as u32
                }
            };
            Self(self.0.saturating_sub(sub))
        }
    }

    /// Saturating addition of another `AssTime`.
    #[must_use]
    pub const fn saturating_add(self, rhs: Self) -> Self {
        Self(self.0.saturating_add(rhs.0))
    }

    /// Saturating subtraction of another `AssTime`.
    #[must_use]
    pub const fn saturating_sub(self, rhs: Self) -> Self {
        Self(self.0.saturating_sub(rhs.0))
    }

    /// Decomposes into `(hours, minutes, seconds, centiseconds)`.
    #[must_use]
    pub const fn to_hms_cs(self) -> (u32, u32, u32, u32) {
        let total_sec = self.0 / 100;
        let cs = self.0 % 100;
        let sec = total_sec % 60;
        let total_min = total_sec / 60;
        let min = total_min % 60;
        let hours = total_min / 60;
        (hours, min, sec, cs)
    }

    /// Parses an ASS timestamp (`H:MM:SS.cc`, also tolerating `.c`, `.ccc`, or missing fraction).
    /// Returns `None` if malformed.
    #[must_use]
    pub fn parse_ass(input: &str) -> Option<Self> {
        let s = input.trim();
        let mut parts = s.split(':');
        let h_str = parts.next()?;
        let m_str = parts.next()?;
        let rest = parts.next()?;
        if parts.next().is_some() {
            return None;
        }

        let hours: u32 = parse_ascii_u32(h_str)?;
        let minutes: u32 = parse_ascii_u32(m_str)?;

        let (sec_str, frac_str) = match rest.split_once('.') {
            Some((sec, frac)) => (sec, Some(frac)),
            None => (rest, None),
        };
        let seconds: u32 = parse_ascii_u32(sec_str)?;
        let cs = match frac_str {
            None => 0,
            Some("") => return None,
            Some(f) => parse_centiseconds_fraction(f)?,
        };

        Some(Self::from_hms_cs(hours, minutes, seconds, cs))
    }

    /// Parses an SRT timestamp (`HH:MM:SS,mmm` or `HH:MM:SS.mmm`, also tolerating 1-2 digit ms).
    #[must_use]
    pub fn parse_srt(input: &str) -> Option<Self> {
        let s = input.trim();
        let mut parts = s.split(':');
        let h_str = parts.next()?;
        let m_str = parts.next()?;
        let rest = parts.next()?;
        if parts.next().is_some() {
            return None;
        }

        let hours = u64::from(parse_ascii_u32(h_str)?);
        let minutes = u64::from(parse_ascii_u32(m_str)?);

        let (sec_str, frac_str) = rest
            .split_once(',')
            .or_else(|| rest.split_once('.'))
            .map_or((rest, None), |(a, b)| (a, Some(b)));
        let seconds = u64::from(parse_ascii_u32(sec_str)?);
        let millis: u64 = match frac_str {
            None => 0,
            Some("") => return None,
            Some(f) => parse_millis_fraction(f)?,
        };

        let total_ms = hours
            .saturating_mul(3_600_000)
            .saturating_add(minutes.saturating_mul(60_000))
            .saturating_add(seconds.saturating_mul(1_000))
            .saturating_add(millis);
        Some(Self::from_ms_rounded(total_ms))
    }

    /// Formats as canonical ASS timestamp `H:MM:SS.cc`.
    #[must_use]
    pub fn to_ass_string(self) -> String {
        let mut out = String::with_capacity(11);
        self.write_ass(&mut out);
        out
    }

    /// Appends canonical ASS timestamp `H:MM:SS.cc` into `out` without intermediate allocations.
    pub fn write_ass(self, out: &mut String) {
        use std::fmt::Write as _;
        let (h, m, s, cs) = self.to_hms_cs();
        let _ = write!(out, "{h}:{m:02}:{s:02}.{cs:02}");
    }

    /// Formats as canonical SRT timestamp `HH:MM:SS,mmm`.
    #[must_use]
    pub fn to_srt_string(self) -> String {
        let (h, m, s, cs) = self.to_hms_cs();
        format!("{h:02}:{m:02}:{s:02},{:03}", cs * 10)
    }
}

impl fmt::Display for AssTime {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        let (h, m, s, cs) = self.to_hms_cs();
        write!(f, "{h}:{m:02}:{s:02}.{cs:02}")
    }
}

impl FromStr for AssTime {
    type Err = ();

    fn from_str(s: &str) -> Result<Self, Self::Err> {
        Self::parse_ass(s).ok_or(())
    }
}

fn parse_ascii_u32(s: &str) -> Option<u32> {
    if s.is_empty() || !s.bytes().all(|b| b.is_ascii_digit()) {
        return None;
    }
    s.parse::<u32>().ok()
}

fn parse_centiseconds_fraction(f: &str) -> Option<u32> {
    if !f.bytes().all(|b| b.is_ascii_digit()) {
        return None;
    }
    let bytes = f.as_bytes();
    match bytes.len() {
        1 => Some(u32::from(bytes[0] - b'0') * 10),
        2 => Some(u32::from(bytes[0] - b'0') * 10 + u32::from(bytes[1] - b'0')),
        _ => {
            let base = u32::from(bytes[0] - b'0') * 10 + u32::from(bytes[1] - b'0');
            let round_up = u32::from(bytes[2] >= b'5');
            Some(base + round_up)
        }
    }
}

fn parse_millis_fraction(f: &str) -> Option<u64> {
    if !f.bytes().all(|b| b.is_ascii_digit()) {
        return None;
    }
    let bytes = f.as_bytes();
    match bytes.len() {
        1 => Some(u64::from(bytes[0] - b'0') * 100),
        2 => Some(u64::from(bytes[0] - b'0') * 100 + u64::from(bytes[1] - b'0') * 10),
        _ => Some(
            u64::from(bytes[0] - b'0') * 100
                + u64::from(bytes[1] - b'0') * 10
                + u64::from(bytes[2] - b'0'),
        ),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_and_formats_ass_timestamps() {
        assert_eq!(AssTime::parse_ass("0:00:00.00"), Some(AssTime::ZERO));
        assert_eq!(
            AssTime::parse_ass("1:02:03.45").map(AssTime::to_ass_string),
            Some("1:02:03.45".to_owned())
        );
        assert_eq!(AssTime::parse_ass("0:00:01.5"), Some(AssTime::from_cs(150)));
        assert_eq!(
            AssTime::parse_ass("0:00:01.556"),
            Some(AssTime::from_cs(156))
        );
        assert_eq!(AssTime::parse_ass("bad"), None);
        assert_eq!(AssTime::parse_ass("0:00:01."), None);
        assert_eq!(AssTime::parse_ass("-0:00:01.00"), None);
    }

    #[test]
    fn saturating_arithmetic_has_no_float_drift() {
        let mut t = AssTime::ZERO;
        for _ in 0..10_000 {
            t = t.saturating_add_cs(1);
        }
        assert_eq!(t.as_cs(), 10_000);
        assert_eq!(t.to_ass_string(), "0:01:40.00");
        assert_eq!(AssTime::ZERO.saturating_sub_cs(500), AssTime::ZERO);
        assert_eq!(AssTime::MAX.saturating_add_cs(1), AssTime::MAX);
        assert_eq!(
            AssTime::from_cs(50).saturating_add_signed_cs(-100),
            AssTime::ZERO
        );
        assert_eq!(
            AssTime::from_cs(50).saturating_add_signed_cs(25),
            AssTime::from_cs(75)
        );
    }

    #[test]
    fn parses_and_formats_srt_timestamps() {
        let t = AssTime::parse_srt("01:02:03,456").expect("valid srt");
        assert_eq!(t, AssTime::from_hms_cs(1, 2, 3, 46));
        assert_eq!(t.to_srt_string(), "01:02:03,460");
        assert_eq!(
            AssTime::parse_srt("00:00:05.250"),
            Some(AssTime::from_cs(525))
        );
    }
}
