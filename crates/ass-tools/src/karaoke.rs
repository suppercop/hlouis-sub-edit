//! Karaoke syllable parser, builder, and editing helpers (`\k`, `\K`, `\kf`, `\ko`, `\kt`).
//!
//! Parsing a line into [`KaraokeSyllable`]s and rebuilding it with [`rebuild_karaoke_line`]
//! reproduces the original ASS `Text` byte-for-byte when no syllable is mutated.

use ass_core::{
    parse_text_blocks, serialize_text_blocks, KaraokeKind, OverrideItem, TagArgs, TextBlock,
};
use std::fmt::Write as _;

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
    let blocks = parse_text_blocks(text);
    let mut state = ParseAccum::default();
    for block in blocks {
        match block {
            TextBlock::Override { items, closed } => state.push_override(items, closed),
            other => {
                let s = serialize_text_blocks(&[other]);
                state.cur_text.push_str(&s);
                state.cur_raw.push_str(&s);
                state.has_started = true;
            }
        }
    }
    state.finish()
}

#[derive(Default)]
struct ParseAccum {
    out: Vec<KaraokeSyllable>,
    cur_kind: Option<KaraokeKind>,
    cur_dur: u32,
    cur_tags: String,
    cur_text: String,
    cur_raw: String,
    has_started: bool,
}

impl ParseAccum {
    fn flush_current(&mut self) {
        self.out.push(KaraokeSyllable {
            kind: self.cur_kind,
            duration_cs: self.cur_dur,
            tags: std::mem::take(&mut self.cur_tags),
            text: std::mem::take(&mut self.cur_text),
            raw_original: Some(std::mem::take(&mut self.cur_raw)),
        });
    }

    fn push_override(&mut self, items: Vec<OverrideItem>, closed: bool) {
        let has_k = items.iter().any(|item| {
            matches!(
                item,
                OverrideItem::Tag(t) if matches!(t.args, TagArgs::Karaoke { .. })
            )
        });
        if !has_k {
            let s = serialize_text_blocks(&[TextBlock::Override { items, closed }]);
            self.cur_tags.push_str(&s);
            self.cur_raw.push_str(&s);
            self.has_started = true;
            return;
        }

        let mut open_brace = false;
        let mut pending: Vec<OverrideItem> = Vec::new();
        for item in items {
            if let OverrideItem::Tag(ref tag) = item {
                if let TagArgs::Karaoke { kind, duration_cs } = tag.args {
                    if self.has_started {
                        if !pending.is_empty() {
                            let s = serialize_override_items(&pending);
                            self.cur_tags.push_str(&s);
                            if !open_brace {
                                self.cur_raw.push('{');
                                open_brace = true;
                            }
                            self.cur_raw.push_str(&s);
                            pending.clear();
                        }
                        self.flush_current();
                    } else if !pending.is_empty() {
                        let s = serialize_override_items(&pending);
                        self.cur_tags.push('{');
                        self.cur_tags.push_str(&s);
                        self.cur_tags.push('}');
                        if !open_brace {
                            self.cur_raw.push('{');
                            open_brace = true;
                        }
                        self.cur_raw.push_str(&s);
                        pending.clear();
                    }
                    if !open_brace {
                        self.cur_raw.push('{');
                        open_brace = true;
                    }
                    self.cur_raw.push('\\');
                    self.cur_raw.push_str(&tag.name);
                    self.cur_raw.push_str(&tag.raw_args);
                    self.cur_kind = Some(kind);
                    self.cur_dur = u32::try_from(duration_cs.max(0)).unwrap_or(u32::MAX);
                    self.has_started = true;
                    continue;
                }
            }
            pending.push(item);
        }

        if !pending.is_empty() {
            let s = serialize_override_items(&pending);
            self.cur_tags.push('{');
            self.cur_tags.push_str(&s);
            self.cur_tags.push('}');
            if !open_brace {
                self.cur_raw.push('{');
            }
            self.cur_raw.push_str(&s);
        }
        if closed {
            self.cur_raw.push('}');
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
    let mut out = String::new();
    for syl in syllables {
        if let Some(ref raw) = syl.raw_original {
            out.push_str(raw);
            continue;
        }
        if let Some(kind) = syl.kind {
            let tag_name = karaoke_kind_str(kind);
            let _ = write!(out, "{{\\{tag_name}{}}}", syl.duration_cs);
        }
        out.push_str(&syl.tags);
        out.push_str(&syl.text);
    }
    out
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

fn serialize_override_items(items: &[OverrideItem]) -> String {
    let mut out = String::new();
    for item in items {
        match item {
            OverrideItem::Comment(c) => out.push_str(c),
            OverrideItem::Tag(tag) => {
                out.push('\\');
                out.push_str(&tag.name);
                out.push_str(&tag.raw_args);
            }
        }
    }
    out
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
