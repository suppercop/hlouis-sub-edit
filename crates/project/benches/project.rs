//! Criterion benchmark: read + write project links on a 20,000-line script (< 0.5 ms).

use ass_core::{AssEvent, AssTime, EventEntry, ScriptFile};
use criterion::{black_box, criterion_group, criterion_main, Criterion};
use project::{read_project_links, write_project_links, ProjectLinks};

fn bench_project(c: &mut Criterion) {
    let mut script = ScriptFile::new_default();
    script.ensure_events_section_mut().entries = (0..20_000u32)
        .map(|i| {
            let s = AssTime::from_cs(i * 100);
            EventEntry::Event(AssEvent::dialogue(s, s.saturating_add_cs(90), "Hello"))
        })
        .collect();
    let initial = ProjectLinks {
        audio_file: Some("?video".to_owned()),
        video_file: Some("../raws/ep01.mkv".to_owned()),
        active_line: Some(10),
        video_position: Some(500),
        ..ProjectLinks::default()
    };
    write_project_links(&mut script, &initial);

    c.bench_function("project_read_write_links_20k_script", |b| {
        b.iter(|| {
            let mut links = read_project_links(black_box(&script));
            links.video_position = Some(links.video_position.unwrap_or(0).wrapping_add(1));
            write_project_links(&mut script, &links);
            black_box(links.video_position)
        });
    });
}

criterion_group!(benches, bench_project);
criterion_main!(benches);
