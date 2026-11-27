use ass_core::{AssEvent, AssTime, ScriptFile};
use audio_peaks::PeakBucket;
use audio_view::TimelineMarkerIndex;
use criterion::{Criterion, black_box, criterion_group, criterion_main};
use egui::{Pos2, RawInput, Rect, Vec2};
use keyframes::Timecodes;
use playback::Clock;
use ui_audio::AudioColumnProvider;
use ui_shell::AppShell;
use ui_video::{RgbaFrameRef, VideoFrameSupplier};

struct ZeroClock;
impl Clock for ZeroClock {
    fn now_ns(&self) -> u64 {
        0
    }
}

struct BenchVideo {
    rgba: Vec<u8>,
}
impl VideoFrameSupplier for BenchVideo {
    fn total_frames(&self) -> u32 {
        24_000
    }
    fn is_keyframe(&self, frame: u32) -> bool {
        frame.is_multiple_of(24)
    }
    fn frame_revision(&self, _frame: u32) -> u64 {
        1
    }
    fn frame_rgba(&self, _frame: u32) -> Option<RgbaFrameRef<'_>> {
        Some(RgbaFrameRef {
            width: 128,
            height: 72,
            rgba: &self.rgba,
        })
    }
}

struct BenchAudio;
impl AudioColumnProvider for BenchAudio {
    fn duration_ms(&self) -> i64 {
        1_000_000
    }
    fn query_columns_into(
        &self,
        _start_ms: i64,
        _end_ms: i64,
        width_px: usize,
        out: &mut Vec<PeakBucket>,
    ) {
        out.clear();
        out.resize(
            width_px,
            PeakBucket {
                min: -12_000,
                max: 12_000,
                rms: 8_000,
            },
        );
    }
}

fn make_script(n: usize) -> ScriptFile {
    let mut script = ScriptFile::new_default();
    let sec = script.ensure_events_section_mut();
    for i in 0..n {
        #[allow(clippy::cast_possible_truncation)]
        let s = (i as u32) * 5;
        sec.insert_event(
            i,
            AssEvent::dialogue(
                AssTime::from_cs(s),
                AssTime::from_cs(s + 120),
                r"{\i1}Sample subtitle dialogue line{\i0}",
            ),
        );
    }
    script
}

fn bench_ui_shell(c: &mut Criterion) {
    let script = make_script(200_000);
    let mut shell = AppShell::new(script);
    if let Ok(tc) = Timecodes::from_cfr(24_000, 1_001) {
        shell.attach_video(tc, 24_000, vec![0, 24, 48], 128, 72);
    }
    shell.attach_audio(
        1_000_000,
        TimelineMarkerIndex::new(vec![0, 2_000], vec![0, 500]),
    );
    shell.grid.model.click(100_000);
    shell.grid.scroll_to_active();

    let vid = BenchVideo {
        rgba: vec![48u8; 128 * 72 * 4],
    };
    let aud = BenchAudio;
    let ctx = egui::Context::default();
    let raw = RawInput {
        screen_rect: Some(Rect::from_min_size(Pos2::ZERO, Vec2::new(1280.0, 800.0))),
        ..Default::default()
    };

    for _ in 0..3 {
        let _ = ctx.run_ui(raw.clone(), |ui| {
            shell.show(ui, Some(&vid), Some(&aud), &ZeroClock);
        });
    }

    c.bench_function("ui_shell_full_frame_200k_lines", |b| {
        b.iter(|| {
            let out = ctx.run_ui(raw.clone(), |ui| {
                shell.show(ui, Some(&vid), Some(&aud), &ZeroClock);
            });
            black_box(out.shapes.len());
        });
    });
}

criterion_group!(benches, bench_ui_shell);
criterion_main!(benches);
