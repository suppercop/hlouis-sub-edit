//! CPU-independent scaling invariant tests for `ass-core`:
//! - `parse_ass` and `serialize_ass` scale linearly O(N) across `10k -> 20k -> 40k` lines.
//! - Single-line `UndoHistory::execute`/`undo`/`redo` is O(1) in allocations and independent of script size.

mod ffi {
    use std::alloc::{GlobalAlloc, Layout, System};
    use std::cell::Cell;

    pub struct CountingAlloc;

    std::thread_local! {
        static COUNTING: Cell<bool> = const { Cell::new(false) };
        static ALLOCS: Cell<usize> = const { Cell::new(0) };
    }

    pub fn measure<T>(f: impl FnOnce() -> T) -> (T, usize) {
        let _ = ALLOCS.try_with(|c| c.set(0));
        let _ = COUNTING.try_with(|flag| flag.set(true));
        let out = f();
        let _ = COUNTING.try_with(|flag| flag.set(false));
        let count = ALLOCS.try_with(Cell::get).unwrap_or(0);
        (out, count)
    }

    #[inline]
    fn record_if_active() {
        let _ = COUNTING.try_with(|flag| {
            if flag.get() {
                let _ = ALLOCS.try_with(|c| c.set(c.get().saturating_add(1)));
            }
        });
    }

    // SAFETY: Delegates allocations to `std::alloc::System` and counts allocations inside `ffi::measure`.
    unsafe impl GlobalAlloc for CountingAlloc {
        unsafe fn alloc(&self, layout: Layout) -> *mut u8 {
            record_if_active();
            // SAFETY: Forwards `GlobalAlloc::alloc` to `System`.
            unsafe { System.alloc(layout) }
        }

        unsafe fn alloc_zeroed(&self, layout: Layout) -> *mut u8 {
            record_if_active();
            // SAFETY: Forwards `GlobalAlloc::alloc_zeroed` to `System`.
            unsafe { System.alloc_zeroed(layout) }
        }

        unsafe fn dealloc(&self, ptr: *mut u8, layout: Layout) {
            // SAFETY: Forwards `GlobalAlloc::dealloc` to `System`.
            unsafe { System.dealloc(ptr, layout) }
        }

        unsafe fn realloc(&self, ptr: *mut u8, layout: Layout, new_size: usize) -> *mut u8 {
            record_if_active();
            // SAFETY: Forwards `GlobalAlloc::realloc` to `System`.
            unsafe { System.realloc(ptr, layout, new_size) }
        }
    }
}

#[global_allocator]
static GLOBAL: ffi::CountingAlloc = ffi::CountingAlloc;

use ass_core::{parse_ass, serialize_ass, AssTime, EditCommand, UndoHistory};
use std::fmt::Write as _;
use std::time::{Duration, Instant};

fn build_script_text(lines: usize) -> String {
    let mut out = String::with_capacity(lines * 96);
    out.push_str(
        "[Script Info]\nTitle: Scaling\nScriptType: v4.00+\nPlayResX: 1920\nPlayResY: 1080\n\n",
    );
    out.push_str("[V4+ Styles]\nFormat: Name, Fontname, Fontsize, PrimaryColour, SecondaryColour, OutlineColour, BackColour, Bold, Italic, Underline, StrikeOut, ScaleX, ScaleY, Spacing, Angle, BorderStyle, Outline, Shadow, Alignment, MarginL, MarginR, MarginV, Encoding\n");
    out.push_str("Style: Default,Arial,48,&H00FFFFFF,&H0000FFFF,&H00000000,&H80000000,-1,0,0,0,100,100,0,0,1,2,1,2,20,20,20,1\n\n");
    out.push_str("[Events]\nFormat: Layer, Start, End, Style, Name, MarginL, MarginR, MarginV, Effect, Text\n");
    for i in 0..lines {
        let start = AssTime::from_cs(u32::try_from(i * 100).unwrap_or(u32::MAX));
        let end = start.saturating_add_cs(90);
        let _ = writeln!(
            out,
            "Dialogue: 0,{start},{end},Default,Speaker,0,0,0,,{{\\an8\\fs40}}Subtitle line #{i}"
        );
    }
    out
}

fn median_time(mut f: impl FnMut()) -> Duration {
    let mut times = [Duration::ZERO; 3];
    for t in &mut times {
        let s = Instant::now();
        f();
        *t = s.elapsed();
    }
    times.sort_unstable();
    times[1]
}

#[test]
fn parse_and_serialize_scale_linearly_10k_20k_40k() {
    let raw_10 = build_script_text(10_000);
    let raw_20 = build_script_text(20_000);
    let raw_40 = build_script_text(40_000);

    let (parsed_10, alloc_p10) = ffi::measure(|| parse_ass(&raw_10).value);
    let (parsed_20, alloc_p20) = ffi::measure(|| parse_ass(&raw_20).value);
    let (parsed_40, alloc_p40) = ffi::measure(|| parse_ass(&raw_40).value);

    // Allocation counts must scale linearly (~2x for 20k, ~4x for 40k).
    assert!(
        alloc_p20 * 10 <= alloc_p10 * 22,
        "parse 20k allocs {alloc_p20} exceeded 2.2x of 10k allocs {alloc_p10}"
    );
    assert!(
        alloc_p40 * 10 <= alloc_p10 * 44,
        "parse 40k allocs {alloc_p40} exceeded 4.4x of 10k allocs {alloc_p10}"
    );

    let (ser_10, ser_alloc_10) = ffi::measure(|| serialize_ass(&parsed_10));
    let (ser_20, ser_alloc_20) = ffi::measure(|| serialize_ass(&parsed_20));
    let (ser_40, ser_alloc_40) = ffi::measure(|| serialize_ass(&parsed_40));
    assert!(!ser_10.is_empty() && !ser_20.is_empty() && !ser_40.is_empty());

    assert!(
        ser_alloc_20 * 10 <= ser_alloc_10 * 24 + 50,
        "serialize 20k allocs {ser_alloc_20} vs 10k {ser_alloc_10}"
    );
    assert!(
        ser_alloc_40 * 10 <= ser_alloc_10 * 48 + 50,
        "serialize 40k allocs {ser_alloc_40} vs 10k {ser_alloc_10}"
    );

    let t_p10 = median_time(|| {
        std::hint::black_box(parse_ass(&raw_10).value.events().count());
    });
    let t_p40 = median_time(|| {
        std::hint::black_box(parse_ass(&raw_40).value.events().count());
    });
    let floor = t_p10.max(Duration::from_millis(2));
    assert!(
        t_p40 <= floor * 8,
        "parse 40k ({t_p40:?}) exceeded O(N) bound vs 10k ({t_p10:?})"
    );
}

#[test]
fn single_line_undo_redo_is_constant_o1() {
    let mut s10 = parse_ass(&build_script_text(10_000)).value;
    let mut s40 = parse_ass(&build_script_text(40_000)).value;

    let mut h10 = UndoHistory::default();
    let mut h40 = UndoHistory::default();

    let (r10, alloc_exec_10) = ffi::measure(|| {
        h10.execute(
            &mut s10,
            EditCommand::ModifyEventText {
                index: 9_999,
                before: r"{\an8\fs40}Subtitle line #9999".to_owned(),
                after: "Edited".to_owned(),
            },
        )
    });
    assert!(r10.is_ok());

    let (r40, alloc_exec_40) = ffi::measure(|| {
        h40.execute(
            &mut s40,
            EditCommand::ModifyEventText {
                index: 39_999,
                before: r"{\an8\fs40}Subtitle line #39999".to_owned(),
                after: "Edited".to_owned(),
            },
        )
    });
    assert!(r40.is_ok());
    assert_eq!(
        alloc_exec_10, alloc_exec_40,
        "single-line execute allocation count must be O(1) independent of N"
    );

    // Warm up once and confirm undo+redo makes zero heap allocations for both 10k and 40k scripts.
    let _ = h10.undo(&mut s10);
    let _ = h10.redo(&mut s10);
    let ((), undo_allocs_10) = ffi::measure(|| {
        for _ in 0..100 {
            let _ = h10.undo(&mut s10);
            let _ = h10.redo(&mut s10);
        }
    });

    let _ = h40.undo(&mut s40);
    let _ = h40.redo(&mut s40);
    let ((), undo_allocs_40) = ffi::measure(|| {
        for _ in 0..100 {
            let _ = h40.undo(&mut s40);
            let _ = h40.redo(&mut s40);
        }
    });

    assert_eq!(
        undo_allocs_10, 0,
        "10k undo/redo allocated {undo_allocs_10}"
    );
    assert_eq!(
        undo_allocs_40, 0,
        "40k undo/redo allocated {undo_allocs_40}"
    );
}
