//! "Open subtitles from video" Matroska track picker model, multi-track importer, and
//! Aegisub-compatible SRT/`WebVTT` → ASS tag converter.
//!
//! Mirrors Aegisub 3.5.1:
//! - `src/dialog_selected_choices.cpp` / `src/mkv_wrap.cpp` (track selection & import)
//! - `src/subtitle_format_srt.cpp` (`SRTSubtitleFormat::ConvertTags`: `<i>`, `<b>`, `<u>`, `<s>`,
//!   `<font color/face/size>`, HTML entities, line breaks → `\N`)
//!
//! Never fails silently on 0-event tracks or image-based subtitles (PGS/VobSub/DVBSub); always
//! surfaces an explicit [`ImportError`] or row-level [`TrackPickerRow::unselectable_reason`].

#![forbid(unsafe_code)]
#![cfg_attr(not(test), deny(clippy::unwrap_used, clippy::expect_used))]

use ass_core::{AssEvent, AssStyle, AssTime, ScriptFile};
use mkv_subs::{ExtractedTrack, MkvError, SubtitleCodec, TrackInfo, TrackType};
use std::fmt::Write as _;
use std::io::{Read, Seek};
use thiserror::Error;

/// Explicit, never-silent errors returned by `mkv-import`.
#[derive(Debug, Error)]
pub enum ImportError {
    #[error("no subtitle tracks were selected for import")]
    NoTracksSelected,
    #[error(
        "track {track} uses image-based subtitle codec `{codec}` which cannot be imported as text"
    )]
    UnsupportedImageTrack { track: u64, codec: String },
    #[error("track {track} contains 0 subtitle events ({reason})")]
    ZeroEventsTrack { track: u64, reason: String },
    #[error("Matroska reader error: {0}")]
    Mkv(#[from] MkvError),
}

/// How multiple selected tracks should be imported.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum MultiTrackMode {
    /// Merge all selected tracks into a single [`ScriptFile`] with a distinct style per track.
    MergeSeparateStyles,
    /// Produce one independent [`ScriptFile`] per selected track.
    SeparateScripts,
}

/// Output of importing one or more Matroska subtitle tracks.
#[derive(Debug, Clone)]
pub enum ImportOutput {
    Merged(ScriptFile),
    Separate(Vec<(TrackInfo, ScriptFile)>),
}

/// One row in the "Open subtitles from video" track picker dialog.
#[derive(Debug, Clone, PartialEq, Eq)]
#[allow(clippy::struct_excessive_bools)]
pub struct TrackPickerRow {
    pub display_index: usize,
    pub track_number: u64,
    pub codec_id: String,
    pub codec: SubtitleCodec,
    pub language: String,
    pub name: String,
    pub is_default: bool,
    pub is_forced: bool,
    pub event_count: Option<usize>,
    pub preview_lines: Vec<String>,
    pub selectable: bool,
    pub unselectable_reason: Option<String>,
    pub selected: bool,
}

/// State model for the "Open subtitles from video" Matroska subtitle track picker.
#[derive(Debug, Default, Clone)]
pub struct TrackPickerModel {
    rows: Vec<TrackPickerRow>,
}

impl TrackPickerModel {
    /// Builds a picker model from Matroska [`TrackInfo`] metadata (no 32-track cap).
    ///
    /// Filters to `TrackType::Subtitle`, marks image-based tracks (`S_HDMV/PGS`, `S_VOBSUB`, etc.)
    /// as unselectable with a clear reason, and pre-selects the first default selectable track (or
    /// the first selectable track if none is marked default).
    #[must_use]
    pub fn from_tracks(tracks: &[TrackInfo]) -> Self {
        let mut rows = Vec::with_capacity(tracks.len());
        let mut first_default_idx: Option<usize> = None;
        let mut first_selectable_idx: Option<usize> = None;

        for t in tracks {
            if t.track_type != TrackType::Subtitle {
                continue;
            }
            let selectable = t.subtitle_codec.is_supported_text();
            let unselectable_reason = match &t.subtitle_codec {
                SubtitleCodec::UnsupportedImage(c) => Some(format!(
                    "Image subtitles ({c}) require OCR and cannot be imported directly as text"
                )),
                SubtitleCodec::Other(c) => Some(format!("Unsupported subtitle codec ({c})")),
                _ => None,
            };

            let row_idx = rows.len();
            if selectable {
                if first_selectable_idx.is_none() {
                    first_selectable_idx = Some(row_idx);
                }
                if t.is_default && first_default_idx.is_none() {
                    first_default_idx = Some(row_idx);
                }
            }

            rows.push(TrackPickerRow {
                display_index: row_idx + 1,
                track_number: t.number,
                codec_id: t.codec_id.clone(),
                codec: t.subtitle_codec.clone(),
                language: t.display_language().unwrap_or("und").to_owned(),
                name: t.name.clone().unwrap_or_default(),
                is_default: t.is_default,
                is_forced: t.is_forced,
                event_count: None,
                preview_lines: Vec::new(),
                selectable,
                unselectable_reason,
                selected: false,
            });
        }

        if let Some(idx) = first_default_idx.or(first_selectable_idx) {
            rows[idx].selected = true;
        }
        Self { rows }
    }

    /// All subtitle rows in container order.
    #[inline]
    #[must_use]
    pub fn rows(&self) -> &[TrackPickerRow] {
        &self.rows
    }

    /// Returns `true` if `track_number` is currently selected (0 allocations).
    #[inline]
    #[must_use]
    pub fn is_selected(&self, track_number: u64) -> bool {
        self.rows
            .iter()
            .find(|r| r.track_number == track_number)
            .is_some_and(|r| r.selected)
    }

    /// Toggles selection of a selectable track (0 allocations). Returns `false` if unselectable.
    pub fn toggle_select(&mut self, track_number: u64) -> bool {
        if let Some(row) = self
            .rows
            .iter_mut()
            .find(|r| r.track_number == track_number)
        {
            if !row.selectable {
                return false;
            }
            row.selected = !row.selected;
            return true;
        }
        false
    }

    /// Selects only `track_number`, clearing other selections (0 allocations).
    pub fn select_only(&mut self, track_number: u64) -> bool {
        let can_select = self
            .rows
            .iter()
            .find(|r| r.track_number == track_number)
            .is_some_and(|r| r.selectable);
        if !can_select {
            return false;
        }
        for row in &mut self.rows {
            row.selected = row.track_number == track_number;
        }
        true
    }

    /// Selects all supported text subtitle tracks (0 allocations).
    pub fn select_all_supported(&mut self) {
        for row in &mut self.rows {
            row.selected = row.selectable;
        }
    }

    /// Writes currently selected track numbers into `out` (0 allocations).
    pub fn selected_track_numbers_into(&self, out: &mut Vec<u64>) {
        out.clear();
        for row in &self.rows {
            if row.selected && row.selectable {
                out.push(row.track_number);
            }
        }
    }

    /// Returns the currently selected track numbers.
    #[must_use]
    pub fn selected_track_numbers(&self) -> Vec<u64> {
        let mut out = Vec::new();
        self.selected_track_numbers_into(&mut out);
        out
    }

    /// Populates `preview_lines` (first `max_lines` events) and updates `event_count` if the
    /// track has fewer events than `max_lines`, using [`mkv_subs::preview_track`].
    ///
    /// # Errors
    /// Returns [`ImportError::UnsupportedImageTrack`], [`ImportError::ZeroEventsTrack`], or
    /// [`ImportError::Mkv`].
    pub fn load_preview<R: Read + Seek>(
        &mut self,
        reader: R,
        track_number: u64,
        max_lines: usize,
    ) -> Result<&[String], ImportError> {
        let pos = self
            .rows
            .iter()
            .position(|r| r.track_number == track_number)
            .ok_or(ImportError::Mkv(MkvError::TrackNotFound(track_number)))?;
        if !self.rows[pos].selectable {
            return Err(ImportError::UnsupportedImageTrack {
                track: track_number,
                codec: self.rows[pos].codec_id.clone(),
            });
        }
        let extracted = mkv_subs::preview_track(reader, track_number, max_lines.max(1), None)?;
        if extracted.event_count == 0 {
            return Err(ImportError::ZeroEventsTrack {
                track: track_number,
                reason: "no subtitle blocks found during preview".to_owned(),
            });
        }
        let mut lines = Vec::with_capacity(extracted.event_count.min(max_lines));
        for ev in extracted.script.events().take(max_lines) {
            lines.push(ev.text().to_owned());
        }
        self.rows[pos].preview_lines = lines;
        if extracted.event_count < max_lines {
            self.rows[pos].event_count = Some(extracted.event_count);
        }
        Ok(&self.rows[pos].preview_lines)
    }
}

/// Imports one or more selected tracks from `reader` in a single pass via `mkv-subs`.
///
/// Converts SRT/`WebVTT` HTML-style tags into ASS override tags and fails explicitly on empty or
/// image-based tracks.
///
/// # Errors
/// Returns [`ImportError`] if no tracks are selected, a track is an unsupported image codec, or a
/// track yields 0 events.
pub fn import_tracks<R: Read + Seek>(
    reader: R,
    track_numbers: &[u64],
    mode: MultiTrackMode,
) -> Result<ImportOutput, ImportError> {
    if track_numbers.is_empty() {
        return Err(ImportError::NoTracksSelected);
    }
    let mut map =
        mkv_subs::extract_tracks(reader, track_numbers, None, |_, _| {}).map_err(|e| match e {
            MkvError::UnsupportedCodec { track, codec } => {
                ImportError::UnsupportedImageTrack { track, codec }
            }
            MkvError::EmptyTrack { track, reason } => {
                ImportError::ZeroEventsTrack { track, reason }
            }
            other => ImportError::Mkv(other),
        })?;

    let mut ordered: Vec<ExtractedTrack> = Vec::with_capacity(track_numbers.len());
    for &num in track_numbers {
        let Some(mut ext) = map.remove(&num) else {
            return Err(ImportError::ZeroEventsTrack {
                track: num,
                reason: "track was not present in extracted output".to_owned(),
            });
        };
        if ext.event_count == 0 {
            return Err(ImportError::ZeroEventsTrack {
                track: num,
                reason: "track contained 0 subtitle events".to_owned(),
            });
        }
        // Normalize SRT / `WebVTT` tags to ASS tags in place.
        if matches!(
            ext.track.subtitle_codec,
            SubtitleCodec::Srt | SubtitleCodec::WebVtt
        ) {
            normalize_script_srt_tags(&mut ext.script);
        }
        ordered.push(ext);
    }

    match mode {
        MultiTrackMode::SeparateScripts => Ok(ImportOutput::Separate(
            ordered.into_iter().map(|e| (e.track, e.script)).collect(),
        )),
        MultiTrackMode::MergeSeparateStyles => Ok(ImportOutput::Merged(merge_extracted(ordered))),
    }
}

fn merge_extracted(mut tracks: Vec<ExtractedTrack>) -> ScriptFile {
    if tracks.len() == 1
        && let Some(first) = tracks.pop()
    {
        return first.script;
    }
    let mut merged = ScriptFile::new_default();
    let base_style_idx = merged
        .styles_section()
        .map_or(0, ass_core::StylesSection::style_count);
    let mut ev_idx = 0usize;

    for (offset, ext) in tracks.into_iter().enumerate() {
        let style_idx = base_style_idx + offset;
        let lang = ext.track.display_language().unwrap_or("und");
        let style_name = format!("Track{}_{}", ext.track.number, lang);
        let mut st = AssStyle::default();
        st.set_name(style_name.clone());
        merged
            .ensure_styles_section_mut()
            .insert_style(style_idx, st);

        let ev_sec = merged.ensure_events_section_mut();
        for ev in ext.script.events() {
            let mut cloned = ev.clone();
            cloned.set_style(style_name.clone());
            ev_sec.insert_event(ev_idx, cloned);
            ev_idx += 1;
        }
    }
    merged
}

fn normalize_script_srt_tags(script: &mut ScriptFile) {
    let mut buf = String::with_capacity(256);
    if let Some(sec) = script.events_section_mut() {
        let count = sec.event_count();
        for i in 0..count {
            if let Some(ev) = sec.get_event_mut(i) {
                srt_text_to_ass_into(ev.text(), &mut buf);
                if buf != ev.text() {
                    ev.set_text(buf.clone());
                }
            }
        }
    }
}

/// Converts a slice of `(start, end, raw_srt_text)` entries into an ASS [`ScriptFile`] in one
/// pass (reusing an internal scratch buffer for minimal allocations).
#[must_use]
pub fn convert_srt_events_to_ass(
    events: &[(AssTime, AssTime, &str)],
    style_name: &str,
) -> ScriptFile {
    let mut script = ScriptFile::new_default();
    let ev_sec = script.ensure_events_section_mut();
    let mut buf = String::with_capacity(256);
    for (i, &(start, end, raw_text)) in events.iter().enumerate() {
        srt_text_to_ass_into(raw_text, &mut buf);
        let mut ev = AssEvent::dialogue(start, end, buf.as_str());
        if style_name != "Default" {
            ev.set_style(style_name);
        }
        ev_sec.insert_event(i, ev);
    }
    script
}

/// Converts SRT / `WebVTT` formatted cue text into ASS dialogue text, writing into `out` (reusing
/// `out`'s capacity for **0 heap allocations** in steady state).
///
/// Mirrors Aegisub 3.5.1 `src/subtitle_format_srt.cpp` (`SRTSubtitleFormat::ConvertTags`):
/// - `<i>`/`</i>` → `{\i1}`/`{\i0}`, `<b>`/`</b>` → `{\b1}`/`{\b0}`,
///   `<u>`/`</u>` → `{\u1}`/`{\u0}`, `<s>`/`</s>` → `{\s1}`/`{\s0}`
/// - `<font color="#RRGGBB" face="..." size="...">` → `{\c&HBBGGRR&\fn...\fs...}`, `</font>` → `{\r}`
/// - HTML entities (`&amp;`, `&lt;`, `&gt;`, `&quot;`, `&apos;`, `&nbsp;`, `&#NNN;`, `&#xHHH;`)
/// - `\r\n` / `\n` → `\N`
#[allow(clippy::too_many_lines)]
pub fn srt_text_to_ass_into(input: &str, out: &mut String) {
    out.clear();
    let bytes = input.as_bytes();
    let len = bytes.len();
    let mut i = 0usize;

    while i < len {
        match bytes[i] {
            b'\r' => {
                if i + 1 < len && bytes[i + 1] == b'\n' {
                    i += 2;
                } else {
                    i += 1;
                }
                out.push_str(r"\N");
            }
            b'\n' => {
                out.push_str(r"\N");
                i += 1;
            }
            b'<' => {
                if let Some(rel_end) = bytes[i + 1..].iter().position(|&b| b == b'>') {
                    let tag = &input[i + 1..i + 1 + rel_end];
                    let trimmed = tag.trim();
                    if emit_html_tag_as_ass(trimmed, out) {
                        i += rel_end + 2;
                        continue;
                    }
                    // Strip known `WebVTT` cue tags (`<c...>`, `</c>`, `<v ...>`, `</v>`, `<00:..>`).
                    if is_webvtt_tag(trimmed) {
                        i += rel_end + 2;
                        continue;
                    }
                }
                out.push('<');
                i += 1;
            }
            b'&' => {
                if let Some((ch, consumed)) = decode_html_entity(&input[i..]) {
                    out.push(ch);
                    i += consumed;
                    continue;
                }
                out.push('&');
                i += 1;
            }
            _ => {
                let start = i;
                i += 1;
                while i < len && !matches!(bytes[i], b'\r' | b'\n' | b'<' | b'&') {
                    i += 1;
                }
                out.push_str(&input[start..i]);
            }
        }
    }
}

fn emit_html_tag_as_ass(tag: &str, out: &mut String) -> bool {
    if tag.eq_ignore_ascii_case("i") {
        out.push_str(r"{\i1}");
        return true;
    }
    if tag.eq_ignore_ascii_case("/i") {
        out.push_str(r"{\i0}");
        return true;
    }
    if tag.eq_ignore_ascii_case("b") {
        out.push_str(r"{\b1}");
        return true;
    }
    if tag.eq_ignore_ascii_case("/b") {
        out.push_str(r"{\b0}");
        return true;
    }
    if tag.eq_ignore_ascii_case("u") {
        out.push_str(r"{\u1}");
        return true;
    }
    if tag.eq_ignore_ascii_case("/u") {
        out.push_str(r"{\u0}");
        return true;
    }
    if tag.eq_ignore_ascii_case("s") || tag.eq_ignore_ascii_case("strike") {
        out.push_str(r"{\s1}");
        return true;
    }
    if tag.eq_ignore_ascii_case("/s") || tag.eq_ignore_ascii_case("/strike") {
        out.push_str(r"{\s0}");
        return true;
    }
    if tag.eq_ignore_ascii_case("/font") {
        out.push_str(r"{\r}");
        return true;
    }
    if tag.eq_ignore_ascii_case("br") || tag.eq_ignore_ascii_case("br/") {
        out.push_str(r"\N");
        return true;
    }
    if tag.len() >= 4 && tag[..4].eq_ignore_ascii_case("font") {
        let rest = tag[4..].trim();
        if rest.is_empty() {
            return true;
        }
        let start_len = out.len();
        out.push('{');
        parse_font_attrs(rest, out);
        if out.len() == start_len + 1 {
            out.truncate(start_len);
        } else {
            out.push('}');
        }
        return true;
    }
    false
}

fn parse_font_attrs(mut s: &str, out: &mut String) {
    while !s.is_empty() {
        s = s.trim_start();
        let Some(eq) = s.find('=') else {
            break;
        };
        let key = s[..eq].trim();
        s = s[eq + 1..].trim_start();
        let (val, rest) = if let Some(after_q) = s.strip_prefix('"') {
            if let Some(end_q) = after_q.find('"') {
                (&after_q[..end_q], &after_q[end_q + 1..])
            } else {
                (after_q, "")
            }
        } else if let Some(after_q) = s.strip_prefix('\'') {
            if let Some(end_q) = after_q.find('\'') {
                (&after_q[..end_q], &after_q[end_q + 1..])
            } else {
                (after_q, "")
            }
        } else if let Some(sp) = s.find(char::is_whitespace) {
            (&s[..sp], &s[sp + 1..])
        } else {
            (s, "")
        };
        s = rest;

        if key.eq_ignore_ascii_case("color") {
            if let Some((r, g, b)) = parse_html_color(val) {
                let _ = write!(out, r"\c&H{b:02X}{g:02X}{r:02X}&");
            }
        } else if key.eq_ignore_ascii_case("face") {
            if !val.is_empty() {
                out.push_str(r"\fn");
                out.push_str(val);
            }
        } else if key.eq_ignore_ascii_case("size")
            && let Ok(sz) = val.trim().parse::<u32>()
        {
            let _ = write!(out, r"\fs{sz}");
        }
    }
}

fn parse_html_color(val: &str) -> Option<(u8, u8, u8)> {
    let v = val.trim();
    if let Some(hex) = v.strip_prefix('#') {
        if hex.len() == 6 {
            let r = u8::from_str_radix(&hex[0..2], 16).ok()?;
            let g = u8::from_str_radix(&hex[2..4], 16).ok()?;
            let b = u8::from_str_radix(&hex[4..6], 16).ok()?;
            return Some((r, g, b));
        }
        if hex.len() == 3 {
            let r = u8::from_str_radix(&hex[0..1], 16).ok()? * 17;
            let g = u8::from_str_radix(&hex[1..2], 16).ok()? * 17;
            let b = u8::from_str_radix(&hex[2..3], 16).ok()? * 17;
            return Some((r, g, b));
        }
    }
    if v.eq_ignore_ascii_case("white") {
        Some((255, 255, 255))
    } else if v.eq_ignore_ascii_case("black") {
        Some((0, 0, 0))
    } else if v.eq_ignore_ascii_case("red") {
        Some((255, 0, 0))
    } else if v.eq_ignore_ascii_case("green") {
        Some((0, 128, 0))
    } else if v.eq_ignore_ascii_case("blue") {
        Some((0, 0, 255))
    } else if v.eq_ignore_ascii_case("yellow") {
        Some((255, 255, 0))
    } else if v.eq_ignore_ascii_case("cyan") {
        Some((0, 255, 255))
    } else if v.eq_ignore_ascii_case("magenta") {
        Some((255, 0, 255))
    } else {
        None
    }
}

fn is_webvtt_tag(tag: &str) -> bool {
    let b = tag.as_bytes();
    if b.is_empty() {
        return false;
    }
    if b[0].is_ascii_digit() && tag.contains(':') {
        return true;
    }
    tag.starts_with("c.")
        || tag == "/c"
        || tag.starts_with("v ")
        || tag == "/v"
        || tag.starts_with("lang ")
        || tag == "/lang"
}

fn decode_html_entity(s: &str) -> Option<(char, usize)> {
    let semi = s.as_bytes().iter().take(12).position(|&b| b == b';')?;
    let body = &s[1..semi];
    let ch = match body {
        "amp" => '&',
        "lt" => '<',
        "gt" => '>',
        "quot" => '"',
        "apos" => '\'',
        "nbsp" => ' ',
        _ => {
            if let Some(hex) = body.strip_prefix("#x").or_else(|| body.strip_prefix("#X")) {
                let code = u32::from_str_radix(hex, 16).ok()?;
                char::from_u32(code)?
            } else {
                let dec = body.strip_prefix('#')?;
                let code = dec.parse::<u32>().ok()?;
                char::from_u32(code)?
            }
        }
    };
    Some((ch, semi + 1))
}

#[cfg(test)]
mod tests {
    use super::*;

    fn fake_track(num: u64, codec_id: &str, lang: &str, is_default: bool) -> TrackInfo {
        TrackInfo {
            number: num,
            track_type: TrackType::Subtitle,
            codec_id: codec_id.to_owned(),
            subtitle_codec: SubtitleCodec::from_codec_id(codec_id),
            language: Some(lang.to_owned()),
            language_bcp47: None,
            name: Some(format!("Track {num}")),
            is_default,
            is_forced: false,
            codec_private: None,
            encodings: Vec::new(),
        }
    }

    #[test]
    fn picker_handles_64_tracks_and_flags_pgs_vobsub() {
        let mut tracks = Vec::with_capacity(64);
        tracks.push(fake_track(1, "S_HDMV/PGS", "jpn", true));
        tracks.push(fake_track(2, "S_VOBSUB", "eng", false));
        for n in 3..=64u64 {
            tracks.push(fake_track(n, "S_TEXT/UTF8", "vie", n == 5));
        }
        let mut picker = TrackPickerModel::from_tracks(&tracks);
        assert_eq!(picker.rows().len(), 64);
        assert!(!picker.rows()[0].selectable);
        assert!(picker.rows()[0].unselectable_reason.is_some());
        assert!(!picker.rows()[1].selectable);
        // Track 5 was the first selectable default track
        assert!(picker.is_selected(5));
        assert!(!picker.toggle_select(1), "PGS cannot be selected");
        assert!(picker.toggle_select(6));
        assert_eq!(picker.selected_track_numbers(), vec![5, 6]);
    }

    #[test]
    fn converts_srt_html_tags_entities_and_newlines() {
        let mut out = String::new();
        srt_text_to_ass_into(
            "<i>Hello</i> &amp; <b>world</b>\r\n<font color=\"#FF8000\" face=\"Arial\" size=\"48\">Line 2 &lt;ok&gt;</font>",
            &mut out,
        );
        assert_eq!(
            out,
            r"{\i1}Hello{\i0} & {\b1}world{\b0}\N{\c&H0080FF&\fnArial\fs48}Line 2 <ok>{\r}"
        );
    }

    #[test]
    fn rejects_empty_selection_explicitly() {
        let cursor = std::io::Cursor::new(Vec::<u8>::new());
        let err = import_tracks(cursor, &[], MultiTrackMode::MergeSeparateStyles).unwrap_err();
        assert!(matches!(err, ImportError::NoTracksSelected));
    }
}
