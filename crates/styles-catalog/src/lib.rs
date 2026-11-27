//! Aegisub 3.5.1 `.sty` style catalog storage, style import with conflict resolution,
//! atomic style rename + `\r` propagation, and unused/missing style diagnostics.
//!
//! Mirrors Aegisub `src/ass_style_storage.cpp` and `src/dialog_style_manager.cpp`.

#![forbid(unsafe_code)]
#![cfg_attr(not(test), deny(clippy::unwrap_used, clippy::expect_used))]

use ass_core::{AssStyle, EditCommand, ScriptFile};
use std::collections::{HashMap, HashSet};
use std::fs;
use std::path::Path;
use thiserror::Error;

/// Errors returned by `styles-catalog`.
#[derive(Debug, Error)]
pub enum StylesCatalogError {
    /// Filesystem I/O error reading or writing `.sty` catalog files.
    #[error("catalog I/O error: {0}")]
    Io(#[from] std::io::Error),
    /// Script is missing a `[V4+ Styles]` section.
    #[error("script has no [V4+ Styles] section")]
    MissingStylesSection,
    /// Requested style name does not exist in the script.
    #[error("style not found: {0}")]
    StyleNotFound(String),
    /// Target new style name is empty or already exists in the script.
    #[error("invalid or duplicate target style name: {0}")]
    InvalidTargetName(String),
}

/// Strategy when importing or copying a style whose name already exists in the destination.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ConflictPolicy {
    /// Replace the existing style in place.
    Overwrite,
    /// Keep the existing style and rename the incoming style with a numeric suffix
    /// (`Name (1)`, `Name (2)`, ...).
    RenameWithSuffix,
    /// Skip the incoming style and keep the existing one unchanged.
    Skip,
}

/// An in-memory Aegisub style catalog (corresponds to one `<name>.sty` file).
#[derive(Debug, Clone)]
pub struct StyleCatalog {
    /// Logical catalog name (filename stem without `.sty`).
    pub name: String,
    /// Ordered list of styles in this catalog.
    pub styles: Vec<AssStyle>,
}

impl StyleCatalog {
    /// Creates an empty catalog with the given name.
    #[must_use]
    pub fn new(name: impl Into<String>) -> Self {
        Self {
            name: name.into(),
            styles: Vec::new(),
        }
    }

    /// Parses Aegisub `.sty` file text into a [`StyleCatalog`].
    ///
    /// Accepts both full `[V4+ Styles]` sections and bare `Style:` lines (matching Aegisub
    /// `AssStyleStorage::Load`).
    #[must_use]
    pub fn parse_sty(name: impl Into<String>, content: &str) -> Self {
        let normalized = if content.contains("[V4+ Styles]") {
            content.to_owned()
        } else {
            let mut buf = String::with_capacity(content.len() + 256);
            buf.push_str(
                "[V4+ Styles]\nFormat: Name, Fontname, Fontsize, PrimaryColour, SecondaryColour, OutlineColour, BackColour, Bold, Italic, Underline, StrikeOut, ScaleX, ScaleY, Spacing, Angle, BorderStyle, Outline, Shadow, Alignment, MarginL, MarginR, MarginV, Encoding\n",
            );
            buf.push_str(content);
            buf
        };
        let parsed = ass_core::parse_ass(&normalized).value;
        let styles = parsed.styles().cloned().collect();
        Self {
            name: name.into(),
            styles,
        }
    }

    /// Serializes this catalog to Aegisub `.sty` text format.
    #[must_use]
    pub fn serialize_sty(&self) -> String {
        let mut script = ScriptFile::new_default();
        let sec = script.ensure_styles_section_mut();
        // Clear the default style before inserting catalog styles.
        while sec.style_count() > 0 {
            sec.remove_style(0);
        }
        for (i, st) in self.styles.iter().enumerate() {
            sec.insert_style(i, st.clone());
        }
        // Emit only the `[V4+ Styles]` section.
        let full = ass_core::serialize_ass(&script);
        if let Some(idx) = full.find("[V4+ Styles]") {
            if let Some(end_rel) = full[idx..].find("[Events]") {
                return full[idx..idx + end_rel].trim_end().to_owned() + "\n";
            }
            return full[idx..].to_owned();
        }
        full
    }

    /// Copies styles from a [`ScriptFile`] into this catalog under the given [`ConflictPolicy`].
    /// Returns the number of styles added or overwritten.
    pub fn copy_from_script(
        &mut self,
        script: &ScriptFile,
        style_names: Option<&[&str]>,
        policy: ConflictPolicy,
    ) -> usize {
        let filter: Option<HashSet<&str>> =
            style_names.map(|names| names.iter().copied().collect());
        let mut existing: HashMap<String, usize> = self
            .styles
            .iter()
            .enumerate()
            .map(|(i, s)| (s.name().to_owned(), i))
            .collect();
        let mut changed = 0usize;

        for st in script.styles() {
            if let Some(ref f) = filter {
                if !f.contains(st.name()) {
                    continue;
                }
            }
            if let Some(&idx) = existing.get(st.name()) {
                match policy {
                    ConflictPolicy::Overwrite => {
                        self.styles[idx] = st.clone();
                        changed += 1;
                    }
                    ConflictPolicy::RenameWithSuffix => {
                        let new_name =
                            unique_suffix_name(st.name(), |cand| existing.contains_key(cand));
                        let mut cloned = st.clone();
                        cloned.set_name(new_name.clone());
                        let new_idx = self.styles.len();
                        self.styles.push(cloned);
                        existing.insert(new_name, new_idx);
                        changed += 1;
                    }
                    ConflictPolicy::Skip => {}
                }
            } else {
                let new_idx = self.styles.len();
                existing.insert(st.name().to_owned(), new_idx);
                self.styles.push(st.clone());
                changed += 1;
            }
        }
        changed
    }
}

/// Lists all `.sty` catalog names (sorted alphabetically, without extension) in `catalog_dir`.
///
/// # Errors
/// Returns [`StylesCatalogError::Io`] if the directory cannot be read.
pub fn list_catalogs(catalog_dir: &Path) -> Result<Vec<String>, StylesCatalogError> {
    if !catalog_dir.exists() {
        return Ok(Vec::new());
    }
    let mut names = Vec::new();
    for entry in fs::read_dir(catalog_dir)? {
        let entry = entry?;
        let path = entry.path();
        if path.extension().and_then(|e| e.to_str()) == Some("sty") {
            if let Some(stem) = path.file_stem().and_then(|s| s.to_str()) {
                names.push(stem.to_owned());
            }
        }
    }
    names.sort();
    Ok(names)
}

/// Loads `<name>.sty` from `catalog_dir`.
///
/// # Errors
/// Returns [`StylesCatalogError::Io`] if the file cannot be read.
pub fn load_catalog(catalog_dir: &Path, name: &str) -> Result<StyleCatalog, StylesCatalogError> {
    let path = catalog_dir.join(format!("{name}.sty"));
    let text = fs::read_to_string(path)?;
    Ok(StyleCatalog::parse_sty(name, &text))
}

/// Writes `catalog` to `<catalog.name>.sty` inside `catalog_dir` (creating `catalog_dir` if
/// needed).
///
/// # Errors
/// Returns [`StylesCatalogError::Io`] if directory creation or writing fails.
pub fn save_catalog(catalog_dir: &Path, catalog: &StyleCatalog) -> Result<(), StylesCatalogError> {
    fs::create_dir_all(catalog_dir)?;
    let path = catalog_dir.join(format!("{}.sty", catalog.name));
    fs::write(path, catalog.serialize_sty())?;
    Ok(())
}

/// Imports styles from `incoming` (e.g. from a [`StyleCatalog`] or another `.ass` script) into
/// `target` as a single atomic [`EditCommand`], resolving name collisions according to `policy`.
///
/// # Errors
/// Returns [`StylesCatalogError::MissingStylesSection`] if `target` has no `[V4+ Styles]`.
pub fn import_styles(
    target: &ScriptFile,
    incoming: &[AssStyle],
    policy: ConflictPolicy,
) -> Result<EditCommand, StylesCatalogError> {
    let sec = target
        .styles_section()
        .ok_or(StylesCatalogError::MissingStylesSection)?;
    let existing_styles: Vec<&AssStyle> = sec.styles().collect();
    let mut index_by_name: HashMap<String, usize> = HashMap::with_capacity(existing_styles.len());
    for (i, st) in existing_styles.iter().enumerate() {
        index_by_name.insert(st.name().to_owned(), i);
    }

    let mut next_insert_idx = existing_styles.len();
    let mut cmds = Vec::with_capacity(incoming.len());

    for st in incoming {
        if let Some(&existing_idx) = index_by_name.get(st.name()) {
            match policy {
                ConflictPolicy::Overwrite => {
                    let before = existing_styles
                        .get(existing_idx)
                        .map_or_else(|| st.clone(), |s| (*s).clone());
                    cmds.push(EditCommand::ModifyStyle {
                        index: existing_idx,
                        before: Box::new(before),
                        after: Box::new(st.clone()),
                    });
                }
                ConflictPolicy::RenameWithSuffix => {
                    let new_name =
                        unique_suffix_name(st.name(), |cand| index_by_name.contains_key(cand));
                    let mut renamed = st.clone();
                    renamed.set_name(new_name.clone());
                    index_by_name.insert(new_name, next_insert_idx);
                    cmds.push(EditCommand::InsertStyle {
                        index: next_insert_idx,
                        style: Box::new(renamed),
                    });
                    next_insert_idx += 1;
                }
                ConflictPolicy::Skip => {}
            }
        } else {
            index_by_name.insert(st.name().to_owned(), next_insert_idx);
            cmds.push(EditCommand::InsertStyle {
                index: next_insert_idx,
                style: Box::new(st.clone()),
            });
            next_insert_idx += 1;
        }
    }

    Ok(EditCommand::Batch(cmds))
}

/// Imports styles from an external `.ass` file on disk into `target` as one atomic [`EditCommand`].
///
/// # Errors
/// Returns [`StylesCatalogError`] on I/O failure or if `target` has no `[V4+ Styles]`.
pub fn import_styles_from_ass_file(
    target: &ScriptFile,
    ass_path: &Path,
    policy: ConflictPolicy,
) -> Result<EditCommand, StylesCatalogError> {
    let raw = fs::read_to_string(ass_path)?;
    let source = ass_core::parse_ass(&raw).value;
    let styles: Vec<AssStyle> = source.styles().cloned().collect();
    import_styles(target, &styles, policy)
}

/// Renames `old_name` to `new_name` in `[V4+ Styles]` and, when `propagate_to_events` is `true`,
/// updates every event's `Style` field AND every `\rOldName` override tag in `[Events]`, all as
/// ONE atomic [`EditCommand::Batch`].
///
/// Mirrors Aegisub `src/dialog_style_editor.cpp` ("Update all lines using this style?").
///
/// # Errors
/// Returns [`StylesCatalogError`] if `[V4+ Styles]` is missing, `old_name` is not found, or
/// `new_name` is empty / already exists.
pub fn rename_style(
    script: &ScriptFile,
    old_name: &str,
    new_name: &str,
    propagate_to_events: bool,
) -> Result<EditCommand, StylesCatalogError> {
    if new_name.is_empty() {
        return Err(StylesCatalogError::InvalidTargetName(new_name.to_owned()));
    }
    let styles_sec = script
        .styles_section()
        .ok_or(StylesCatalogError::MissingStylesSection)?;
    let styles: Vec<&AssStyle> = styles_sec.styles().collect();

    let mut target_idx = None;
    for (i, st) in styles.iter().enumerate() {
        if st.name() == old_name {
            target_idx = Some(i);
        } else if st.name() == new_name {
            return Err(StylesCatalogError::InvalidTargetName(new_name.to_owned()));
        }
    }
    let style_idx =
        target_idx.ok_or_else(|| StylesCatalogError::StyleNotFound(old_name.to_owned()))?;
    if old_name == new_name {
        return Ok(EditCommand::Batch(Vec::new()));
    }

    let mut cmds = Vec::new();
    let before_style = styles[style_idx].clone();
    let mut after_style = before_style.clone();
    after_style.set_name(new_name);
    cmds.push(EditCommand::ModifyStyle {
        index: style_idx,
        before: Box::new(before_style),
        after: Box::new(after_style),
    });

    if propagate_to_events {
        if let Some(ev_sec) = script.events_section() {
            for (idx, ev) in ev_sec.events().enumerate() {
                let style_matches = ev.style() == old_name;
                let replaced_text = replace_r_tag_style(ev.text(), old_name, new_name);
                if style_matches || replaced_text.is_some() {
                    let before = ev.clone();
                    let mut after = before.clone();
                    if style_matches {
                        after.set_style(new_name);
                    }
                    if let Some(new_text) = replaced_text {
                        after.set_text(new_text);
                    }
                    cmds.push(EditCommand::ModifyEvent {
                        index: idx,
                        before: Box::new(before),
                        after: Box::new(after),
                    });
                }
            }
        }
    }

    Ok(EditCommand::Batch(cmds))
}

/// Returns the names of all styles defined in `[V4+ Styles]` that are not referenced by any
/// event's `Style` field or `\rStyleName` override tag.
#[must_use]
pub fn find_unused_styles(script: &ScriptFile) -> Vec<String> {
    let mut used: HashSet<&str> = HashSet::new();
    for ev in script.events() {
        used.insert(ev.style());
        collect_r_tag_styles(ev.text(), &mut used);
    }
    script
        .styles()
        .filter(|st| !used.contains(st.name()))
        .map(|st| st.name().to_owned())
        .collect()
}

/// Returns `(event_index, missing_style_name)` for every event whose `Style` field or `\rStyle`
/// override references a style not present in `[V4+ Styles]`.
#[must_use]
pub fn find_missing_style_events(script: &ScriptFile) -> Vec<(usize, String)> {
    let defined: HashSet<&str> = script.styles().map(AssStyle::name).collect();
    let mut missing = Vec::new();
    let mut r_styles = Vec::new();

    for (idx, ev) in script.events().enumerate() {
        if !defined.contains(ev.style()) {
            missing.push((idx, ev.style().to_owned()));
        }
        r_styles.clear();
        collect_r_tag_styles_vec(ev.text(), &mut r_styles);
        for name in &r_styles {
            if !name.is_empty() && !defined.contains(name) {
                missing.push((idx, (*name).to_owned()));
            }
        }
    }
    missing
}

fn unique_suffix_name<F>(base: &str, mut exists: F) -> String
where
    F: FnMut(&str) -> bool,
{
    let mut n = 1usize;
    loop {
        let cand = format!("{base} ({n})");
        if !exists(&cand) {
            return cand;
        }
        n += 1;
    }
}

/// Zero-allocation fast path: returns `None` when `text` contains no `\r<old_name>` inside `{...}`.
/// Only allocates a new `String` when at least one `\r<old_name>` is actually replaced.
fn replace_r_tag_style(text: &str, old_name: &str, new_name: &str) -> Option<String> {
    let bytes = text.as_bytes();
    // Ultra-fast rejection: if there is no `{`, no override tags exist.
    if !bytes.contains(&b'{') {
        return None;
    }
    let old_bytes = old_name.as_bytes();
    let len = bytes.len();
    let mut pos = 0usize;
    let mut out: Option<String> = None;
    let mut last_copied = 0usize;

    while pos < len {
        let Some(rel_open) = bytes[pos..].iter().position(|&b| b == b'{') else {
            break;
        };
        let block_start = pos + rel_open + 1;
        let Some(rel_close) = bytes[block_start..].iter().position(|&b| b == b'}') else {
            break;
        };
        let block_end = block_start + rel_close;

        // Scan inside `{...}` for `\r<old_name>` bounded by `\`, `}`, or block_end.
        let mut i = block_start;
        while i + 2 <= block_end {
            if bytes[i] == b'\\' && bytes[i + 1] == b'r' {
                let name_start = i + 2;
                let mut name_end = name_start;
                while name_end < block_end && bytes[name_end] != b'\\' {
                    name_end += 1;
                }
                if &bytes[name_start..name_end] == old_bytes {
                    let buf = out.get_or_insert_with(|| String::with_capacity(text.len() + 16));
                    buf.push_str(&text[last_copied..name_start]);
                    buf.push_str(new_name);
                    last_copied = name_end;
                }
                i = name_end;
            } else {
                i += 1;
            }
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

fn collect_r_tag_styles<'a>(text: &'a str, out: &mut HashSet<&'a str>) {
    let bytes = text.as_bytes();
    if !bytes.contains(&b'{') {
        return;
    }
    let mut pos = 0usize;
    while pos < bytes.len() {
        let Some(rel_open) = bytes[pos..].iter().position(|&b| b == b'{') else {
            break;
        };
        let block_start = pos + rel_open + 1;
        let Some(rel_close) = bytes[block_start..].iter().position(|&b| b == b'}') else {
            break;
        };
        let block_end = block_start + rel_close;
        let mut i = block_start;
        while i + 2 <= block_end {
            if bytes[i] == b'\\' && bytes[i + 1] == b'r' {
                let name_start = i + 2;
                let mut name_end = name_start;
                while name_end < block_end && bytes[name_end] != b'\\' {
                    name_end += 1;
                }
                if name_end > name_start {
                    out.insert(&text[name_start..name_end]);
                }
                i = name_end;
            } else {
                i += 1;
            }
        }
        pos = block_end + 1;
    }
}

fn collect_r_tag_styles_vec<'a>(text: &'a str, out: &mut Vec<&'a str>) {
    let bytes = text.as_bytes();
    if !bytes.contains(&b'{') {
        return;
    }
    let mut pos = 0usize;
    while pos < bytes.len() {
        let Some(rel_open) = bytes[pos..].iter().position(|&b| b == b'{') else {
            break;
        };
        let block_start = pos + rel_open + 1;
        let Some(rel_close) = bytes[block_start..].iter().position(|&b| b == b'}') else {
            break;
        };
        let block_end = block_start + rel_close;
        let mut i = block_start;
        while i + 2 <= block_end {
            if bytes[i] == b'\\' && bytes[i + 1] == b'r' {
                let name_start = i + 2;
                let mut name_end = name_start;
                while name_end < block_end && bytes[name_end] != b'\\' {
                    name_end += 1;
                }
                if name_end > name_start {
                    out.push(&text[name_start..name_end]);
                }
                i = name_end;
            } else {
                i += 1;
            }
        }
        pos = block_end + 1;
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use tempfile::tempdir;

    fn sample_script() -> ScriptFile {
        let raw = "\
[Script Info]
ScriptType: v4.00+

[V4+ Styles]
Format: Name, Fontname, Fontsize, PrimaryColour, SecondaryColour, OutlineColour, BackColour, Bold, Italic, Underline, StrikeOut, ScaleX, ScaleY, Spacing, Angle, BorderStyle, Outline, Shadow, Alignment, MarginL, MarginR, MarginV, Encoding
Style: Default,Arial,48,&H00FFFFFF,&H000000FF,&H00000000,&H00000000,0,0,0,0,100,100,0,0,1,2,2,2,10,10,10,1
Style: Sign,Arial,36,&H00FFFFFF,&H000000FF,&H00000000,&H00000000,0,0,0,0,100,100,0,0,1,2,2,8,10,10,10,1
Style: UnusedStyle,Arial,36,&H00FFFFFF,&H000000FF,&H00000000,&H00000000,0,0,0,0,100,100,0,0,1,2,2,2,10,10,10,1

[Events]
Format: Layer, Start, End, Style, Name, MarginL, MarginR, MarginV, Effect, Text
Dialogue: 0,0:00:01.00,0:00:03.00,Default,,0,0,0,,Hello {\\rSign\\b1}world{\\rDefault}
Dialogue: 0,0:00:04.00,0:00:06.00,GhostStyle,,0,0,0,,Missing style here
";
        ass_core::parse_ass(raw).value
    }

    #[test]
    fn catalog_roundtrip_and_import_conflicts() {
        let dir = tempdir().expect("tmp");
        let mut script = sample_script();

        let mut cat = StyleCatalog::new("DefaultCatalog");
        let copied = cat.copy_from_script(&script, None, ConflictPolicy::Overwrite);
        assert_eq!(copied, 3);
        save_catalog(dir.path(), &cat).expect("save");
        assert_eq!(
            list_catalogs(dir.path()).expect("list"),
            vec!["DefaultCatalog"]
        );

        let loaded = load_catalog(dir.path(), "DefaultCatalog").expect("load");
        assert_eq!(loaded.styles.len(), 3);

        // Import with RenameWithSuffix creates "Default (1)", "Sign (1)", "UnusedStyle (1)"
        let cmd = import_styles(&script, &loaded.styles, ConflictPolicy::RenameWithSuffix)
            .expect("import");
        cmd.apply(&mut script).expect("apply");
        assert_eq!(script.styles().count(), 6);
        assert!(script.styles().any(|s| s.name() == "Default (1)"));
    }

    #[test]
    fn rename_propagates_to_style_and_r_tags_and_diagnostics_work() {
        let mut script = sample_script();
        assert_eq!(find_unused_styles(&script), vec!["UnusedStyle"]);
        assert_eq!(
            find_missing_style_events(&script),
            vec![(1, "GhostStyle".to_owned())]
        );

        let cmd = rename_style(&script, "Sign", "SignTop", true).expect("rename");
        cmd.apply(&mut script).expect("apply");

        assert!(script.styles().any(|s| s.name() == "SignTop"));
        assert!(!script.styles().any(|s| s.name() == "Sign"));
        let ev0 = script.events().next().expect("ev0");
        assert_eq!(ev0.text(), r"Hello {\rSignTop\b1}world{\rDefault}");
    }
}
