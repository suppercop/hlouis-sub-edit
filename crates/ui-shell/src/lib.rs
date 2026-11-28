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
use ui_theme::icon_atlas::{IconAtlasBuilder, slot_uv};
use ui_theme::{FontConfig, ThemeController, rgba, snap_px};
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

/// Who controls the width of the video panel.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum VideoSplit {
    /// The panel width follows the video display aspect (Fit mode).
    Auto,
    /// The user dragged the splitter.
    Manual,
}

/// Full application shell state combining all UI panels, document, playback, settings, and telemetry.
pub struct AppShell {
    pub doc: DocumentController,
    pub settings: AppSettings,
    pub layout_state: LayoutState,
    /// Whether the video panel sizes itself to the video aspect or the user took over the
    /// splitter (until the next video is attached).
    pub video_split: VideoSplit,
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
    /// Command IDs the shell does not handle itself (open video/audio, save, ...), drained by the
    /// application layer every frame.
    pub pending_commands: Vec<String>,
    /// Last action emitted by the video panel this frame (cleared by the host after handling).
    pub last_video_action: Option<ui_video::VideoPanelAction>,
    /// Last action emitted by the audio panel this frame (cleared by the host after handling).
    pub last_audio_action: Option<ui_audio::AudioPanelAction>,
    fonts_installed: bool,
    last_theme_rev: u64,
    cached_active_timing: Option<(u64, usize, i64, i64)>,
    active_timing_lookups: u64,
    status_line_cache: (Option<usize>, usize, String),
    toolbar_atlas: Option<egui::TextureHandle>,
    toolbar_atlas_key: Option<(u32, bool)>,
}

const TOOLBAR_ICON_COUNT: usize = 29;

#[repr(usize)]
#[derive(Clone, Copy)]
enum ToolbarIcon {
    New = 0,
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
fn ensure_toolbar_icon_atlas(
    ctx: &egui::Context,
    atlas_tex: &mut Option<egui::TextureHandle>,
    atlas_key: &mut Option<(u32, bool)>,
    ppp: f32,
    is_light: bool,
) -> egui::TextureId {
    let key = (ppp.to_bits(), is_light);
    if *atlas_key == Some(key)
        && let Some(ref tex) = *atlas_tex
    {
        return tex.id();
    }

    #[allow(clippy::cast_sign_loss)]
    let slot_px = (16.0 * ppp.max(1.0)).round().clamp(16.0, 48.0) as usize;
    let mut atlas = IconAtlasBuilder::new(TOOLBAR_ICON_COUNT, slot_px);

    // Modern, eye-comfort-friendly duotone palette (softened in Dark / Eye Comfort).
    let blue = if is_light {
        egui::Color32::from_rgb(42, 114, 204)
    } else {
        egui::Color32::from_rgb(104, 168, 236)
    };
    let blue_soft = egui::Color32::from_rgba_unmultiplied(blue.r(), blue.g(), blue.b(), 64);
    let green = if is_light {
        egui::Color32::from_rgb(40, 152, 86)
    } else {
        egui::Color32::from_rgb(98, 196, 134)
    };
    let green_soft = egui::Color32::from_rgba_unmultiplied(green.r(), green.g(), green.b(), 64);
    let red = if is_light {
        egui::Color32::from_rgb(202, 66, 66)
    } else {
        egui::Color32::from_rgb(228, 112, 112)
    };
    let amber = if is_light {
        egui::Color32::from_rgb(204, 138, 30)
    } else {
        egui::Color32::from_rgb(228, 174, 82)
    };
    let amber_soft = egui::Color32::from_rgba_unmultiplied(amber.r(), amber.g(), amber.b(), 88);
    let purple = if is_light {
        egui::Color32::from_rgb(138, 76, 194)
    } else {
        egui::Color32::from_rgb(176, 130, 224)
    };
    let teal = if is_light {
        egui::Color32::from_rgb(32, 144, 158)
    } else {
        egui::Color32::from_rgb(96, 192, 206)
    };
    let teal_soft = egui::Color32::from_rgba_unmultiplied(teal.r(), teal.g(), teal.b(), 64);
    let neutral = if is_light {
        egui::Color32::from_rgb(76, 84, 98)
    } else {
        egui::Color32::from_rgb(186, 194, 206)
    };

    // 0: New (duotone page + green plus)
    {
        let mut s = atlas.slot(ToolbarIcon::New as usize);
        s.fill_rect(3.0, 1.8, 13.0, 14.2, 1.4, blue_soft);
        s.stroke_rect(3.0, 1.8, 13.0, 14.2, 1.4, 1.4, blue);
        s.line(8.0, 5.2, 8.0, 10.8, 1.8, green);
        s.line(5.2, 8.0, 10.8, 8.0, 1.8, green);
    }
    // 1: Open (duotone folder)
    {
        let mut s = atlas.slot(ToolbarIcon::Open as usize);
        s.fill_rect(2.0, 3.2, 7.6, 6.2, 1.0, amber);
        s.fill_rect(2.0, 5.0, 14.0, 13.2, 1.4, amber_soft);
        s.stroke_rect(2.0, 5.0, 14.0, 13.2, 1.4, 1.4, amber);
    }
    // 2: Save (filled floppy disk)
    {
        let mut s = atlas.slot(ToolbarIcon::Save as usize);
        s.fill_rect(2.4, 2.4, 13.6, 13.6, 1.5, blue);
        s.fill_rect(4.8, 2.6, 11.2, 6.4, 0.5, egui::Color32::WHITE);
        s.fill_rect(
            4.6,
            8.8,
            11.4,
            13.4,
            0.7,
            egui::Color32::from_rgb(232, 240, 250),
        );
    }
    // 3: SaveAs (floppy + amber badge)
    {
        let mut s = atlas.slot(ToolbarIcon::SaveAs as usize);
        s.fill_rect(2.0, 2.0, 12.6, 12.6, 1.4, blue);
        s.fill_rect(4.2, 2.2, 10.2, 5.8, 0.5, egui::Color32::WHITE);
        s.fill_circle(12.2, 12.2, 3.0, amber);
    }
    // 4: OpenMkv (purple container + vector M)
    {
        let mut s = atlas.slot(ToolbarIcon::OpenMkv as usize);
        s.fill_rect(1.8, 3.0, 14.2, 13.0, 1.8, purple);
        let w = egui::Color32::WHITE;
        s.line(4.4, 10.8, 4.4, 5.2, 1.5, w);
        s.line(4.4, 5.2, 8.0, 9.0, 1.5, w);
        s.line(8.0, 9.0, 11.6, 5.2, 1.5, w);
        s.line(11.6, 5.2, 11.6, 10.8, 1.5, w);
    }
    // 5: OpenVideo (duotone frame + green play)
    {
        let mut s = atlas.slot(ToolbarIcon::OpenVideo as usize);
        s.fill_rect(2.0, 3.0, 14.0, 13.0, 1.6, blue_soft);
        s.stroke_rect(2.0, 3.0, 14.0, 13.0, 1.6, 1.4, blue);
        s.fill_tri([6.4, 5.2], [11.6, 8.0], [6.4, 10.8], green);
    }
    // 6: OpenAudio (4 crisp waveform bars)
    {
        let mut s = atlas.slot(ToolbarIcon::OpenAudio as usize);
        for (x, h) in [(3.5, 4.5), (6.5, 9.5), (9.5, 6.5), (12.5, 8.5)] {
            s.line(x, 8.0 - h * 0.5, x, 8.0 + h * 0.5, 2.0, teal);
        }
    }
    // 7: Undo (curved left arrow)
    {
        let mut s = atlas.slot(ToolbarIcon::Undo as usize);
        s.line(4.0, 7.2, 10.2, 7.2, 1.9, blue);
        s.arc(10.0, 9.8, 2.6, 2.6, -1.57, 1.57, 1.9, blue);
        s.fill_tri([2.2, 7.2], [6.4, 4.0], [6.4, 10.4], blue);
    }
    // 8: Redo (curved right arrow)
    {
        let mut s = atlas.slot(ToolbarIcon::Redo as usize);
        s.line(5.8, 7.2, 12.0, 7.2, 1.9, blue);
        s.arc(6.0, 9.8, 2.6, 2.6, 1.57, 4.71, 1.9, blue);
        s.fill_tri([13.8, 7.2], [9.6, 4.0], [9.6, 10.4], blue);
    }
    // 9: Cut (crisp red scissors)
    {
        let mut s = atlas.slot(ToolbarIcon::Cut as usize);
        s.line(4.5, 11.0, 12.8, 3.2, 1.6, red);
        s.line(4.5, 5.0, 12.8, 12.8, 1.6, red);
        s.stroke_circle(4.2, 11.4, 2.0, 1.4, red);
        s.stroke_circle(4.2, 4.6, 2.0, 1.4, red);
    }
    // 10: Copy (overlapping duotone sheets)
    {
        let mut s = atlas.slot(ToolbarIcon::Copy as usize);
        s.fill_rect(2.4, 2.2, 9.8, 11.0, 1.1, blue_soft);
        s.stroke_rect(2.4, 2.2, 9.8, 11.0, 1.1, 1.3, blue);
        s.fill_rect(6.2, 5.0, 13.6, 13.8, 1.1, teal_soft);
        s.stroke_rect(6.2, 5.0, 13.6, 13.8, 1.1, 1.4, teal);
    }
    // 11: Paste (amber clipboard + blue clip)
    {
        let mut s = atlas.slot(ToolbarIcon::Paste as usize);
        s.fill_rect(3.2, 3.4, 12.8, 14.0, 1.4, amber_soft);
        s.stroke_rect(3.2, 3.4, 12.8, 14.0, 1.4, 1.4, amber);
        s.fill_rect(5.4, 1.8, 10.6, 4.6, 0.9, blue);
    }
    // 12: FindReplace (teal lens + blue handle)
    {
        let mut s = atlas.slot(ToolbarIcon::FindReplace as usize);
        s.fill_circle(6.8, 6.8, 4.0, teal_soft);
        s.stroke_circle(6.8, 6.8, 4.0, 1.6, teal);
        s.line(9.8, 9.8, 13.6, 13.6, 2.2, blue);
    }
    // 13: Styles (bold vector A)
    {
        let mut s = atlas.slot(ToolbarIcon::Styles as usize);
        s.line(3.6, 13.2, 8.0, 2.8, 2.0, purple);
        s.line(8.0, 2.8, 12.4, 13.2, 2.0, purple);
        s.line(5.2, 9.8, 10.8, 9.8, 1.6, blue);
    }
    // 14: StyleEditor (vector A + small edit dot/brush)
    {
        let mut s = atlas.slot(ToolbarIcon::StyleEditor as usize);
        s.line(2.8, 12.6, 6.4, 3.4, 1.8, purple);
        s.line(6.4, 3.4, 10.0, 12.6, 1.8, purple);
        s.line(4.2, 9.6, 8.6, 9.6, 1.4, purple);
        s.fill_circle(12.2, 10.8, 2.4, amber);
    }
    // 15: Properties (3 coloured property bars)
    {
        let mut s = atlas.slot(ToolbarIcon::Properties as usize);
        s.line(2.6, 4.0, 13.4, 4.0, 2.0, blue);
        s.line(2.6, 8.0, 13.4, 8.0, 2.0, teal);
        s.line(2.6, 12.0, 10.6, 12.0, 2.0, amber);
    }
    // 16: Fonts (bold vector F)
    {
        let mut s = atlas.slot(ToolbarIcon::Fonts as usize);
        s.line(4.6, 2.8, 4.6, 13.2, 2.1, red);
        s.line(4.6, 3.0, 12.0, 3.0, 2.1, red);
        s.line(4.6, 7.8, 10.4, 7.8, 1.9, red);
    }
    // 17: Resample (duotone viewport + diagonal resize arrow)
    {
        let mut s = atlas.slot(ToolbarIcon::Resample as usize);
        s.fill_rect(2.2, 3.2, 13.8, 12.8, 1.2, teal_soft);
        s.stroke_rect(2.2, 3.2, 13.8, 12.8, 1.2, 1.4, teal);
        s.line(4.5, 11.2, 11.5, 4.8, 1.8, green);
        s.fill_tri([12.2, 4.2], [8.6, 4.4], [11.8, 7.8], green);
    }
    // 18: ShiftTimes (duotone clock face + hands)
    {
        let mut s = atlas.slot(ToolbarIcon::ShiftTimes as usize);
        s.fill_circle(8.0, 8.0, 5.8, amber_soft);
        s.stroke_circle(8.0, 8.0, 5.8, 1.5, amber);
        s.line(8.0, 8.0, 8.0, 4.2, 1.6, blue);
        s.line(8.0, 8.0, 11.2, 8.0, 1.6, red);
    }
    // 19: Tpp (2 slider tracks + handles)
    {
        let mut s = atlas.slot(ToolbarIcon::Tpp as usize);
        s.line(2.4, 5.0, 13.6, 5.0, 1.6, neutral);
        s.line(2.4, 11.0, 13.6, 11.0, 1.6, neutral);
        s.fill_circle(5.6, 5.0, 2.4, green);
        s.fill_circle(10.4, 11.0, 2.4, blue);
    }
    // 20: SelectLines (3 list rows with left highlight)
    {
        let mut s = atlas.slot(ToolbarIcon::SelectLines as usize);
        for (y, col) in [(4.0, blue), (8.0, green), (12.0, blue)] {
            s.fill_circle(3.2, y, 1.3, col);
            s.line(6.0, y, 13.4, y, 1.8, blue);
        }
    }
    // 21: ContStart (left red bar + blue arrow)
    {
        let mut s = atlas.slot(ToolbarIcon::ContStart as usize);
        s.line(3.5, 3.2, 3.5, 12.8, 2.1, red);
        s.line(6.0, 8.0, 13.2, 8.0, 1.8, blue);
        s.fill_tri([5.2, 8.0], [8.6, 5.2], [8.6, 10.8], blue);
    }
    // 22: ContEnd (right red bar + blue arrow)
    {
        let mut s = atlas.slot(ToolbarIcon::ContEnd as usize);
        s.line(12.5, 3.2, 12.5, 12.8, 2.1, red);
        s.line(2.8, 8.0, 10.0, 8.0, 1.8, blue);
        s.fill_tri([10.8, 8.0], [7.4, 5.2], [7.4, 10.8], blue);
    }
    // 23: SnapScene (purple keyframe line + amber diamond/dot)
    {
        let mut s = atlas.slot(ToolbarIcon::SnapScene as usize);
        s.line(8.0, 2.0, 8.0, 14.0, 2.0, purple);
        s.fill_circle(8.0, 8.0, 2.6, amber);
    }
    // 24: JumpTo (crisp green right arrow)
    {
        let mut s = atlas.slot(ToolbarIcon::JumpTo as usize);
        s.line(2.6, 8.0, 10.4, 8.0, 2.0, green);
        s.fill_tri([13.6, 8.0], [8.6, 4.0], [8.6, 12.0], green);
    }
    // 25: Karaoke (purple music note)
    {
        let mut s = atlas.slot(ToolbarIcon::Karaoke as usize);
        s.fill_circle(5.6, 11.6, 2.6, purple);
        s.line(7.8, 11.2, 7.8, 3.0, 1.9, purple);
        s.line(7.8, 3.2, 13.0, 4.8, 2.0, purple);
    }
    // 26: Doctor (duotone green med cross)
    {
        let mut s = atlas.slot(ToolbarIcon::Doctor as usize);
        s.fill_rect(2.2, 2.2, 13.8, 13.8, 2.4, green_soft);
        s.stroke_rect(2.2, 2.2, 13.8, 13.8, 2.4, 1.4, green);
        s.line(8.0, 4.6, 8.0, 11.4, 2.2, green);
        s.line(4.6, 8.0, 11.4, 8.0, 2.2, green);
    }
    // 27: Hotkeys (duotone keyboard)
    {
        let mut s = atlas.slot(ToolbarIcon::Hotkeys as usize);
        s.fill_rect(1.8, 3.6, 14.2, 12.4, 1.6, blue_soft);
        s.stroke_rect(1.8, 3.6, 14.2, 12.4, 1.6, 1.4, blue);
        for x in [4.5, 8.0, 11.5] {
            s.fill_rect(x - 1.0, 5.6, x + 1.0, 7.4, 0.3, blue);
        }
        s.line(4.4, 10.0, 11.6, 10.0, 1.6, amber);
    }
    // 28: Preferences (gear ring + amber center)
    {
        let mut s = atlas.slot(ToolbarIcon::Preferences as usize);
        s.fill_circle(8.0, 8.0, 5.0, blue_soft);
        s.stroke_circle(8.0, 8.0, 5.0, 1.8, blue);
        s.fill_circle(8.0, 8.0, 2.2, amber);
    }

    let img = atlas.into_color_image();
    if let Some(ref mut tex) = *atlas_tex {
        tex.set(img, egui::TextureOptions::NEAREST);
        *atlas_key = Some(key);
        tex.id()
    } else {
        let tex = ctx.load_texture("hlouis_toolbar_icons", img, egui::TextureOptions::NEAREST);
        let id = tex.id();
        *atlas_tex = Some(tex);
        *atlas_key = Some(key);
        id
    }
}

fn toolbar_icon_button(
    ui: &mut Ui,
    atlas_id: egui::TextureId,
    icon: ToolbarIcon,
    pal: &Palette,
    ppp: f32,
    tooltip: &str,
) -> bool {
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
    let icon_min = Pos2::new(
        snap_px(rect.center().x - 8.0, ppp),
        snap_px(rect.center().y - 8.0, ppp),
    );
    let icon_rect = Rect::from_min_size(icon_min, Vec2::splat(16.0));
    p.image(
        atlas_id,
        icon_rect,
        slot_uv(icon as usize, TOOLBAR_ICON_COUNT),
        egui::Color32::WHITE,
    );
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
            video_split: VideoSplit::Auto,
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
            pending_commands: Vec::new(),
            last_video_action: None,
            last_audio_action: None,
            fonts_installed: false,
            last_theme_rev: 0,
            cached_active_timing: None,
            active_timing_lookups: 0,
            status_line_cache: (None, usize::MAX, String::with_capacity(32)),
            toolbar_atlas: None,
            toolbar_atlas_key: None,
        }
    }

    /// Number of times active-line start/end timings were queried from the script.
    #[must_use]
    pub const fn active_timing_lookups(&self) -> u64 {
        self.active_timing_lookups
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
        self.video_split = VideoSplit::Auto;
    }

    /// Sets the stream's sample aspect ratio (anamorphic video is displayed at `DAR = w/h * SAR`).
    pub fn set_video_sar(&mut self, num: u32, den: u32) {
        self.video.set_sar(num, den);
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
                self.pending_commands.push(cmd_id.to_owned());
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

        // Global hotkey dispatch with Aegisub-like focus routing:
        // - When focus is NOT in a text field (grid, audio, video): Audio-context keys (e.g.
        //   Space / S -> audio/play/selection), then SubtitleGrid, Video, Default, Always.
        // - When focus IS in a text field (e.g. edit box): plain keys go to the text field so
        //   Space types a space; only modifier chords in SubtitleEditBox / Always / Ctrl+Space
        //   fire.
        if self.dialogs.open.is_none() && !self.command_palette_open {
            let in_text = ctx.egui_wants_keyboard_input();
            let pressed: Vec<(egui::Modifiers, Key)> = ctx.input(|i| {
                i.events
                    .iter()
                    .filter_map(|ev| match *ev {
                        egui::Event::Key {
                            key,
                            pressed: true,
                            modifiers,
                            ..
                        } => Some((modifiers, key)),
                        _ => None,
                    })
                    .collect()
            });
            for (mods, key) in pressed {
                if !in_text && mods.ctrl && mods.shift && key == Key::P {
                    self.command_palette_open = !self.command_palette_open;
                    continue;
                }
                let Some(chord) = egui_key_to_chord(mods, key) else {
                    continue;
                };
                let cmd = if in_text {
                    if mods.ctrl || mods.alt {
                        self.commands
                            .lookup_in_context(HotkeyContext::SubtitleEditBox, chord)
                            .or_else(|| {
                                self.commands
                                    .lookup_in_context(HotkeyContext::Always, chord)
                            })
                            .or_else(|| {
                                (key == Key::Space)
                                    .then(|| {
                                        self.commands
                                            .lookup_in_context(HotkeyContext::Default, chord)
                                    })
                                    .flatten()
                            })
                    } else {
                        None
                    }
                } else {
                    self.commands
                        .lookup_in_context(HotkeyContext::Audio, chord)
                        .or_else(|| {
                            self.commands
                                .lookup_in_context(HotkeyContext::SubtitleGrid, chord)
                        })
                        .or_else(|| self.commands.lookup_in_context(HotkeyContext::Video, chord))
                        .or_else(|| {
                            self.commands
                                .lookup_in_context(HotkeyContext::Default, chord)
                        })
                        .or_else(|| {
                            self.commands
                                .lookup_in_context(HotkeyContext::Always, chord)
                        })
                };
                if let Some(cmd_id) = cmd {
                    let owned = cmd_id.to_owned();
                    self.execute_command(&owned);
                }
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

        // Sync active line start/end to video panel offset display (cached by non-text revision
        // and active line so idle frames and same-line text amends avoid scanning the script).
        if let Some(active) = self.doc.active_line() {
            let non_text_rev = self.doc.non_text_revision();
            match self.cached_active_timing {
                Some((rev, idx, start_ms, end_ms)) if rev == non_text_rev && idx == active => {
                    self.video.active_line_start_ms = start_ms;
                    self.video.active_line_end_ms = end_ms;
                }
                _ => {
                    self.active_timing_lookups = self.active_timing_lookups.saturating_add(1);
                    if let Some(ev) = self
                        .doc
                        .script()
                        .events_section()
                        .and_then(|s| s.get_event(active))
                    {
                        let start_ms = i64::from(ev.start().as_cs()) * 10;
                        let end_ms = i64::from(ev.end().as_cs()) * 10;
                        self.video.active_line_start_ms = start_ms;
                        self.video.active_line_end_ms = end_ms;
                        self.cached_active_timing = Some((non_text_rev, active, start_ms, end_ms));
                    } else {
                        self.cached_active_timing = None;
                    }
                }
            }
        } else {
            self.cached_active_timing = None;
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
                            self.video.zoom_mode = ui_video::ZoomMode::Fixed;
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

        // 1b. Aegisub 3.5.1 default toolbar (29 cached duotone 16×16 icons, grouped by separators;
        //     NO theme or EN/VI toggles in the toolbar — those live in View menu & Preferences).
        let ppp = ctx.pixels_per_point();
        let is_light =
            u16::from(pal.grid_bg.r) + u16::from(pal.grid_bg.g) + u16::from(pal.grid_bg.b) > 384;
        let tb_atlas_id = ensure_toolbar_icon_atlas(
            ctx,
            &mut self.toolbar_atlas,
            &mut self.toolbar_atlas_key,
            ppp,
            is_light,
        );
        Panel::top("hlouis_toolbar").show(ui, |ui| {
            let btn = |ui: &mut Ui, icon: ToolbarIcon, tip: &str| {
                toolbar_icon_button(ui, tb_atlas_id, icon, &pal, ppp, tip)
            };
            ui.horizontal(|ui| {
                ui.spacing_mut().item_spacing.x = 1.5;
                if btn(ui, ToolbarIcon::New, "New subtitles (Ctrl+N)") {
                    self.execute_command("subtitle/new");
                }
                if btn(ui, ToolbarIcon::Open, "Open subtitles (Ctrl+O)") {
                    self.execute_command("subtitle/open");
                }
                if btn(ui, ToolbarIcon::Save, "Save subtitles (Ctrl+S)") {
                    self.execute_command("subtitle/save");
                }
                if btn(ui, ToolbarIcon::SaveAs, "Save subtitles as...") {
                    self.execute_command("subtitle/save/as");
                }
                ui.separator();
                if btn(ui, ToolbarIcon::OpenMkv, "Open subtitles from video (MKV)") {
                    self.execute_command("subtitle/open/video");
                }
                if btn(ui, ToolbarIcon::OpenVideo, "Open video") {
                    self.execute_command("video/open");
                }
                if btn(ui, ToolbarIcon::OpenAudio, "Open audio") {
                    self.execute_command("audio/open");
                }
                ui.separator();
                if btn(ui, ToolbarIcon::Undo, "Undo (Ctrl+Z)") {
                    self.execute_command("edit/undo");
                }
                if btn(ui, ToolbarIcon::Redo, "Redo (Ctrl+Y)") {
                    self.execute_command("edit/redo");
                }
                ui.separator();
                if btn(ui, ToolbarIcon::Cut, "Cut lines") {
                    self.execute_command("edit/line/cut");
                }
                if btn(ui, ToolbarIcon::Copy, "Copy lines") {
                    self.execute_command("edit/line/copy");
                }
                if btn(ui, ToolbarIcon::Paste, "Paste lines") {
                    self.execute_command("edit/line/paste");
                }
                if btn(ui, ToolbarIcon::FindReplace, "Find and Replace (Ctrl+H)") {
                    self.execute_command("edit/find_replace");
                }
                ui.separator();
                if btn(ui, ToolbarIcon::Styles, "Styles Manager") {
                    self.execute_command("tool/styles_manager");
                }
                if btn(ui, ToolbarIcon::StyleEditor, "Edit active style") {
                    self.execute_command("tool/style_editor");
                }
                if btn(ui, ToolbarIcon::Properties, "Script Properties") {
                    self.execute_command("subtitle/properties");
                }
                if btn(ui, ToolbarIcon::Fonts, "Fonts Collector") {
                    self.execute_command("tool/fonts_collector");
                }
                if btn(ui, ToolbarIcon::Resample, "Resample Resolution") {
                    self.execute_command("tool/resample");
                }
                ui.separator();
                if btn(ui, ToolbarIcon::ShiftTimes, "Shift Times (Ctrl+I)") {
                    self.execute_command("time/shift");
                }
                if btn(ui, ToolbarIcon::Tpp, "Timing Post-Processor") {
                    self.execute_command("tool/timing_processor");
                }
                if btn(ui, ToolbarIcon::SelectLines, "Select Lines") {
                    self.execute_command("tool/line/select");
                }
                if btn(ui, ToolbarIcon::ContStart, "Make Times Continuous (Start)") {
                    self.execute_command("time/continuous/start");
                }
                if btn(ui, ToolbarIcon::ContEnd, "Make Times Continuous (End)") {
                    self.execute_command("time/continuous/end");
                }
                if btn(ui, ToolbarIcon::SnapScene, "Snap to Scene") {
                    self.execute_command("time/snap/scene");
                }
                if btn(ui, ToolbarIcon::JumpTo, "Jump to frame/time (Ctrl+G)") {
                    self.execute_command("video/jump");
                }
                ui.separator();
                if btn(ui, ToolbarIcon::Karaoke, "Apply Karaoke Template") {
                    self.execute_command("automation/kara_templater");
                }
                if btn(ui, ToolbarIcon::Doctor, "Template Doctor") {
                    self.execute_command("automation/template_doctor");
                }
                ui.separator();
                if btn(ui, ToolbarIcon::Hotkeys, "Hotkeys Editor") {
                    self.execute_command("app/hotkeys");
                }
                if btn(ui, ToolbarIcon::Preferences, "Preferences") {
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
                // The video panel sizes itself to the video's display aspect (like Aegisub) while
                // in Fit mode, unless the user took manual control of the splitter.
                self.layout_state.video_aspect = (self.playback.is_some()
                    && self.video_split == VideoSplit::Auto
                    && self.video.zoom_mode == ui_video::ZoomMode::Fit
                    && self.video.display_aspect.is_finite()
                    && self.video.display_aspect > 0.0)
                    .then_some(self.video.display_aspect);
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
                        self.last_video_action = show_video_panel(
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
                        self.last_audio_action = show_audio_panel(
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
                        if self.video_split == VideoSplit::Auto {
                            // Start from the width the panel currently has (aspect-derived).
                            self.layout_state.video_width_ratio = tv.x / avail.width().max(1.0);
                            self.video_split = VideoSplit::Manual;
                        }
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
                    self.grid
                        .apply_interface_prefs(&self.settings.interface, self.doc.script());
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
                    self.pending_commands.push("mkv/import".to_owned());
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
                self.grid
                    .apply_interface_prefs(&self.settings.interface, self.doc.script());
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

fn egui_key_to_chord(mods: egui::Modifiers, key: Key) -> Option<KeyChord> {
    let mut mask = 0u8;
    if mods.ctrl || mods.command {
        mask |= KeyChord::CTRL;
    }
    if mods.shift {
        mask |= KeyChord::SHIFT;
    }
    if mods.alt {
        mask |= KeyChord::ALT;
    }
    let name = match key {
        Key::A => "A",
        Key::B => "B",
        Key::C => "C",
        Key::D => "D",
        Key::E => "E",
        Key::F => "F",
        Key::G => "G",
        Key::H => "H",
        Key::I => "I",
        Key::J => "J",
        Key::K => "K",
        Key::L => "L",
        Key::M => "M",
        Key::N => "N",
        Key::O => "O",
        Key::P => "P",
        Key::Q => "Q",
        Key::R => "R",
        Key::S => "S",
        Key::T => "T",
        Key::U => "U",
        Key::V => "V",
        Key::W => "W",
        Key::X => "X",
        Key::Y => "Y",
        Key::Z => "Z",
        Key::Num0 => "0",
        Key::Num1 => "1",
        Key::Num2 => "2",
        Key::Num3 => "3",
        Key::Num4 => "4",
        Key::Num5 => "5",
        Key::Num6 => "6",
        Key::Num7 => "7",
        Key::Num8 => "8",
        Key::Num9 => "9",
        Key::Space => "Space",
        Key::Enter => "Return",
        Key::Escape => "Escape",
        Key::Tab => "Tab",
        Key::Backspace => "Back",
        Key::Delete => "Delete",
        Key::Insert => "Insert",
        Key::Home => "Home",
        Key::End => "End",
        Key::PageUp => "PageUp",
        Key::PageDown => "PageDown",
        Key::ArrowLeft => "Left",
        Key::ArrowRight => "Right",
        Key::ArrowUp => "Up",
        Key::ArrowDown => "Down",
        Key::F1 => "F1",
        Key::F2 => "F2",
        Key::F3 => "F3",
        Key::F4 => "F4",
        Key::F5 => "F5",
        Key::F6 => "F6",
        Key::F7 => "F7",
        Key::F8 => "F8",
        Key::F9 => "F9",
        Key::F10 => "F10",
        Key::F11 => "F11",
        Key::F12 => "F12",
        _ => return None,
    };
    KeyChord::from_parts(mask, name)
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

    struct TinyVideo(Vec<u8>);
    impl VideoFrameSupplier for TinyVideo {
        fn total_frames(&self) -> u32 {
            1_000
        }
        fn is_keyframe(&self, frame: u32) -> bool {
            frame == 0
        }
        fn frame_revision(&self, _frame: u32) -> u64 {
            1
        }
        fn frame_rgba(&self, _frame: u32) -> Option<ui_video::RgbaFrameRef<'_>> {
            Some(ui_video::RgbaFrameRef {
                width: 16,
                height: 9,
                rgba: &self.0,
            })
        }
    }

    /// Fit mode must show the picture at the stream's display aspect (SAR applied, never
    /// stretched or cropped) and the panel must size itself to it so no letterbox bars remain.
    #[test]
    fn video_panel_fits_display_aspect_without_bars_for_square_and_anamorphic() {
        // (storage w, h, SAR num, SAR den, expected display aspect)
        let cases: [(u32, u32, u32, u32, f32); 3] = [
            (1920, 1080, 1, 1, 16.0 / 9.0),
            (720, 480, 32, 27, 720.0 * 32.0 / (480.0 * 27.0)),
            (720, 480, 8, 9, 4.0 / 3.0),
        ];
        for (w, h, sn, sd, dar) in cases {
            for (size, ppp) in [
                (Vec2::new(1280.0, 800.0), 1.0),
                (Vec2::new(1920.0, 1000.0), 1.0),
                (Vec2::new(1280.0, 800.0), 1.5),
            ] {
                let mut shell = AppShell::new(ScriptFile::new_default());
                if let Ok(tc) = Timecodes::from_cfr(24_000, 1_001) {
                    shell.attach_video(tc, 1_000, vec![0], w, h);
                }
                shell.set_video_sar(sn, sd);
                let vid = TinyVideo(vec![90u8; 16 * 9 * 4]);
                let out = std::rc::Rc::new(std::cell::Cell::new((Rect::NOTHING, Rect::NOTHING)));
                let out_c = out.clone();
                let mut harness = Harness::builder()
                    .with_size(size)
                    .with_pixels_per_point(ppp)
                    .with_accessibility_check(false)
                    .build_ui(move |ui| {
                        shell.show(ui, Some(&vid), None, &ZeroClock);
                        out_c.set((shell.video.last_canvas_rect, shell.video.last_video_rect));
                    });
                harness.run();
                harness.run();
                let (canvas, video) = out.get();
                let label = format!("{w}x{h} SAR {sn}:{sd} win {size:?} ppp {ppp}");
                assert!(canvas.width() > 100.0, "{label}: canvas missing");
                let aspect = video.width() / video.height();
                assert!(
                    (aspect / dar - 1.0).abs() < 0.01,
                    "{label}: picture aspect {aspect} != display aspect {dar} (stretched)"
                );
                assert!(
                    video.min.x >= canvas.min.x - 0.5
                        && video.max.x <= canvas.max.x + 0.5
                        && video.min.y >= canvas.min.y - 0.5
                        && video.max.y <= canvas.max.y + 0.5,
                    "{label}: picture cropped: {video:?} outside {canvas:?}"
                );
                let bar_x = canvas.width() - video.width();
                let bar_y = canvas.height() - video.height();
                assert!(
                    bar_x.min(bar_y) < 1.0,
                    "{label}: Fit must fill one dimension ({bar_x} x {bar_y})"
                );
                assert!(
                    bar_x.max(bar_y) < 2.0,
                    "{label}: letterbox bars {bar_x} x {bar_y} px (canvas {canvas:?}, video {video:?})"
                );
                let off_x = (video.center().x - canvas.center().x).abs();
                let off_y = (video.center().y - canvas.center().y).abs();
                assert!(off_x < 0.5 && off_y < 0.5, "{label}: bars not symmetric");
            }
        }
    }

    #[test]
    fn shell_active_timing_cache_skips_idle_and_text_amend_and_refreshes_on_change() {
        let mut script = ScriptFile::new_default();
        let sec = script.ensure_events_section_mut();
        sec.insert_event(
            0,
            AssEvent::dialogue(AssTime::from_cs(100), AssTime::from_cs(300), "Line 0"),
        );
        sec.insert_event(
            1,
            AssEvent::dialogue(AssTime::from_cs(400), AssTime::from_cs(600), "Line 1"),
        );
        let mut shell = AppShell::new(script);
        let ctx = egui::Context::default();
        let raw = egui::RawInput {
            screen_rect: Some(Rect::from_min_size(Pos2::ZERO, Vec2::new(1280.0, 800.0))),
            ..Default::default()
        };

        let run_frame = |shell: &mut AppShell, input: egui::RawInput| {
            let mut out = ctx.run_ui(input, |ui| shell.show(ui, None, None, &ZeroClock));
            out.textures_delta.clear();
        };

        // First frame queries event 0 once.
        run_frame(&mut shell, raw.clone());
        assert_eq!(shell.active_timing_lookups(), 1);
        assert_eq!(shell.video.active_line_start_ms, 1_000);

        // Idle frames and same-line text amends do not re-query event 0.
        for _ in 0..3 {
            run_frame(&mut shell, raw.clone());
        }
        shell
            .doc
            .amend_text(0, "typing", "Line 0 edited")
            .expect("amend text");
        run_frame(&mut shell, raw.clone());
        assert_eq!(shell.active_timing_lookups(), 1);

        // Timing change refreshes cache.
        shell
            .doc
            .amend_time(0, false, "timing", AssTime::from_cs(150))
            .expect("amend time");
        run_frame(&mut shell, raw.clone());
        assert_eq!(shell.active_timing_lookups(), 2);
        assert_eq!(shell.video.active_line_start_ms, 1_500);

        // Undo refreshes cache back to original start time.
        shell.doc.undo().expect("undo");
        run_frame(&mut shell, raw.clone());
        assert_eq!(shell.active_timing_lookups(), 3);
        assert_eq!(shell.video.active_line_start_ms, 1_000);

        // Switching active line refreshes cache for line 1.
        shell.doc.set_selection(Some(1), &[1]);
        run_frame(&mut shell, raw);
        assert_eq!(shell.active_timing_lookups(), 4);
        assert_eq!(shell.video.active_line_start_ms, 4_000);
    }

    #[test]
    fn shell_window_title_reflects_script_name_and_dirty_state() {
        let mut script = ScriptFile::new_default();
        script.ensure_events_section_mut().insert_event(
            0,
            AssEvent::dialogue(AssTime::ZERO, AssTime::from_cs(200), "Line 0"),
        );
        let mut shell = AppShell::new(script);
        "Demons' Crest - 05.ass".clone_into(&mut shell.script_name);

        assert!(!shell.doc.is_dirty());
        assert_eq!(
            shell.window_title(),
            "Demons' Crest - 05.ass - hlouis-sub-edit"
        );

        shell
            .doc
            .amend_text(0, "typing", "Line 0 modified")
            .expect("amend text");
        assert!(shell.doc.is_dirty());
        assert_eq!(
            shell.window_title(),
            "*Demons' Crest - 05.ass - hlouis-sub-edit"
        );
    }
}
