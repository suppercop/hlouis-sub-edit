//! `hlouis-sub-edit`: the real subtitle editor. Opens a video and an `.ass` file (menus, drag
//! and drop or `hlouis-sub-edit.exe video.mkv subs.ass`) and edits them with libass-rendered
//! subtitles on the video, audio playback in sync, waveform/spectrum, grid and edit box.

use app_core::EditorApp;
use app_native::{native_editor, InstantClock};
use ass_core::ScriptFile;
use std::path::{Path, PathBuf};
use std::sync::Arc;

fn settings_file_path() -> Option<PathBuf> {
    let base = std::env::var_os("APPDATA")
        .or_else(|| std::env::var_os("LOCALAPPDATA"))
        .map(PathBuf::from)?;
    Some(base.join("hlouis-sub-edit").join("editor-settings.json"))
}

struct EditorShell {
    app: EditorApp,
    clock: InstantClock,
    startup: Vec<PathBuf>,
    startup_done: bool,
}

impl EditorShell {
    fn new(files: Vec<PathBuf>) -> Self {
        let (mut app, problems) = native_editor(ScriptFile::new_default());
        app.new_script();
        let mut first_run = true;
        if let Some(path) = settings_file_path() {
            if path.is_file() {
                first_run = false;
                if let Err(e) = app.load_settings(&path) {
                    app.notify_error(format!("Settings could not be loaded: {e}"));
                }
            }
            app.settings_path = Some(path);
        }
        if first_run {
            app.check_first_run_aegisub_import(None);
        }
        for p in problems {
            app.notify_error(p);
        }
        let _ = app.check_crash_recovery();
        Self {
            app,
            clock: InstantClock::default(),
            startup: files,
            startup_done: false,
        }
    }
}

impl eframe::App for EditorShell {
    fn ui(&mut self, ui: &mut egui::Ui, _frame: &mut eframe::Frame) {
        let ctx = ui.ctx().clone();
        if !self.startup_done {
            self.startup_done = true;
            for p in std::mem::take(&mut self.startup) {
                if let Err(e) = self.app.open_any(&p, &ctx) {
                    self.app
                        .notify_error(format!("Cannot open {}: {e}", p.display()));
                }
            }
        }
        if ctx.input(|i| i.viewport().close_requested()) && !self.app.request_close() {
            ctx.send_viewport_cmd(egui::ViewportCommand::CancelClose);
        }
        self.app.show(ui, &self.clock);
        let now = playback::Clock::now_ns(&self.clock);
        if let Err(e) = self.app.poll_autosave(now) {
            self.app.notify_error(format!("Autosave failed: {e}"));
        }
        ctx.send_viewport_cmd(egui::ViewportCommand::Title(self.app.shell.window_title()));
    }

    fn on_exit(&mut self) {
        if let Some(path) = self.app.settings_path.clone() {
            if let Some(dir) = path.parent() {
                let _ = std::fs::create_dir_all(dir);
            }
            if let Err(e) = self.app.save_settings(&path) {
                eprintln!("could not save settings: {e}");
            }
        }
    }
}

fn wgpu_options() -> eframe::egui_wgpu::WgpuConfiguration {
    let mut wgpu_options = eframe::egui_wgpu::WgpuConfiguration::default();
    if let eframe::egui_wgpu::WgpuSetup::CreateNew(ref mut setup) = wgpu_options.wgpu_setup {
        if std::env::var_os("WGPU_BACKEND").is_none() {
            setup.instance_descriptor.backends = wgpu::Backends::DX12;
        }
        if std::env::var_os("WGPU_POWER_PREF").is_none() {
            setup.power_preference = wgpu::PowerPreference::LowPower;
        }
        setup.device_descriptor = Arc::new(|adapter| {
            let base_limits = if adapter.get_info().backend == wgpu::Backend::Gl {
                wgpu::Limits::downlevel_webgl2_defaults()
            } else {
                wgpu::Limits::default()
            };
            wgpu::DeviceDescriptor {
                label: Some("hlouis_wgpu_device"),
                required_limits: wgpu::Limits {
                    max_texture_dimension_2d: 8192,
                    ..base_limits
                },
                memory_hints: wgpu::MemoryHints::MemoryUsage,
                ..Default::default()
            }
        });
    }
    wgpu_options
}

fn is_media(p: &Path) -> bool {
    app_core::transport::is_video_path(p) || app_core::transport::is_audio_path(p)
}

fn main() -> anyhow::Result<()> {
    // Command line: any mix of video / audio / subtitle paths; media first, subtitles last so the
    // video is already opening when the script loads.
    let mut files: Vec<PathBuf> = std::env::args_os()
        .skip(1)
        .map(PathBuf::from)
        .filter(|p| !p.to_string_lossy().starts_with("--"))
        .collect();
    files.sort_by_key(|p| u8::from(!is_media(p)));
    let native_options = eframe::NativeOptions {
        viewport: egui::ViewportBuilder::default()
            .with_inner_size([1400.0, 860.0])
            .with_min_inner_size([840.0, 520.0])
            .with_drag_and_drop(true)
            .with_title("hlouis-sub-edit"),
        renderer: eframe::Renderer::Wgpu,
        wgpu_options: wgpu_options(),
        ..Default::default()
    };
    eframe::run_native(
        "hlouis-sub-edit",
        native_options,
        Box::new(move |_cc| Ok(Box::new(EditorShell::new(files)))),
    )
    .map_err(|e| anyhow::anyhow!("eframe failed: {e}"))
}
