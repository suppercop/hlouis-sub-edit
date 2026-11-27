//! Criterion benchmark running every `ass-tools` operation over a 20,000-line script (target < 50 ms each).

use ass_core::{AssEvent, AssTime, EventEntry, ScriptFile};
use ass_tools::{
    find_and_replace, join_lines, select_lines, shift_times, split_line, timing_post_process,
    AdjacentLinkConfig, FindReplaceOptions, JoinMode, KeyframeSnapConfig, MatchMode, SearchField,
    SearchScope, SelectField, SelectLinesOptions, SelectionAction, ShiftAffect, ShiftAmount,
    ShiftDirection, ShiftOptions, ShiftTarget, SplitMode, TppOptions,
};
use criterion::{black_box, criterion_group, criterion_main, Criterion};
use std::collections::BTreeSet;

fn generate_20k_script() -> ScriptFile {
    let mut script = ScriptFile::new_default();
    let entries: Vec<EventEntry> = (0..20_000u32)
        .map(|i| {
            let start = AssTime::from_cs(i * 150);
            let end = start.saturating_add_cs(135);
            let text = if i % 4 == 0 {
                format!(r"{{\an8\k30}}Hello\NWorld line #{i}")
            } else {
                format!("Dialogue line #{i} with sample text")
            };
            EventEntry::Event(AssEvent::dialogue(start, end, text))
        })
        .collect();
    script.ensure_events_section_mut().entries = entries;
    script
}

fn bench_ass_tools(c: &mut Criterion) {
    let script = generate_20k_script();
    let keyframes: Vec<u32> = (0..1_000u32).map(|k| k * 3_000).collect();
    let empty_sel = BTreeSet::new();

    c.bench_function("shift_times_20k", |b| {
        b.iter(|| {
            let cmd = shift_times(
                black_box(&script),
                ShiftOptions {
                    amount: ShiftAmount::TimeCs(25),
                    direction: ShiftDirection::Forward,
                    target: ShiftTarget::Both,
                    affect: ShiftAffect::All,
                },
            );
            black_box(cmd).expect("shift")
        });
    });

    c.bench_function("find_replace_skip_tags_20k", |b| {
        b.iter(|| {
            let res = find_and_replace(
                black_box(&script),
                FindReplaceOptions {
                    query: "sample",
                    replacement: "edited",
                    field: SearchField::Text,
                    case_sensitive: true,
                    use_regex: false,
                    skip_tags: true,
                    scope: SearchScope::All,
                },
            );
            black_box(res).expect("replace")
        });
    });

    c.bench_function("select_lines_20k", |b| {
        b.iter(|| {
            let res = select_lines(
                black_box(&script),
                &empty_sel,
                SelectLinesOptions {
                    pattern: "World",
                    mode: MatchMode::Contains,
                    field: SelectField::Text,
                    case_sensitive: false,
                    match_dialogue: true,
                    match_comments: true,
                    invert: false,
                    action: SelectionAction::Set,
                },
            );
            black_box(res).expect("select")
        });
    });

    let join_sel: BTreeSet<usize> = (100..200).collect();
    c.bench_function("join_100_lines_in_20k", |b| {
        b.iter(|| {
            let res = join_lines(
                black_box(&script),
                &join_sel,
                &JoinMode::Concatenate {
                    separator: r"\N".to_owned(),
                },
            );
            black_box(res).expect("join")
        });
    });

    c.bench_function("split_by_newline_in_20k", |b| {
        b.iter(|| {
            let res = split_line(black_box(&script), 19_996, SplitMode::ByHardNewline);
            black_box(res).expect("split")
        });
    });

    c.bench_function("tpp_full_pipeline_20k", |b| {
        b.iter(|| {
            let cmd = timing_post_process(
                black_box(&script),
                TppOptions {
                    lead_in_ms: Some(50),
                    lead_out_ms: Some(100),
                    adjacent: Some(AdjacentLinkConfig {
                        threshold_ms: 250,
                        bias: 0.5,
                        prevent_overlap: true,
                    }),
                    keyframes: Some(KeyframeSnapConfig {
                        keyframes_cs: &keyframes,
                        start_before_ms: 100,
                        start_after_ms: 100,
                        end_before_ms: 100,
                        end_after_ms: 100,
                    }),
                    skip_comments: true,
                    styles_filter: None,
                },
            );
            black_box(cmd)
        });
    });
}

criterion_group!(benches, bench_ass_tools);
criterion_main!(benches);
