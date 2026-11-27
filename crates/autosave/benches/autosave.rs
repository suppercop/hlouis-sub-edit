//! Criterion benchmark: 20,000-line (~2 MB) autosave end-to-end incl. fsync (< 30 ms)
//! and caller-thread snapshot hand-off (< 1 ms).

use ass_core::{serialize_ass_bytes, AssEvent, AssTime, EventEntry, ScriptFile};
use autosave::{write_autosave_now, AutosaveConfig, AutosaveWorker};
use criterion::{black_box, criterion_group, criterion_main, Criterion};

fn generate_20k_bytes() -> Vec<u8> {
    let mut script = ScriptFile::new_default();
    script.ensure_events_section_mut().entries = (0..20_000u32)
        .map(|i| {
            let s = AssTime::from_cs(i * 150);
            let e = s.saturating_add_cs(140);
            EventEntry::Event(AssEvent::dialogue(
                s,
                e,
                format!(r"{{\an8\k30}}Dialogue line #{i} — Tiếng Việt & 日本語"),
            ))
        })
        .collect();
    serialize_ass_bytes(&script)
}

fn bench_autosave(c: &mut Criterion) {
    let bytes = generate_20k_bytes();
    let base = std::env::temp_dir().join(format!("hlouis_autosave_bench_{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&base);
    let cfg = AutosaveConfig {
        max_keep: 3,
        ..AutosaveConfig::from_base_dir(&base)
    };

    c.bench_function("autosave_20k_end_to_end_with_fsync", |b| {
        b.iter(|| {
            let p = write_autosave_now(
                &cfg.autosave_dir,
                "bench20k",
                black_box(&bytes),
                cfg.max_keep,
                None,
            )
            .expect("autosave");
            black_box(p)
        });
    });

    let worker = AutosaveWorker::spawn(cfg).expect("spawn");
    c.bench_function("autosave_20k_snapshot_handoff", |b| {
        b.iter(|| {
            // Measure caller-thread hand-off (closure or bytes) without waiting for disk I/O.
            let len = bytes.len();
            worker
                .enqueue_snapshot("handoff", move || vec![b'a'; len.min(64)])
                .expect("enqueue");
            let _ = worker.drain_results();
        });
    });
    drop(worker);
    let _ = std::fs::remove_dir_all(&base);
}

criterion_group!(benches, bench_autosave);
criterion_main!(benches);
