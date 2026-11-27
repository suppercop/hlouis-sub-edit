//! Select Lines tool matching Aegisub 3.5.1 `Subtitle -> Select Lines` dialog semantics.
//!
//! Reference behaviour (Aegisub 3.5.1 manual — Select Lines):
//! - Match mode: `Contains`, `Exact`, or `Regex` (case-sensitive or insensitive).
//! - Field: `Text`, `Style`, `Actor` (`Name`), or `Effect`.
//! - Line kind filter: `match_dialogue` and/or `match_comments`.
//! - Condition: `invert = false` ("Match") or `invert = true` ("Doesn't match").
//! - Action on current selection: `Set`, `Add`, `Subtract`, or `Intersect`.

use crate::ToolsError;
use ass_core::{EventKind, ScriptFile};
use regex::RegexBuilder;
use std::collections::BTreeSet;

/// How the pattern is matched against the target field.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum MatchMode {
    Contains,
    Exact,
    Regex,
}

/// Event field evaluated by Select Lines.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum SelectField {
    Text,
    Style,
    Actor,
    Effect,
}

/// How matching rows combine with the existing selection.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum SelectionAction {
    Set,
    Add,
    Subtract,
    Intersect,
}

/// Configuration for [`select_lines`].
#[allow(clippy::struct_excessive_bools)]
#[derive(Debug, Clone, Copy)]
pub struct SelectLinesOptions<'a> {
    pub pattern: &'a str,
    pub mode: MatchMode,
    pub field: SelectField,
    pub case_sensitive: bool,
    pub match_dialogue: bool,
    pub match_comments: bool,
    pub invert: bool,
    pub action: SelectionAction,
}

/// Evaluates Select Lines against `script` and returns the resulting selection set.
pub fn select_lines(
    script: &ScriptFile,
    current_selection: &BTreeSet<usize>,
    opts: SelectLinesOptions<'_>,
) -> Result<BTreeSet<usize>, ToolsError> {
    let Some(events_sec) = script.events_section() else {
        return Ok(match opts.action {
            SelectionAction::Set | SelectionAction::Intersect => BTreeSet::new(),
            SelectionAction::Add | SelectionAction::Subtract => current_selection.clone(),
        });
    };

    let regex = if opts.mode == MatchMode::Regex {
        Some(
            RegexBuilder::new(opts.pattern)
                .case_insensitive(!opts.case_sensitive)
                .build()?,
        )
    } else {
        None
    };

    let needle_lower = if !opts.case_sensitive && opts.mode != MatchMode::Regex {
        Some(opts.pattern.to_lowercase())
    } else {
        None
    };

    let mut matched = BTreeSet::new();

    for (idx, ev) in events_sec.events().enumerate() {
        let kind_ok = match ev.kind() {
            EventKind::Dialogue => opts.match_dialogue,
            EventKind::Comment => opts.match_comments,
        };
        if !kind_ok {
            continue;
        }

        let value = match opts.field {
            SelectField::Text => ev.text(),
            SelectField::Style => ev.style(),
            SelectField::Actor => ev.name(),
            SelectField::Effect => ev.effect(),
        };

        let raw_match = match opts.mode {
            MatchMode::Contains => {
                if let Some(ref nl) = needle_lower {
                    if value.is_ascii() && nl.is_ascii() {
                        contains_ascii_ignore_case(value.as_bytes(), nl.as_bytes())
                    } else {
                        value.to_lowercase().contains(nl.as_str())
                    }
                } else {
                    value.contains(opts.pattern)
                }
            }
            MatchMode::Exact => {
                if let Some(ref nl) = needle_lower {
                    if value.is_ascii() && nl.is_ascii() {
                        value.eq_ignore_ascii_case(nl)
                    } else {
                        value.to_lowercase() == *nl
                    }
                } else {
                    value == opts.pattern
                }
            }
            MatchMode::Regex => regex.as_ref().is_some_and(|re| re.is_match(value)),
        };

        if raw_match ^ opts.invert {
            matched.insert(idx);
        }
    }

    let result = match opts.action {
        SelectionAction::Set => matched,
        SelectionAction::Add => current_selection.union(&matched).copied().collect(),
        SelectionAction::Subtract => current_selection.difference(&matched).copied().collect(),
        SelectionAction::Intersect => current_selection.intersection(&matched).copied().collect(),
    };
    Ok(result)
}

#[inline]
fn contains_ascii_ignore_case(haystack: &[u8], needle_lower: &[u8]) -> bool {
    if needle_lower.is_empty() {
        return true;
    }
    if needle_lower.len() > haystack.len() {
        return false;
    }
    let first = needle_lower[0];
    let nlen = needle_lower.len();
    let end = haystack.len() - nlen;
    for i in 0..=end {
        if haystack[i].to_ascii_lowercase() == first
            && haystack[i..i + nlen].eq_ignore_ascii_case(needle_lower)
        {
            return true;
        }
    }
    false
}

#[cfg(test)]
mod tests {
    use super::*;
    use ass_core::{AssEvent, AssTime, EventEntry};

    #[test]
    fn supports_dialogue_comment_filtering_and_all_selection_actions() {
        let mut script = ScriptFile::new_default();
        let mut comment = AssEvent::dialogue(AssTime::ZERO, AssTime::from_cs(100), "Note: sign");
        comment.set_kind(EventKind::Comment);
        let mut sign = AssEvent::dialogue(AssTime::from_cs(100), AssTime::from_cs(200), "Sign");
        sign.set_style("Typeset");
        let dial = AssEvent::dialogue(AssTime::from_cs(200), AssTime::from_cs(300), "Hello");
        script.ensure_events_section_mut().entries = vec![
            EventEntry::Event(comment),
            EventEntry::Event(sign),
            EventEntry::Event(dial),
        ];

        // Match only comments containing "note" (case-insensitive).
        let sel = select_lines(
            &script,
            &BTreeSet::new(),
            SelectLinesOptions {
                pattern: "note",
                mode: MatchMode::Contains,
                field: SelectField::Text,
                case_sensitive: false,
                match_dialogue: false,
                match_comments: true,
                invert: false,
                action: SelectionAction::Set,
            },
        )
        .expect("select");
        assert_eq!(sel, BTreeSet::from([0]));

        // Add lines with exact style "Typeset".
        let sel2 = select_lines(
            &script,
            &sel,
            SelectLinesOptions {
                pattern: "Typeset",
                mode: MatchMode::Exact,
                field: SelectField::Style,
                case_sensitive: true,
                match_dialogue: true,
                match_comments: false,
                invert: false,
                action: SelectionAction::Add,
            },
        )
        .expect("add");
        assert_eq!(sel2, BTreeSet::from([0, 1]));

        // Intersect with regex "^Sign$".
        let sel3 = select_lines(
            &script,
            &sel2,
            SelectLinesOptions {
                pattern: "^Sign$",
                mode: MatchMode::Regex,
                field: SelectField::Text,
                case_sensitive: true,
                match_dialogue: true,
                match_comments: true,
                invert: false,
                action: SelectionAction::Intersect,
            },
        )
        .expect("intersect");
        assert_eq!(sel3, BTreeSet::from([1]));

        // Subtract index 1 -> empty.
        let sel4 = select_lines(
            &script,
            &sel3,
            SelectLinesOptions {
                pattern: "Sign",
                mode: MatchMode::Contains,
                field: SelectField::Text,
                case_sensitive: true,
                match_dialogue: true,
                match_comments: true,
                invert: false,
                action: SelectionAction::Subtract,
            },
        )
        .expect("subtract");
        assert!(sel4.is_empty());
    }
}
