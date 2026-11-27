//! Zero-allocation amend-while-typing & scaling (`n` vs `2n`) tests for `document`.

use ass_core::{AssEvent, AssTime, EditCommand, ScriptFile};
use document::{ChangeNotification, CommitType, DocumentController, DocumentListener};
use std::alloc::{GlobalAlloc, Layout, System};
use std::sync::atomic::{AtomicUsize, Ordering};
use std::time::Instant;

struct CountingAlloc;

static ALLOC_COUNT: AtomicUsize = AtomicUsize::new(0);

#[global_allocator]
static GLOBAL: CountingAlloc = CountingAlloc;

#[allow(unsafe_code)]
unsafe impl GlobalAlloc for CountingAlloc {
    unsafe fn alloc(&self, layout: Layout) -> *mut u8 {
        ALLOC_COUNT.fetch_add(1, Ordering::Relaxed);
        unsafe { System.alloc(layout) }
    }

    unsafe fn dealloc(&self, ptr: *mut u8, layout: Layout) {
        unsafe { System.dealloc(ptr, layout) }
    }
}

fn allocs() -> usize {
    ALLOC_COUNT.load(Ordering::Relaxed)
}

struct NoopListener(usize);
impl DocumentListener for NoopListener {
    fn on_document_changed(&mut self, event: ChangeNotification) {
        self.0 = self.0.wrapping_add(usize::from(event.is_amend));
    }
}

fn make_doc(lines: usize) -> DocumentController {
    let mut script = ScriptFile::new_default();
    let sec = script.ensure_events_section_mut();
    for i in 0..lines {
        #[allow(clippy::cast_possible_truncation)]
        let t = (i as u32) * 100;
        sec.insert_event(
            i,
            AssEvent::dialogue(
                AssTime::from_cs(t),
                AssTime::from_cs(t + 90),
                "Initial text",
            ),
        );
    }
    let mut doc = DocumentController::new(script);
    doc.add_listener(Box::new(NoopListener(0)));
    doc
}

fn hot_path_amend_while_typing_zero_alloc() {
    let mut doc = make_doc(100);
    // First keystroke initializes the amend session.
    doc.amend_text(0, "typing", "H").unwrap();

    let before = allocs();
    for step in 0..200 {
        let s = match step % 4 {
            0 => "He",
            1 => "Hel",
            2 => "Hell",
            _ => "Hello world!",
        };
        doc.amend_text(0, "typing", s).unwrap();
        let cur = doc.effective_event_text(0).unwrap();
        assert_ne!(cur.len(), 0);
    }
    let after = allocs();
    assert_eq!(
        after - before,
        0,
        "amend_text while typing allocated {} times",
        after - before
    );
}

fn scaling_n_vs_2n_commits() {
    let mut d1 = make_doc(500);
    let mut d2 = make_doc(1000);

    let t1 = Instant::now();
    for i in 0..200 {
        d1.commit(
            "edit",
            EditCommand::ModifyEventText {
                index: i % 500,
                before: "Initial text".to_owned(),
                after: "Updated text".to_owned(),
            },
            CommitType::TEXT,
        )
        .unwrap();
    }
    let e1 = t1.elapsed().as_nanos().max(1);

    let t2 = Instant::now();
    for i in 0..400 {
        d2.commit(
            "edit",
            EditCommand::ModifyEventText {
                index: i % 1000,
                before: "Initial text".to_owned(),
                after: "Updated text".to_owned(),
            },
            CommitType::TEXT,
        )
        .unwrap();
    }
    let e2 = t2.elapsed().as_nanos().max(1);

    assert!(e2 <= e1 * 5, "expected linear scaling: n={e1}ns, 2n={e2}ns");
}

#[test]
fn zero_alloc_hot_path_and_scaling() {
    hot_path_amend_while_typing_zero_alloc();
    scaling_n_vs_2n_commits();
}
