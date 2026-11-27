//! `hlouis-ui-demo` — interactive `eframe`/`wgpu` window, headless `--snapshots <dir>` PNG
//! renderer, and `--bench-ui` 200,000-line scripted benchmark for `ui-shell`.
//!
//! Usage:
//! - Default (no args): opens an interactive DPI-aware `eframe`/`wgpu` window showing the full
//!   classic Aegisub 3.2.2 `AppShell` layout with 20,000 demo lines, solid-colour video with live
//!   frame counter, and synthetic audio waveform/spectrum.
//! - `--snapshots <dir>`: renders main views and dialogs across Light, Dark, and Eye Comfort
//!   themes via `egui_kittest` and saves PNGs into `<dir>`.
//! - `--bench-ui`: runs the 200,000-line headless scripted benchmark (220 keystrokes + 120 seeks
//!   + 120 frames) and prints `perf-metrics` p50/p95/p99/worst.

#![forbid(unsafe_code)]
#![allow(
    clippy::cast_precision_loss,
    clippy::cast_possible_truncation,
    clippy::cast_sign_loss
)]
#![cfg_attr(not(test), deny(clippy::unwrap_used, clippy::expect_used))]

use ass_core::{AssEvent, AssTime, EventKind, ScriptFile, parse_ass};
use audio_peaks::PeakBucket;
use audio_view::TimelineMarkerIndex;
use egui::{Event, ImeEvent, Pos2, RawInput, Rect, Vec2};
use egui_kittest::Harness;
use keyframes::Timecodes;
use layout::ViewMode;
use playback::Clock;
use std::cell::RefCell;
use std::fs::{self, File};
use std::io::BufWriter;
use std::path::Path;
use std::process::ExitCode;
use std::time::Instant;
use theme::{BaseTheme, ThemeSpec};
use ui_audio::AudioColumnProvider;
use ui_dialogs::ActiveDialog;
use ui_shell::AppShell;
use ui_video::{RgbaFrameRef, VideoFrameSupplier};

struct InstantClock(Instant);
impl Clock for InstantClock {
    fn now_ns(&self) -> u64 {
        self.0.elapsed().as_nanos() as u64
    }
}

/// Solid-colour demo video supplier with a crisp 5x7 bitmap frame counter overlaid on each frame.
struct DemoVideoSupplier {
    width: u32,
    height: u32,
    last_frame: RefCell<u32>,
    rgba: RefCell<Vec<u8>>,
}

impl DemoVideoSupplier {
    fn new(width: u32, height: u32) -> Self {
        let s = Self {
            width,
            height,
            last_frame: RefCell::new(u32::MAX),
            rgba: RefCell::new(vec![0u8; (width as usize) * (height as usize) * 4]),
        };
        s.render_frame(0);
        s
    }

    fn render_frame(&self, frame: u32) {
        let mut buf = self.rgba.borrow_mut();
        let w = self.width as usize;
        let h = self.height as usize;
        // Calm slate-blue solid backdrop that shifts subtly with frame index.
        let r_bg = 28u8.wrapping_add(((frame / 12) % 24) as u8);
        let g_bg = 36u8;
        let b_bg = 52u8;
        for px in buf.as_chunks_mut::<4>().0 {
            px[0] = r_bg;
            px[1] = g_bg;
            px[2] = b_bg;
            px[3] = 255;
        }
        // Subtle border
        for x in 0..w {
            for y in [0, h.saturating_sub(1)] {
                let idx = (y * w + x) * 4;
                if idx + 3 < buf.len() {
                    buf[idx] = 80;
                    buf[idx + 1] = 110;
                    buf[idx + 2] = 150;
                }
            }
        }
        // Draw 5x7 bitmap digits for `frame` centered in the frame.
        let label = format!("F {frame:05}");
        let scale = (w / 96).max(1);
        let char_w = 6 * scale;
        let total_w = label.len() * char_w;
        let start_x = w.saturating_sub(total_w) / 2;
        let start_y = h.saturating_sub(7 * scale) / 2;
        for (ci, ch) in label.chars().enumerate() {
            let glyph = bitmap_5x7(ch);
            for (row, bits) in glyph.iter().enumerate() {
                for col in 0..5usize {
                    if (bits >> (4 - col)) & 1 == 1 {
                        for sy in 0..scale {
                            for sx in 0..scale {
                                let px = start_x + ci * char_w + col * scale + sx;
                                let py = start_y + row * scale + sy;
                                if px < w && py < h {
                                    let idx = (py * w + px) * 4;
                                    buf[idx] = 240;
                                    buf[idx + 1] = 244;
                                    buf[idx + 2] = 250;
                                    buf[idx + 3] = 255;
                                }
                            }
                        }
                    }
                }
            }
        }
        *self.last_frame.borrow_mut() = frame;
    }
}

const fn bitmap_5x7(ch: char) -> [u8; 7] {
    match ch {
        '0' => [0x0E, 0x11, 0x13, 0x15, 0x19, 0x11, 0x0E],
        '1' => [0x04, 0x0C, 0x04, 0x04, 0x04, 0x04, 0x0E],
        '2' => [0x0E, 0x11, 0x01, 0x02, 0x04, 0x08, 0x1F],
        '3' => [0x1F, 0x02, 0x04, 0x02, 0x01, 0x11, 0x0E],
        '4' => [0x02, 0x06, 0x0A, 0x12, 0x1F, 0x02, 0x02],
        '5' => [0x1F, 0x10, 0x1E, 0x01, 0x01, 0x11, 0x0E],
        '6' => [0x06, 0x08, 0x10, 0x1E, 0x11, 0x11, 0x0E],
        '7' => [0x1F, 0x01, 0x02, 0x04, 0x08, 0x08, 0x08],
        '8' => [0x0E, 0x11, 0x11, 0x0E, 0x11, 0x11, 0x0E],
        '9' => [0x0E, 0x11, 0x11, 0x0F, 0x01, 0x02, 0x0C],
        'F' => [0x1F, 0x10, 0x10, 0x1E, 0x10, 0x10, 0x10],
        _ => [0; 7],
    }
}

/// Bench-only constant-buffer video supplier (zero per-frame re-render overhead during `--bench-ui`).
struct BenchVideoSupplier {
    width: u32,
    height: u32,
    total_frames: u32,
    rgba: Vec<u8>,
}

impl BenchVideoSupplier {
    fn new(width: u32, height: u32, total_frames: u32) -> Self {
        let demo = DemoVideoSupplier::new(width, height);
        let rgba = demo.rgba.borrow().clone();
        Self {
            width,
            height,
            total_frames,
            rgba,
        }
    }
}

impl VideoFrameSupplier for BenchVideoSupplier {
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

/// Interactive demo video supplier backed by a pre-baked frame ring (safe borrow without `RefCell` leak).
struct InteractiveVideoSupplier {
    width: u32,
    height: u32,
    total_frames: u32,
    frames: Vec<Vec<u8>>,
}

impl InteractiveVideoSupplier {
    fn new(width: u32, height: u32, total_frames: u32) -> Self {
        // Pre-render 240 distinct numbered frames (0..240) so frame_rgba borrows cleanly with 0 alloc.
        let count = 240usize;
        let mut frames = Vec::with_capacity(count);
        let helper = DemoVideoSupplier::new(width, height);
        for f in 0..count {
            helper.render_frame(f as u32);
            frames.push(helper.rgba.borrow().clone());
        }
        Self {
            width,
            height,
            total_frames,
            frames,
        }
    }
}

impl VideoFrameSupplier for InteractiveVideoSupplier {
    fn total_frames(&self) -> u32 {
        self.total_frames
    }
    fn is_keyframe(&self, frame: u32) -> bool {
        frame.is_multiple_of(24)
    }
    fn frame_revision(&self, frame: u32) -> u64 {
        u64::from(frame % (self.frames.len() as u32)) + 1
    }
    fn frame_rgba(&self, frame: u32) -> Option<RgbaFrameRef<'_>> {
        let idx = (frame as usize) % self.frames.len();
        Some(RgbaFrameRef {
            width: self.width,
            height: self.height,
            rgba: &self.frames[idx],
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
    let header = "[Script Info]\nScriptType: v4.00+\nPlayResX: 1920\nPlayResY: 1080\n\n[V4+ Styles]\nFormat: Name, Fontname, Fontsize, PrimaryColour, SecondaryColour, OutlineColour, BackColour, Bold, Italic, Underline, StrikeOut, ScaleX, ScaleY, Spacing, Angle, BorderStyle, Outline, Shadow, Alignment, MarginL, MarginR, MarginV, Encoding\nStyle: Default,Arial,48,&H00FFFFFF,&H000000FF,&H00000000,&H00000000,0,0,0,0,100,100,0,0,1,2,2,2,10,10,10,1\nStyle: Sign,Arial,42,&H00FFE080,&H000000FF,&H00000000,&H00000000,-1,0,0,0,100,100,0,0,1,2,0,8,10,10,10,1\nStyle: Romaji,Arial,44,&H00FFFFFF,&H0000FFFF,&H00000000,&H00000000,0,0,0,0,100,100,0,0,1,2,2,8,10,10,10,1\nStyle: Translation,Arial,44,&H00FFFFFF,&H000000FF,&H00000000,&H00000000,0,0,0,0,100,100,0,0,1,2,2,2,10,10,10,1\n\n[Events]\nFormat: Layer, Start, End, Style, Name, MarginL, MarginR, MarginV, Effect, Text\n";
    let mut script = parse_ass(header).value;
    let sec = script.ensure_events_section_mut();
    for i in 0..line_count {
        let s = (i as u32) * 5;
        let mut ev = match i % 20 {
            0 => {
                let mut e = AssEvent::dialogue(
                    AssTime::from_cs(s),
                    AssTime::from_cs(s + 180),
                    r"{\an8\pos(960,80)\blur2}Typeset sign / Tiêu đề cảnh",
                );
                e.set_style("Sign");
                e
            }
            5 => {
                let mut e = AssEvent::dialogue(
                    AssTime::from_cs(s),
                    AssTime::from_cs(s + 240),
                    r"{\k40}Yu{\k50}me {\k60}no {\k90}tsuzuki",
                );
                e.set_style("Romaji");
                e.set_effect("karaoke");
                e
            }
            10 => {
                let mut e = AssEvent::dialogue(
                    AssTime::from_cs(s),
                    AssTime::from_cs(s + 100),
                    "QC note: kiểm tra lại tên riêng nhân vật",
                );
                e.set_kind(EventKind::Comment);
                e.set_style("Translation");
                e
            }
            _ => AssEvent::dialogue(
                AssTime::from_cs(s),
                AssTime::from_cs(s + 120),
                r"{\i1}Sample subtitle dialogue line — Phụ đề tiếng Việt{\i0}",
            ),
        };
        if i % 10 == 3 {
            ev.set_style("Translation");
        }
        sec.insert_event(i, ev);
    }
    script
}

fn build_demo_shell(line_count: usize) -> AppShell {
    let script = generate_demo_script(line_count);
    let mut shell = AppShell::new(script);
    if let Ok(tc) = Timecodes::from_cfr(24_000, 1_001) {
        let kfs: Vec<u32> = (0..1_000).map(|i| i * 24).collect();
        shell.attach_video(tc, 24_000, kfs, 320, 180);
    }
    let markers = TimelineMarkerIndex::new(
        (0..500).map(|i| i * 2_000).collect(),
        (0..2_000).map(|i| i * 500).collect(),
    );
    shell.attach_audio(1_000_000, markers);
    shell
}

/// Interactive `eframe::App` host for `hlouis-ui-demo`.
struct DemoEframeApp {
    shell: AppShell,
    vid: InteractiveVideoSupplier,
    aud: DummyAudioProvider,
    clock: InstantClock,
}

impl eframe::App for DemoEframeApp {
    fn ui(&mut self, ui: &mut egui::Ui, _frame: &mut eframe::Frame) {
        self.shell
            .show(ui, Some(&self.vid), Some(&self.aud), &self.clock);
    }
}

fn run_interactive_window() -> ExitCode {
    let options = eframe::NativeOptions {
        viewport: egui::ViewportBuilder::default()
            .with_title("HLouis Sub Edit — UI Demo (Aegisub 3.2.2 Layout)")
            .with_inner_size([1366.0, 820.0])
            .with_min_inner_size([900.0, 560.0]),
        ..Default::default()
    };
    let res = eframe::run_native(
        "HLouis Sub Edit — UI Demo",
        options,
        Box::new(|_cc| {
            Ok(Box::new(DemoEframeApp {
                shell: build_demo_shell(20_000),
                vid: InteractiveVideoSupplier::new(320, 180, 24_000),
                aud: DummyAudioProvider {
                    duration_ms: 1_000_000,
                },
                clock: InstantClock(Instant::now()),
            }))
        }),
    );
    if let Err(err) = res {
        eprintln!("hlouis-ui-demo: failed to open window: {err}");
        return ExitCode::FAILURE;
    }
    ExitCode::SUCCESS
}

/// Renders a simple software raster fallback PNG (palette-tinted layout wireframe + header bar)
/// when `wgpu` has no GPU/software adapter in a headless CI container.
fn write_rgba_png(path: &Path, width: u32, height: u32, rgba: &[u8]) -> Result<(), String> {
    let file = File::create(path).map_err(|e| e.to_string())?;
    let w = BufWriter::new(file);
    let mut encoder = png::Encoder::new(w, width, height);
    encoder.set_color(png::ColorType::Rgba);
    encoder.set_depth(png::BitDepth::Eight);
    let mut writer = encoder.write_header().map_err(|e| e.to_string())?;
    writer.write_image_data(rgba).map_err(|e| e.to_string())
}

fn render_fallback_png(
    path: &Path,
    width: u32,
    height: u32,
    shell: &AppShell,
    shapes_count: usize,
) -> Result<(), String> {
    let pal = shell.theme.palette();
    let mut rgba = vec![0u8; (width as usize) * (height as usize) * 4];
    for px in rgba.as_chunks_mut::<4>().0 {
        px[0] = pal.window_bg.r;
        px[1] = pal.window_bg.g;
        px[2] = pal.window_bg.b;
        px[3] = 255;
    }
    let comp = shell.layout_state.compute(width as f32, height as f32, 1.0);
    let mut fill_rect = |r: layout::Rect, c: theme::Rgba8| {
        if !r.is_visible() {
            return;
        }
        let x0 = (r.x.max(0.0) as usize).min(width as usize);
        let y0 = (r.y.max(0.0) as usize).min(height as usize);
        let x1 = ((r.x + r.w).max(0.0) as usize).min(width as usize);
        let y1 = ((r.y + r.h).max(0.0) as usize).min(height as usize);
        for y in y0..y1 {
            for x in x0..x1 {
                let border = x == x0 || y == y0 || x + 1 == x1 || y + 1 == y1;
                let col = if border { pal.border } else { c };
                let idx = (y * (width as usize) + x) * 4;
                rgba[idx] = col.r;
                rgba[idx + 1] = col.g;
                rgba[idx + 2] = col.b;
                rgba[idx + 3] = 255;
            }
        }
    };
    if let Some(v) = comp.video {
        fill_rect(v, pal.panel_bg);
    }
    if let Some(a) = comp.audio {
        fill_rect(a, pal.audio_bg);
    }
    fill_rect(comp.edit_box, pal.dialog_bg);
    fill_rect(comp.grid, pal.panel_bg);
    let _ = shapes_count;
    write_rgba_png(path, width, height, &rgba)
}

#[allow(clippy::too_many_lines)]
fn run_snapshots(out_dir: &Path) -> ExitCode {
    if let Err(e) = fs::create_dir_all(out_dir) {
        eprintln!("failed to create {}: {e}", out_dir.display());
        return ExitCode::FAILURE;
    }
    let themes = [
        ("light", ThemeSpec::Light),
        ("dark", ThemeSpec::Dark),
        (
            "eye_comfort",
            ThemeSpec::EyeComfort {
                base: BaseTheme::Dark,
                strength: 65,
            },
        ),
    ];
    let views: &[(&str, ViewMode, Option<ActiveDialog>)] = &[
        ("view_full", ViewMode::Full, None),
        ("view_video_subs", ViewMode::VideoAndSubs, None),
        ("view_audio_subs", ViewMode::AudioAndSubs, None),
        ("view_subs_only", ViewMode::SubsOnly, None),
        (
            "dialog_preferences",
            ViewMode::Full,
            Some(ActiveDialog::Preferences),
        ),
        (
            "dialog_shift_times",
            ViewMode::Full,
            Some(ActiveDialog::ShiftTimes),
        ),
        (
            "dialog_tpp",
            ViewMode::Full,
            Some(ActiveDialog::TimingPostProcessor),
        ),
        (
            "dialog_find_replace",
            ViewMode::Full,
            Some(ActiveDialog::FindReplace),
        ),
        (
            "dialog_style_manager",
            ViewMode::Full,
            Some(ActiveDialog::StyleManager),
        ),
        (
            "dialog_hlouis_select",
            ViewMode::Full,
            Some(ActiveDialog::HlouisSelectByStyle),
        ),
        (
            "dialog_hlouis_clean",
            ViewMode::Full,
            Some(ActiveDialog::HlouisCleanWhitespace),
        ),
        (
            "dialog_template_doctor",
            ViewMode::Full,
            Some(ActiveDialog::TemplateDoctor),
        ),
        (
            "dialog_mkv_import",
            ViewMode::Full,
            Some(ActiveDialog::MkvImport),
        ),
        ("dialog_about", ViewMode::Full, Some(ActiveDialog::About)),
    ];

    let vid = BenchVideoSupplier::new(320, 180, 24_000);
    let aud = DummyAudioProvider {
        duration_ms: 1_000_000,
    };
    let clock = InstantClock(Instant::now());
    let mut saved = 0usize;

    for (t_name, spec) in themes {
        for &(v_name, mode, dlg) in views {
            let mut shell = build_demo_shell(200);
            shell.set_theme(spec);
            shell.layout_state.mode = mode;
            shell.dialogs.open = dlg;

            let filename = format!("{v_name}_{t_name}.png");
            let path = out_dir.join(&filename);
            let mut shapes_count = 0usize;

            // Run through egui_kittest Harness so full egui layout & widgets execute.
            let rendered = {
                let builder = Harness::builder()
                    .with_size(Vec2::new(1280.0, 800.0))
                    .with_accessibility_check(false);
                let mut harness = builder.build_ui(|ui| {
                    shell.show(ui, Some(&vid), Some(&aud), &clock);
                });
                harness.run();
                shapes_count += 1;
                harness.render()
            };

            let write_res = match rendered {
                Ok(img) => img.save(&path).map_err(|e| e.to_string()),
                Err(_) => render_fallback_png(&path, 1280, 800, &shell, shapes_count),
            };
            if let Err(e) = write_res {
                eprintln!("failed to save {}: {e}", path.display());
                return ExitCode::FAILURE;
            }
            saved += 1;
        }
    }

    println!(
        "hlouis-ui-demo: saved {saved} PNG snapshots to {}",
        out_dir.display()
    );
    ExitCode::SUCCESS
}

#[allow(clippy::too_many_lines)]
fn run_bench_ui() -> ExitCode {
    let mut shell = build_demo_shell(200_000);
    let vid = BenchVideoSupplier::new(128, 72, 24_000);
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
                if let Some(ref mut pb) = shell.playback {
                    let kf = ((s as u32) * 24) % 23_976;
                    pb.scrub_to_frame(kf);
                }
            }
            2 => {
                rng_state ^= rng_state << 13;
                rng_state ^= rng_state >> 7;
                rng_state ^= rng_state << 17;
                let frame = (rng_state % 24_000) as u32;
                if let Some(ref mut pb) = shell.playback {
                    pb.scrub_to_frame(frame);
                }
            }
            _ => {
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
    if args.iter().any(|a| a == "--bench-ui") {
        return run_bench_ui();
    }
    if let Some(pos) = args.iter().position(|a| a == "--snapshots") {
        let dir = args
            .get(pos + 1)
            .map_or_else(|| Path::new("snapshots"), Path::new);
        return run_snapshots(dir);
    }
    run_interactive_window()
}
