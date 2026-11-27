//! Main window shell composing the classic Aegisub 3.2.2 workspace layout (`egui 0.37`):
//! video top-left, audio top-right, edit box under audio, virtualised 200k-line grid at the
//! bottom, menu bar + compact toolbar + command palette from [`commands`], context-aware hotkey
//! dispatch, drag-and-drop file handling, autosave timer hook, live Light/Dark/System/EyeComfort
//! theme switching, and strict `request_repaint` discipline (zero continuous repaint when idle).

#![forbid(unsafe_code)]
#![allow(clippy::cast_precision_loss, clippy::cast_possible_truncation)]
#![cfg_attr(not(test), deny(clippy::unwrap_used, clippy::expect_used))]

use ass_core::ScriptFile;
use audio_view::TimelineMarkerIndex;
use commands::{CommandRegistry, HotkeyContext, KeyChord};
use document::DocumentController;
use egui::{Key, Panel, Pos2, Rect, Ui, Vec2};
use hlouis_tools::{build_style_counts, clean_whitespace, preview_clean, quick_select_by_style};
use i18n_catalog::{Catalog, Locale};
use keyframes::Timecodes;
use layout::{LayoutState, ViewMode};
use perf_metrics::PerfMetrics;
use playback::{Clock, PlaybackConfig, PlaybackController};
use settings::{AppSettings, ThemeMode};
use std::collections::BTreeSet;
use std::path::PathBuf;
use template_doctor::{fix_all_safe, lint_script};
use theme::{BaseTheme, ThemeSpec};
use ui_audio::{AudioColumnProvider, AudioPanelState, show_audio_panel};
use ui_dialogs::{ActiveDialog, DialogAction, DialogHostState, show_active_dialog};
use ui_editbox::{EditBoxState, show_edit_box};
use ui_grid::{GridAction, SubtitleGridState, show_subtitle_grid};
use ui_theme::{FontConfig, ThemeController};
use ui_video::{VideoFrameSupplier, VideoPanelState, show_video_panel};

/// Full application shell state combining all UI panels, document, playback, settings, and telemetry.
pub struct AppShell {
    pub doc: DocumentController,
    pub settings: AppSettings,
    pub layout_state: LayoutState,
    pub theme: ThemeController,
    pub catalog: Catalog,
    pub commands: CommandRegistry,
    pub playback: Option<PlaybackController>,
    pub grid: SubtitleGridState,
    pub editbox: EditBoxState,
    pub audio: AudioPanelState,
    pub audio_markers: TimelineMarkerIndex,
    pub video: VideoPanelState,
    pub dialogs: DialogHostState,
    pub metrics: PerfMetrics,
    pub command_palette_open: bool,
    pub command_palette_query: String,
    pub status_message: String,
    pub dropped_files: Vec<PathBuf>,
    pub last_autosave_ns: u64,
    pub autosave_interval_ns: u64,
    pub system_is_dark: bool,
    fonts_installed: bool,
    last_theme_rev: u64,
}

impl AppShell {
    /// Creates a new shell initialized with `script` and no media attached.
    ///
    /// Call [`Self::attach_video`] and [`Self::attach_audio`] when real media is opened.
    #[must_use]
    pub fn new(script: ScriptFile) -> Self {
        let mut grid = SubtitleGridState::default();
        grid.sync_from_script(&script);
        let doc = DocumentController::new(script);
        Self {
            doc,
            settings: AppSettings::default(),
            layout_state: LayoutState::default(),
            theme: ThemeController::new(ThemeSpec::Dark, FontConfig::default()),
            catalog: Catalog::for_locale(Locale::En),
            commands: CommandRegistry::new_default(),
            playback: None,
            grid,
            editbox: EditBoxState::default(),
            audio: AudioPanelState::new(0),
            audio_markers: TimelineMarkerIndex::new(Vec::new(), Vec::new()),
            video: VideoPanelState::new(0, 0),
            dialogs: DialogHostState::new(),
            metrics: PerfMetrics::default(),
            command_palette_open: false,
            command_palette_query: String::new(),
            status_message: "Ready".to_owned(),
            dropped_files: Vec::new(),
            last_autosave_ns: 0,
            autosave_interval_ns: 60_000_000_000,
            system_is_dark: true,
            fonts_installed: false,
            last_theme_rev: 0,
        }
    }

    /// Attaches real video timing metadata (timecodes, frame count, keyframes, dimensions).
    pub fn attach_video(
        &mut self,
        timecodes: Timecodes,
        frame_count: u32,
        keyframes: Vec<u32>,
        width: u32,
        height: u32,
    ) {
        self.playback = Some(PlaybackController::new(
            timecodes,
            frame_count,
            keyframes,
            PlaybackConfig::default(),
        ));
        self.video = VideoPanelState::new(width, height);
    }

    /// Detaches the active video source and clears playback state.
    pub fn detach_video(&mut self) {
        self.playback = None;
        self.video = VideoPanelState::new(0, 0);
        self.audio.video_marker_ms = None;
    }

    /// Attaches real audio duration and timeline markers.
    pub fn attach_audio(&mut self, duration_ms: i64, markers: TimelineMarkerIndex) {
        self.audio = AudioPanelState::new(duration_ms.max(0));
        self.audio_markers = markers;
    }

    /// Detaches the active audio source and clears timeline markers.
    pub fn detach_audio(&mut self) {
        self.audio = AudioPanelState::new(0);
        self.audio_markers = TimelineMarkerIndex::new(Vec::new(), Vec::new());
    }

    /// Switches theme specification live at runtime.
    pub fn set_theme(&mut self, spec: ThemeSpec) {
        let _ = self.theme.set_spec(spec);
    }

    /// Switches UI locale (`En` / `Vi`) live at runtime.
    pub fn set_locale(&mut self, locale: Locale) {
        if self.catalog.locale() != locale {
            self.catalog = Catalog::for_locale(locale);
        }
    }

    /// Returns `true` when the autosave timer interval has elapsed and the document has unsaved
    /// changes (0 heap allocations).
    #[inline]
    #[must_use]
    pub fn should_autosave(&self, now_ns: u64) -> bool {
        self.doc.is_autosave_dirty()
            && now_ns.saturating_sub(self.last_autosave_ns) >= self.autosave_interval_ns
    }

    /// Dispatches a canonical command ID (from menus, toolbar, context menu, palette, or hotkey).
    pub fn execute_command(&mut self, cmd_id: &str) {
        match cmd_id {
            "edit/undo" => {
                if let Ok(Some(desc)) = self.doc.undo() {
                    self.grid.sync_from_script(self.doc.script());
                    self.status_message = format!("Undo: {desc}");
                }
            }
            "edit/redo" => {
                if let Ok(Some(desc)) = self.doc.redo() {
                    self.grid.sync_from_script(self.doc.script());
                    self.status_message = format!("Redo: {desc}");
                }
            }
            "time/shift" => self.dialogs.open = Some(ActiveDialog::ShiftTimes),
            "tool/timing_processor" => self.dialogs.open = Some(ActiveDialog::TimingPostProcessor),
            "edit/find_replace" | "subtitle/find" => {
                self.dialogs.open = Some(ActiveDialog::FindReplace);
            }
            "tool/line/select" => self.dialogs.open = Some(ActiveDialog::SelectLines),
            "tool/resample" => self.dialogs.open = Some(ActiveDialog::Resample),
            "tool/styles_manager" => self.dialogs.open = Some(ActiveDialog::StyleManager),
            "subtitle/properties" => self.dialogs.open = Some(ActiveDialog::Properties),
            "tool/fonts_collector" => self.dialogs.open = Some(ActiveDialog::FontsCheck),
            "video/jump" => self.dialogs.open = Some(ActiveDialog::JumpTo),
            "app/options" => self.dialogs.open = Some(ActiveDialog::Preferences),
            "subtitle/open/video" => self.dialogs.open = Some(ActiveDialog::MkvImport),
            "app/about" => self.dialogs.open = Some(ActiveDialog::About),
            "app/log" => self.dialogs.open = Some(ActiveDialog::LogWindow),
            "app/display/full" => self.layout_state.mode = ViewMode::Full,
            "app/display/standard" => self.layout_state.mode = ViewMode::VideoAndSubs,
            "app/display/audio_subs" => self.layout_state.mode = ViewMode::AudioAndSubs,
            "app/display/subs" => self.layout_state.mode = ViewMode::SubsOnly,
            "hlouis/select_by_style" => {
                let active = self.doc.active_line().unwrap_or(0);
                let report = quick_select_by_style(self.doc.script(), active, false);
                if !report.selection.is_empty() {
                    self.doc
                        .set_selection(report.active_line, &report.selection);
                }
                self.status_message = if self.catalog.locale() == Locale::Vi {
                    report.message_vi
                } else {
                    report.message_en
                };
            }
            "hlouis/select_by_style_dialog" => {
                self.dialogs.hlouis_styles = build_style_counts(self.doc.script());
                self.dialogs.open = Some(ActiveDialog::HlouisSelectByStyle);
            }
            "hlouis/clean_whitespace" => {
                let sel: BTreeSet<usize> = self.doc.selection().iter().copied().collect();
                self.dialogs.hlouis_clean_preview = Some(preview_clean(
                    self.doc.script(),
                    &sel,
                    self.dialogs.hlouis_clean_opts,
                ));
                self.dialogs.open = Some(ActiveDialog::HlouisCleanWhitespace);
            }
            "automation/template_doctor" => {
                self.dialogs.template_diags = lint_script(self.doc.script());
                self.dialogs.open = Some(ActiveDialog::TemplateDoctor);
            }
            _ => {
                self.status_message = format!("Command: {cmd_id}");
            }
        }
    }

    /// Renders one complete application frame into `ctx` with strict repaint discipline.
    #[allow(clippy::too_many_lines)]
    pub fn show(
        &mut self,
        ui: &mut Ui,
        video_supplier: Option<&dyn VideoFrameSupplier>,
        audio_provider: Option<&dyn AudioColumnProvider>,
        clock: &dyn Clock,
    ) {
        let ctx = ui.ctx().clone();
        let ctx = &ctx;
        if !self.fonts_installed {
            self.theme.install_fonts(ctx);
            self.fonts_installed = true;
        }
        if self.last_theme_rev != self.theme.revision() {
            self.theme.apply_style(ctx);
            self.last_theme_rev = self.theme.revision();
        }

        // Collect drag-and-drop files from OS.
        ctx.input(|i| {
            for f in &i.raw.dropped_files {
                self.dropped_files.push(f.path().to_path_buf());
            }
        });

        // Global hotkey dispatch (Ctrl+Shift+P opens command palette; otherwise consults registry).
        if !ctx.egui_wants_keyboard_input() {
            if ctx.input(|i| i.modifiers.ctrl && i.modifiers.shift && i.key_pressed(Key::P)) {
                self.command_palette_open = !self.command_palette_open;
            } else if ctx.input(|i| i.modifiers.ctrl && i.key_pressed(Key::Z))
                && let Some(chord) = KeyChord::from_parts(KeyChord::CTRL, "Z")
                && let Some(cmd) = self.commands.lookup(HotkeyContext::Default, chord)
            {
                let owned = cmd.to_owned();
                self.execute_command(&owned);
            }
        }

        // Advance playback tick ONLY if playing (repaint discipline: never continuous when idle!).
        if let Some(ref mut pb) = self.playback
            && pb.is_playing()
        {
            let mut decode_q = [0u32; 4];
            let dec = pb.tick(clock, &mut decode_q);
            self.audio.video_marker_ms = Some(dec.current_time_ms);
            ctx.request_repaint();
        }

        let pal = self.theme.palette().clone();
        let pal_rev = self.theme.revision();

        // 1. Top menu bar + compact toolbar.
        Panel::top("hlouis_menubar").show(ui, |ui| {
            egui::MenuBar::new().ui(ui, |ui| {
                ui.menu_button(self.catalog.get("menu-file").unwrap_or("File"), |ui| {
                    if ui.button("Open Subtitles from Video...").clicked() {
                        self.execute_command("subtitle/open/video");
                        ui.close();
                    }
                    if ui.button("Properties...").clicked() {
                        self.execute_command("subtitle/properties");
                        ui.close();
                    }
                });
                ui.menu_button(self.catalog.get("menu-edit").unwrap_or("Edit"), |ui| {
                    if ui.button("Undo").clicked() {
                        self.execute_command("edit/undo");
                        ui.close();
                    }
                    if ui.button("Redo").clicked() {
                        self.execute_command("edit/redo");
                        ui.close();
                    }
                    if ui.button("Find and Replace...").clicked() {
                        self.execute_command("edit/find_replace");
                        ui.close();
                    }
                });
                ui.menu_button(
                    self.catalog.get("menu-subtitle").unwrap_or("Subtitle"),
                    |ui| {
                        if ui.button("Styles Manager...").clicked() {
                            self.execute_command("tool/styles_manager");
                            ui.close();
                        }
                        if ui.button("Resample Resolution...").clicked() {
                            self.execute_command("tool/resample");
                            ui.close();
                        }
                        if ui.button("Select Lines...").clicked() {
                            self.execute_command("tool/line/select");
                            ui.close();
                        }
                        ui.separator();
                        ui.menu_button("HLouis", |ui| {
                            if ui.button("Select Lines by Style (Quick)").clicked() {
                                self.execute_command("hlouis/select_by_style");
                                ui.close();
                            }
                            if ui.button("Select Lines by Style...").clicked() {
                                self.execute_command("hlouis/select_by_style_dialog");
                                ui.close();
                            }
                            if ui.button("Clean Whitespace & Line Breaks...").clicked() {
                                self.execute_command("hlouis/clean_whitespace");
                                ui.close();
                            }
                        });
                    },
                );
                ui.menu_button(self.catalog.get("menu-timing").unwrap_or("Timing"), |ui| {
                    if ui.button("Shift Times...").clicked() {
                        self.execute_command("time/shift");
                        ui.close();
                    }
                    if ui.button("Timing Post-Processor...").clicked() {
                        self.execute_command("tool/timing_processor");
                        ui.close();
                    }
                });
                ui.menu_button("Automation", |ui| {
                    if ui.button("Template Doctor...").clicked() {
                        self.execute_command("automation/template_doctor");
                        ui.close();
                    }
                });
                ui.menu_button(self.catalog.get("menu-view").unwrap_or("View"), |ui| {
                    if ui.button("Full View").clicked() {
                        self.execute_command("app/display/full");
                        ui.close();
                    }
                    if ui.button("Standard View").clicked() {
                        self.execute_command("app/display/standard");
                        ui.close();
                    }
                    if ui.button("Subs Only").clicked() {
                        self.execute_command("app/display/subs");
                        ui.close();
                    }
                    ui.separator();
                    if ui.button("Theme: Light").clicked() {
                        self.settings.appearance.theme_mode = ThemeMode::Light;
                        self.set_theme(ThemeSpec::Light);
                        ui.close();
                    }
                    if ui.button("Theme: Dark").clicked() {
                        self.settings.appearance.theme_mode = ThemeMode::Dark;
                        self.set_theme(ThemeSpec::Dark);
                        ui.close();
                    }
                    if ui.button("Theme: Eye Comfort Dark").clicked() {
                        self.settings.appearance.theme_mode = ThemeMode::EyeComfortDark;
                        self.set_theme(ThemeSpec::EyeComfort {
                            base: BaseTheme::Dark,
                            strength: self.settings.appearance.eye_comfort_strength,
                        });
                        ui.close();
                    }
                    ui.separator();
                    if ui.button("English").clicked() {
                        self.set_locale(Locale::En);
                        ui.close();
                    }
                    if ui.button("Tiếng Việt").clicked() {
                        self.set_locale(Locale::Vi);
                        ui.close();
                    }
                    ui.separator();
                    if ui.button("Preferences...").clicked() {
                        self.execute_command("app/options");
                        ui.close();
                    }
                });
                ui.menu_button(self.catalog.get("menu-help").unwrap_or("Help"), |ui| {
                    if ui.button("Event Log").clicked() {
                        self.execute_command("app/log");
                        ui.close();
                    }
                    if ui.button("About").clicked() {
                        self.execute_command("app/about");
                        ui.close();
                    }
                });
            });
        });

        // 2. Bottom status bar.
        Panel::bottom("hlouis_statusbar").show(ui, |ui| {
            ui.horizontal(|ui| {
                ui.label(&self.status_message);
                ui.separator();
                let total = self.grid.model.row_count();
                let active = self.doc.active_line().map_or(0, |i| i + 1);
                ui.label(format!("Line {active} / {total}"));
                if self.doc.is_dirty() {
                    ui.separator();
                    ui.label(self.catalog.get("status-unsaved").unwrap_or("Modified"));
                }
                if self.settings.appearance.show_performance_overlay {
                    ui.separator();
                    ui.monospace(format!(
                        "p50={}us p95={}us worst={}us",
                        self.metrics.frame_time.p50(),
                        self.metrics.frame_time.p95(),
                        self.metrics.frame_time.worst()
                    ));
                }
            });
        });

        // 3. Central Aegisub 3.2.2 workspace layout (computed via `layout::LayoutState`).
        egui::CentralPanel::default().show(ui, |ui| {
            let avail = ui.available_rect_before_wrap();
            let comp = self.layout_state.compute(
                avail.width().max(320.0),
                avail.height().max(240.0),
                ctx.pixels_per_point(),
            );

            // Top-left: Video panel
            if let Some(v) = comp.video
                && v.is_visible()
            {
                let vr = Rect::from_min_size(
                    Pos2::new(avail.left() + v.x, avail.top() + v.y),
                    Vec2::new(v.w, v.h),
                );
                let mut child = ui.new_child(egui::UiBuilder::new().max_rect(vr));
                if let (Some(vs), Some(pb)) = (video_supplier, self.playback.as_mut()) {
                    let _ = show_video_panel(
                        &mut child,
                        &mut self.video,
                        vs,
                        pb,
                        clock,
                        &pal,
                        &self.catalog,
                    );
                } else {
                    child.centered_and_justified(|ui| {
                        ui.label(
                            self.catalog
                                .get("video-no-video")
                                .unwrap_or("No video loaded"),
                        );
                    });
                }
            }

            // Top-right: Audio panel
            if let Some(a) = comp.audio
                && a.is_visible()
            {
                let ar = Rect::from_min_size(
                    Pos2::new(avail.left() + a.x, avail.top() + a.y),
                    Vec2::new(a.w, a.h),
                );
                let mut child = ui.new_child(egui::UiBuilder::new().max_rect(ar));
                if let Some(ap) = audio_provider {
                    let _ = show_audio_panel(
                        &mut child,
                        &mut self.audio,
                        ap,
                        &self.audio_markers,
                        None,
                        None,
                        &pal,
                        pal_rev,
                        &self.catalog,
                    );
                } else {
                    child.centered_and_justified(|ui| {
                        ui.label(
                            self.catalog
                                .get("audio-no-audio")
                                .unwrap_or("No audio loaded"),
                        );
                    });
                }
            }

            // Middle-right (under audio): Edit Box
            if comp.edit_box.is_visible() {
                let er = Rect::from_min_size(
                    Pos2::new(
                        avail.left() + comp.edit_box.x,
                        avail.top() + comp.edit_box.y,
                    ),
                    Vec2::new(comp.edit_box.w, comp.edit_box.h),
                );
                let mut child = ui.new_child(egui::UiBuilder::new().max_rect(er));
                if show_edit_box(
                    &mut child,
                    &mut self.doc,
                    &mut self.editbox,
                    None,
                    &pal,
                    &self.catalog,
                ) && let Some(active) = self.doc.active_line()
                    && let Some(ev) = self
                        .doc
                        .script()
                        .events_section()
                        .and_then(|s| s.get_event(active))
                {
                    let mut synced = ev.clone();
                    if let Some(txt) = self.doc.effective_event_text(active) {
                        synced.set_text(txt);
                    }
                    self.grid.sync_row(active, &synced);
                }
            }

            // Bottom: Virtualised Subtitle Grid
            if comp.grid.is_visible() {
                let gr = Rect::from_min_size(
                    Pos2::new(avail.left() + comp.grid.x, avail.top() + comp.grid.y),
                    Vec2::new(comp.grid.w, comp.grid.h),
                );
                let mut child = ui.new_child(egui::UiBuilder::new().max_rect(gr));
                let vid_cs = self
                    .playback
                    .as_ref()
                    .and_then(|pb| u32::try_from(pb.current_time_ms().max(0) / 10).ok());
                if let Some(act) = show_subtitle_grid(
                    &mut child,
                    self.doc.script(),
                    &mut self.grid,
                    vid_cs,
                    &pal,
                    &self.catalog,
                ) {
                    match act {
                        GridAction::SelectionChanged { active } => {
                            if let Some(row) = active {
                                self.doc.set_selection(Some(row), &[row]);
                            }
                        }
                        GridAction::Command(cmd) => self.execute_command(cmd),
                    }
                }
            }
        });

        // 4. Active modal dialog window (if any).
        if self.dialogs.open.is_some() {
            let mut dialog_action = None;
            egui::Window::new("Dialog")
                .collapsible(false)
                .resizable(true)
                .show(ctx, |ui| {
                    dialog_action = show_active_dialog(
                        ui,
                        &mut self.dialogs,
                        &mut self.settings,
                        &self.commands,
                        None,
                        &pal,
                        &self.catalog,
                    );
                });
            if let Some(act) = dialog_action {
                self.handle_dialog_action(&act);
            }
        }

        // 5. Command Palette (Ctrl+Shift+P) incl. HLouis & Template Doctor commands.
        if self.command_palette_open {
            let mut chosen: Option<&'static str> = None;
            egui::Window::new("Command Palette")
                .collapsible(false)
                .show(ctx, |ui| {
                    ui.text_edit_singleline(&mut self.command_palette_query);
                    let q = self.command_palette_query.to_ascii_lowercase();
                    for cmd in commands::COMMANDS.iter().take(180) {
                        if (q.is_empty() || cmd.id.to_ascii_lowercase().contains(&q))
                            && ui.button(cmd.id).clicked()
                        {
                            chosen = Some(cmd.id);
                        }
                    }
                    if (q.is_empty() || "automation/template_doctor".contains(&q))
                        && ui.button("automation/template_doctor").clicked()
                    {
                        chosen = Some("automation/template_doctor");
                    }
                });
            if let Some(id) = chosen {
                self.command_palette_open = false;
                self.execute_command(id);
            }
        }
    }

    fn handle_dialog_action(&mut self, act: &DialogAction) {
        match act {
            DialogAction::SavePreferences => {
                self.theme
                    .sync_from_prefs(&self.settings.appearance, self.system_is_dark, 12 * 60);
                self.dialogs.open = None;
            }
            DialogAction::ApplyHlouisClean(opts) => {
                let sel: BTreeSet<usize> = self.doc.selection().iter().copied().collect();
                let report = clean_whitespace(self.doc.script(), &sel, *opts);
                if report.changed_count > 0 {
                    let _ = self.doc.commit(
                        "HLouis clean whitespace",
                        report.command,
                        document::CommitType::TEXT,
                    );
                    self.grid.sync_from_script(self.doc.script());
                }
                self.status_message = if self.catalog.locale() == Locale::Vi {
                    report.message_vi
                } else {
                    report.message_en
                };
                self.dialogs.open = None;
            }
            DialogAction::TemplateDoctorFixAllSafe => {
                let fixed = fix_all_safe(self.doc.script());
                if fixed.fixed_lines > 0 {
                    let _ = self.doc.commit(
                        "Template Doctor: fix all safe",
                        fixed.command,
                        document::CommitType::TEXT,
                    );
                    self.grid.sync_from_script(self.doc.script());
                    self.dialogs.template_diags = lint_script(self.doc.script());
                }
            }
            _ => {
                self.dialogs.open = None;
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use ass_core::{AssEvent, AssTime};
    use egui_kittest::Harness;

    struct ZeroClock;
    impl Clock for ZeroClock {
        fn now_ns(&self) -> u64 {
            0
        }
    }

    #[test]
    fn shell_starts_without_media_and_switches_themes() {
        let mut script = ScriptFile::new_default();
        script.ensure_events_section_mut().insert_event(
            0,
            AssEvent::dialogue(AssTime::ZERO, AssTime::from_cs(250), "Hello world"),
        );
        let mut shell = AppShell::new(script);
        assert!(shell.playback.is_none());
        assert!(shell.audio.video_marker_ms.is_none());

        let builder = Harness::builder()
            .with_size(Vec2::new(1280.0, 800.0))
            .with_accessibility_check(false);
        let mut harness = builder.build_ui(move |ui| {
            shell.show(ui, None, None, &ZeroClock);
            shell.set_theme(ThemeSpec::EyeComfort {
                base: BaseTheme::Dark,
                strength: 65,
            });
        });
        harness.run();
        harness.step();
    }
}
