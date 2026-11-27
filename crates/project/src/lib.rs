//! Aegisub 3.5.1 `[Aegisub Project Garbage]` reader/writer and Windows/UNC path normalization.
//!
//! Reference behaviour (Aegisub 3.5.1 `src/project.cpp` & `src/ass_file.cpp`):
//! - Reads and updates `Audio File`, `Video File`, `Timecodes File`, `Keyframes File`,
//!   `Video AR Mode`, `Video AR Value`, `Video Zoom Percent`, `Scroll Position`, `Active Line`,
//!   `Video Position`, `Automation Scripts`, and `Export Filters`.
//! - Preserves all unknown keys verbatim and never touches `[Events]`.
//! - Stores media paths relative to the script directory when on the same drive / UNC share,
//!   preserving Aegisub dummy markers (`?video`, `?audio`, `?script`, `?dummy:...`).

#![forbid(unsafe_code)]
#![cfg_attr(not(test), deny(clippy::unwrap_used, clippy::expect_used))]

use ass_core::{KeyValueEntry, KeyValueSection, ScriptFile, Section};

/// Canonical `[Aegisub Project Garbage]` keys as written by Aegisub 3.5.1.
pub mod keys {
    pub const AUDIO_FILE: &str = "Audio File";
    pub const VIDEO_FILE: &str = "Video File";
    pub const TIMECODES_FILE: &str = "Timecodes File";
    pub const KEYFRAMES_FILE: &str = "Keyframes File";
    pub const VIDEO_AR_MODE: &str = "Video AR Mode";
    pub const VIDEO_AR_VALUE: &str = "Video AR Value";
    pub const VIDEO_ZOOM_PERCENT: &str = "Video Zoom Percent";
    pub const SCROLL_POSITION: &str = "Scroll Position";
    pub const ACTIVE_LINE: &str = "Active Line";
    pub const VIDEO_POSITION: &str = "Video Position";
    pub const AUTOMATION_SCRIPTS: &str = "Automation Scripts";
    pub const EXPORT_FILTERS: &str = "Export Filters";
}

/// Typed UI-facing model of `[Aegisub Project Garbage]`.
#[derive(Debug, Clone, Default, PartialEq)]
pub struct ProjectLinks {
    pub audio_file: Option<String>,
    pub video_file: Option<String>,
    pub timecodes_file: Option<String>,
    pub keyframes_file: Option<String>,
    pub video_ar_mode: Option<i32>,
    pub video_ar_value: Option<f64>,
    pub video_zoom_percent: Option<f64>,
    pub scroll_position: Option<usize>,
    pub active_line: Option<usize>,
    pub video_position: Option<u32>,
    pub automation_scripts: Option<String>,
    pub export_filters: Option<String>,
}

impl ProjectLinks {
    /// Returns `true` if every known field is `None`.
    #[must_use]
    pub const fn is_empty(&self) -> bool {
        self.audio_file.is_none()
            && self.video_file.is_none()
            && self.timecodes_file.is_none()
            && self.keyframes_file.is_none()
            && self.video_ar_mode.is_none()
            && self.video_ar_value.is_none()
            && self.video_zoom_percent.is_none()
            && self.scroll_position.is_none()
            && self.active_line.is_none()
            && self.video_position.is_none()
            && self.automation_scripts.is_none()
            && self.export_filters.is_none()
    }
}

/// Reads `[Aegisub Project Garbage]` from `script` without touching `[Events]`.
///
/// Values that are present but unparseable are dropped; use
/// [`read_project_links_with_warnings`] to learn which ones (the UI must show them).
#[must_use]
pub fn read_project_links(script: &ScriptFile) -> ProjectLinks {
    read_project_links_with_warnings(script).0
}

/// Like [`read_project_links`] but also returns one human-readable warning per key whose value
/// was present but could not be parsed (so a saved video position / zoom is not lost silently).
#[must_use]
pub fn read_project_links_with_warnings(script: &ScriptFile) -> (ProjectLinks, Vec<String>) {
    let mut warnings = Vec::new();
    let Some(sec) = script.aegisub_garbage() else {
        return (ProjectLinks::default(), warnings);
    };
    let typed = |key: &str| -> Option<String> { sec.get(key).map(|v| v.trim().to_owned()) };
    let raw_mode = typed(keys::VIDEO_AR_MODE);
    let raw_value = typed(keys::VIDEO_AR_VALUE);
    let raw_zoom = typed(keys::VIDEO_ZOOM_PERCENT);
    let raw_scroll = typed(keys::SCROLL_POSITION);
    let raw_active = typed(keys::ACTIVE_LINE);
    let raw_pos = typed(keys::VIDEO_POSITION);
    let links = ProjectLinks {
        audio_file: sec.get(keys::AUDIO_FILE).map(str::to_owned),
        video_file: sec.get(keys::VIDEO_FILE).map(str::to_owned),
        timecodes_file: sec.get(keys::TIMECODES_FILE).map(str::to_owned),
        keyframes_file: sec.get(keys::KEYFRAMES_FILE).map(str::to_owned),
        video_ar_mode: parse_or_warn(keys::VIDEO_AR_MODE, raw_mode, &mut warnings),
        video_ar_value: parse_or_warn(keys::VIDEO_AR_VALUE, raw_value, &mut warnings),
        video_zoom_percent: parse_or_warn(keys::VIDEO_ZOOM_PERCENT, raw_zoom, &mut warnings),
        scroll_position: parse_or_warn(keys::SCROLL_POSITION, raw_scroll, &mut warnings),
        active_line: parse_or_warn(keys::ACTIVE_LINE, raw_active, &mut warnings),
        video_position: parse_or_warn(keys::VIDEO_POSITION, raw_pos, &mut warnings),
        automation_scripts: sec.get(keys::AUTOMATION_SCRIPTS).map(str::to_owned),
        export_filters: sec.get(keys::EXPORT_FILTERS).map(str::to_owned),
    };
    (links, warnings)
}

fn parse_or_warn<T: std::str::FromStr>(
    key: &str,
    raw: Option<String>,
    warnings: &mut Vec<String>,
) -> Option<T> {
    let raw = raw?;
    if let Ok(v) = raw.parse::<T>() {
        Some(v)
    } else {
        warnings.push(format!(
            "project key `{key}` has invalid value `{raw}`; it was ignored"
        ));
        None
    }
}

/// Writes `links` into `[Aegisub Project Garbage]` on `script`, preserving all unknown keys
/// and leaving untouched entries byte-identical (never touching `[Events]`).
pub fn write_project_links(script: &mut ScriptFile, links: &ProjectLinks) {
    // If nothing to write and section doesn't exist, keep script completely untouched.
    if links.is_empty() && script.aegisub_garbage().is_none() {
        return;
    }

    let sec = ensure_garbage_section_mut(script);
    sync_opt_str(sec, keys::AUDIO_FILE, links.audio_file.as_deref());
    sync_opt_str(sec, keys::VIDEO_FILE, links.video_file.as_deref());
    sync_opt_str(sec, keys::TIMECODES_FILE, links.timecodes_file.as_deref());
    sync_opt_str(sec, keys::KEYFRAMES_FILE, links.keyframes_file.as_deref());
    sync_opt_i32(sec, keys::VIDEO_AR_MODE, links.video_ar_mode);
    sync_opt_f64(sec, keys::VIDEO_AR_VALUE, links.video_ar_value);
    sync_opt_f64(sec, keys::VIDEO_ZOOM_PERCENT, links.video_zoom_percent);
    sync_opt_usize(sec, keys::SCROLL_POSITION, links.scroll_position);
    sync_opt_usize(sec, keys::ACTIVE_LINE, links.active_line);
    sync_opt_u32(sec, keys::VIDEO_POSITION, links.video_position);
    sync_opt_str(
        sec,
        keys::AUTOMATION_SCRIPTS,
        links.automation_scripts.as_deref(),
    );
    sync_opt_str(sec, keys::EXPORT_FILTERS, links.export_filters.as_deref());
}

fn ensure_garbage_section_mut(script: &mut ScriptFile) -> &mut KeyValueSection {
    if let Some(pos) = script
        .sections
        .iter()
        .position(|s| matches!(s, Section::AegisubGarbage(_)))
    {
        match &mut script.sections[pos] {
            Section::AegisubGarbage(sec) => return sec,
            _ => unreachable!(),
        }
    }

    // Insert right after [Script Info] (Aegisub 3.5.1 convention).
    let insert_pos = script
        .sections
        .iter()
        .position(|s| matches!(s, Section::ScriptInfo(_)))
        .map_or(0, |i| i + 1);
    script.sections.insert(
        insert_pos,
        Section::AegisubGarbage(KeyValueSection {
            header: "[Aegisub Project Garbage]".to_owned(),
            entries: vec![KeyValueEntry::Blank],
        }),
    );
    match &mut script.sections[insert_pos] {
        Section::AegisubGarbage(sec) => sec,
        _ => unreachable!(),
    }
}

fn sync_opt_str(sec: &mut KeyValueSection, key: &str, value: Option<&str>) {
    match value {
        Some(v) => {
            if sec.get(key) != Some(v) {
                sec.set(key, v);
            }
        }
        None => remove_key(sec, key),
    }
}

fn sync_opt_i32(sec: &mut KeyValueSection, key: &str, value: Option<i32>) {
    match value {
        Some(v) => {
            if sec.get(key).and_then(|s| s.trim().parse::<i32>().ok()) != Some(v) {
                sec.set(key, v.to_string());
            }
        }
        None => remove_key(sec, key),
    }
}

fn sync_opt_usize(sec: &mut KeyValueSection, key: &str, value: Option<usize>) {
    match value {
        Some(v) => {
            if sec.get(key).and_then(|s| s.trim().parse::<usize>().ok()) != Some(v) {
                sec.set(key, v.to_string());
            }
        }
        None => remove_key(sec, key),
    }
}

fn sync_opt_u32(sec: &mut KeyValueSection, key: &str, value: Option<u32>) {
    match value {
        Some(v) => {
            if sec.get(key).and_then(|s| s.trim().parse::<u32>().ok()) != Some(v) {
                sec.set(key, v.to_string());
            }
        }
        None => remove_key(sec, key),
    }
}

fn sync_opt_f64(sec: &mut KeyValueSection, key: &str, value: Option<f64>) {
    match value {
        Some(v) => {
            let existing = sec.get(key).and_then(|s| s.trim().parse::<f64>().ok());
            if existing.is_none_or(|e| (e - v).abs() > 1e-9) {
                sec.set(key, format!("{v:.6}"));
            }
        }
        None => remove_key(sec, key),
    }
}

fn remove_key(sec: &mut KeyValueSection, key: &str) {
    sec.entries.retain(|e| match e {
        KeyValueEntry::KeyValue { key: k, .. } => !k.eq_ignore_ascii_case(key),
        _ => true,
    });
}

/// Returns `true` if `path` is an Aegisub dummy/virtual path marker (`?video`, `?audio`,
/// `?script`, or `?dummy:...`).
#[must_use]
pub fn is_aegisub_marker(path: &str) -> bool {
    let trimmed = path.trim();
    trimmed.starts_with('?')
}

/// Computes a relative path from `script_dir` to `target_path` when both share the same root
/// (Windows drive letter, UNC `\\server\share`, or POSIX `/`), using case-insensitive component
/// comparison and accepting both `/` and `\`. Preserves Aegisub `?` markers unchanged.
#[must_use]
pub fn make_relative_to_script(script_dir: &str, target_path: &str) -> String {
    if is_aegisub_marker(target_path) {
        return target_path.to_owned();
    }
    let Some((base_root, base_parts)) = split_win_path(script_dir) else {
        return target_path.to_owned();
    };
    let Some((tgt_root, tgt_parts)) = split_win_path(target_path) else {
        return target_path.to_owned();
    };

    if !base_root.eq_ignore_ascii_case(&tgt_root) {
        return target_path.to_owned();
    }

    let common = base_parts
        .iter()
        .zip(tgt_parts.iter())
        .take_while(|(a, b)| a.eq_ignore_ascii_case(b))
        .count();

    let mut rel = vec![".."; base_parts.len() - common];
    for part in &tgt_parts[common..] {
        rel.push(part);
    }

    if rel.is_empty() {
        ".".to_owned()
    } else {
        rel.join("/")
    }
}

/// Resolves a stored project path (relative or absolute) against `script_dir`.
/// Preserves Aegisub `?` markers and absolute Windows/UNC paths unchanged.
#[must_use]
pub fn resolve_project_path(script_dir: &str, stored: &str) -> String {
    if is_aegisub_marker(stored) || split_win_path(stored).is_some() {
        return stored.to_owned();
    }
    let Some((root, mut parts)) = split_win_path(script_dir) else {
        return stored.to_owned();
    };
    for seg in stored.split(['/', '\\']) {
        match seg {
            "" | "." => {}
            ".." => {
                let _ = parts.pop();
            }
            other => parts.push(other),
        }
    }
    let sep = if root.starts_with(r"\\") || root.ends_with(':') {
        "\\"
    } else {
        "/"
    };
    if parts.is_empty() {
        format!("{root}{sep}")
    } else {
        format!("{root}{sep}{}", parts.join(sep))
    }
}

/// Splits an absolute Windows drive (`C:\...`), UNC (`\\server\share\...`), or POSIX (`/...`)
/// path into `(root, normalized_components)`. Returns `None` for relative paths.
fn split_win_path(path: &str) -> Option<(String, Vec<&str>)> {
    let trimmed = path.trim();
    // UNC path: \\server\share\... or //server/share/...
    if let Some(rest) = trimmed
        .strip_prefix(r"\\")
        .or_else(|| trimmed.strip_prefix("//"))
    {
        let mut iter = rest.split(['\\', '/']).filter(|s| !s.is_empty());
        let server = iter.next()?;
        let share = iter.next()?;
        let root = format!(r"\\{server}\{share}");
        let parts = normalize_parts(iter);
        return Some((root, parts));
    }
    // Windows drive letter: C:\... or C:/...
    let bytes = trimmed.as_bytes();
    if bytes.len() >= 2 && bytes[0].is_ascii_alphabetic() && bytes[1] == b':' {
        let root = trimmed[..2].to_ascii_uppercase();
        let rest = &trimmed[2..];
        let parts = normalize_parts(rest.split(['\\', '/']).filter(|s| !s.is_empty()));
        return Some((root, parts));
    }
    // POSIX absolute path: /...
    if let Some(rest) = trimmed.strip_prefix('/') {
        let parts = normalize_parts(rest.split('/').filter(|s| !s.is_empty()));
        return Some((String::new(), parts));
    }
    None
}

fn normalize_parts<'a>(iter: impl Iterator<Item = &'a str>) -> Vec<&'a str> {
    let mut out = Vec::new();
    for seg in iter {
        match seg {
            "" | "." => {}
            ".." => {
                let _ = out.pop();
            }
            other => out.push(other),
        }
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;
    use ass_core::{parse_ass, serialize_ass};

    #[test]
    fn round_trips_aegisub_project_garbage_byte_identically_and_preserves_unknown_keys() {
        let raw = "\u{FEFF}[Script Info]\r\nTitle: Test\r\nScriptType: v4.00+\r\n\r\n[Aegisub Project Garbage]\r\nAudio File: ?video\r\nVideo File: ../raws/ep01.mkv\r\nVideo AR Mode: 4\r\nVideo AR Value: 1.777778\r\nVideo Zoom Percent: 0.500000\r\nScroll Position: 42\r\nActive Line: 50\r\nVideo Position: 1200\r\nCustom-Unknown-Key: keep-me\r\n\r\n[Events]\r\nFormat: Layer, Start, End, Style, Name, MarginL, MarginR, MarginV, Effect, Text\r\nDialogue: 0,0:00:01.00,0:00:03.00,Default,,0,0,0,,Hello\r\n";

        let mut script = parse_ass(raw).value;
        let links = read_project_links(&script);
        assert_eq!(links.audio_file.as_deref(), Some("?video"));
        assert_eq!(links.video_file.as_deref(), Some("../raws/ep01.mkv"));
        assert_eq!(links.video_ar_mode, Some(4));
        assert_eq!(links.active_line, Some(50));
        assert_eq!(links.video_position, Some(1200));

        // Writing unchanged links produces byte-identical output!
        write_project_links(&mut script, &links);
        assert_eq!(serialize_ass(&script), raw);

        // Updating one field preserves unknown keys.
        let mut edited = links;
        edited.active_line = Some(99);
        write_project_links(&mut script, &edited);
        assert_eq!(
            script
                .aegisub_garbage()
                .and_then(|s| s.get("Custom-Unknown-Key")),
            Some("keep-me")
        );
        assert_eq!(
            script.aegisub_garbage().and_then(|s| s.get("Active Line")),
            Some("99")
        );
    }

    #[test]
    fn handles_windows_drives_unc_paths_and_aegisub_markers() {
        assert_eq!(
            make_relative_to_script(r"D:\Fansub\Ep01\subs", r"d:/fansub/ep01/video.mkv"),
            "../video.mkv"
        );
        assert_eq!(
            make_relative_to_script(r"C:\Subs", r"D:\Videos\ep01.mkv"),
            r"D:\Videos\ep01.mkv"
        );
        assert_eq!(
            make_relative_to_script(r"\\NAS\Anime\Ep01\Subs", r"\\nas\anime\Ep01\raw.mkv"),
            "../raw.mkv"
        );
        assert_eq!(make_relative_to_script(r"D:\Subs", "?video"), "?video");
        assert_eq!(
            resolve_project_path(r"D:\Fansub\Ep01\subs", "../video.mkv"),
            r"D:\Fansub\Ep01\video.mkv"
        );
        assert_eq!(resolve_project_path(r"D:\Subs", "?audio"), "?audio");
    }

    #[test]
    fn unparseable_project_values_are_reported() {
        let raw = "[Script Info]
ScriptType: v4.00+

[Aegisub Project Garbage]
Video Position: abc
Active Line: 7
Video Zoom Percent: 1,5
";
        let script = parse_ass(raw).value;
        let (links, warnings) = read_project_links_with_warnings(&script);
        assert_eq!(links.active_line, Some(7));
        assert_eq!(links.video_position, None);
        assert_eq!(warnings.len(), 2, "{warnings:?}");
        assert!(warnings.iter().any(|w| w.contains("Video Position")));
    }
}
