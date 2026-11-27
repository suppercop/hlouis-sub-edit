//! `hlouis-ui-demo` — cross-platform headless benchmark & demo binary for `ui-shell`.
//!
//! Usage:
//!   `cargo run -p ui-shell --release --bin hlouis-ui-demo -- --bench-ui`
//!
//! Generates a 200,000-line script, dummy checkerboard video, and procedural waveform, scripts
//! scrolling, typing in the edit box, selecting rows, and dragging audio markers across 120 frames,
//! and prints p50 / p95 / p99 / worst frame CPU times via `perf-metrics`.

#![forbid(unsafe_code)]
#![cfg_attr(not(test), deny(clippy::unwrap_used, clippy::expect_used))]

use egui::{Pos2, RawInput, Rect, Vec2};
use playback::Clock;
use std::time::Instant;
use ui_shell::{AppShell, DummyAudioProvider, DummyVideoSupplier, generate_demo_script};

struct InstantClock(Instant);
impl Clock for InstantClock {
    fn now_ns(&self) -> u64 {
        #[allow(clippy::cast_possible_truncation)]
        {
            self.0.elapsed().as_nanos() as u64
        }
    }
}

fn run_bench_ui() {
    let script = generate_demo_script(200_000);
    let mut shell = AppShell::new(script);
    let vid = DummyVideoSupplier::new(128, 72, 24_000);
    let aud = DummyAudioProvider {
        duration_ms: 1_000_000,
    };
    let clock = InstantClock(Instant::now());
    let ctx = egui::Context::default();
    let raw = RawInput {
        screen_rect: Some(Rect::from_min_size(Pos2::ZERO, Vec2::new(1280.0, 800.0))),
        ..Default::default()
    };

    // Warm up 5 frames (font atlas + initial waveform/video texture uploads).
    for _ in 0..5 {
        let _ = ctx.run_ui(raw.clone(), |ui| {
            shell.show(ui, &vid, &aud, &clock);
        });
    }

    // Script 120 realistic UI frames: scrolling, selecting, typing (amend), dragging markers.
    for frame in 0..120usize {
        match frame % 4 {
            0 => {
                // Scroll & select around middle of 200,000 lines.
                let row = 95_000 + frame * 17;
                shell.grid.model.click(row);
                shell.grid.scroll_to_active();
                shell.doc.set_selection(Some(row), &[row]);
            }
            1 => {
                // Typing in edit box (0-alloc amend + O(1) grid row sync).
                let row = shell.doc.active_line().unwrap_or(0);
                let _ = shell
                    .doc
                    .amend_text(row, "typing", "Edited line during bench");
                if let Some(ev) = shell
                    .doc
                    .script()
                    .events_section()
                    .and_then(|s| s.get_event(row))
                {
                    let mut updated = ev.clone();
                    updated.set_text("Edited line during bench");
                    shell.grid.sync_row(row, &updated);
                }
            }
            2 => {
                // Dragging audio selection marker.
                let delta = i64::try_from(frame).unwrap_or(0) * 15;
                shell.audio.active_selection.start_ms = 1_000 + delta;
                shell.audio.active_selection.end_ms = 3_000 + delta;
            }
            _ => {
                // Scrubbing video frame.
                #[allow(clippy::cast_possible_truncation)]
                shell.playback.scrub_to_frame((frame as u32) * 12);
            }
        }

        let t0 = Instant::now();
        let _ = ctx.run_ui(raw.clone(), |ui| {
            shell.show(ui, &vid, &aud, &clock);
        });
        #[allow(clippy::cast_possible_truncation)]
        let elapsed_us = t0.elapsed().as_micros() as u64;
        shell.metrics.record_frame_us(elapsed_us, clock.now_ns());
    }

    let mut table = String::new();
    shell.metrics.format_text_table(&mut table);
    println!("=== hlouis-ui-demo --bench-ui (200,000 lines, 120 scripted frames) ===");
    println!("{table}");
}

fn main() {
    let args: Vec<String> = std::env::args().collect();
    if args.iter().any(|a| a == "--bench-ui") {
        run_bench_ui();
    } else {
        println!("hlouis-ui-demo: pass --bench-ui to run the 200,000-line scripted UI benchmark.");
        run_bench_ui();
    }
}
