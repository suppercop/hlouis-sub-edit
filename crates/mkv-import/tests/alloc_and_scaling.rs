//! Zero-allocation SRT->ASS tag conversion & picker selection + scaling tests for `mkv-import`.

use ass_core::AssTime;
use mkv_import::{TrackPickerModel, convert_srt_events_to_ass, srt_text_to_ass_into};
use mkv_subs::{SubtitleCodec, TrackInfo, TrackType};
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

fn make_tracks(n: usize) -> Vec<TrackInfo> {
    (0..n)
        .map(|i| TrackInfo {
            number: (i as u64) + 1,
            track_type: TrackType::Subtitle,
            codec_id: "S_TEXT/UTF8".to_owned(),
            subtitle_codec: SubtitleCodec::Srt,
            language: Some("vie".to_owned()),
            language_bcp47: None,
            name: Some(format!("Sub {i}")),
            is_default: i == 0,
            is_forced: false,
            codec_private: None,
            encodings: Vec::new(),
        })
        .collect()
}

fn hot_path_srt_convert_and_picker_toggle_zero_alloc() {
    let tracks = make_tracks(64);
    let mut picker = TrackPickerModel::from_tracks(&tracks);
    let mut out = String::with_capacity(512);
    let mut sel_buf = Vec::with_capacity(64);
    let sample = "<i>Xin ch\u{00e0}o</i> &amp; <font color=\"#FFFF00\">th\u{1ebf} gi\u{1edb}i</font>\nD\u{00f2}ng 2";

    let before = allocs();
    for i in 1..=64u64 {
        srt_text_to_ass_into(sample, &mut out);
        let _ = picker.toggle_select(i);
        picker.selected_track_numbers_into(&mut sel_buf);
    }
    let after = allocs();
    assert_ne!(out.len(), 0);
    assert_eq!(
        after - before,
        0,
        "mkv-import hot path allocated {} times",
        after - before
    );
}

fn scaling_n_vs_2n_srt_conversion() {
    let sample = "<i>Hello</i> &amp; <b>world</b>\n<font color=\"#00FF80\">Subtitle line</font>";
    let ev1: Vec<(AssTime, AssTime, &str)> = (0..1_000u32)
        .map(|i| {
            #[allow(clippy::cast_possible_truncation)]
            let s = i * 100;
            (AssTime::from_cs(s), AssTime::from_cs(s + 90), sample)
        })
        .collect();
    let ev2: Vec<(AssTime, AssTime, &str)> = (0..2_000u32)
        .map(|i| {
            #[allow(clippy::cast_possible_truncation)]
            let s = i * 100;
            (AssTime::from_cs(s), AssTime::from_cs(s + 90), sample)
        })
        .collect();

    let t1 = Instant::now();
    let s1 = convert_srt_events_to_ass(&ev1, "Default");
    let d1 = t1.elapsed().as_nanos().max(1);

    let t2 = Instant::now();
    let s2 = convert_srt_events_to_ass(&ev2, "Default");
    let d2 = t2.elapsed().as_nanos().max(1);

    assert_eq!(s1.events().count(), 1_000);
    assert_eq!(s2.events().count(), 2_000);
    assert!(d2 <= d1 * 4, "expected O(n) scaling: n={d1}ns, 2n={d2}ns");
}

#[test]
fn zero_alloc_hot_path_and_scaling() {
    hot_path_srt_convert_and_picker_toggle_zero_alloc();
    scaling_n_vs_2n_srt_conversion();
}
