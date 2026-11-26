//! Application theme definitions (Light, Dark, Follow Windows).
//!
//! All UI, grid, syntax-highlighting, and overlay colors live exclusively in this module.

use egui::{Color32, Stroke, Visuals};

/// User-selectable theme mode.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum ThemeMode {
    Light,
    Dark,
    #[default]
    FollowWindows,
}

/// Complete color palette for the active theme.
#[derive(Debug, Clone, Copy)]
pub struct ThemePalette {
    pub is_dark: bool,
    pub bg_primary: Color32,
    pub bg_secondary: Color32,
    pub row_even: Color32,
    pub row_odd: Color32,
    pub row_selected: Color32,
    pub row_active_border: Color32,
    pub text_primary: Color32,
    pub text_muted: Color32,
    pub cps_normal: Color32,
    pub cps_warning: Color32,
    pub cps_critical: Color32,
    pub syn_braces: Color32,
    pub syn_tag: Color32,
    pub syn_karaoke: Color32,
    pub syn_value: Color32,
    pub syn_escape: Color32,
    pub syn_drawing_cmd: Color32,
    pub syn_drawing_coord: Color32,
    pub syn_comment: Color32,
    pub syn_text: Color32,
    pub video_letterbox: Color32,
    pub overlay_bg: Color32,
    pub overlay_border: Color32,
    pub overlay_text: Color32,
    pub keyframe_badge: Color32,
    pub non_keyframe_badge: Color32,
    pub error_bg: Color32,
    pub error_text: Color32,
}

impl ThemePalette {
    /// Light theme palette.
    #[must_use]
    pub const fn light() -> Self {
        Self {
            is_dark: false,
            bg_primary: Color32::from_rgb(248, 249, 251),
            bg_secondary: Color32::from_rgb(238, 241, 246),
            row_even: Color32::from_rgb(255, 255, 255),
            row_odd: Color32::from_rgb(244, 246, 250),
            row_selected: Color32::from_rgb(208, 227, 255),
            row_active_border: Color32::from_rgb(28, 100, 216),
            text_primary: Color32::from_rgb(24, 28, 36),
            text_muted: Color32::from_rgb(95, 104, 120),
            cps_normal: Color32::from_rgb(32, 120, 64),
            cps_warning: Color32::from_rgb(176, 108, 0),
            cps_critical: Color32::from_rgb(196, 36, 44),
            syn_braces: Color32::from_rgb(142, 48, 168),
            syn_tag: Color32::from_rgb(16, 94, 198),
            syn_karaoke: Color32::from_rgb(186, 72, 12),
            syn_value: Color32::from_rgb(14, 124, 86),
            syn_escape: Color32::from_rgb(188, 32, 92),
            syn_drawing_cmd: Color32::from_rgb(148, 44, 152),
            syn_drawing_coord: Color32::from_rgb(0, 118, 136),
            syn_comment: Color32::from_rgb(112, 120, 132),
            syn_text: Color32::from_rgb(24, 28, 36),
            video_letterbox: Color32::from_rgb(24, 26, 30),
            overlay_bg: Color32::from_rgba_premultiplied(245, 247, 250, 204),
            overlay_border: Color32::from_rgb(190, 198, 212),
            overlay_text: Color32::from_rgb(20, 24, 32),
            keyframe_badge: Color32::from_rgb(20, 132, 72),
            non_keyframe_badge: Color32::from_rgb(110, 118, 132),
            error_bg: Color32::from_rgb(253, 232, 234),
            error_text: Color32::from_rgb(164, 24, 32),
        }
    }

    /// Dark theme palette.
    #[must_use]
    pub const fn dark() -> Self {
        Self {
            is_dark: true,
            bg_primary: Color32::from_rgb(24, 26, 32),
            bg_secondary: Color32::from_rgb(32, 35, 43),
            row_even: Color32::from_rgb(28, 31, 38),
            row_odd: Color32::from_rgb(34, 37, 45),
            row_selected: Color32::from_rgb(46, 82, 134),
            row_active_border: Color32::from_rgb(104, 168, 255),
            text_primary: Color32::from_rgb(232, 236, 242),
            text_muted: Color32::from_rgb(154, 164, 180),
            cps_normal: Color32::from_rgb(114, 214, 146),
            cps_warning: Color32::from_rgb(244, 188, 72),
            cps_critical: Color32::from_rgb(248, 108, 114),
            syn_braces: Color32::from_rgb(210, 144, 242),
            syn_tag: Color32::from_rgb(110, 184, 255),
            syn_karaoke: Color32::from_rgb(255, 168, 92),
            syn_value: Color32::from_rgb(132, 224, 168),
            syn_escape: Color32::from_rgb(255, 128, 174),
            syn_drawing_cmd: Color32::from_rgb(236, 136, 244),
            syn_drawing_coord: Color32::from_rgb(112, 224, 232),
            syn_comment: Color32::from_rgb(136, 144, 158),
            syn_text: Color32::from_rgb(232, 236, 242),
            video_letterbox: Color32::from_rgb(12, 13, 16),
            overlay_bg: Color32::from_rgba_premultiplied(20, 22, 28, 200),
            overlay_border: Color32::from_rgb(72, 80, 96),
            overlay_text: Color32::from_rgb(232, 238, 246),
            keyframe_badge: Color32::from_rgb(92, 214, 140),
            non_keyframe_badge: Color32::from_rgb(142, 150, 164),
            error_bg: Color32::from_rgb(78, 26, 30),
            error_text: Color32::from_rgb(255, 178, 182),
        }
    }
}

/// Resolves whether dark mode should be active for a given `ThemeMode`.
#[must_use]
pub fn resolve_palette(mode: ThemeMode) -> ThemePalette {
    let is_dark = match mode {
        ThemeMode::Light => false,
        ThemeMode::Dark => true,
        ThemeMode::FollowWindows => matches!(dark_light::detect(), Ok(dark_light::Mode::Dark)),
    };
    if is_dark {
        ThemePalette::dark()
    } else {
        ThemePalette::light()
    }
}

/// Applies the palette to `egui::Context`.
pub fn apply_to_egui(ctx: &egui::Context, palette: &ThemePalette) {
    let mut visuals = if palette.is_dark {
        Visuals::dark()
    } else {
        Visuals::light()
    };
    visuals.panel_fill = palette.bg_primary;
    visuals.window_fill = palette.bg_secondary;
    visuals.extreme_bg_color = palette.row_even;
    visuals.selection.bg_fill = palette.row_selected;
    visuals.selection.stroke = Stroke::new(1.0, palette.row_active_border);
    ctx.set_visuals(visuals);
}
