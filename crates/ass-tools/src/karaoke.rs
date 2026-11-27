//! Karaoke syllable parser, builder, and editing helpers (`\k`, `\K`, `\kf`, `\ko`, `\kt`).
//!
//! Parsing a line into [`KaraokeSyllable`]s and rebuilding it with [`rebuild_karaoke_line`]
//! reproduces the original ASS `Text` byte-for-byte when no syllable is mutated.

use ass_core::{parse_karaoke_tag, scan_override_block, KaraokeKind, RawOverrideItem};

/// A single karaoke syllable (or leading untimed prefix) parsed from an ASS line.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct KaraokeSyllable {
    /// Karaoke tag kind (`None` for any leading text before the first karaoke tag).
    pub kind: Option<KaraokeKind>,
    /// Syllable duration in centiseconds.
    pub duration_cs: u32,
    /// Non-karaoke override tags belonging to this syllable (e.g. `{\c&H00FFFF&}`).
    pub tags: String,
    /// Syllable dialogue text (including escapes like `\N` / `\h`).
    pub text: String,
    /// Exact original raw slice for lossless round-trip when unmodified.
    raw_original: Option<String>,
}

impl KaraokeSyllable {
    /// Updates the syllable's duration in centiseconds and invalidates cached raw text.
    pub fn set_duration_cs(&mut self, duration_cs: u32) {
        self.duration_cs = duration_cs;
        self.raw_original = None;
    }

    /// Updates the syllable's text and invalidates cached raw text.
    pub fn set_text(&mut self, text: impl Into<String>) {
        self.text = text.into();
        self.raw_original = None;
    }
}

/// Parses an ASS event `Text` into karaoke syllables.
///
/// Rebuilding the returned slice with [`rebuild_karaoke_line`] without mutations yields the
/// exact original `text` byte-for-byte.
#[must_use]
pub fn parse_karaoke_syllables(text: &str) -> Vec<KaraokeSyllable> {
    let bytes = text.as_bytes();
    let est_syls = bytes.len().div_ceil(10).clamp(1, 32);
    let mut state = ParseAccum::new(text, est_syls);
    let mut i = 0usize;
    while i < bytes.len() {
        if bytes[i] == b'{' {
            if let Some((kind, dur, next)) = try_parse_canonical_karaoke_block(bytes, i) {
                state.push_canonical_karaoke(i, next, kind, dur);
                i = next;
            } else {
                let start = i + 1;
                let mut has_karaoke = false;
                let (next, closed) = scan_override_block(text, start, |item| {
                    if let RawOverrideItem::Tag { name, raw_args } = item {
                        if parse_karaoke_tag(name, raw_args).is_some() {
                            has_karaoke = true;
                        }
                    }
                });
                state.push_override(i, start, next, closed, has_karaoke);
                i = next;
            }
        } else {
            let end = text[i..].find('{').map_or(bytes.len(), |rel| i + rel);
            state.cur_text.push_span(text, i, end);
            state.push_raw_range(i, end);
            state.has_started = true;
            i = end;
        }
    }
    state.finish()
}

#[inline]
fn try_parse_canonical_karaoke_block(bytes: &[u8], i: usize) -> Option<(KaraokeKind, u32, usize)> {
    let len = bytes.len();
    if i + 4 >= len || bytes[i] != b'{' || bytes[i + 1] != b'\\' {
        return None;
    }
    let (kind, mut p) = match (bytes[i + 2], bytes[i + 3]) {
        (b'k', b'f') => (KaraokeKind::Kf, i + 4),
        (b'k', b'o') => (KaraokeKind::Ko, i + 4),
        (b'k', b't') => (KaraokeKind::Kt, i + 4),
        (b'k', _) => (KaraokeKind::K, i + 3),
        (b'K', _) => (KaraokeKind::CapitalK, i + 3),
        _ => return None,
    };
    if p >= len || !bytes[p].is_ascii_digit() {
        return None;
    }
    if bytes[p] == b'0' {
        return (p + 1 < len && bytes[p + 1] == b'}').then_some((kind, 0, p + 2));
    }
    let mut val = 0u64;
    while p < len && bytes[p].is_ascii_digit() {
        val = val * 10 + u64::from(bytes[p] - b'0');
        if val > u64::from(u32::MAX) {
            return None;
        }
        p += 1;
    }
    if p < len && bytes[p] == b'}' {
        #[allow(clippy::cast_possible_truncation)]
        Some((kind, val as u32, p + 1))
    } else {
        None
    }
}

/// Zero-allocation span accumulator that only allocates a `String` if non-contiguous segments
/// are pushed (e.g. synthesizing `{...}` around split non-karaoke tags).
#[derive(Default)]
struct SpanBuf {
    span: Option<(usize, usize)>,
    owned: String,
    is_owned: bool,
}

impl SpanBuf {
    #[inline]
    fn is_empty(&self) -> bool {
        self.span.is_none() && (!self.is_owned || self.owned.is_empty())
    }

    fn clear(&mut self) {
        self.span = None;
        self.owned.clear();
        self.is_owned = false;
    }

    fn push_span(&mut self, src: &str, start: usize, end: usize) {
        if start >= end {
            return;
        }
        if self.is_owned {
            self.owned.push_str(&src[start..end]);
            return;
        }
        match self.span {
            None => self.span = Some((start, end)),
            Some((a, b)) if b == start => self.span = Some((a, end)),
            Some((a, b)) => {
                self.owned.clear();
                self.owned.reserve((b - a) + (end - start));
                self.owned.push_str(&src[a..b]);
                self.owned.push_str(&src[start..end]);
                self.is_owned = true;
                self.span = None;
            }
        }
    }

    fn push_char(&mut self, src: &str, ch: char) {
        if !self.is_owned {
            self.owned.clear();
            if let Some((a, b)) = self.span.take() {
                self.owned.reserve((b - a) + 1);
                self.owned.push_str(&src[a..b]);
            }
            self.is_owned = true;
        }
        self.owned.push(ch);
    }

    fn take_string(&mut self, src: &str) -> String {
        if self.is_owned {
            let s = self.owned.clone();
            self.clear();
            s
        } else if let Some((a, b)) = self.span.take() {
            src[a..b].to_owned()
        } else {
            String::new()
        }
    }
}

struct ParseAccum<'a> {
    src: &'a str,
    out: Vec<KaraokeSyllable>,
    cur_kind: Option<KaraokeKind>,
    cur_dur: u32,
    cur_tags: SpanBuf,
    cur_text: SpanBuf,
    raw_start: usize,
    raw_end: usize,
    has_raw: bool,
    has_started: bool,
    needs_raw: bool,
}

impl<'a> ParseAccum<'a> {
    fn new(src: &'a str, cap: usize) -> Self {
        Self {
            src,
            out: Vec::with_capacity(cap),
            cur_kind: None,
            cur_dur: 0,
            cur_tags: SpanBuf::default(),
            cur_text: SpanBuf::default(),
            raw_start: 0,
            raw_end: 0,
            has_raw: false,
            has_started: false,
            needs_raw: false,
        }
    }

    #[inline]
    fn push_raw_range(&mut self, start: usize, end: usize) {
        if start < end {
            if !self.has_raw {
                self.raw_start = start;
                self.has_raw = true;
            }
            self.raw_end = end;
        }
    }

    #[inline]
    fn flush_current(&mut self) {
        let raw_original = if self.needs_raw && self.has_raw {
            Some(self.src[self.raw_start..self.raw_end].to_owned())
        } else {
            None
        };
        self.has_raw = false;
        self.needs_raw = false;
        let out = KaraokeSyllable {
            kind: self.cur_kind,
            duration_cs: self.cur_dur,
            tags: self.cur_tags.take_string(self.src),
            text: self.cur_text.take_string(self.src),
            raw_original,
        };
        self.out.push(out);
    }

    #[inline]
    fn push_canonical_karaoke(
        &mut self,
        brace_pos: usize,
        next: usize,
        kind: KaraokeKind,
        dur: u32,
    ) {
        if self.has_started {
            self.flush_current();
        }
        self.raw_start = brace_pos;
        self.raw_end = next;
        self.has_raw = true;
        self.cur_kind = Some(kind);
        self.cur_dur = dur;
        self.has_started = true;
        self.needs_raw = false;
    }

    fn push_override(
        &mut self,
        brace_pos: usize,
        start: usize,
        next: usize,
        closed: bool,
        has_karaoke: bool,
    ) {
        if !has_karaoke {
            if !self.cur_text.is_empty() || !closed {
                self.needs_raw = true;
            }
            self.cur_tags.push_span(self.src, brace_pos, next);
            self.push_raw_range(brace_pos, next);
            self.has_started = true;
            return;
        }

        let mut cursor = start;
        let mut open_brace = false;
        let mut pending: Option<(usize, usize)> = None;
        let src = self.src;

        let _ = scan_override_block(src, start, |item| {
            let item_len = match item {
                RawOverrideItem::Comment(c) => c.len(),
                RawOverrideItem::Tag { name, raw_args } => 1 + name.len() + raw_args.len(),
            };
            let item_start = cursor;
            let item_end = cursor + item_len;
            cursor = item_end;

            let karaoke = match item {
                RawOverrideItem::Tag { name, raw_args } => parse_karaoke_tag(name, raw_args),
                RawOverrideItem::Comment(_) => None,
            };

            if let Some((kind, duration_cs)) = karaoke {
                if self.has_started {
                    if let Some((ps, pe)) = pending.take() {
                        self.needs_raw = true;
                        self.cur_tags.push_span(src, ps, pe);
                        if !open_brace {
                            self.push_raw_range(brace_pos, brace_pos + 1);
                            open_brace = true;
                        }
                        self.push_raw_range(ps, pe);
                    }
                    self.flush_current();
                } else if let Some((ps, pe)) = pending.take() {
                    self.needs_raw = true;
                    self.cur_tags.push_char(src, '{');
                    self.cur_tags.push_span(src, ps, pe);
                    self.cur_tags.push_char(src, '}');
                    if !open_brace {
                        self.push_raw_range(brace_pos, brace_pos + 1);
                        open_brace = true;
                    }
                    self.push_raw_range(ps, pe);
                }
                if !open_brace {
                    self.push_raw_range(brace_pos, brace_pos + 1);
                    open_brace = true;
                }
                self.push_raw_range(item_start, item_end);
                self.cur_kind = Some(kind);
                self.cur_dur = u32::try_from(duration_cs.max(0)).unwrap_or(u32::MAX);
                self.has_started = true;
                self.needs_raw = true;
            } else {
                pending = Some(match pending {
                    None => (item_start, item_end),
                    Some((ps, _)) => (ps, item_end),
                });
            }
        });

        if let Some((ps, pe)) = pending {
            self.needs_raw = true;
            self.cur_tags.push_char(src, '{');
            self.cur_tags.push_span(src, ps, pe);
            self.cur_tags.push_char(src, '}');
            if !open_brace {
                self.push_raw_range(brace_pos, brace_pos + 1);
            }
            self.push_raw_range(ps, pe);
        }
        if closed {
            self.push_raw_range(next - 1, next);
        } else {
            self.needs_raw = true;
        }
    }

    fn finish(mut self) -> Vec<KaraokeSyllable> {
        if self.has_started {
            self.flush_current();
        }
        self.out
    }
}

/// Rebuilds an ASS `Text` string from parsed or edited [`KaraokeSyllable`]s.
#[must_use]
pub fn rebuild_karaoke_line(syllables: &[KaraokeSyllable]) -> String {
    let cap: usize = syllables
        .iter()
        .map(|s| {
            s.raw_original
                .as_ref()
                .map_or(s.tags.len() + s.text.len() + 8, String::len)
        })
        .sum();
    let mut out = String::with_capacity(cap);
    for syl in syllables {
        if let Some(ref raw) = syl.raw_original {
            out.push_str(raw);
            continue;
        }
        if let Some(kind) = syl.kind {
            write_karaoke_block(&mut out, kind, syl.duration_cs);
        }
        out.push_str(&syl.tags);
        out.push_str(&syl.text);
    }
    out
}

#[inline]
fn write_karaoke_block(out: &mut String, kind: KaraokeKind, mut val: u32) {
    let mut buf = [0u8; 16];
    let mut pos = buf.len() - 1;
    buf[pos] = b'}';
    if val == 0 {
        pos -= 1;
        buf[pos] = b'0';
    } else {
        while val > 0 {
            pos -= 1;
            #[allow(clippy::cast_possible_truncation)]
            {
                buf[pos] = b'0' + (val % 10) as u8;
            }
            val /= 10;
        }
    }
    let kbytes = karaoke_kind_str(kind).as_bytes();
    pos -= kbytes.len();
    buf[pos..pos + kbytes.len()].copy_from_slice(kbytes);
    pos -= 2;
    buf[pos] = b'{';
    buf[pos + 1] = b'\\';
    if let Ok(s) = std::str::from_utf8(&buf[pos..]) {
        out.push_str(s);
    }
}

/// Sets the duration of syllable `idx` in centiseconds. Returns `false` if `idx` is out of range.
pub fn set_syllable_duration(
    syllables: &mut [KaraokeSyllable],
    idx: usize,
    duration_cs: u32,
) -> bool {
    let Some(syl) = syllables.get_mut(idx) else {
        return false;
    };
    syl.set_duration_cs(duration_cs);
    true
}

/// Splits syllable `idx` at UTF-8 byte offset `byte_offset` inside its `text`, dividing
/// `duration_cs` proportionally by character count. Returns `false` if unable to split.
pub fn split_syllable(
    syllables: &mut Vec<KaraokeSyllable>,
    idx: usize,
    byte_offset: usize,
) -> bool {
    let Some(syl) = syllables.get(idx) else {
        return false;
    };
    let mut split_at = byte_offset.min(syl.text.len());
    while split_at > 0 && !syl.text.is_char_boundary(split_at) {
        split_at -= 1;
    }
    if split_at == 0 || split_at == syl.text.len() {
        return false;
    }

    let left_text = syl.text[..split_at].to_owned();
    let right_text = syl.text[split_at..].to_owned();
    let left_chars = left_text.chars().count();
    let total_chars = left_chars + right_text.chars().count();

    let total_dur = syl.duration_cs;
    let left_dur = if total_chars > 0 {
        u32::try_from((u64::from(total_dur) * (left_chars as u64)) / (total_chars as u64))
            .unwrap_or(total_dur / 2)
    } else {
        total_dur / 2
    };
    let right_dur = total_dur.saturating_sub(left_dur);

    let kind = syl.kind.or(Some(KaraokeKind::K));
    let left_syl = KaraokeSyllable {
        kind: syl.kind,
        duration_cs: left_dur,
        tags: syl.tags.clone(),
        text: left_text,
        raw_original: None,
    };
    let right_syl = KaraokeSyllable {
        kind,
        duration_cs: right_dur,
        tags: String::new(),
        text: right_text,
        raw_original: None,
    };

    syllables[idx] = left_syl;
    syllables.insert(idx + 1, right_syl);
    true
}

/// Merges syllables `start_idx..=end_idx_inclusive` into one syllable, summing their durations.
/// Returns `false` if the range is invalid or contains fewer than 2 syllables.
pub fn merge_syllables(
    syllables: &mut Vec<KaraokeSyllable>,
    start_idx: usize,
    end_idx_inclusive: usize,
) -> bool {
    if start_idx >= end_idx_inclusive || end_idx_inclusive >= syllables.len() {
        return false;
    }

    let mut total_dur = 0u32;
    let mut merged_text = String::new();
    let mut merged_tags = String::new();
    let kind = syllables[start_idx].kind;

    for syl in &syllables[start_idx..=end_idx_inclusive] {
        total_dur = total_dur.saturating_add(syl.duration_cs);
        merged_tags.push_str(&syl.tags);
        merged_text.push_str(&syl.text);
    }

    syllables[start_idx] = KaraokeSyllable {
        kind,
        duration_cs: total_dur,
        tags: merged_tags,
        text: merged_text,
        raw_original: None,
    };
    syllables.drain((start_idx + 1)..=end_idx_inclusive);
    true
}

const fn karaoke_kind_str(kind: KaraokeKind) -> &'static str {
    match kind {
        KaraokeKind::K => "k",
        KaraokeKind::Kf => "kf",
        KaraokeKind::CapitalK => "K",
        KaraokeKind::Ko => "ko",
        KaraokeKind::Kt => "kt",
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn karaoke_parse_and_rebuild_round_trip_exact() {
        let line = r"{\an8\k30}Ka{\kf45\c&H00FFFF&}ra{\K20}o{\ko15}ke\NLine";
        let syls = parse_karaoke_syllables(line);
        assert_eq!(syls.len(), 4);
        assert_eq!(syls[0].duration_cs, 30);
        assert_eq!(syls[0].text, "Ka");
        assert_eq!(syls[1].duration_cs, 45);
        assert_eq!(syls[1].text, "ra");
        assert_eq!(syls[2].duration_cs, 20);
        assert_eq!(syls[3].duration_cs, 15);
        assert_eq!(rebuild_karaoke_line(&syls), line);
    }

    #[test]
    fn set_duration_split_and_merge_syllables_work() {
        let mut syls = parse_karaoke_syllables(r"{\k40}Hell{\kf60}o!");
        assert!(set_syllable_duration(&mut syls, 0, 50));
        assert_eq!(rebuild_karaoke_line(&syls), r"{\k50}Hell{\kf60}o!");

        // Split "Hell" (50 cs) at byte 2 -> "He" (25 cs) + "ll" (25 cs).
        assert!(split_syllable(&mut syls, 0, 2));
        assert_eq!(syls.len(), 3);
        assert_eq!(syls[0].duration_cs, 25);
        assert_eq!(syls[0].text, "He");
        assert_eq!(syls[1].duration_cs, 25);
        assert_eq!(syls[1].text, "ll");

        // Merge back 0..=1 -> "Hell" (50 cs).
        assert!(merge_syllables(&mut syls, 0, 1));
        assert_eq!(syls.len(), 2);
        assert_eq!(syls[0].duration_cs, 50);
        assert_eq!(syls[0].text, "Hell");
    }
}
