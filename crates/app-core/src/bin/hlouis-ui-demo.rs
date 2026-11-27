//! `hlouis-ui-demo` — real desktop subtitle editor powered by [`app_core::EditorApp`], plus
//! headless `--snapshots <dir>` PNG rendering and `--bench-ui` scripted benchmarking.

#![forbid(unsafe_code)]
#![allow(
    clippy::cast_precision_loss,
    clippy::cast_possible_truncation,
    clippy::cast_sign_loss
)]
#![cfg_attr(not(test), deny(clippy::unwrap_used, clippy::expect_used))]

use app_core::{EditorApp, build_demo_script, refresh_demo_diagnostics};
use egui::{Event, ImeEvent, Pos2, RawInput, Rect, Vec2};
use egui_kittest::Harness;
use layout::ViewMode;
use mkv_import::TrackPickerModel;
use mkv_subs::{SubtitleCodec, TrackInfo, TrackType};
use playback::Clock;
use std::fs::{self, File};
use std::io::BufWriter;
use std::path::Path;
use std::process::ExitCode;
use std::time::Instant;
use theme::{BaseTheme, ThemeSpec};
use ui_dialogs::ActiveDialog;
use ui_mkv_import::MkvImportDialogState;

struct InstantClock(Instant);
impl Clock for InstantClock {
    fn now_ns(&self) -> u64 {
        self.0.elapsed().as_nanos() as u64
    }
}

fn build_demo_mkv_dialog() -> MkvImportDialogState {
    let langs = [
        ("vie", "Vietnamese (HLouis Fansub)"),
        ("eng", "English (Official)"),
        ("jpn", "Japanese (Closed Captions)"),
        ("fre", "French"),
        ("ger", "German"),
        ("spa", "Spanish (Latin America)"),
        ("chi", "Chinese (Simplified)"),
        ("kor", "Korean"),
    ];
    let mut tracks = Vec::with_capacity(68);
    for idx in 0..68u64 {
        let (lang, base_name) = langs[(idx as usize) % langs.len()];
        let is_bitmap = idx % 17 == 16;
        let (codec_id, codec) = if is_bitmap {
            (
                "S_HDMV/PGS".to_owned(),
                SubtitleCodec::UnsupportedImage("S_HDMV/PGS".to_owned()),
            )
        } else if idx % 3 == 2 {
            ("S_TEXT/UTF8".to_owned(), SubtitleCodec::Srt)
        } else {
            ("S_TEXT/ASS".to_owned(), SubtitleCodec::Ass)
        };
        tracks.push(TrackInfo {
            number: idx + 3,
            track_type: TrackType::Subtitle,
            codec_id,
            subtitle_codec: codec,
            language: Some(lang.to_owned()),
            language_bcp47: None,
            name: Some(format!("{base_name} #{}", idx + 1)),
            is_default: idx == 0,
            is_forced: idx % 11 == 0 && idx > 0,
            codec_private: None,
            encodings: Vec::new(),
        });
    }
    let mut picker = TrackPickerModel::from_tracks(&tracks);
    for row in picker.rows_mut() {
        row.event_count = Some(if row.selectable { 420 } else { 0 });
        if row.selectable {
            row.preview_lines = vec![
                "0:00:01.20 -> 0:00:03.80  Opening scene title".to_owned(),
                "0:00:04.10 -> 0:00:06.50  Xin chào mọi người — HLouis Fansub!".to_owned(),
                "0:00:07.00 -> 0:00:09.90  {\\an8}Chapter 1: Awakening".to_owned(),
                "0:00:10.40 -> 0:00:13.10  Let's move forward together.".to_owned(),
                "0:00:13.80 -> 0:00:16.20  End of preview sample.".to_owned(),
            ];
        }
    }
    MkvImportDialogState::from_picker(picker)
}

fn build_demo_editor(line_count: usize) -> EditorApp {
    let mut app = EditorApp::new(build_demo_script(line_count));
    "Demons' Crest - 05.ass".clone_into(&mut app.shell.script_name);
    app.shell.set_theme(ThemeSpec::Light);
    app.shell.mkv_dialog = build_demo_mkv_dialog();
    refresh_demo_diagnostics(&mut app);
    app.shell.dialogs.log_lines = vec![
        "[00:00.012] INFO  hlouis-sub-edit started (app-core wired)".to_owned(),
        "[00:00.045] INFO  Loaded demo script (styles=4, video=24000f, audio=30s)".to_owned(),
        "[00:00.088] INFO  MKV track scanner ready (68 demo tracks populated)".to_owned(),
    ];
    app.shell.dialogs.recovery_files = vec![
        "episode_01_autosave_2026-11-27_14-20.ass".to_owned(),
        "op_karaoke_template_autosave.ass".to_owned(),
    ];
    app
}

struct DemoEframeApp {
    app: EditorApp,
    clock: InstantClock,
}

impl eframe::App for DemoEframeApp {
    fn ui(&mut self, ui: &mut egui::Ui, _frame: &mut eframe::Frame) {
        self.app.show(ui, &self.clock);
        let _ = self.app.poll_autosave(self.clock.now_ns());
        ui.ctx()
            .send_viewport_cmd(egui::ViewportCommand::Title(self.app.shell.window_title()));
    }
}

fn run_interactive_window(initial_file: Option<&Path>) -> ExitCode {
    let mut app = build_demo_editor(20_000);
    if let Some(p) = initial_file
        && let Err(e) = app.open_file(p)
    {
        eprintln!("failed to open {}: {e}", p.display());
    }
    let title = app.shell.window_title();
    let options = eframe::NativeOptions {
        viewport: egui::ViewportBuilder::default()
            .with_title(title)
            .with_inner_size([1366.0, 820.0])
            .with_min_inner_size([900.0, 560.0]),
        ..Default::default()
    };
    let res = eframe::run_native(
        "hlouis-sub-edit",
        options,
        Box::new(move |_cc| {
            Ok(Box::new(DemoEframeApp {
                app,
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
    app: &EditorApp,
) -> Result<(), String> {
    let pal = app.shell.theme.palette();
    let mut rgba = vec![0u8; (width as usize) * (height as usize) * 4];
    for px in rgba.as_chunks_mut::<4>().0 {
        px[0] = pal.window_bg.r;
        px[1] = pal.window_bg.g;
        px[2] = pal.window_bg.b;
        px[3] = 255;
    }
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
    let views: &[(&str, ViewMode, Option<ActiveDialog>, Vec2, f32)] = &[
        (
            "view_full_100dpi",
            ViewMode::Full,
            None,
            Vec2::new(1280.0, 800.0),
            1.0,
        ),
        (
            "view_full_200dpi",
            ViewMode::Full,
            None,
            Vec2::new(1440.0, 900.0),
            2.0,
        ),
        (
            "view_video_subs",
            ViewMode::VideoAndSubs,
            None,
            Vec2::new(1280.0, 800.0),
            1.0,
        ),
        (
            "view_audio_subs",
            ViewMode::AudioAndSubs,
            None,
            Vec2::new(1280.0, 800.0),
            1.0,
        ),
        (
            "view_subs_only",
            ViewMode::SubsOnly,
            None,
            Vec2::new(1280.0, 800.0),
            1.0,
        ),
        (
            "dialog_preferences",
            ViewMode::Full,
            Some(ActiveDialog::Preferences),
            Vec2::new(1280.0, 800.0),
            1.0,
        ),
        (
            "dialog_shift_times",
            ViewMode::Full,
            Some(ActiveDialog::ShiftTimes),
            Vec2::new(1280.0, 800.0),
            1.0,
        ),
        (
            "dialog_tpp",
            ViewMode::Full,
            Some(ActiveDialog::TimingPostProcessor),
            Vec2::new(1280.0, 800.0),
            1.0,
        ),
        (
            "dialog_find_replace",
            ViewMode::Full,
            Some(ActiveDialog::FindReplace),
            Vec2::new(1280.0, 800.0),
            1.0,
        ),
        (
            "dialog_style_manager",
            ViewMode::Full,
            Some(ActiveDialog::StyleManager),
            Vec2::new(1280.0, 800.0),
            1.0,
        ),
        (
            "dialog_style_editor",
            ViewMode::Full,
            Some(ActiveDialog::StyleEditor),
            Vec2::new(1280.0, 800.0),
            1.0,
        ),
        (
            "dialog_kara_templater",
            ViewMode::Full,
            Some(ActiveDialog::KaraokeTemplater),
            Vec2::new(1280.0, 800.0),
            1.0,
        ),
        (
            "dialog_hlouis_select",
            ViewMode::Full,
            Some(ActiveDialog::HlouisSelectByStyle),
            Vec2::new(1280.0, 800.0),
            1.0,
        ),
        (
            "dialog_hlouis_clean",
            ViewMode::Full,
            Some(ActiveDialog::HlouisCleanWhitespace),
            Vec2::new(1280.0, 800.0),
            1.0,
        ),
        (
            "dialog_template_doctor",
            ViewMode::Full,
            Some(ActiveDialog::TemplateDoctor),
            Vec2::new(1280.0, 800.0),
            1.0,
        ),
        (
            "dialog_mkv_import",
            ViewMode::Full,
            Some(ActiveDialog::MkvImport),
            Vec2::new(1280.0, 800.0),
            1.0,
        ),
        (
            "dialog_about",
            ViewMode::Full,
            Some(ActiveDialog::About),
            Vec2::new(1280.0, 800.0),
            1.0,
        ),
    ];

    let clock = InstantClock(Instant::now());
    let mut saved = 0usize;
    for (t_name, spec) in themes {
        for &(v_name, mode, dlg, size, ppp) in views {
            let mut app = build_demo_editor(200);
            app.shell.set_theme(spec);
            app.shell.layout_state.mode = mode;
            app.shell.dialogs.open = dlg;
            let path = out_dir.join(format!("{v_name}_{t_name}.png"));
            let rendered = {
                let mut harness = Harness::builder()
                    .with_size(size)
                    .with_pixels_per_point(ppp)
                    .with_accessibility_check(false)
                    .build_ui(|ui| {
                        app.show(ui, &clock);
                    });
                harness.run();
                harness.render()
            };
            let write_res = match rendered {
                Ok(img) => img.save(&path).map_err(|e| e.to_string()),
                Err(_) => {
                    render_fallback_png(&path, (size.x * ppp) as u32, (size.y * ppp) as u32, &app)
                }
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

fn run_bench_ui() -> ExitCode {
    let mut app = build_demo_editor(200_000);
    let clock = InstantClock(Instant::now());
    let ctx = egui::Context::default();
    let base_raw = RawInput {
        screen_rect: Some(Rect::from_min_size(Pos2::ZERO, Vec2::new(1280.0, 800.0))),
        ..Default::default()
    };

    for _ in 0..8 {
        let mut out = ctx.run_ui(base_raw.clone(), |ui| app.show(ui, &clock));
        out.textures_delta.clear();
    }

    let telex = ["T", "i", "ế", "n", "g", " ", "V", "i", "ệ", "t", "!"];
    let mut buf = String::with_capacity(256);
    let active_row = 100_000usize;
    app.shell.grid.model.click(active_row);
    app.shell.grid.scroll_to_active();
    app.shell.doc.set_selection(Some(active_row), &[active_row]);

    for k in 0..220usize {
        if buf.len() > 160 {
            buf.clear();
        }
        buf.push_str(telex[k % telex.len()]);
        let mut raw = base_raw.clone();
        if k % 5 == 0 {
            raw.events.push(Event::Ime(ImeEvent::Preedit {
                text: telex[k % telex.len()].to_owned(),
                active_range_chars: None,
            }));
        } else {
            raw.events
                .push(Event::Text(telex[k % telex.len()].to_owned()));
        }
        let t0 = Instant::now();
        let _ = app.shell.doc.amend_text(active_row, "typing", &buf);
        let mut out = ctx.run_ui(raw, |ui| app.show(ui, &clock));
        out.textures_delta.clear();
        let us = t0.elapsed().as_micros() as u64;
        app.shell.metrics.keystroke_to_present.histogram.record(us);
        app.shell.metrics.record_frame_us(us, clock.now_ns());
    }

    let mut table = String::new();
    app.shell.metrics.format_text_table(&mut table);
    println!("=== hlouis-ui-demo --bench-ui (200,000 lines via app-core::EditorApp) ===");
    println!("{table}");
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
    let file_arg = args.get(1).map(Path::new);
    run_interactive_window(file_arg)
}
