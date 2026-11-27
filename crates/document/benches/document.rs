use ass_core::{AssEvent, AssTime, EditCommand, ScriptFile};
use criterion::{Criterion, black_box, criterion_group, criterion_main};
use document::{ChangeNotification, CommitType, DocumentController, DocumentListener};

struct SinkListener(u64);
impl DocumentListener for SinkListener {
    fn on_document_changed(&mut self, event: ChangeNotification) {
        self.0 = self.0.wrapping_add(event.commit_id);
    }
}

fn bench_document(c: &mut Criterion) {
    let mut script = ScriptFile::new_default();
    script.ensure_events_section_mut().insert_event(
        0,
        AssEvent::dialogue(AssTime::ZERO, AssTime::from_cs(200), "Initial text"),
    );

    c.bench_function("document_commit_text_with_notify", |b| {
        let mut doc = DocumentController::new(script.clone());
        doc.add_listener(Box::new(SinkListener(0)));
        b.iter(|| {
            let cmd = EditCommand::ModifyEventText {
                index: 0,
                before: "Initial text".to_owned(),
                after: "Edited subtitle line".to_owned(),
            };
            let id = doc
                .commit("edit text", cmd, CommitType::TEXT)
                .expect("commit");
            let _ = black_box(id);
            let _ = doc.undo();
        });
    });

    c.bench_function("document_undo_redo_single_line", |b| {
        let mut doc = DocumentController::new(script.clone());
        doc.add_listener(Box::new(SinkListener(0)));
        doc.commit(
            "edit",
            EditCommand::ModifyEventText {
                index: 0,
                before: "Initial text".to_owned(),
                after: "Edited subtitle line".to_owned(),
            },
            CommitType::TEXT,
        )
        .expect("commit");
        b.iter(|| {
            let _ = black_box(doc.undo().expect("undo"));
            let _ = black_box(doc.redo().expect("redo"));
        });
    });

    c.bench_function("document_amend_while_typing_zero_alloc", |b| {
        let mut doc = DocumentController::new(script.clone());
        doc.add_listener(Box::new(SinkListener(0)));
        doc.amend_text(0, "typing", "a").expect("init amend");
        b.iter(|| {
            let id = doc
                .amend_text(0, "typing", black_box("Hello world from typing"))
                .expect("amend");
            black_box(id);
        });
    });
}

criterion_group!(benches, bench_document);
criterion_main!(benches);
