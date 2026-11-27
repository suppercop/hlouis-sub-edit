//! Main window shell composing the classic Aegisub 3.2.2 workspace layout (`egui 0.37`):
//! video top-left, audio top-right, edit box under audio, virtualised 200k-line grid at the
//! bottom, classic 9 top-level menus + single-row 16×16 vector icon toolbar + command palette
//! from [`commands`], draggable splitters, strict per-pane clip rects, full `ui-preferences` &
//! `ui-mkv-import` wiring, localized opaque foreground dialog chrome, live Light/Dark/EyeComfort
//! theme switching, and strict `request_repaint` discipline (zero continuous repaint when idle).

#![forbid(unsafe_code)]
#![allow(clippy::cast_precision_loss, clippy::cast_possible_truncation)]
#![cfg_attr(not(test), deny(clippy::unwrap_used, clippy::expect_used))]

use ass_core::ScriptFile;
use audio_view::TimelineMarkerIndex;
use commands::{CommandRegistry, HotkeyContext, KeyChord};
use document::DocumentController;
use egui::{Align2, CursorIcon, Key, Order, Panel, Pos2, Rect, Sense, Stroke, Ui, Vec2};
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
use theme::{BaseTheme, Palette, ThemeSpec};
use ui_audio::{AudioColumnProvider, AudioPanelState, show_audio_panel};
use ui_dialogs::{ActiveDialog, DialogAction, DialogHostState, show_active_dialog};
use ui_editbox::{EditBoxState, show_edit_box};
use ui_grid::{GridAction, SubtitleGridState, show_subtitle_grid};
use ui_mkv_import::{MkvImportAction, MkvImportDialogState, show_mkv_import_dialog};
use ui_preferences::{PreferencesDialogState, PrefsDialogAction, show_preferences_dialog};
use ui_theme::{FontConfig, ThemeController, rgba};
use ui_video::{VideoFrameSupplier, VideoPanelState, show_video_panel};

/// Snapshot of the last computed logical pane rectangles (used for layout & zero-overlap checks).
#[derive(Debug, Clone, Copy)]
pub struct LastPaneRects {
    pub workspace: Rect,
    pub video: Option<Rect>,
    pub audio: Option<Rect>,
    pub edit_box: Option<Rect>,
    pub grid: Option<Rect>,
}

impl Default for LastPaneRects {
    fn default() -> Self {
        Self {
            workspace: Rect::NOTHING,
            video: None,
            audio: None,
            edit_box: None,
            grid: None,
        }
    }
}

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
    pub prefs_dialog: PreferencesDialogState,
    pub mkv_dialog: MkvImportDialogState,
    pub metrics: PerfMetrics,
    pub command_palette_open: bool,
    pub command_palette_query: String,
    pub script_name: String,
    pub status_message: String,
    pub dropped_files: Vec<PathBuf>,
    pub last_autosave_ns: u64,
    pub autosave_interval_ns: u64,
    pub system_is_dark: bool,
    pub last_pane_rects: LastPaneRects,
    fonts_installed: bool,
    last_theme_rev: u64,
    status_line_cache: (Option<usize>, usize, String),
}

#[derive(Clone, Copy)]
enum ToolbarIcon {
    New,
    Open,
    Save,
    SaveAs,
    OpenMkv,
    OpenVideo,
    OpenAudio,
    Undo,
    Redo,
    Cut,
    Copy,
    Paste,
    FindReplace,
    Styles,
    StyleEditor,
    Properties,
    Fonts,
    Resample,
    ShiftTimes,
    Tpp,
    SelectLines,
    ContStart,
    ContEnd,
    SnapScene,
    JumpTo,
    Karaoke,
    Doctor,
    Hotkeys,
    Preferences,
}

#[allow(clippy::too_many_lines)]
fn toolbar_icon_button(ui: &mut Ui, icon: ToolbarIcon, pal: &Palette, tooltip: &str) -> bool {
    let (rect, resp) = ui.allocate_exact_size(Vec2::splat(20.0), Sense::click());
    let p = ui.painter();
    let bg = if resp.hovered() {
        rgba(pal.hover)
    } else {
        rgba(pal.toolbar_bg)
    };
    p.rect_filled(rect, 2.0, bg);
    if resp.hovered() {
        p.rect_stroke(
            rect,
            2.0,
            Stroke::new(1.0, rgba(pal.border)),
            egui::StrokeKind::Inside,
        );
    }
    let blue = egui::Color32::from_rgb(42, 120, 214);
    let green = egui::Color32::from_rgb(38, 168, 84);
    let red = egui::Color32::from_rgb(214, 62, 62);
    let amber = egui::Color32::from_rgb(218, 148, 24);
    let purple = egui::Color32::from_rgb(154, 72, 206);
    let teal = egui::Color32::from_rgb(24, 152, 168);
    let fg = rgba(pal.text_primary);
    let st = Stroke::new(1.3, fg);
    let c = rect.center();

    match icon {
        ToolbarIcon::New => {
            let b = Rect::from_center_size(c, Vec2::new(10.0, 12.0));
            p.rect_filled(b, 1.0, egui::Color32::from_rgb(248, 250, 255));
            p.rect_stroke(b, 1.0, Stroke::new(1.1, blue), egui::StrokeKind::Inside);
            p.line_segment(
                [Pos2::new(c.x, c.y - 3.0), Pos2::new(c.x, c.y + 3.0)],
                Stroke::new(1.5, green),
            );
            p.line_segment(
                [Pos2::new(c.x - 3.0, c.y), Pos2::new(c.x + 3.0, c.y)],
                Stroke::new(1.5, green),
            );
        }
        ToolbarIcon::Open => {
            let b = Rect::from_center_size(Pos2::new(c.x, c.y + 1.0), Vec2::new(12.0, 8.0));
            p.rect_filled(b, 1.0, egui::Color32::from_rgb(245, 206, 98));
            p.rect_stroke(b, 1.0, Stroke::new(1.1, amber), egui::StrokeKind::Inside);
        }
        ToolbarIcon::Save | ToolbarIcon::SaveAs => {
            let b = Rect::from_center_size(c, Vec2::new(11.0, 11.0));
            p.rect_filled(b, 1.0, blue);
            p.rect_filled(
                Rect::from_center_size(Pos2::new(c.x, c.y - 3.0), Vec2::new(6.0, 3.0)),
                0.0,
                egui::Color32::WHITE,
            );
            if matches!(icon, ToolbarIcon::SaveAs) {
                p.circle_filled(Pos2::new(c.x + 4.5, c.y + 4.5), 2.5, amber);
            }
        }
        ToolbarIcon::OpenMkv => {
            let b = Rect::from_center_size(c, Vec2::new(12.0, 9.0));
            p.rect_filled(b, 1.0, purple);
            let mst = Stroke::new(1.3, egui::Color32::WHITE);
            p.line_segment(
                [
                    Pos2::new(c.x - 3.5, c.y + 2.8),
                    Pos2::new(c.x - 3.5, c.y - 2.8),
                ],
                mst,
            );
            p.line_segment(
                [Pos2::new(c.x - 3.5, c.y - 2.8), Pos2::new(c.x, c.y + 0.8)],
                mst,
            );
            p.line_segment(
                [Pos2::new(c.x, c.y + 0.8), Pos2::new(c.x + 3.5, c.y - 2.8)],
                mst,
            );
            p.line_segment(
                [
                    Pos2::new(c.x + 3.5, c.y - 2.8),
                    Pos2::new(c.x + 3.5, c.y + 2.8),
                ],
                mst,
            );
        }
        ToolbarIcon::OpenVideo => {
            let b = Rect::from_center_size(c, Vec2::new(12.0, 9.0));
            p.rect_stroke(b, 1.0, Stroke::new(1.3, blue), egui::StrokeKind::Inside);
            p.add(egui::Shape::convex_polygon(
                vec![
                    Pos2::new(c.x - 2.0, c.y - 3.0),
                    Pos2::new(c.x + 3.5, c.y),
                    Pos2::new(c.x - 2.0, c.y + 3.0),
                ],
                green,
                Stroke::NONE,
            ));
        }
        ToolbarIcon::OpenAudio => {
            for (dx, h) in [(-4.0, 4.0), (-1.5, 9.0), (1.5, 6.0), (4.0, 8.0)] {
                p.line_segment(
                    [
                        Pos2::new(c.x + dx, c.y + h * 0.5),
                        Pos2::new(c.x + dx, c.y - h * 0.5),
                    ],
                    Stroke::new(1.8, teal),
                );
            }
        }
        ToolbarIcon::Undo => {
            let s = Stroke::new(1.6, blue);
            p.line_segment([Pos2::new(c.x - 5.0, c.y), Pos2::new(c.x + 4.0, c.y)], s);
            p.line_segment(
                [Pos2::new(c.x - 5.0, c.y), Pos2::new(c.x - 1.5, c.y - 3.5)],
                s,
            );
            p.line_segment(
                [Pos2::new(c.x - 5.0, c.y), Pos2::new(c.x - 1.5, c.y + 3.5)],
                s,
            );
        }
        ToolbarIcon::Redo => {
            let s = Stroke::new(1.6, blue);
            p.line_segment([Pos2::new(c.x - 4.0, c.y), Pos2::new(c.x + 5.0, c.y)], s);
            p.line_segment(
                [Pos2::new(c.x + 5.0, c.y), Pos2::new(c.x + 1.5, c.y - 3.5)],
                s,
            );
            p.line_segment(
                [Pos2::new(c.x + 5.0, c.y), Pos2::new(c.x + 1.5, c.y + 3.5)],
                s,
            );
        }
        ToolbarIcon::Cut => {
            p.line_segment(
                [
                    Pos2::new(c.x - 4.5, c.y - 4.5),
                    Pos2::new(c.x + 4.5, c.y + 4.5),
                ],
                Stroke::new(1.4, red),
            );
            p.line_segment(
                [
                    Pos2::new(c.x + 4.5, c.y - 4.5),
                    Pos2::new(c.x - 4.5, c.y + 4.5),
                ],
                Stroke::new(1.4, red),
            );
        }
        ToolbarIcon::Copy => {
            p.rect_stroke(
                Rect::from_center_size(Pos2::new(c.x - 1.5, c.y - 1.5), Vec2::new(7.0, 9.0)),
                0.5,
                Stroke::new(1.2, blue),
                egui::StrokeKind::Inside,
            );
            p.rect_stroke(
                Rect::from_center_size(Pos2::new(c.x + 2.0, c.y + 1.5), Vec2::new(7.0, 9.0)),
                0.5,
                Stroke::new(1.2, teal),
                egui::StrokeKind::Inside,
            );
        }
        ToolbarIcon::Paste => {
            p.rect_stroke(
                Rect::from_center_size(c, Vec2::new(9.0, 11.0)),
                1.0,
                Stroke::new(1.3, amber),
                egui::StrokeKind::Inside,
            );
            p.rect_filled(
                Rect::from_center_size(Pos2::new(c.x, c.y - 4.5), Vec2::new(5.0, 2.5)),
                0.5,
                blue,
            );
        }
        ToolbarIcon::FindReplace => {
            p.circle_stroke(Pos2::new(c.x - 1.5, c.y - 1.5), 4.0, Stroke::new(1.5, teal));
            p.line_segment(
                [
                    Pos2::new(c.x + 1.5, c.y + 1.5),
                    Pos2::new(c.x + 5.5, c.y + 5.5),
                ],
                Stroke::new(2.0, blue),
            );
        }
        ToolbarIcon::Styles => {
            p.line_segment(
                [Pos2::new(c.x - 4.0, c.y + 5.0), Pos2::new(c.x, c.y - 5.0)],
                Stroke::new(1.8, purple),
            );
            p.line_segment(
                [Pos2::new(c.x, c.y - 5.0), Pos2::new(c.x + 4.0, c.y + 5.0)],
                Stroke::new(1.8, purple),
            );
            p.line_segment(
                [
                    Pos2::new(c.x - 2.5, c.y + 1.5),
                    Pos2::new(c.x + 2.5, c.y + 1.5),
                ],
                Stroke::new(1.4, blue),
            );
        }
        ToolbarIcon::StyleEditor => {
            let ast = Stroke::new(1.5, purple);
            p.line_segment(
                [
                    Pos2::new(c.x - 4.5, c.y + 4.0),
                    Pos2::new(c.x - 1.5, c.y - 4.5),
                ],
                ast,
            );
            p.line_segment(
                [
                    Pos2::new(c.x - 1.5, c.y - 4.5),
                    Pos2::new(c.x + 1.5, c.y + 4.0),
                ],
                ast,
            );
            p.line_segment(
                [
                    Pos2::new(c.x - 3.3, c.y + 1.0),
                    Pos2::new(c.x + 0.3, c.y + 1.0),
                ],
                ast,
            );
            p.circle_stroke(Pos2::new(c.x + 3.5, c.y + 1.8), 2.0, Stroke::new(1.3, blue));
        }
        ToolbarIcon::Properties => {
            for (dy, col) in [(-4.0, blue), (0.0, teal), (4.0, amber)] {
                p.line_segment(
                    [
                        Pos2::new(c.x - 5.0, c.y + dy),
                        Pos2::new(c.x + 5.0, c.y + dy),
                    ],
                    Stroke::new(1.6, col),
                );
            }
        }
        ToolbarIcon::Fonts => {
            let fst = Stroke::new(1.7, red);
            p.line_segment(
                [
                    Pos2::new(c.x - 3.0, c.y - 4.5),
                    Pos2::new(c.x - 3.0, c.y + 4.5),
                ],
                fst,
            );
            p.line_segment(
                [
                    Pos2::new(c.x - 3.0, c.y - 4.5),
                    Pos2::new(c.x + 3.5, c.y - 4.5),
                ],
                fst,
            );
            p.line_segment(
                [
                    Pos2::new(c.x - 3.0, c.y - 0.5),
                    Pos2::new(c.x + 2.0, c.y - 0.5),
                ],
                fst,
            );
        }
        ToolbarIcon::Resample => {
            let b = Rect::from_center_size(c, Vec2::new(11.0, 9.0));
            p.rect_stroke(b, 0.5, Stroke::new(1.3, teal), egui::StrokeKind::Inside);
            p.line_segment([b.left_bottom(), b.right_top()], Stroke::new(1.4, green));
        }
        ToolbarIcon::ShiftTimes => {
            p.circle_stroke(c, 5.5, Stroke::new(1.5, amber));
            p.line_segment([c, Pos2::new(c.x, c.y - 3.5)], Stroke::new(1.4, blue));
            p.line_segment([c, Pos2::new(c.x + 3.0, c.y)], Stroke::new(1.4, red));
        }
        ToolbarIcon::Tpp => {
            p.line_segment(
                [
                    Pos2::new(c.x - 5.0, c.y - 3.0),
                    Pos2::new(c.x + 5.0, c.y - 3.0),
                ],
                st,
            );
            p.line_segment(
                [
                    Pos2::new(c.x - 5.0, c.y + 3.0),
                    Pos2::new(c.x + 5.0, c.y + 3.0),
                ],
                st,
            );
            p.circle_filled(Pos2::new(c.x - 2.0, c.y - 3.0), 2.2, green);
            p.circle_filled(Pos2::new(c.x + 2.0, c.y + 3.0), 2.2, blue);
        }
        ToolbarIcon::SelectLines => {
            for dy in [-3.5, 0.0, 3.5] {
                p.line_segment(
                    [
                        Pos2::new(c.x - 5.0, c.y + dy),
                        Pos2::new(c.x + 5.0, c.y + dy),
                    ],
                    Stroke::new(1.5, blue),
                );
            }
        }
        ToolbarIcon::ContStart => {
            p.line_segment(
                [
                    Pos2::new(c.x - 4.0, c.y - 4.0),
                    Pos2::new(c.x - 4.0, c.y + 4.0),
                ],
                Stroke::new(1.8, red),
            );
            p.line_segment(
                [Pos2::new(c.x - 2.0, c.y), Pos2::new(c.x + 5.0, c.y)],
                Stroke::new(1.5, blue),
            );
        }
        ToolbarIcon::ContEnd => {
            p.line_segment(
                [
                    Pos2::new(c.x + 4.0, c.y - 4.0),
                    Pos2::new(c.x + 4.0, c.y + 4.0),
                ],
                Stroke::new(1.8, red),
            );
            p.line_segment(
                [Pos2::new(c.x - 5.0, c.y), Pos2::new(c.x + 2.0, c.y)],
                Stroke::new(1.5, blue),
            );
        }
        ToolbarIcon::SnapScene => {
            p.line_segment(
                [Pos2::new(c.x, c.y - 5.5), Pos2::new(c.x, c.y + 5.5)],
                Stroke::new(1.8, purple),
            );
            p.circle_filled(c, 2.2, amber);
        }
        ToolbarIcon::JumpTo => {
            p.line_segment(
                [Pos2::new(c.x - 5.0, c.y), Pos2::new(c.x + 4.0, c.y)],
                Stroke::new(1.6, green),
            );
            p.line_segment(
                [Pos2::new(c.x + 4.0, c.y), Pos2::new(c.x + 1.0, c.y - 3.5)],
                Stroke::new(1.6, green),
            );
            p.line_segment(
                [Pos2::new(c.x + 4.0, c.y), Pos2::new(c.x + 1.0, c.y + 3.5)],
                Stroke::new(1.6, green),
            );
        }
        ToolbarIcon::Karaoke => {
            p.circle_filled(Pos2::new(c.x - 2.5, c.y + 3.5), 2.4, purple);
            p.line_segment(
                [
                    Pos2::new(c.x - 0.5, c.y + 3.0),
                    Pos2::new(c.x - 0.5, c.y - 5.0),
                ],
                Stroke::new(1.6, purple),
            );
            p.line_segment(
                [
                    Pos2::new(c.x - 0.5, c.y - 5.0),
                    Pos2::new(c.x + 4.5, c.y - 3.0),
                ],
                Stroke::new(1.6, purple),
            );
        }
        ToolbarIcon::Doctor => {
            p.rect_stroke(
                Rect::from_center_size(c, Vec2::new(11.0, 11.0)),
                2.0,
                Stroke::new(1.2, green),
                egui::StrokeKind::Inside,
            );
            p.line_segment(
                [Pos2::new(c.x, c.y - 3.5), Pos2::new(c.x, c.y + 3.5)],
                Stroke::new(2.0, green),
            );
            p.line_segment(
                [Pos2::new(c.x - 3.5, c.y), Pos2::new(c.x + 3.5, c.y)],
                Stroke::new(2.0, green),
            );
        }
        ToolbarIcon::Hotkeys => {
            p.rect_stroke(
                Rect::from_center_size(c, Vec2::new(12.0, 8.0)),
                1.5,
                Stroke::new(1.3, blue),
                egui::StrokeKind::Inside,
            );
            p.line_segment(
                [Pos2::new(c.x - 3.5, c.y), Pos2::new(c.x + 3.5, c.y)],
                Stroke::new(1.2, amber),
            );
        }
        ToolbarIcon::Preferences => {
            p.circle_stroke(c, 4.5, Stroke::new(1.6, blue));
            p.circle_filled(c, 2.0, amber);
        }
    }
    resp.on_hover_text(tooltip).clicked()
}

impl AppShell {
    /// Creates a new shell initialized with `script` and no media attached.
    #[must_use]
    pub fn new(script: ScriptFile) -> Self {
        let mut grid = SubtitleGridState::default();
        grid.sync_from_script(&script);
        let mut dialogs = DialogHostState::new();
        let script_style_names: Vec<String> =
            script.styles().map(|st| st.name().to_owned()).collect();
        if !script_style_names.is_empty() {
            dialogs.script_styles = script_style_names;
        }
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
            dialogs,
            prefs_dialog: PreferencesDialogState::default(),
            mkv_dialog: MkvImportDialogState::default(),
            metrics: PerfMetrics::default(),
            command_palette_open: false,
            command_palette_query: String::new(),
            script_name: "Untitled.ass".to_owned(),
            status_message: String::new(),
            dropped_files: Vec::new(),
            last_autosave_ns: 0,
            autosave_interval_ns: 60_000_000_000,
            system_is_dark: true,
            last_pane_rects: LastPaneRects::default(),
            fonts_installed: false,
            last_theme_rev: 0,
            status_line_cache: (None, usize::MAX, String::with_capacity(32)),
        }
    }

    /// Returns the window title (`"<script name> - hlouis-sub-edit"`, prefixed with `"*"` when modified).
    #[must_use]
    pub fn window_title(&self) -> String {
        if self.doc.is_dirty() {
            format!("*{} - hlouis-sub-edit", self.script_name)
        } else {
            format!("{} - hlouis-sub-edit", self.script_name)
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
            "tool/style_editor" => self.dialogs.open = Some(ActiveDialog::StyleEditor),
            "subtitle/properties" => self.dialogs.open = Some(ActiveDialog::Properties),
            "tool/fonts_collector" => self.dialogs.open = Some(ActiveDialog::FontsCheck),
            "video/jump" => self.dialogs.open = Some(ActiveDialog::JumpTo),
            "app/options" => self.dialogs.open = Some(ActiveDialog::Preferences),
            "app/hotkeys" => self.dialogs.open = Some(ActiveDialog::Hotkeys),
            "subtitle/open/video" => self.dialogs.open = Some(ActiveDialog::MkvImport),
            "app/about" => self.dialogs.open = Some(ActiveDialog::About),
            "app/log" => self.dialogs.open = Some(ActiveDialog::LogWindow),
            "automation/kara_templater" => {
                self.dialogs.open = Some(ActiveDialog::KaraokeTemplater);
            }
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

    /// Renders one complete application frame into `ui` with strict repaint discipline.
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

        // Sync active line start/end to video panel offset display.
        if let Some(active) = self.doc.active_line()
            && let Some(ev) = self
                .doc
                .script()
                .events_section()
                .and_then(|s| s.get_event(active))
        {
            self.video.active_line_start_ms = i64::from(ev.start().as_cs()) * 10;
            self.video.active_line_end_ms = i64::from(ev.end().as_cs()) * 10;
        }

        let pal = self.theme.palette().clone();
        let pal_rev = self.theme.revision();
        let is_vi = self.catalog.locale() == Locale::Vi;

        // 1. Classic 9 top-level Aegisub 3.2.2 menus in exact order:
        //    File, Edit, Subtitle, Timing, Video, Audio, Automation, View, Help.
        Panel::top("hlouis_menubar").show(ui, |ui| {
            egui::MenuBar::new().ui(ui, |ui| {
                ui.menu_button(self.catalog.get("menu-file").unwrap_or("File"), |ui| {
                    if ui.button("New Subtitles").clicked() {
                        self.execute_command("subtitle/new");
                        ui.close();
                    }
                    if ui.button("Open Subtitles...").clicked() {
                        self.execute_command("subtitle/open");
                        ui.close();
                    }
                    if ui.button("Open Subtitles from Video...").clicked() {
                        self.execute_command("subtitle/open/video");
                        ui.close();
                    }
                    ui.separator();
                    if ui.button("Save Subtitles").clicked() {
                        self.execute_command("subtitle/save");
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
                    ui.separator();
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
                        if ui.button("Style Editor...").clicked() {
                            self.execute_command("tool/style_editor");
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
                        if ui.button("Fonts Collector...").clicked() {
                            self.execute_command("tool/fonts_collector");
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
                ui.menu_button(self.catalog.get("menu-video").unwrap_or("Video"), |ui| {
                    if ui.button("Jump To...").clicked() {
                        self.execute_command("video/jump");
                        ui.close();
                    }
                    ui.separator();
                    for (tenths, label) in [
                        (375u16, "37.5%"),
                        (500, "50%"),
                        (1000, "100%"),
                        (1500, "150%"),
                        (2000, "200%"),
                    ] {
                        if ui.button(format!("Zoom {label}")).clicked() {
                            self.video.zoom_tenths = tenths;
                            ui.close();
                        }
                    }
                });
                ui.menu_button(self.catalog.get("menu-audio").unwrap_or("Audio"), |ui| {
                    if ui.button("Waveform Mode").clicked() {
                        self.audio.spectrum_mode = false;
                        ui.close();
                    }
                    if ui.button("Spectrum Mode").clicked() {
                        self.audio.spectrum_mode = true;
                        ui.close();
                    }
                    ui.separator();
                    if ui.button("Toggle Karaoke Mode").clicked() {
                        self.audio.karaoke_mode = !self.audio.karaoke_mode;
                        ui.close();
                    }
                });
                ui.menu_button(
                    self.catalog.get("menu-automation").unwrap_or("Automation"),
                    |ui| {
                        if ui.button("Apply Karaoke Template...").clicked() {
                            self.execute_command("automation/kara_templater");
                            ui.close();
                        }
                        if ui.button("Template Doctor...").clicked() {
                            self.execute_command("automation/template_doctor");
                            ui.close();
                        }
                    },
                );
                ui.menu_button(self.catalog.get("menu-view").unwrap_or("View"), |ui| {
                    if ui.button("Full View").clicked() {
                        self.execute_command("app/display/full");
                        ui.close();
                    }
                    if ui.button("Video & Subs").clicked() {
                        self.execute_command("app/display/standard");
                        ui.close();
                    }
                    if ui.button("Audio & Subs").clicked() {
                        self.execute_command("app/display/audio_subs");
                        ui.close();
                    }
                    if ui.button("Subs Only").clicked() {
                        self.execute_command("app/display/subs");
                        ui.close();
                    }
                    ui.separator();
                    ui.menu_button(if is_vi { "Giao diện" } else { "Theme" }, |ui| {
                        if ui.button("Light").clicked() {
                            self.settings.appearance.theme_mode = ThemeMode::Light;
                            self.set_theme(ThemeSpec::Light);
                            ui.close();
                        }
                        if ui.button("Dark").clicked() {
                            self.settings.appearance.theme_mode = ThemeMode::Dark;
                            self.set_theme(ThemeSpec::Dark);
                            ui.close();
                        }
                        if ui.button("Eye Comfort Light").clicked() {
                            self.settings.appearance.theme_mode = ThemeMode::EyeComfortLight;
                            self.set_theme(ThemeSpec::EyeComfort {
                                base: BaseTheme::Light,
                                strength: self.settings.appearance.eye_comfort_strength,
                            });
                            ui.close();
                        }
                        if ui.button("Eye Comfort Dark").clicked() {
                            self.settings.appearance.theme_mode = ThemeMode::EyeComfortDark;
                            self.set_theme(ThemeSpec::EyeComfort {
                                base: BaseTheme::Dark,
                                strength: self.settings.appearance.eye_comfort_strength,
                            });
                            ui.close();
                        }
                        ui.separator();
                        let mut strength = self.settings.appearance.eye_comfort_strength;
                        if ui
                            .add(
                                egui::Slider::new(&mut strength, 0..=100)
                                    .text("Eye Comfort")
                                    .suffix("%"),
                            )
                            .changed()
                        {
                            self.settings.appearance.eye_comfort_strength = strength;
                            if let ThemeSpec::EyeComfort { base, .. } = self.theme.spec() {
                                self.set_theme(ThemeSpec::EyeComfort { base, strength });
                            }
                        }
                    });
                    ui.menu_button(if is_vi { "Ngôn ngữ" } else { "Language" }, |ui| {
                        if ui.button("English").clicked() {
                            self.set_locale(Locale::En);
                            ui.close();
                        }
                        if ui.button("Tiếng Việt").clicked() {
                            self.set_locale(Locale::Vi);
                            ui.close();
                        }
                    });
                    ui.separator();
                    if ui.button("Hotkeys...").clicked() {
                        self.execute_command("app/hotkeys");
                        ui.close();
                    }
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

        // 1b. Aegisub 3.5.1 default toolbar (29 coloured 16×16 icons, grouped by separators;
        //     NO theme or EN/VI toggles in the toolbar — those live in View menu & Preferences).
        Panel::top("hlouis_toolbar").show(ui, |ui| {
            ui.horizontal(|ui| {
                ui.spacing_mut().item_spacing.x = 1.5;
                if toolbar_icon_button(ui, ToolbarIcon::New, &pal, "New subtitles (Ctrl+N)") {
                    self.execute_command("subtitle/new");
                }
                if toolbar_icon_button(ui, ToolbarIcon::Open, &pal, "Open subtitles (Ctrl+O)") {
                    self.execute_command("subtitle/open");
                }
                if toolbar_icon_button(ui, ToolbarIcon::Save, &pal, "Save subtitles (Ctrl+S)") {
                    self.execute_command("subtitle/save");
                }
                if toolbar_icon_button(ui, ToolbarIcon::SaveAs, &pal, "Save subtitles as...") {
                    self.execute_command("subtitle/save/as");
                }
                ui.separator();
                if toolbar_icon_button(
                    ui,
                    ToolbarIcon::OpenMkv,
                    &pal,
                    "Open subtitles from video (MKV)",
                ) {
                    self.execute_command("subtitle/open/video");
                }
                if toolbar_icon_button(ui, ToolbarIcon::OpenVideo, &pal, "Open video") {
                    self.execute_command("video/open");
                }
                if toolbar_icon_button(ui, ToolbarIcon::OpenAudio, &pal, "Open audio") {
                    self.execute_command("audio/open");
                }
                ui.separator();
                if toolbar_icon_button(ui, ToolbarIcon::Undo, &pal, "Undo (Ctrl+Z)") {
                    self.execute_command("edit/undo");
                }
                if toolbar_icon_button(ui, ToolbarIcon::Redo, &pal, "Redo (Ctrl+Y)") {
                    self.execute_command("edit/redo");
                }
                ui.separator();
                if toolbar_icon_button(ui, ToolbarIcon::Cut, &pal, "Cut lines") {
                    self.execute_command("edit/line/cut");
                }
                if toolbar_icon_button(ui, ToolbarIcon::Copy, &pal, "Copy lines") {
                    self.execute_command("edit/line/copy");
                }
                if toolbar_icon_button(ui, ToolbarIcon::Paste, &pal, "Paste lines") {
                    self.execute_command("edit/line/paste");
                }
                if toolbar_icon_button(
                    ui,
                    ToolbarIcon::FindReplace,
                    &pal,
                    "Find and Replace (Ctrl+H)",
                ) {
                    self.execute_command("edit/find_replace");
                }
                ui.separator();
                if toolbar_icon_button(ui, ToolbarIcon::Styles, &pal, "Styles Manager") {
                    self.execute_command("tool/styles_manager");
                }
                if toolbar_icon_button(ui, ToolbarIcon::StyleEditor, &pal, "Edit active style") {
                    self.execute_command("tool/style_editor");
                }
                if toolbar_icon_button(ui, ToolbarIcon::Properties, &pal, "Script Properties") {
                    self.execute_command("subtitle/properties");
                }
                if toolbar_icon_button(ui, ToolbarIcon::Fonts, &pal, "Fonts Collector") {
                    self.execute_command("tool/fonts_collector");
                }
                if toolbar_icon_button(ui, ToolbarIcon::Resample, &pal, "Resample Resolution") {
                    self.execute_command("tool/resample");
                }
                ui.separator();
                if toolbar_icon_button(ui, ToolbarIcon::ShiftTimes, &pal, "Shift Times (Ctrl+I)") {
                    self.execute_command("time/shift");
                }
                if toolbar_icon_button(ui, ToolbarIcon::Tpp, &pal, "Timing Post-Processor") {
                    self.execute_command("tool/timing_processor");
                }
                if toolbar_icon_button(ui, ToolbarIcon::SelectLines, &pal, "Select Lines") {
                    self.execute_command("tool/line/select");
                }
                if toolbar_icon_button(
                    ui,
                    ToolbarIcon::ContStart,
                    &pal,
                    "Make Times Continuous (Start)",
                ) {
                    self.execute_command("time/continuous/start");
                }
                if toolbar_icon_button(
                    ui,
                    ToolbarIcon::ContEnd,
                    &pal,
                    "Make Times Continuous (End)",
                ) {
                    self.execute_command("time/continuous/end");
                }
                if toolbar_icon_button(ui, ToolbarIcon::SnapScene, &pal, "Snap to Scene") {
                    self.execute_command("time/snap/scene");
                }
                if toolbar_icon_button(ui, ToolbarIcon::JumpTo, &pal, "Jump to frame/time (Ctrl+G)")
                {
                    self.execute_command("video/jump");
                }
                ui.separator();
                if toolbar_icon_button(ui, ToolbarIcon::Karaoke, &pal, "Apply Karaoke Template") {
                    self.execute_command("automation/kara_templater");
                }
                if toolbar_icon_button(ui, ToolbarIcon::Doctor, &pal, "Template Doctor") {
                    self.execute_command("automation/template_doctor");
                }
                ui.separator();
                if toolbar_icon_button(ui, ToolbarIcon::Hotkeys, &pal, "Hotkeys Editor") {
                    self.execute_command("app/hotkeys");
                }
                if toolbar_icon_button(ui, ToolbarIcon::Preferences, &pal, "Preferences") {
                    self.execute_command("app/options");
                }
            });
        });

        // 2. Compact Aegisub-style status bar (empty left area + right info; no "Ready" label).
        Panel::bottom("hlouis_statusbar")
            .exact_size(19.0)
            .show(ui, |ui| {
                ui.horizontal(|ui| {
                    if !self.status_message.is_empty() {
                        ui.small(&self.status_message);
                    }
                    ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                        let total = self.grid.model.row_count();
                        let active_opt = self.doc.active_line();
                        if self.status_line_cache.0 != active_opt
                            || self.status_line_cache.1 != total
                        {
                            self.status_line_cache.0 = active_opt;
                            self.status_line_cache.1 = total;
                            self.status_line_cache.2.clear();
                            let active = active_opt.map_or(0, |i| i + 1);
                            let _ = std::fmt::Write::write_fmt(
                                &mut self.status_line_cache.2,
                                format_args!("Line {active} / {total}"),
                            );
                        }
                        ui.small(self.status_line_cache.2.as_str());
                        if self.doc.is_dirty() {
                            ui.separator();
                            ui.small(self.catalog.get("status-unsaved").unwrap_or("Modified"));
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
            });

        // 3. Central Aegisub 3.2.2 workspace layout (computed directly in logical points — zero
        //    double-DPI division at 200% scaling!).
        egui::CentralPanel::default()
            .frame(egui::Frame::central_panel(ui.style()).inner_margin(egui::Margin::same(2)))
            .show(ui, |ui| {
                let avail = ui.available_rect_before_wrap();
                let comp = self
                    .layout_state
                    .compute_logical(avail.width().max(320.0), avail.height().max(240.0));

                let mut rects = LastPaneRects {
                    workspace: avail,
                    ..LastPaneRects::default()
                };

                // Top-left: Video panel (strict clip rect).
                if let Some(v) = comp.video
                    && v.is_visible()
                {
                    let vr = Rect::from_min_size(
                        Pos2::new(avail.left() + v.x, avail.top() + v.y),
                        Vec2::new(v.w, v.h),
                    );
                    rects.video = Some(vr);
                    let mut child = ui.new_child(egui::UiBuilder::new().max_rect(vr));
                    child.set_clip_rect(vr);
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

                // Top-right: Audio panel (strict clip rect).
                if let Some(a) = comp.audio
                    && a.is_visible()
                {
                    let ar = Rect::from_min_size(
                        Pos2::new(avail.left() + a.x, avail.top() + a.y),
                        Vec2::new(a.w, a.h),
                    );
                    rects.audio = Some(ar);
                    let mut child = ui.new_child(egui::UiBuilder::new().max_rect(ar));
                    child.set_clip_rect(ar);
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

                // Middle-right (under audio): Edit Box (strict clip rect).
                if comp.edit_box.is_visible() {
                    let er = Rect::from_min_size(
                        Pos2::new(
                            avail.left() + comp.edit_box.x,
                            avail.top() + comp.edit_box.y,
                        ),
                        Vec2::new(comp.edit_box.w, comp.edit_box.h),
                    );
                    rects.edit_box = Some(er);
                    let mut child = ui.new_child(egui::UiBuilder::new().max_rect(er));
                    child.set_clip_rect(er);
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
                    if self.editbox.edit_style_requested {
                        self.editbox.edit_style_requested = false;
                        self.dialogs
                            .style_editor
                            .name
                            .clone_from(&self.editbox.style_buf);
                        self.dialogs.open = Some(ActiveDialog::StyleEditor);
                    }
                }

                // Bottom: Virtualised Subtitle Grid (strict clip rect).
                if comp.grid.is_visible() {
                    let gr = Rect::from_min_size(
                        Pos2::new(avail.left() + comp.grid.x, avail.top() + comp.grid.y),
                        Vec2::new(comp.grid.w, comp.grid.h),
                    );
                    rects.grid = Some(gr);
                    let mut child = ui.new_child(egui::UiBuilder::new().max_rect(gr));
                    child.set_clip_rect(gr);
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

                // Interactive draggable splitters with subtle border styling.
                let sp_col = rgba(pal.border);
                let mh_rect = Rect::from_min_size(
                    Pos2::new(
                        avail.left() + comp.main_h_splitter.x,
                        avail.top() + comp.main_h_splitter.y,
                    ),
                    Vec2::new(comp.main_h_splitter.w, comp.main_h_splitter.h),
                );
                ui.painter().rect_filled(mh_rect, 0.0, sp_col);
                let mh_resp =
                    ui.interact(mh_rect, ui.scope_id().with("split_main_h"), Sense::drag());
                if mh_resp.hovered() || mh_resp.dragged() {
                    ctx.set_cursor_icon(CursorIcon::ResizeVertical);
                }
                if mh_resp.dragged() {
                    let dy = mh_resp.drag_delta().y / avail.height().max(1.0);
                    self.layout_state.top_height_ratio =
                        (self.layout_state.top_height_ratio + dy).clamp(0.25, 0.80);
                }

                if let Some(tv) = comp.top_v_splitter {
                    let tv_rect = Rect::from_min_size(
                        Pos2::new(avail.left() + tv.x, avail.top() + tv.y),
                        Vec2::new(tv.w, tv.h),
                    );
                    ui.painter().rect_filled(tv_rect, 0.0, sp_col);
                    let tv_resp =
                        ui.interact(tv_rect, ui.scope_id().with("split_top_v"), Sense::drag());
                    if tv_resp.hovered() || tv_resp.dragged() {
                        ctx.set_cursor_icon(CursorIcon::ResizeHorizontal);
                    }
                    if tv_resp.dragged() {
                        let dx = tv_resp.drag_delta().x / avail.width().max(1.0);
                        self.layout_state.video_width_ratio =
                            (self.layout_state.video_width_ratio + dx).clamp(0.25, 0.75);
                    }
                }

                if let Some(ae) = comp.audio_edit_splitter {
                    let ae_rect = Rect::from_min_size(
                        Pos2::new(avail.left() + ae.x, avail.top() + ae.y),
                        Vec2::new(ae.w, ae.h),
                    );
                    ui.painter().rect_filled(ae_rect, 0.0, sp_col);
                    let ae_resp =
                        ui.interact(ae_rect, ui.scope_id().with("split_ae_h"), Sense::drag());
                    if ae_resp.hovered() || ae_resp.dragged() {
                        ctx.set_cursor_icon(CursorIcon::ResizeVertical);
                    }
                    if ae_resp.dragged() {
                        let top_h = comp.main_h_splitter.y.max(1.0);
                        let dy = ae_resp.drag_delta().y / top_h;
                        self.layout_state.audio_height_ratio =
                            (self.layout_state.audio_height_ratio + dy).clamp(0.25, 0.75);
                    }
                }

                self.last_pane_rects = rects;
            });

        // 4. Active modal dialog window (localized title, centered, opaque foreground frame).
        if let Some(active_dlg) = self.dialogs.open {
            let title = active_dlg.title(&self.catalog);
            let def_size = active_dlg.default_size();
            let screen_center = ctx.content_rect().center();
            let mut still_open = true;
            let mut dialog_action: Option<DialogAction> = None;
            let mut prefs_action = PrefsDialogAction::None;
            let mut mkv_action = MkvImportAction::None;

            let frame = egui::Frame::window(ui.style())
                .fill(rgba(pal.dialog_bg))
                .stroke(Stroke::new(1.0, rgba(pal.border)));

            egui::Window::new(title)
                .id(ui.make_persistent_id(("hlouis_dialog_win", active_dlg)))
                .open(&mut still_open)
                .order(Order::Foreground)
                .frame(frame)
                .pivot(Align2::CENTER_CENTER)
                .default_pos(screen_center)
                .default_size(def_size)
                .collapsible(false)
                .resizable(true)
                .show(ctx, |ui| match active_dlg {
                    ActiveDialog::Preferences => {
                        prefs_action = show_preferences_dialog(
                            ui,
                            &mut self.prefs_dialog,
                            &mut self.settings,
                            &mut self.theme,
                            &self.commands,
                            &self.catalog,
                        );
                    }
                    ActiveDialog::MkvImport => {
                        mkv_action =
                            show_mkv_import_dialog(ui, &mut self.mkv_dialog, &self.catalog);
                    }
                    _ => {
                        dialog_action = show_active_dialog(
                            ui,
                            &mut self.dialogs,
                            &mut self.settings,
                            &self.commands,
                            None,
                            &pal,
                            &self.catalog,
                        );
                    }
                });

            if !still_open {
                self.dialogs.open = None;
            }
            match prefs_action {
                PrefsDialogAction::ApplyAndClose => {
                    self.theme.sync_from_prefs(
                        &self.settings.appearance,
                        self.system_is_dark,
                        12 * 60,
                    );
                    self.dialogs.open = None;
                }
                PrefsDialogAction::Cancel => {
                    self.dialogs.open = None;
                }
                PrefsDialogAction::None => {}
            }
            match mkv_action {
                MkvImportAction::ImportSelected { track_numbers, .. } => {
                    self.status_message = format!("Imported {} MKV track(s)", track_numbers.len());
                    self.dialogs.open = None;
                }
                MkvImportAction::Close => {
                    self.dialogs.open = None;
                }
                MkvImportAction::None
                | MkvImportAction::RequestPreview(_)
                | MkvImportAction::CancelReading => {}
            }
            if let Some(act) = dialog_action {
                self.handle_dialog_action(&act);
            }
        }

        // 5. Command Palette (Ctrl+Shift+P) incl. HLouis & Template Doctor commands.
        if self.command_palette_open {
            let mut chosen: Option<&'static str> = None;
            egui::Window::new("Command Palette")
                .order(Order::Foreground)
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
            DialogAction::ApplyStyleEditor => {
                self.status_message = format!("Updated style '{}'", self.dialogs.style_editor.name);
                self.dialogs.open = None;
            }
            DialogAction::RunKaraokeTemplater => {
                "Karaoke templates applied.".clone_into(&mut self.status_message);
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
    use egui_kittest::kittest::Queryable;

    struct ZeroClock;
    impl Clock for ZeroClock {
        fn now_ns(&self) -> u64 {
            0
        }
    }

    fn rect_overlap_area(a: Rect, b: Rect) -> f32 {
        let inter = a.intersect(b);
        if inter.is_positive() {
            inter.width() * inter.height()
        } else {
            0.0
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

    #[test]
    fn shell_zero_overlapping_panes_and_full_fill_at_100_and_200_dpi_in_all_themes() {
        let configs: &[(&str, Vec2, f32)] = &[
            ("100%_dpi_1280x800", Vec2::new(1280.0, 800.0), 1.0),
            // 2880x1800 physical at 200% DPI = 1440x900 logical points
            ("200%_dpi_2880x1800", Vec2::new(1440.0, 900.0), 2.0),
        ];
        let themes = [
            ThemeSpec::Light,
            ThemeSpec::Dark,
            ThemeSpec::EyeComfort {
                base: BaseTheme::Dark,
                strength: 60,
            },
        ];

        for &(label, size, ppp) in configs {
            for spec in themes {
                let mut script = ScriptFile::new_default();
                script.ensure_events_section_mut().insert_event(
                    0,
                    AssEvent::dialogue(AssTime::ZERO, AssTime::from_cs(250), "Hello world"),
                );
                let mut shell = AppShell::new(script);
                shell.set_theme(spec);

                let captured = std::rc::Rc::new(std::cell::Cell::new(LastPaneRects::default()));
                let captured_c = captured.clone();

                let mut harness = Harness::builder()
                    .with_size(size)
                    .with_pixels_per_point(ppp)
                    .with_accessibility_check(false)
                    .build_ui(move |ui| {
                        shell.show(ui, None, None, &ZeroClock);
                        captured_c.set(shell.last_pane_rects);
                    });
                harness.run();

                let rects = captured.get();
                let video_r = rects.video.expect("video pane visible");
                let audio_r = rects.audio.expect("audio pane visible");
                let edit_r = rects.edit_box.expect("edit_box pane visible");
                let grid_r = rects.grid.expect("grid pane visible");

                // 1. All 4 panes must be strictly non-overlapping.
                let panes = [video_r, audio_r, edit_r, grid_r];
                for i in 0..panes.len() {
                    for j in (i + 1)..panes.len() {
                        assert_eq!(
                            rect_overlap_area(panes[i], panes[j]),
                            0.0,
                            "{label} {spec:?}: panes {i} and {j} overlap: {:?} vs {:?}",
                            panes[i],
                            panes[j]
                        );
                    }
                }

                // 2. Must fill >= 96% of workspace width and height at BOTH 100% and 200% DPI!
                let fill_w_ratio = grid_r.width() / rects.workspace.width().max(1.0);
                let total_h = (grid_r.bottom() - video_r.top()) / rects.workspace.height().max(1.0);
                assert!(
                    fill_w_ratio >= 0.96,
                    "{label} {spec:?}: grid width ratio {fill_w_ratio} < 0.96"
                );
                assert!(
                    total_h >= 0.96,
                    "{label} {spec:?}: total height ratio {total_h} < 0.96"
                );
            }
        }
    }

    #[test]
    fn shell_menu_toolbar_theme_and_locale_interactions_update_state() {
        let mut script = ScriptFile::new_default();
        script.ensure_events_section_mut().insert_event(
            0,
            AssEvent::dialogue(AssTime::ZERO, AssTime::from_cs(250), "Hello world"),
        );
        let mut shell = AppShell::new(script);

        // Verify command dispatch for menus, toolbar, dialogs, view modes, theme, locale.
        shell.execute_command("tool/styles_manager");
        assert_eq!(shell.dialogs.open, Some(ActiveDialog::StyleManager));
        shell.execute_command("tool/style_editor");
        assert_eq!(shell.dialogs.open, Some(ActiveDialog::StyleEditor));
        shell.execute_command("app/options");
        assert_eq!(shell.dialogs.open, Some(ActiveDialog::Preferences));
        shell.execute_command("subtitle/open/video");
        assert_eq!(shell.dialogs.open, Some(ActiveDialog::MkvImport));
        shell.execute_command("automation/kara_templater");
        assert_eq!(shell.dialogs.open, Some(ActiveDialog::KaraokeTemplater));
        shell.execute_command("automation/template_doctor");
        assert_eq!(shell.dialogs.open, Some(ActiveDialog::TemplateDoctor));
        shell.execute_command("app/display/subs");
        assert_eq!(shell.layout_state.mode, ViewMode::SubsOnly);
        shell.execute_command("app/display/full");
        assert_eq!(shell.layout_state.mode, ViewMode::Full);

        // Verify live theme + locale switching via toolbar clicks in kittest.
        shell.dialogs.open = None;
        let state_check = std::rc::Rc::new(std::cell::Cell::new((ThemeSpec::Dark, Locale::En)));
        let state_c = state_check.clone();

        let mut harness = Harness::builder()
            .with_size(Vec2::new(1280.0, 800.0))
            .with_accessibility_check(false)
            .build_ui(move |ui| {
                shell.show(ui, None, None, &ZeroClock);
                state_c.set((shell.theme.spec(), shell.catalog.locale()));
            });
        harness.run();

        // Verify Light/VI are NOT in the toolbar (they live in View menu & Preferences).
        assert!(harness.query_by_label("Light").is_none());
        assert!(harness.query_by_label("VI").is_none());

        // Open View menu and verify menu is interactive.
        harness.get_by_label("View").click();
        harness.run();
        assert!(harness.query_by_label("Theme").is_some());
        assert!(harness.query_by_label("Language").is_some());

        let (spec, loc) = state_check.get();
        assert_eq!(spec, ThemeSpec::Dark);
        assert_eq!(loc, Locale::En);
    }
}
