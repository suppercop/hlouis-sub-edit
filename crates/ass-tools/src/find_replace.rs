//! Find & Replace tool matching Aegisub 3.5.1 `Edit -> Find and Replace` semantics.
//!
//! Reference behaviour (Aegisub 3.5.1 manual — Find and Replace):
//! - Searches `Text`, `Style`, `Actor` (`Name`), or `Effect`.
//! - Supports plain text or regular expressions, case-sensitive or case-insensitive.
//! - `skip_tags`: when searching `Text`, ignores `{...}` override blocks, `\N`/`\n`/`\h`,
//!   and `\p>0` drawing commands, replacing only inside plain dialogue spans.
//! - Returns a single `EditCommand::Batch` so Replace All is undoable in one step.

use crate::ToolsError;
use ass_core::{parse_text_blocks, serialize_text_blocks, EditCommand, ScriptFile, TextBlock};
use regex::RegexBuilder;
use std::collections::BTreeSet;

/// Event field targeted by Find & Replace.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum SearchField {
    Text,
    Style,
    Actor,
    Effect,
}

/// Scope of lines scanned by Find & Replace.
#[derive(Debug, Clone, Copy)]
pub enum SearchScope<'a> {
    All,
    Selected(&'a BTreeSet<usize>),
}

/// Configuration for [`find_and_replace`].
#[derive(Debug, Clone, Copy)]
pub struct FindReplaceOptions<'a> {
    pub query: &'a str,
    pub replacement: &'a str,
    pub field: SearchField,
    pub case_sensitive: bool,
    pub use_regex: bool,
    pub skip_tags: bool,
    pub scope: SearchScope<'a>,
}

/// Runs Find & Replace across `script` and returns `(EditCommand::Batch, replacement_count)`.
pub fn find_and_replace(
    script: &ScriptFile,
    opts: FindReplaceOptions<'_>,
) -> Result<(EditCommand, usize), ToolsError> {
    if opts.query.is_empty() {
        return Ok((EditCommand::Batch(Vec::new()), 0));
    }

    let Some(events_sec) = script.events_section() else {
        return Ok((EditCommand::Batch(Vec::new()), 0));
    };

    let regex = if opts.use_regex || !opts.case_sensitive {
        let pattern = if opts.use_regex {
            opts.query.to_owned()
        } else {
            regex::escape(opts.query)
        };
        Some(
            RegexBuilder::new(&pattern)
                .case_insensitive(!opts.case_sensitive)
                .build()?,
        )
    } else {
        None
    };

    let mut cmds = Vec::new();
    let mut total_replacements = 0usize;

    for (idx, ev) in events_sec.events().enumerate() {
        if let SearchScope::Selected(sel) = opts.scope {
            if !sel.contains(&idx) {
                continue;
            }
        }

        let current_val = match opts.field {
            SearchField::Text => ev.text(),
            SearchField::Style => ev.style(),
            SearchField::Actor => ev.name(),
            SearchField::Effect => ev.effect(),
        };

        let (replaced, count) = if opts.field == SearchField::Text && opts.skip_tags {
            replace_skipping_tags(
                current_val,
                opts.query,
                opts.replacement,
                regex.as_ref(),
                opts.use_regex,
            )
        } else {
            replace_segment(
                current_val,
                opts.query,
                opts.replacement,
                regex.as_ref(),
                opts.use_regex,
            )
        };

        if count == 0 || replaced == current_val {
            continue;
        }
        total_replacements += count;

        if opts.field == SearchField::Text {
            cmds.push(EditCommand::ModifyEventText {
                index: idx,
                before: current_val.to_owned(),
                after: replaced,
            });
        } else {
            let mut after = ev.clone();
            match opts.field {
                SearchField::Style => after.set_style(replaced),
                SearchField::Actor => after.set_name(replaced),
                SearchField::Effect => after.set_effect(replaced),
                SearchField::Text => unreachable!(),
            }
            cmds.push(EditCommand::ModifyEvent {
                index: idx,
                before: Box::new(ev.clone()),
                after: Box::new(after),
            });
        }
    }

    Ok((EditCommand::Batch(cmds), total_replacements))
}

fn replace_skipping_tags(
    text: &str,
    query: &str,
    replacement: &str,
    regex: Option<&regex::Regex>,
    use_regex: bool,
) -> (String, usize) {
    let mut blocks = parse_text_blocks(text);
    let mut total = 0usize;
    for block in &mut blocks {
        if let TextBlock::Plain(ref mut plain) = block {
            let (new_s, count) = replace_segment(plain, query, replacement, regex, use_regex);
            if count > 0 {
                *plain = new_s;
                total += count;
            }
        }
    }
    if total == 0 {
        (text.to_owned(), 0)
    } else {
        (serialize_text_blocks(&blocks), total)
    }
}

fn replace_segment(
    input: &str,
    query: &str,
    replacement: &str,
    regex: Option<&regex::Regex>,
    use_regex: bool,
) -> (String, usize) {
    if let Some(re) = regex {
        let mut count = 0usize;
        let out = re.replace_all(input, |caps: &regex::Captures<'_>| {
            count += 1;
            if use_regex {
                let mut expanded = String::new();
                caps.expand(replacement, &mut expanded);
                expanded
            } else {
                replacement.to_owned()
            }
        });
        (out.into_owned(), count)
    } else {
        let count = input.matches(query).count();
        if count == 0 {
            (input.to_owned(), 0)
        } else {
            (input.replace(query, replacement), count)
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use ass_core::{AssEvent, AssTime, EventEntry, UndoHistory};

    #[test]
    fn skip_tags_preserves_override_blocks_and_is_undoable_in_one_step() {
        let mut script = ScriptFile::new_default();
        script.ensure_events_section_mut().entries = vec![
            EventEntry::Event(AssEvent::dialogue(
                AssTime::ZERO,
                AssTime::from_cs(200),
                r"{\fnArial\c&H00FF00&}Arial text and arial",
            )),
            EventEntry::Event(AssEvent::dialogue(
                AssTime::from_cs(200),
                AssTime::from_cs(400),
                r"{\p1}m 0 0 l 10 10{\p0}Arial",
            )),
        ];

        let (cmd, count) = find_and_replace(
            &script,
            FindReplaceOptions {
                query: "arial",
                replacement: "Segoe",
                field: SearchField::Text,
                case_sensitive: false,
                use_regex: false,
                skip_tags: true,
                scope: SearchScope::All,
            },
        )
        .expect("replace");
        assert_eq!(count, 3);

        let mut history = UndoHistory::default();
        history.execute(&mut script, cmd).expect("exec");
        let evs: Vec<&AssEvent> = script.events().collect();
        assert_eq!(evs[0].text(), r"{\fnArial\c&H00FF00&}Segoe text and Segoe");
        assert_eq!(evs[1].text(), r"{\p1}m 0 0 l 10 10{\p0}Segoe");

        // Single undo reverts both lines.
        history.undo(&mut script).expect("undo");
        assert_eq!(
            script.events().next().unwrap().text(),
            r"{\fnArial\c&H00FF00&}Arial text and arial"
        );
    }

    #[test]
    fn regex_capture_replacement_works_on_selection() {
        let mut script = ScriptFile::new_default();
        script.ensure_events_section_mut().entries = vec![
            EventEntry::Event(AssEvent::dialogue(
                AssTime::ZERO,
                AssTime::from_cs(100),
                "Ep 01",
            )),
            EventEntry::Event(AssEvent::dialogue(
                AssTime::from_cs(100),
                AssTime::from_cs(200),
                "Ep 02",
            )),
        ];
        let sel = BTreeSet::from([1usize]);
        let (cmd, count) = find_and_replace(
            &script,
            FindReplaceOptions {
                query: r"Ep (\d+)",
                replacement: "Episode #$1",
                field: SearchField::Text,
                case_sensitive: true,
                use_regex: true,
                skip_tags: false,
                scope: SearchScope::Selected(&sel),
            },
        )
        .expect("regex replace");
        assert_eq!(count, 1);
        cmd.apply(&mut script).expect("apply");
        let evs: Vec<&AssEvent> = script.events().collect();
        assert_eq!(evs[0].text(), "Ep 01");
        assert_eq!(evs[1].text(), "Episode #02");
    }
}
