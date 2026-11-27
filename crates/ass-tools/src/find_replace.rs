//! Find & Replace tool matching Aegisub 3.5.1 `Edit -> Find and Replace` semantics.
//!
//! Reference behaviour (Aegisub 3.5.1 manual — Find and Replace):
//! - Searches `Text`, `Style`, `Actor` (`Name`), or `Effect`.
//! - Supports plain text or regular expressions, case-sensitive or case-insensitive.
//! - `skip_tags`: when searching `Text`, ignores `{...}` override blocks, `\N`/`\n`/`\h`,
//!   and `\p>0` drawing commands, replacing only inside plain dialogue spans.
//! - Returns a single `EditCommand::Batch` so Replace All is undoable in one step.

use crate::ToolsError;
use ass_core::{scan_override_block, EditCommand, RawOverrideItem, ScriptFile};
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
            match replace_skipping_tags(
                current_val,
                opts.query,
                opts.replacement,
                regex.as_ref(),
                opts.use_regex,
            ) {
                Some(res) => res,
                None => continue,
            }
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
) -> Option<(String, usize)> {
    // Fast pre-filter: if no regex is used and `query` does not occur anywhere in `text`,
    // it cannot match any plain dialogue span either.
    if regex.is_none() && !text.contains(query) {
        return None;
    }

    let bytes = text.as_bytes();
    let len = bytes.len();
    let mut out = String::new();
    let mut total = 0usize;
    let mut drawing_level = 0u32;
    let mut copied_until = 0usize;
    let mut i = 0usize;

    while i < len {
        if bytes[i] == b'{' {
            let (next, _) = scan_override_block(text, i + 1, |item| {
                if let RawOverrideItem::Tag {
                    name: "p",
                    raw_args,
                } = item
                {
                    if let Ok(lvl) = raw_args.trim().parse::<u32>() {
                        drawing_level = lvl;
                    }
                }
            });
            i = next;
        } else {
            let next_brace = bytes[i..]
                .iter()
                .position(|&b| b == b'{')
                .map_or(len, |rel| i + rel);
            if drawing_level == 0 {
                // Split plain text around `\N`, `\n`, `\h` escapes so they are skipped like tags.
                let mut seg_start = i;
                let mut j = i;
                while j < next_brace {
                    if bytes[j] == b'\\'
                        && j + 1 < next_brace
                        && matches!(bytes[j + 1], b'N' | b'n' | b'h')
                    {
                        if j > seg_start {
                            let plain = &text[seg_start..j];
                            apply_plain_replace(
                                text,
                                seg_start,
                                plain,
                                query,
                                replacement,
                                regex,
                                use_regex,
                                &mut out,
                                &mut copied_until,
                                &mut total,
                            );
                        }
                        j += 2;
                        seg_start = j;
                    } else {
                        j += 1;
                    }
                }
                if next_brace > seg_start {
                    let plain = &text[seg_start..next_brace];
                    apply_plain_replace(
                        text,
                        seg_start,
                        plain,
                        query,
                        replacement,
                        regex,
                        use_regex,
                        &mut out,
                        &mut copied_until,
                        &mut total,
                    );
                }
            }
            i = next_brace;
        }
    }

    if total == 0 {
        None
    } else {
        out.push_str(&text[copied_until..]);
        Some((out, total))
    }
}

#[allow(clippy::too_many_arguments)]
fn apply_plain_replace(
    full_text: &str,
    plain_start: usize,
    plain: &str,
    query: &str,
    replacement: &str,
    regex: Option<&regex::Regex>,
    use_regex: bool,
    out: &mut String,
    copied_until: &mut usize,
    total: &mut usize,
) {
    if let Some(re) = regex {
        let mut iter = re.captures_iter(plain).peekable();
        if iter.peek().is_none() {
            return;
        }
        if out.capacity() == 0 {
            out.reserve(full_text.len() + 16);
        }
        out.push_str(&full_text[*copied_until..plain_start]);
        let mut last = 0usize;
        for caps in iter {
            let Some(m) = caps.get(0) else { continue };
            out.push_str(&plain[last..m.start()]);
            if use_regex {
                caps.expand(replacement, out);
            } else {
                out.push_str(replacement);
            }
            last = m.end();
            *total += 1;
        }
        out.push_str(&plain[last..]);
        *copied_until = plain_start + plain.len();
    } else {
        let mut iter = plain.match_indices(query).peekable();
        if iter.peek().is_none() {
            return;
        }
        if out.capacity() == 0 {
            out.reserve(full_text.len() + 16);
        }
        out.push_str(&full_text[*copied_until..plain_start]);
        let mut last = 0usize;
        for (start, matched) in iter {
            out.push_str(&plain[last..start]);
            out.push_str(replacement);
            last = start + matched.len();
            *total += 1;
        }
        out.push_str(&plain[last..]);
        *copied_until = plain_start + plain.len();
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
