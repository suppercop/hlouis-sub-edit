//! `hlouis-ui-demo` — cross-platform headless benchmark & demo binary for `ui-shell`.
//!
//! Usage:
//!   `cargo run -p ui-shell --release --bin hlouis-ui-demo -- --bench-ui`
//!
//! All synthetic demo data (200,000-line script, dummy checkerboard video, procedural waveform,
//! CFR 24000/1001 timecodes, keyframes, audio markers) lives strictly in this binary — never in
//! the `ui-shell` library crate.

#![forbid(unsafe_code)]
#![allow(clippy::cast_precision_loss, clippy::cast_possible_truncation)]
#![cfg_attr(not(test), deny(clippy::unwrap_used, clippy::expect_used))]

use ass_core::{AssEvent, AssTime, ScriptFile};
use audio_peaks::PeakBucket;
use audio_view::TimelineMarkerIndex;
use egui::{Event, ImeEvent, Pos2, RawInput, Rect, Vec2};
use keyframes::Timecodes;
use playback::Clock;
use std::process::ExitCode;
use std::time::Instant;
use ui_audio::AudioColumnProvider;
use ui_shell::AppShell;
use ui_video::{RgbaFrameRef, VideoFrameSupplier};

struct InstantClock(Instant);
impl Clock for InstantClock {
    fn now_ns(&self) -> u64 {
        self.0.elapsed().as_nanos() as u64
    }
}

struct DummyVideoSupplier {
    width: u32,
    height: u32,
    total_frames: u32,
    rgba: Vec<u8>,
}

impl DummyVideoSupplier {
    fn new(width: u32, height: u32, total_frames: u32) -> Self {
        let mut rgba = vec![0u8; (width as usize) * (height as usize) * 4];
        for y in 0..height {
            for x in 0..width {
                let dark = ((x / 16) + (y / 16)).is_multiple_of(2);
                let v = if dark { 42u8 } else { 58u8 };
                let idx = ((y as usize) * (width as usize) + (x as usize)) * 4;
                rgba[idx] = v;
                rgba[idx + 1] = v;
                rgba[idx + 2] = v + 6;
                rgba[idx + 3] = 255;
            }
        }
        Self {
            width,
            height,
            total_frames,
            rgba,
        }
    }
}

impl VideoFrameSupplier for DummyVideoSupplier {
    fn total_frames(&self) -> u32 {
        self.total_frames
    }
    fn is_keyframe(&self, frame: u32) -> bool {
        frame.is_multiple_of(24)
    }
    fn frame_revision(&self, _frame: u32) -> u64 {
        1
    }
    fn frame_rgba(&self, _frame: u32) -> Option<RgbaFrameRef<'_>> {
        Some(RgbaFrameRef {
            width: self.width,
            height: self.height,
            rgba: &self.rgba,
        })
    }
}

struct DummyAudioProvider {
    duration_ms: i64,
}

impl AudioColumnProvider for DummyAudioProvider {
    fn duration_ms(&self) -> i64 {
        self.duration_ms
    }
    fn query_columns_into(
        &self,
        start_ms: i64,
        end_ms: i64,
        width_px: usize,
        out: &mut Vec<PeakBucket>,
    ) {
        out.clear();
        let span = (end_ms - start_ms).max(1) as f32;
        for i in 0..width_px {
            let t = (start_ms as f32) + span * (i as f32 / width_px.max(1) as f32);
            #[allow(clippy::cast_sign_loss)]
            let amp = (((t * 0.015).sin() * (t * 0.0023).cos()).abs() * 24_000.0) as i16;
            out.push(PeakBucket {
                min: -amp,
                max: amp,
                rms: amp.unsigned_abs(),
            });
        }
    }
}

fn generate_demo_script(line_count: usize) -> ScriptFile {
    let mut script = ScriptFile::new_default();
    let sec = script.ensure_events_section_mut();
    for i in 0..line_count {
        let s = (i as u32) * 5;
        let text = if i.is_multiple_of(20) {
            r"{\an8\blur1}Typeset sign / Tiêu đề cảnh"
        } else {
            r"{\i1}Sample subtitle dialogue line{\i0}"
        };
        let mut ev = AssEvent::dialogue(AssTime::from_cs(s), AssTime::from_cs(s + 120), text);
        if i.is_multiple_of(10) {
            ev.set_style("Sign");
        }
        sec.insert_event(i, ev);
    }
    script
}

#[allow(clippy::too_many_lines)]
fn run_bench_ui() -> ExitCode {
    let script = generate_demo_script(200_000);
    let mut shell = AppShell::new(script);
    if let Ok(tc) = Timecodes::from_cfr(24_000, 1_001) {
        let kfs: Vec<u32> = (0..1_000).map(|i| i * 24).collect();
        shell.attach_video(tc, 24_000, kfs, 128, 72);
    }
    let markers = TimelineMarkerIndex::new(
        (0..500).map(|i| i * 2_000).collect(),
        (0..2_000).map(|i| i * 500).collect(),
    );
    shell.attach_audio(1_000_000, markers);

    let vid = DummyVideoSupplier::new(128, 72, 24_000);
    let aud = DummyAudioProvider {
        duration_ms: 1_000_000,
    };
    let clock = InstantClock(Instant::now());
    let ctx = egui::Context::default();
    let base_raw = RawInput {
        screen_rect: Some(Rect::from_min_size(Pos2::ZERO, Vec2::new(1280.0, 800.0))),
        ..Default::default()
    };

    // Warm up 10 frames (font atlas incl. Vietnamese glyphs + initial waveform/video textures).
    let warmup_text = "Tiếng Việt có dấu — ăn uống, đường phố, phụ đề anime 0123456789";
    let _ = shell.doc.amend_text(0, "warmup", warmup_text);
    for _ in 0..10 {
        let _ = ctx.run_ui(base_raw.clone(), |ui| {
            shell.show(ui, Some(&vid), Some(&aud), &clock);
        });
    }

    // Phase 1: 220 keystrokes (ASCII + Vietnamese Telex-composed + IME preedit/commit).
    let telex_chunks = [
        "T", "i", "ế", "n", "g", " ", "V", "i", "ệ", "t", " ", "c", "ó", " ", "d", "ấ", "u", " ",
        "đ", "ư", "ờ", "n", "g", " ", "p", "h", "ố", " ", "ă", "n", " ", "u", "ố", "n", "g", " ",
        "a", "n", "i", "m", "e", " ", "{", "\\", "i", "1", "}", "H", "L", "o", "u", "i", "s", "!",
    ];
    let mut typed_buf = String::with_capacity(256);
    let active_row = 100_000usize;
    shell.grid.model.click(active_row);
    shell.grid.scroll_to_active();
    shell.doc.set_selection(Some(active_row), &[active_row]);
    let _ = ctx.run_ui(base_raw.clone(), |ui| {
        shell.show(ui, Some(&vid), Some(&aud), &clock);
    });

    for k in 0..220usize {
        let chunk = telex_chunks[k % telex_chunks.len()];
        if typed_buf.len() > 160 {
            typed_buf.clear();
        }
        typed_buf.push_str(chunk);

        let mut raw = base_raw.clone();
        if k % 5 == 0 {
            raw.events.push(Event::Ime(ImeEvent::Preedit {
                text: chunk.to_owned(),
                active_range_chars: None,
            }));
        } else if k % 5 == 1 {
            raw.events
                .push(Event::Ime(ImeEvent::Commit(chunk.to_owned())));
        } else {
            raw.events.push(Event::Text(chunk.to_owned()));
        }

        let t0 = Instant::now();
        let _ = shell.doc.amend_text(active_row, "typing", &typed_buf);
        if let Some(ev) = shell
            .doc
            .script()
            .events_section()
            .and_then(|s| s.get_event(active_row))
        {
            let mut updated = ev.clone();
            updated.set_text(&typed_buf);
            shell.grid.sync_row(active_row, &updated);
        }
        let _ = ctx.run_ui(raw, |ui| {
            shell.show(ui, Some(&vid), Some(&aud), &clock);
        });
        let elapsed_us = t0.elapsed().as_micros() as u64;
        shell
            .metrics
            .keystroke_to_present
            .histogram
            .record(elapsed_us);
        shell.metrics.record_frame_us(elapsed_us, clock.now_ns());
    }

    // Phase 2: 120 seeks (frame step, keyframe jump, random jump, subtitle grid click).
    let mut rng_state: u64 = 0x5EED_1234_5678_9ABC;
    for s in 0..120usize {
        let t0 = Instant::now();
        match s % 4 {
            0 => {
                // Frame step (+1 / -1)
                if let Some(ref mut pb) = shell.playback {
                    let cur = pb.current_frame();
                    let next = if s % 8 == 0 {
                        cur.saturating_add(1).min(23_999)
                    } else {
                        cur.saturating_sub(1)
                    };
                    pb.scrub_to_frame(next);
                }
            }
            1 => {
                // Keyframe jump (next / prev)
                if let Some(ref mut pb) = shell.playback {
                    let kf = ((s as u32) * 24) % 23_976;
                    pb.scrub_to_frame(kf);
                }
            }
            2 => {
                // Random jump
                rng_state ^= rng_state << 13;
                rng_state ^= rng_state >> 7;
                rng_state ^= rng_state << 17;
                let frame = (rng_state % 24_000) as u32;
                if let Some(ref mut pb) = shell.playback {
                    pb.scrub_to_frame(frame);
                }
            }
            _ => {
                // Subtitle grid click syncing video to row's start time
                let row = 50_000 + (s * 413) % 140_000;
                shell.grid.model.click(row);
                shell.grid.scroll_to_active();
                shell.doc.set_selection(Some(row), &[row]);
                if let Some(ref mut pb) = shell.playback {
                    let target_frame = ((row as u32) * 3) % 24_000;
                    pb.scrub_to_frame(target_frame);
                }
            }
        }
        let _ = ctx.run_ui(base_raw.clone(), |ui| {
            shell.show(ui, Some(&vid), Some(&aud), &clock);
        });
        let elapsed_us = t0.elapsed().as_micros() as u64;
        shell.metrics.seek_latency.histogram.record(elapsed_us);
        shell.metrics.record_frame_us(elapsed_us, clock.now_ns());
    }

    // Phase 3: 120 general workspace frames (scroll + audio selection drag).
    for frame in 0..120usize {
        if frame.is_multiple_of(2) {
            let row = 95_000 + frame * 17;
            shell.grid.model.click(row);
            shell.grid.scroll_to_active();
            shell.doc.set_selection(Some(row), &[row]);
        } else {
            let delta = i64::try_from(frame).unwrap_or(0) * 15;
            shell.audio.active_selection.start_ms = 1_000 + delta;
            shell.audio.active_selection.end_ms = 3_000 + delta;
        }
        let t0 = Instant::now();
        let _ = ctx.run_ui(base_raw.clone(), |ui| {
            shell.show(ui, Some(&vid), Some(&aud), &clock);
        });
        let elapsed_us = t0.elapsed().as_micros() as u64;
        shell.metrics.record_frame_us(elapsed_us, clock.now_ns());
    }

    let mut table = String::new();
    shell.metrics.format_text_table(&mut table);
    println!(
        "=== hlouis-ui-demo --bench-ui (200,000 lines, 220 keystrokes + 120 seeks + 120 frames) ==="
    );
    println!("{table}");

    if shell.metrics.frame_time.count() == 0
        || shell.metrics.keystroke_to_present.histogram.count() < 200
        || shell.metrics.seek_latency.histogram.count() < 100
    {
        eprintln!(
            "BENCH FAILED: missing samples (frames={}, keystrokes={}, seeks={})",
            shell.metrics.frame_time.count(),
            shell.metrics.keystroke_to_present.histogram.count(),
            shell.metrics.seek_latency.histogram.count()
        );
        return ExitCode::FAILURE;
    }

    ExitCode::SUCCESS
}

fn main() -> ExitCode {
    let args: Vec<String> = std::env::args().collect();
    if !args.iter().any(|a| a == "--bench-ui") {
        println!("hlouis-ui-demo: pass --bench-ui to run the 200,000-line scripted UI benchmark.");
    }
    run_bench_ui()
}
