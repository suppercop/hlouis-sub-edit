//! Scripted and headless UI/end-to-end benchmark runner (`--bench-ui`).

use crate::ffi;
use crate::grid::GridState;
use crate::metrics::{print_bench_table, BenchReportRow, LatencyHistogram, LatencyStats};
use crate::sample_ass;
use crate::syntax;
use crate::theme::ThemePalette;
use ass_core::{parse_ass_bytes, serialize_ass, serialize_ass_bytes, AssTime, ScriptFile};
use eframe::egui;
use egui::{Event, FontId, Modifiers, Pos2, Rect, Vec2};
use media::FfmsVideo;
use render::AssSubRenderer;
use std::fmt::Write as _;
use std::path::Path;
use std::sync::atomic::AtomicBool;
use std::time::Instant;

/// Results from scripted non-interactive UI/script/render measurements.
#[derive(Debug, Clone, Default)]
pub struct ScriptedBenchMetrics {
    pub open_20k_ms: f64,
    pub save_20k_ms: f64,
    pub rerender_20k: LatencyStats,
    pub rerender_active_only: LatencyStats,
    pub generate_200k_ms: f64,
    pub select_all_200k_ms: f64,
}

/// Optional headless video benchmark results when `--video` or `HLOUIS_TEST_VIDEO` is given.
#[derive(Debug, Clone, Default)]
pub struct VideoBenchStats {
    pub open_first_frame_ms: f64,
    pub seeks_200: LatencyStats,
    pub step_1_50: LatencyStats,
    pub next_kf_20: LatencyStats,
    pub playback_libass: LatencyStats,
    pub private_mib: f64,
}

/// Generates a synthetic 20,000-line ASS script suitable for open/save and re-render benchmarks.
#[must_use]
pub fn build_20k_ass_string() -> String {
    let mut out = String::with_capacity(2_500_000);
    out.push_str("\u{FEFF}[Script Info]\r\nTitle: Bench 20k\r\nScriptType: v4.00+\r\nPlayResX: 1920\r\nPlayResY: 1080\r\n\r\n");
    out.push_str("[V4+ Styles]\r\nFormat: Name, Fontname, Fontsize, PrimaryColour, SecondaryColour, OutlineColour, BackColour, Bold, Italic, Underline, StrikeOut, ScaleX, ScaleY, Spacing, Angle, BorderStyle, Outline, Shadow, Alignment, MarginL, MarginR, MarginV, Encoding\r\n");
    out.push_str("Style: Default,Segoe UI,54,&H00FFFFFF,&H0000FFFF,&H00181820,&H90000000,-1,0,0,0,100,100,0,0,1,3,1.5,2,40,40,36,1\r\n\r\n");
    out.push_str("[Events]\r\nFormat: Layer, Start, End, Style, Name, MarginL, MarginR, MarginV, Effect, Text\r\n");

    let templates = [
        r"{\an8\fs44\c&H00FFFF&}Xin chào Tiếng Việt\N{\k30}Ka{\kf45}raoke — 日本語テスト",
        r"{\pos(960,980)\fad(150,150)}日本語の字幕テスト：漢字・ひらがな・カタカナ",
        r"{\an5\frz-12\t(0,400,\fscx115\fscy115)}Styled sign\hwith\hnon-breaking spaces",
        "Plain dialogue line with punctuation and normal text.",
    ];

    for i in 0..20_000u32 {
        let start = if i == 0 {
            AssTime::ZERO
        } else {
            AssTime::from_cs(i * 150)
        };
        let end = if i == 0 {
            AssTime::from_cs(1_000)
        } else {
            start.saturating_add_cs(140)
        };
        let text = templates[(i as usize) % templates.len()];
        let _ = writeln!(
            out,
            "Dialogue: 0,{start},{end},Default,Speaker,0,0,0,,{text} [#{i}]\r"
        );
    }
    out
}

/// Measures disk read + charset + parse + grid build and serialize + disk write on 20,000 lines.
pub fn bench_open_save_20k_ass() -> anyhow::Result<(f64, f64, ScriptFile)> {
    let raw = build_20k_ass_string();
    let dir = std::env::temp_dir().join(format!("hlouis_bench_ui_{}", std::process::id()));
    std::fs::create_dir_all(&dir)?;
    let in_path = dir.join("bench_20k.ass");
    let out_path = dir.join("bench_20k_save.ass");
    std::fs::write(&in_path, raw.as_bytes())?;

    let t_open = Instant::now();
    let read_bytes = std::fs::read(&in_path)?;
    let (outcome, _enc) = parse_ass_bytes(&read_bytes);
    let grid = GridState::from_script(&outcome.value);
    let open_ms = t_open.elapsed().as_secs_f64() * 1000.0;
    std::hint::black_box(grid.lines.len());

    let t_save = Instant::now();
    let out_bytes = serialize_ass_bytes(&outcome.value);
    std::fs::write(&out_path, &out_bytes)?;
    let save_ms = t_save.elapsed().as_secs_f64() * 1000.0;

    let _ = std::fs::remove_file(&in_path);
    let _ = std::fs::remove_file(&out_path);
    let _ = std::fs::remove_dir(&dir);

    Ok((open_ms, save_ms, outcome.value))
}

/// Benchmarks subtitle re-render latency after an edit on both a 20k script and single active line.
pub fn bench_subtitle_rerender(
    script_20k: &mut ScriptFile,
) -> anyhow::Result<(LatencyStats, LatencyStats)> {
    let clean_rgba = vec![64u8; 1920 * 1080 * 4];
    let mut work_rgba = vec![0u8; clean_rgba.len()];
    let mut renderer = AssSubRenderer::new(1920, 1080)?;

    // Warm font provider on frame 0 once.
    renderer.load_ass_memory(&serialize_ass(script_20k))?;
    work_rgba.copy_from_slice(&clean_rgba);
    let _ = renderer.render_and_blend_rgba(500, &mut work_rgba, 1920, 1080)?;

    let mut full_hist = LatencyHistogram::new();
    for i in 0..25u32 {
        if let Some(ev) = script_20k
            .events_section_mut()
            .and_then(|s| s.get_event_mut(0))
        {
            ev.set_text(format!(
                r"{{\an8\fs44\c&H00FFFF&}}Edited subtitle line #{i} — Tiếng Việt"
            ));
        }
        let t = Instant::now();
        let ass_str = serialize_ass(script_20k);
        renderer.load_ass_memory(&ass_str)?;
        work_rgba.copy_from_slice(&clean_rgba);
        let _ = renderer.render_and_blend_rgba(500, &mut work_rgba, 1920, 1080)?;
        full_hist.record(t.elapsed().as_secs_f64() * 1000.0);
    }

    let sample_ass = sample_ass::build_sample_ass(1920, 1080);
    renderer.load_ass_memory(&sample_ass)?;
    let _ = renderer.render_and_blend_rgba(500, &mut work_rgba, 1920, 1080)?;
    let mut active_hist = LatencyHistogram::new();
    for i in 0..40u32 {
        let mut single = String::with_capacity(sample_ass.len() + 64);
        single.push_str(&sample_ass);
        let _ = writeln!(
            single,
            "Dialogue: 0,0:00:00.00,0:00:10.00,Default,,0,0,0,,Edit #{i} — Tiếng Việt"
        );
        let t = Instant::now();
        renderer.load_ass_memory(&single)?;
        work_rgba.copy_from_slice(&clean_rgba);
        let _ = renderer.render_and_blend_rgba(500, &mut work_rgba, 1920, 1080)?;
        active_hist.record(t.elapsed().as_secs_f64() * 1000.0);
    }

    Ok((full_hist.summary(), active_hist.summary()))
}

/// Benchmarks video open, 200 deterministic random seeks, 50 +-1 steps, and 20 next-keyframe seeks.
pub fn bench_video_headless(path: &Path) -> anyhow::Result<VideoBenchStats> {
    let cancel = AtomicBool::new(false);
    let t_open = Instant::now();
    let mut video = FfmsVideo::open_with_progress(path, &cancel, |_, _| {})?;
    let info = video.info().clone();
    let mut rgba = Vec::new();
    let pts0 = video.decode_frame_into(0, &mut rgba)?;
    let mut renderer = AssSubRenderer::new(info.width, info.height)?;
    renderer.load_ass_memory(&sample_ass::build_sample_ass(info.width, info.height))?;
    let _ = renderer.render_and_blend_rgba(pts0, &mut rgba, info.width, info.height)?;
    let open_first_frame_ms = t_open.elapsed().as_secs_f64() * 1000.0;

    let mut seeks_200 = LatencyHistogram::new();
    let mut state = 0x9E37_79B9_7F4A_7C15_u64;
    let total = info.num_frames.max(1);
    for _ in 0..200 {
        state ^= state << 13;
        state ^= state >> 7;
        state ^= state << 17;
        #[allow(clippy::cast_possible_truncation)]
        let target = (state as usize) % total;
        let t = Instant::now();
        let pts = video.decode_frame_into(target, &mut rgba)?;
        let _ = renderer.render_and_blend_rgba(pts, &mut rgba, info.width, info.height)?;
        seeks_200.record(t.elapsed().as_secs_f64() * 1000.0);
    }

    let mut step_1_50 = LatencyHistogram::new();
    let mid = total / 2;
    let _ = video.decode_frame_into(mid, &mut rgba)?;
    for i in 1..=50 {
        let target = if i <= 25 {
            mid + i
        } else {
            (mid + 25).saturating_sub(i - 25)
        }
        .min(total - 1);
        let t = Instant::now();
        let pts = video.decode_frame_into(target, &mut rgba)?;
        let _ = renderer.render_and_blend_rgba(pts, &mut rgba, info.width, info.height)?;
        step_1_50.record(t.elapsed().as_secs_f64() * 1000.0);
    }

    let mut next_kf_20 = LatencyHistogram::new();
    let mut cur_kf = 0usize;
    for _ in 0..20 {
        let next = video.next_keyframe(cur_kf).unwrap_or(0);
        let t = Instant::now();
        let pts = video.decode_frame_into(next, &mut rgba)?;
        let _ = renderer.render_and_blend_rgba(pts, &mut rgba, info.width, info.height)?;
        next_kf_20.record(t.elapsed().as_secs_f64() * 1000.0);
        cur_kf = next;
    }

    let mut playback_libass = LatencyHistogram::new();
    let sample_frames = total.min(120);
    for idx in 0..sample_frames {
        let pts = video.decode_frame_into(idx, &mut rgba)?;
        let t = Instant::now();
        let _ = renderer.render_and_blend_rgba(pts, &mut rgba, info.width, info.height)?;
        playback_libass.record(t.elapsed().as_secs_f64() * 1000.0);
    }

    let private_mib = ffi::process_memory_mib().map_or(0.0, |m| m.private_mib);
    Ok(VideoBenchStats {
        open_first_frame_ms,
        seeks_200: seeks_200.summary(),
        step_1_50: step_1_50.summary(),
        next_kf_20: next_kf_20.summary(),
        playback_libass: playback_libass.summary(),
        private_mib,
    })
}

/// Runs all UI, ASS, syntax, render, and optional video benchmarks in `--bench-ui --headless` mode.
pub fn run_headless_bench(video_path: Option<&Path>) -> anyhow::Result<()> {
    let t_gen = Instant::now();
    let mut grid = GridState::generate(200_000);
    let gen_ms = t_gen.elapsed().as_secs_f64() * 1000.0;

    let t_sel = Instant::now();
    grid.select_all();
    let select_all_ms = t_sel.elapsed().as_secs_f64() * 1000.0;

    let scroll_stats = bench_headless_scroll_200k(&mut grid);
    let keystroke_stats = bench_headless_edit_keystrokes();

    let (open_20k_ms, save_20k_ms, mut script_20k) = bench_open_save_20k_ass()?;
    let (rerender_20k, rerender_single) = bench_subtitle_rerender(&mut script_20k)?;
    let mem_no_video = ffi::process_memory_mib().map_or(0.0, |m| m.private_mib);

    let video_stats = match video_path {
        Some(p) => Some(bench_video_headless(p)?),
        None => None,
    };

    let mut rows = vec![
        row_ms(
            "Cold start to usable window",
            300.0,
            None,
            "headless skipped",
        ),
        row_limit("Open 20k ASS (charset + I/O + grid)", 40.0, open_20k_ms),
        row_limit("Save 20k ASS (serialize + I/O)", 25.0, save_20k_ms),
        BenchReportRow {
            area: "Grid 200k scroll frame time".to_owned(),
            budget: "worst < 8.0 ms (120 Hz)".to_owned(),
            measured: format!(
                "p50 {:.2} / p95 {:.2} / worst {:.2} ms",
                scroll_stats.p50, scroll_stats.p95, scroll_stats.worst
            ),
            status: pass_fail(scroll_stats.worst < 8.0),
        },
        row_limit("Grid 200k select-all", 10.0, select_all_ms),
        BenchReportRow {
            area: "Grid 200k generate".to_owned(),
            budget: "info".to_owned(),
            measured: format!("{gen_ms:.2} ms"),
            status: "INFO",
        },
        BenchReportRow {
            area: "Keystroke in EditBox (headless layout)".to_owned(),
            budget: "< 8.0 ms".to_owned(),
            measured: format!(
                "p50 {:.2} / p95 {:.2} / worst {:.2} ms",
                keystroke_stats.p50, keystroke_stats.p95, keystroke_stats.worst
            ),
            status: pass_fail(keystroke_stats.p95 < 8.0),
        },
        BenchReportRow {
            area: "Subtitle re-render 20k full reload".to_owned(),
            budget: "< 8.0 ms".to_owned(),
            measured: format!(
                "p50 {:.2} / p95 {:.2} / worst {:.2} ms",
                rerender_20k.p50, rerender_20k.p95, rerender_20k.worst
            ),
            status: pass_fail(rerender_20k.p95 < 8.0),
        },
        BenchReportRow {
            area: "Subtitle re-render single active track".to_owned(),
            budget: "< 8.0 ms".to_owned(),
            measured: format!(
                "p50 {:.2} / p95 {:.2} / worst {:.2} ms",
                rerender_single.p50, rerender_single.p95, rerender_single.worst
            ),
            status: pass_fail(rerender_single.p95 < 8.0),
        },
        BenchReportRow {
            area: "Private memory (no video, 200k grid)".to_owned(),
            budget: "< 400 MB".to_owned(),
            measured: format!("{mem_no_video:.1} MiB"),
            status: pass_fail(mem_no_video < 400.0),
        },
    ];

    if let Some(vs) = video_stats {
        append_video_rows(&mut rows, &vs);
    }
    print_bench_table("headless", &rows);
    Ok(())
}

/// Appends formatted video measurement rows to a report table.
pub fn append_video_rows(rows: &mut Vec<BenchReportRow>, vs: &VideoBenchStats) {
    rows.push(row_limit(
        "Open video -> first frame",
        500.0,
        vs.open_first_frame_ms,
    ));
    rows.push(BenchReportRow {
        area: "Seek any frame (200 random)".to_owned(),
        budget: "p95 < 60.0 / worst < 150.0 ms".to_owned(),
        measured: format!(
            "p50 {:.2} / p95 {:.2} / worst {:.2} ms",
            vs.seeks_200.p50, vs.seeks_200.p95, vs.seeks_200.worst
        ),
        status: pass_fail(vs.seeks_200.p95 < 60.0 && vs.seeks_200.worst < 150.0),
    });
    rows.push(BenchReportRow {
        area: "Step +-1 frame (50 steps)".to_owned(),
        budget: "p95 < 10.0 ms".to_owned(),
        measured: format!(
            "p50 {:.2} / p95 {:.2} / worst {:.2} ms",
            vs.step_1_50.p50, vs.step_1_50.p95, vs.step_1_50.worst
        ),
        status: pass_fail(vs.step_1_50.p95 < 10.0),
    });
    rows.push(BenchReportRow {
        area: "Seek next keyframe (20 jumps)".to_owned(),
        budget: "p95 < 30.0 ms".to_owned(),
        measured: format!(
            "p50 {:.2} / p95 {:.2} / worst {:.2} ms",
            vs.next_kf_20.p50, vs.next_kf_20.p95, vs.next_kf_20.worst
        ),
        status: pass_fail(vs.next_kf_20.p95 < 30.0),
    });
    rows.push(BenchReportRow {
        area: "libass render + blend 1080p".to_owned(),
        budget: "p99 < 2.0 ms".to_owned(),
        measured: format!(
            "p50 {:.2} / p99 {:.2} / worst {:.2} ms",
            vs.playback_libass.p50, vs.playback_libass.p99, vs.playback_libass.worst
        ),
        status: pass_fail(vs.playback_libass.p99 < 2.0),
    });
    rows.push(BenchReportRow {
        area: "Private memory (video loaded, no audio)".to_owned(),
        budget: "< 400 MB".to_owned(),
        measured: format!("{:.1} MiB", vs.private_mib),
        status: pass_fail(vs.private_mib < 400.0),
    });
}

fn bench_headless_scroll_200k(grid: &mut GridState) -> LatencyStats {
    let ctx = egui::Context::default();
    let palette = ThemePalette::dark();
    let headers = ["#", "Start", "End", "CPS", "Style", "Text"];
    let mut hist = LatencyHistogram::new();

    for frame_idx in 0..125usize {
        let t = Instant::now();
        let dy = if frame_idx % 2 == 0 { -180.0 } else { -120.0 };
        let raw = egui::RawInput {
            screen_rect: Some(Rect::from_min_max(Pos2::ZERO, Pos2::new(960.0, 600.0))),
            events: if frame_idx > 0 {
                vec![
                    Event::PointerMoved(Pos2::new(400.0, 300.0)),
                    Event::MouseWheel {
                        unit: egui::MouseWheelUnit::Point,
                        delta: Vec2::new(0.0, dy),
                        phase: egui::TouchPhase::Move,
                        source: egui::MouseWheelSource::Unknown,
                        modifiers: Modifiers::NONE,
                    },
                ]
            } else {
                Vec::new()
            },
            ..Default::default()
        };
        let mut out = ctx.run_ui(raw, |ui| {
            crate::render_grid_table(ui, grid, &palette, headers);
        });
        out.textures_delta.clear();
        if frame_idx >= 5 {
            hist.record(t.elapsed().as_secs_f64() * 1000.0);
        }
    }
    hist.summary()
}

fn bench_headless_edit_keystrokes() -> LatencyStats {
    let ctx = egui::Context::default();
    let palette = ThemePalette::dark();
    let mut buf = r"{\an8\fs44}Hello ".to_owned();
    let mut scratch = Vec::with_capacity(32);
    let mut hist = LatencyHistogram::new();

    // Prime focus and fonts first.
    for pass in 0..2 {
        let raw = egui::RawInput {
            screen_rect: Some(Rect::from_min_max(Pos2::ZERO, Pos2::new(800.0, 300.0))),
            events: if pass == 1 {
                vec![
                    Event::PointerButton {
                        pos: Pos2::new(40.0, 40.0),
                        button: egui::PointerButton::Primary,
                        pressed: true,
                        modifiers: Modifiers::NONE,
                    },
                    Event::PointerButton {
                        pos: Pos2::new(40.0, 40.0),
                        button: egui::PointerButton::Primary,
                        pressed: false,
                        modifiers: Modifiers::NONE,
                    },
                ]
            } else {
                Vec::new()
            },
            ..Default::default()
        };
        let mut out = ctx.run_ui(raw, |ui| {
            run_bench_edit_widget(ui, &mut buf, &palette, &mut scratch);
        });
        out.textures_delta.clear();
    }

    for i in 0..200u32 {
        let ev = if i % 5 == 4 {
            Event::Key {
                key: egui::Key::Backspace,
                physical_key: None,
                pressed: true,
                repeat: false,
                modifiers: Modifiers::NONE,
            }
        } else {
            let ch = char::from(b'a' + u8::try_from(i % 26).unwrap_or(0));
            let mut s = [0u8; 4];
            Event::Text(ch.encode_utf8(&mut s).to_owned())
        };
        let t = Instant::now();
        let raw = egui::RawInput {
            screen_rect: Some(Rect::from_min_max(Pos2::ZERO, Pos2::new(800.0, 300.0))),
            events: vec![ev],
            ..Default::default()
        };
        let mut out = ctx.run_ui(raw, |ui| {
            run_bench_edit_widget(ui, &mut buf, &palette, &mut scratch);
        });
        out.textures_delta.clear();
        hist.record(t.elapsed().as_secs_f64() * 1000.0);
    }

    hist.summary()
}

fn run_bench_edit_widget(
    ui: &mut egui::Ui,
    buf: &mut String,
    palette: &ThemePalette,
    scratch: &mut Vec<syntax::HighlightSpan>,
) {
    let mono = FontId::monospace(15.0);
    let mut layouter = |ui: &egui::Ui, text: &dyn egui::TextBuffer, wrap_width: f32| {
        let job = syntax::build_layout_job_with_scratch(
            text.as_str(),
            &mono,
            wrap_width,
            palette,
            scratch,
        );
        ui.fonts_mut(|f| f.layout_job(job))
    };
    let resp = ui.add(
        egui::TextEdit::multiline(buf)
            .desired_width(720.0)
            .desired_rows(4)
            .layouter(&mut layouter),
    );
    if !resp.has_focus() {
        resp.request_focus();
    }
}

/// Formats a budget-checked latency row in milliseconds.
#[must_use]
pub fn row_limit(name: &str, budget_ms: f64, measured_ms: f64) -> BenchReportRow {
    BenchReportRow {
        area: name.to_owned(),
        budget: format!("< {budget_ms:.1} ms"),
        measured: format!("{measured_ms:.2} ms"),
        status: pass_fail(measured_ms < budget_ms),
    }
}

/// Formats an optional latency row cleanly.
#[must_use]
pub fn row_ms(
    name: &str,
    budget_ms: f64,
    measured_ms: Option<f64>,
    fallback: &str,
) -> BenchReportRow {
    match measured_ms {
        Some(v) => row_limit(name, budget_ms, v),
        None => BenchReportRow {
            area: name.to_owned(),
            budget: format!("< {budget_ms:.1} ms"),
            measured: fallback.to_owned(),
            status: "SKIP",
        },
    }
}

/// Returns `"PASS"` when `ok` is true and `"MISS"` otherwise.
#[must_use]
pub const fn pass_fail(ok: bool) -> &'static str {
    if ok {
        "PASS"
    } else {
        "MISS"
    }
}

/// Complete telemetry bundle collected by the real `eframe`/`wgpu` windowed `--bench-ui` driver.
#[derive(Debug, Clone, Default)]
pub struct WindowedBenchSummary {
    pub cold_start_ui_ms: f64,
    pub cold_start_present_ms: f64,
    pub open_20k_ms: f64,
    pub save_20k_ms: f64,
    pub generate_200k_ms: f64,
    pub scroll_cpu: LatencyStats,
    pub scroll_present: LatencyStats,
    pub select_all_200k_ms: f64,
    pub keystroke_present: LatencyStats,
    pub rerender_20k: LatencyStats,
    pub rerender_single: LatencyStats,
    pub idle_repaints_per_sec: f64,
    pub idle_cpu_pct: f64,
    pub private_mib_no_video: f64,
    pub video: Option<WindowedVideoSummary>,
}

/// Video portion collected through the real worker + `wgpu` swapchain during windowed `--bench-ui`.
#[derive(Debug, Clone, Default)]
pub struct WindowedVideoSummary {
    pub open_first_frame_ms: f64,
    pub seeks_200: LatencyStats,
    pub step_1_50: LatencyStats,
    pub next_kf_20: LatencyStats,
    pub libass_playback: LatencyStats,
    pub upload_playback: LatencyStats,
    pub dropped_frames: u64,
    pub rendered_frames: u64,
    pub private_mib_with_video: f64,
}

fn row_hist(area: &str, budget: &str, stats: LatencyStats, pass: bool) -> BenchReportRow {
    BenchReportRow {
        area: area.to_owned(),
        budget: budget.to_owned(),
        measured: format!(
            "p50 {:.2} / p95 {:.2} / worst {:.2} ms",
            stats.p50, stats.p95, stats.worst
        ),
        status: pass_fail(pass),
    }
}

/// Prints the windowed `--bench-ui` report table against all tightened `AGENTS.md` budgets.
pub fn print_windowed_report(s: &WindowedBenchSummary) {
    let mut rows = vec![
        BenchReportRow {
            area: "Cold start to usable window".to_owned(),
            budget: "< 300.0 ms".to_owned(),
            measured: format!(
                "{:.2} ms present (UI init {:.2} ms)",
                s.cold_start_present_ms, s.cold_start_ui_ms
            ),
            status: pass_fail(s.cold_start_present_ms < 300.0),
        },
        row_limit("Open 20k ASS (charset + I/O + grid)", 40.0, s.open_20k_ms),
        row_limit("Save 20k ASS (serialize + I/O)", 25.0, s.save_20k_ms),
        row_hist(
            "Grid 200k scroll CPU frame time",
            "worst < 8.0 ms (120 Hz)",
            s.scroll_cpu,
            s.scroll_cpu.worst < 8.0,
        ),
        BenchReportRow {
            area: "Grid 200k scroll present interval".to_owned(),
            budget: "vsync / <= 120 Hz".to_owned(),
            measured: format!(
                "p50 {:.2} / p95 {:.2} / worst {:.2} ms",
                s.scroll_present.p50, s.scroll_present.p95, s.scroll_present.worst
            ),
            status: "INFO",
        },
        row_limit("Grid 200k select-all", 10.0, s.select_all_200k_ms),
        BenchReportRow {
            area: "Grid 200k generate".to_owned(),
            budget: "info".to_owned(),
            measured: format!("{:.2} ms", s.generate_200k_ms),
            status: "INFO",
        },
        row_hist(
            "Keystroke to screen in EditBox",
            "p95 < 8.0 ms",
            s.keystroke_present,
            s.keystroke_present.p95 < 8.0,
        ),
        row_hist(
            "Subtitle re-render 20k full reload",
            "< 8.0 ms",
            s.rerender_20k,
            s.rerender_20k.p95 < 8.0,
        ),
        row_hist(
            "Subtitle re-render single active track",
            "< 8.0 ms",
            s.rerender_single,
            s.rerender_single.p95 < 8.0,
        ),
        BenchReportRow {
            area: "Idle paused (no input)".to_owned(),
            budget: "CPU < 0.2%, 0 repaints/s".to_owned(),
            measured: format!(
                "{:.2} repaints/s, {:.3}% core CPU",
                s.idle_repaints_per_sec, s.idle_cpu_pct
            ),
            status: pass_fail(s.idle_repaints_per_sec < 0.1 && s.idle_cpu_pct < 0.2),
        },
        BenchReportRow {
            area: "Private memory (no video)".to_owned(),
            budget: "< 400 MB".to_owned(),
            measured: format!("{:.1} MiB", s.private_mib_no_video),
            status: pass_fail(s.private_mib_no_video < 400.0),
        },
    ];

    if let Some(vs) = &s.video {
        append_video_rows(
            &mut rows,
            &VideoBenchStats {
                open_first_frame_ms: vs.open_first_frame_ms,
                seeks_200: vs.seeks_200,
                step_1_50: vs.step_1_50,
                next_kf_20: vs.next_kf_20,
                playback_libass: vs.libass_playback,
                private_mib: vs.private_mib_with_video,
            },
        );
        rows.push(BenchReportRow {
            area: "wgpu RGBA upload (1080p playback)".to_owned(),
            budget: "p99 < 2.0 ms".to_owned(),
            measured: format!(
                "p50 {:.2} / p99 {:.2} / worst {:.2} ms",
                vs.upload_playback.p50, vs.upload_playback.p99, vs.upload_playback.worst
            ),
            status: pass_fail(vs.upload_playback.p99 < 2.0),
        });
        rows.push(BenchReportRow {
            area: "Playback dropped frames".to_owned(),
            budget: "0 dropped".to_owned(),
            measured: format!("{} / {}", vs.dropped_frames, vs.rendered_frames),
            status: pass_fail(vs.dropped_frames == 0),
        });
    }

    print_bench_table("windowed wgpu", &rows);
}
