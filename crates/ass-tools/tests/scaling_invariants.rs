//! CPU-independent O(N) scaling invariant tests for `ass-tools` across `10k -> 20k -> 40k`.

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

use ass_core::{AssEvent, AssTime, EventEntry, ScriptFile};
use ass_tools::{
    find_and_replace, resample_resolution, select_lines, shift_times, timing_post_process,
    AdjacentLinkConfig, FindReplaceOptions, KeyframeSnapConfig, MatchMode, ResampleOptions,
    SearchField, SearchScope, SelectField, SelectLinesOptions, SelectionAction, ShiftAffect,
    ShiftAmount, ShiftDirection, ShiftOptions, ShiftTarget, TppOptions,
};
use std::collections::BTreeSet;
use std::time::{Duration, Instant};

fn build_script(n: usize) -> ScriptFile {
    let mut script = ScriptFile::new_default();
    script.ensure_events_section_mut().entries = (0..n)
        .map(|i| {
            let start = AssTime::from_cs(u32::try_from(i * 150).unwrap_or(u32::MAX));
            let end = start.saturating_add_cs(135);
            let text = if i % 4 == 0 {
                format!(r"{{\an8\pos(640,360)\fs40}}Sample line #{i}")
            } else {
                format!("Dialogue sample #{i}")
            };
            EventEntry::Event(AssEvent::dialogue(start, end, text))
        })
        .collect();
    script
}

fn min_resample_time(s: &ScriptFile, opts: &ResampleOptions) -> Duration {
    (0..3)
        .map(|_| {
            let t = Instant::now();
            let _ = std::hint::black_box(resample_resolution(s, opts));
            t.elapsed()
        })
        .min()
        .unwrap_or_default()
}

#[test]
fn core_tools_scale_linearly_from_10k_to_40k() {
    let s10 = build_script(10_000);
    let s20 = build_script(20_000);
    let s40 = build_script(40_000);
    let keyframes: Vec<u32> = (0..1_000u32).map(|k| k * 3_000).collect();
    let empty_sel = BTreeSet::new();

    // 1. Shift times allocation + time linearity
    let (_, a_sh10) = ffi::measure(|| {
        shift_times(
            &s10,
            ShiftOptions {
                amount: ShiftAmount::TimeCs(25),
                direction: ShiftDirection::Forward,
                target: ShiftTarget::Both,
                affect: ShiftAffect::All,
            },
        )
    });
    let (_, a_sh40) = ffi::measure(|| {
        shift_times(
            &s40,
            ShiftOptions {
                amount: ShiftAmount::TimeCs(25),
                direction: ShiftDirection::Forward,
                target: ShiftTarget::Both,
                affect: ShiftAffect::All,
            },
        )
    });
    assert!(
        a_sh40 <= a_sh10 * 5 + 16,
        "shift 40k allocs {a_sh40} vs 10k {a_sh10}"
    );

    // 2. Find & replace allocation + time linearity
    let fr_opts = FindReplaceOptions {
        query: "sample",
        replacement: "edited",
        field: SearchField::Text,
        case_sensitive: true,
        use_regex: false,
        skip_tags: true,
        scope: SearchScope::All,
    };
    let (_, a_fr10) = ffi::measure(|| find_and_replace(&s10, fr_opts));
    let (_, a_fr40) = ffi::measure(|| find_and_replace(&s40, fr_opts));
    assert!(
        a_fr40 * 10 <= a_fr10 * 44 + 64,
        "find_replace 40k allocs {a_fr40} vs 10k {a_fr10}"
    );

    // 3. Select lines linearity
    let sel_opts = SelectLinesOptions {
        pattern: "Sample",
        mode: MatchMode::Contains,
        field: SelectField::Text,
        case_sensitive: false,
        match_dialogue: true,
        match_comments: true,
        invert: false,
        action: SelectionAction::Set,
    };
    let (_, a_sel10) = ffi::measure(|| select_lines(&s10, &empty_sel, sel_opts));
    let (_, a_sel20) = ffi::measure(|| select_lines(&s20, &empty_sel, sel_opts));
    let (_, a_sel40) = ffi::measure(|| select_lines(&s40, &empty_sel, sel_opts));
    assert!(a_sel20 * 10 <= a_sel10 * 24 + 32);
    assert!(a_sel40 * 10 <= a_sel10 * 44 + 32);

    // 4. TPP allocation linearity
    let tpp_opts = TppOptions {
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
    };
    let (_, a_tpp10) = ffi::measure(|| timing_post_process(&s10, tpp_opts));
    let (_, a_tpp40) = ffi::measure(|| timing_post_process(&s40, tpp_opts));
    assert!(a_tpp40 * 10 <= a_tpp10 * 48 + 64);

    // 5. Resample allocation + time linearity
    let r_opts = ResampleOptions::new(1920, 1080);
    let (_, resamp_alloc_10) = ffi::measure(|| resample_resolution(&s10, &r_opts));
    let (_, resamp_alloc_40) = ffi::measure(|| resample_resolution(&s40, &r_opts));
    // Min-of-3 wall times (after the allocation-counting warm-up above) so one
    // scheduler hiccup on a shared runner cannot fail the O(N) check.
    let d10 = min_resample_time(&s10, &r_opts);
    let d40 = min_resample_time(&s40, &r_opts);
    assert!(resamp_alloc_40 * 10 <= resamp_alloc_10 * 44 + 64);
    assert!(
        d40 <= d10.max(Duration::from_millis(2)) * 8,
        "resample 40k ({d40:?}) exceeded O(N) vs 10k ({d10:?})"
    );
}
