//! Pure-Rust palette & Eye Comfort theme engine for `hlouis-sub-edit` (no GUI dependency).
//!
//! Provides refined classic Aegisub 3.2.2 Light and Dark palettes plus a physically grounded
//! [`ThemeSpec::EyeComfort`] mode using Bradford chromatic adaptation in linear light, luminance
//! compression, chroma capping (`C* <= 12` on large surfaces), WCAG contrast enforcement, and
//! deuteranopia/protanopia color-blind safety (`ΔE >= 20` on CPS & overlap indicators).

#![forbid(unsafe_code)]
#![cfg_attr(not(test), deny(clippy::unwrap_used, clippy::expect_used))]

pub mod color;

pub use color::{
    cap_chroma, contrast, delta_e_lab, lab_to_rgba, nudge_contrast, oklab_to_rgba,
    relative_luminance, rgba_to_lab, rgba_to_oklab, simulate_cvd, srgb_u8_to_linear, warm, CvdKind,
    Lab, OkLab, Rgba8,
};

/// Base theme polarity.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum BaseTheme {
    Light,
    Dark,
}

/// Selected theme specification (`System` is resolved to `Light`/`Dark` by the caller).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ThemeSpec {
    Light,
    Dark,
    EyeComfort { base: BaseTheme, strength: u8 },
}

/// Token identifier used by [`is_true_colour_token`] to exempt color-critical surfaces from
/// Eye Comfort warming.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum TokenId {
    UiChrome,
    Grid,
    Syntax,
    Audio,
    VideoPreview,
    ColorSwatch,
    ColorPicker,
}

/// Returns `true` for tokens that MUST NEVER be warmed (video preview, ASS color swatches,
/// and color pickers).
#[must_use]
pub const fn is_true_colour_token(token: TokenId) -> bool {
    matches!(
        token,
        TokenId::VideoPreview | TokenId::ColorSwatch | TokenId::ColorPicker
    )
}

/// Returns `true` if Eye Comfort schedule is active at `now_minutes` (`0..1440`) given a
/// `[from_minutes, to_minutes)` window (correctly handles overnight windows like `20:00..06:00`).
#[must_use]
pub const fn eye_comfort_active(now_minutes: u16, from_minutes: u16, to_minutes: u16) -> bool {
    let now = now_minutes % 1440;
    let from = from_minutes % 1440;
    let to = to_minutes % 1440;
    if from == to {
        return false;
    }
    if from < to {
        now >= from && now < to
    } else {
        now >= from || now < to
    }
}

/// Complete semantic color palette for `hlouis-sub-edit`.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Palette {
    // Surfaces & chrome
    pub window_bg: Rgba8,
    pub panel_bg: Rgba8,
    pub menu_bg: Rgba8,
    pub toolbar_bg: Rgba8,
    pub dialog_bg: Rgba8,
    pub border: Rgba8,
    pub hover: Rgba8,
    pub focus_ring: Rgba8,

    // Text
    pub text_primary: Rgba8,
    pub text_secondary: Rgba8,
    pub text_disabled: Rgba8,
    pub text_inverse: Rgba8,

    // Selection & active
    pub selection_bg: Rgba8,
    pub selection_text: Rgba8,
    pub active_line_bg: Rgba8,

    // Subtitle grid
    pub grid_bg: Rgba8,
    pub grid_zebra_bg: Rgba8,
    pub grid_comment_bg: Rgba8,
    pub grid_selected_bg: Rgba8,
    pub grid_active_bg: Rgba8,
    pub grid_overlap: Rgba8,
    pub grid_cps_ok: Rgba8,
    pub grid_cps_warn: Rgba8,
    pub grid_cps_error: Rgba8,

    // Edit-box syntax tokens
    pub syntax_plain: Rgba8,
    pub syntax_brace: Rgba8,
    pub syntax_tag_name: Rgba8,
    pub syntax_tag_value: Rgba8,
    pub syntax_karaoke: Rgba8,
    pub syntax_escape: Rgba8,
    pub syntax_comment: Rgba8,
    pub syntax_drawing: Rgba8,
    pub syntax_error: Rgba8,

    // Audio display
    pub audio_bg: Rgba8,
    pub audio_waveform: Rgba8,
    pub audio_waveform_selected: Rgba8,
    pub audio_selection: Rgba8,
    pub audio_inactive: Rgba8,
    pub audio_keyframe: Rgba8,
    pub audio_line_boundary: Rgba8,
    pub audio_play_cursor: Rgba8,
    pub audio_video_marker: Rgba8,
    pub audio_karaoke_label: Rgba8,

    // Spectrum gradient stops (5-stop ramp from floor to peak)
    pub spectrum_stops: [Rgba8; 5],

    // Video overlay
    pub overlay_handle: Rgba8,
    pub overlay_cross: Rgba8,
    pub overlay_clip_outline: Rgba8,

    // Status
    pub warning: Rgba8,
    pub error: Rgba8,
}

impl Palette {
    /// Returns all color tokens as a flat array (used for blue-light mean measurement & swatch
    /// printing).
    #[must_use]
    pub fn all_tokens(&self) -> Vec<(&'static str, Rgba8)> {
        vec![
            ("window_bg", self.window_bg),
            ("panel_bg", self.panel_bg),
            ("menu_bg", self.menu_bg),
            ("toolbar_bg", self.toolbar_bg),
            ("dialog_bg", self.dialog_bg),
            ("border", self.border),
            ("hover", self.hover),
            ("focus_ring", self.focus_ring),
            ("text_primary", self.text_primary),
            ("text_secondary", self.text_secondary),
            ("text_disabled", self.text_disabled),
            ("text_inverse", self.text_inverse),
            ("selection_bg", self.selection_bg),
            ("selection_text", self.selection_text),
            ("active_line_bg", self.active_line_bg),
            ("grid_bg", self.grid_bg),
            ("grid_zebra_bg", self.grid_zebra_bg),
            ("grid_comment_bg", self.grid_comment_bg),
            ("grid_selected_bg", self.grid_selected_bg),
            ("grid_active_bg", self.grid_active_bg),
            ("grid_overlap", self.grid_overlap),
            ("grid_cps_ok", self.grid_cps_ok),
            ("grid_cps_warn", self.grid_cps_warn),
            ("grid_cps_error", self.grid_cps_error),
            ("syntax_plain", self.syntax_plain),
            ("syntax_brace", self.syntax_brace),
            ("syntax_tag_name", self.syntax_tag_name),
            ("syntax_tag_value", self.syntax_tag_value),
            ("syntax_karaoke", self.syntax_karaoke),
            ("syntax_escape", self.syntax_escape),
            ("syntax_comment", self.syntax_comment),
            ("syntax_drawing", self.syntax_drawing),
            ("syntax_error", self.syntax_error),
            ("audio_bg", self.audio_bg),
            ("audio_waveform", self.audio_waveform),
            ("audio_waveform_selected", self.audio_waveform_selected),
            ("audio_selection", self.audio_selection),
            ("audio_inactive", self.audio_inactive),
            ("audio_keyframe", self.audio_keyframe),
            ("audio_line_boundary", self.audio_line_boundary),
            ("audio_play_cursor", self.audio_play_cursor),
            ("audio_video_marker", self.audio_video_marker),
            ("audio_karaoke_label", self.audio_karaoke_label),
            ("overlay_handle", self.overlay_handle),
            ("overlay_cross", self.overlay_cross),
            ("overlay_clip_outline", self.overlay_clip_outline),
            ("warning", self.warning),
            ("error", self.error),
        ]
    }

    /// Computes mean linear-light blue channel across all palette tokens.
    #[must_use]
    pub fn mean_linear_blue(&self) -> f64 {
        let toks = self.all_tokens();
        let sum: f64 = toks.iter().map(|(_, c)| srgb_u8_to_linear(c.b)).sum();
        #[allow(clippy::cast_precision_loss)]
        {
            sum / (toks.len() as f64)
        }
    }
}

/// Builds the complete validated [`Palette`] for `theme`.
#[must_use]
pub fn palette(theme: ThemeSpec) -> Palette {
    match theme {
        ThemeSpec::Light => enforce_rules(base_light(), BaseTheme::Light, false, 0),
        ThemeSpec::Dark => enforce_rules(base_dark(), BaseTheme::Dark, false, 0),
        ThemeSpec::EyeComfort { base, strength } => {
            let s = strength.min(100);
            let mut p = match base {
                BaseTheme::Light => base_light(),
                BaseTheme::Dark => base_dark(),
            };
            if s > 0 {
                apply_warm_to_palette(&mut p, s);
            }
            enforce_rules(p, base, true, s)
        }
    }
}

/// Refined classic Aegisub 3.2.2 Light palette (calm off-white, zero pure `#FFFFFF`/`#000000`).
fn base_light() -> Palette {
    Palette {
        window_bg: Rgba8::rgb(242, 241, 238),
        panel_bg: Rgba8::rgb(246, 245, 242),
        menu_bg: Rgba8::rgb(236, 235, 232),
        toolbar_bg: Rgba8::rgb(238, 237, 234),
        dialog_bg: Rgba8::rgb(244, 243, 240),
        border: Rgba8::rgb(198, 196, 190),
        hover: Rgba8::rgb(226, 232, 238),
        focus_ring: Rgba8::rgb(30, 102, 178),

        text_primary: Rgba8::rgb(42, 44, 48),
        text_secondary: Rgba8::rgb(90, 93, 99),
        text_disabled: Rgba8::rgb(120, 122, 126),
        text_inverse: Rgba8::rgb(245, 244, 240),

        selection_bg: Rgba8::rgb(204, 232, 218),
        selection_text: Rgba8::rgb(24, 34, 28),
        active_line_bg: Rgba8::rgb(230, 238, 246),

        grid_bg: Rgba8::rgb(246, 245, 242),
        grid_zebra_bg: Rgba8::rgb(237, 236, 232),
        grid_comment_bg: Rgba8::rgb(224, 228, 240),
        grid_selected_bg: Rgba8::rgb(204, 232, 218),
        grid_active_bg: Rgba8::rgb(222, 236, 246),

        // Distinct L* steps for deuteranopia/protanopia ΔE >= 20
        grid_cps_ok: Rgba8::rgb(16, 118, 92),    // L* ~44
        grid_cps_warn: Rgba8::rgb(148, 92, 0),   // L* ~45 -> nudged to distinct L* in enforce
        grid_cps_error: Rgba8::rgb(164, 24, 34), // L* ~35
        grid_overlap: Rgba8::rgb(106, 46, 148),  // L* ~30

        syntax_plain: Rgba8::rgb(42, 44, 48),
        syntax_brace: Rgba8::rgb(112, 72, 148),
        syntax_tag_name: Rgba8::rgb(24, 92, 168),
        syntax_tag_value: Rgba8::rgb(24, 118, 72),
        syntax_karaoke: Rgba8::rgb(164, 82, 12),
        syntax_escape: Rgba8::rgb(142, 46, 118),
        syntax_comment: Rgba8::rgb(104, 108, 114),
        syntax_drawing: Rgba8::rgb(16, 116, 128),
        syntax_error: Rgba8::rgb(184, 30, 38),

        audio_bg: Rgba8::rgb(28, 32, 36),
        audio_waveform: Rgba8::rgb(78, 186, 128),
        audio_waveform_selected: Rgba8::rgb(236, 178, 72),
        audio_selection: Rgba8::rgba(72, 148, 118, 72),
        audio_inactive: Rgba8::rgba(128, 134, 144, 52),
        audio_keyframe: Rgba8::rgb(228, 96, 198),
        audio_line_boundary: Rgba8::rgb(118, 152, 214),
        audio_play_cursor: Rgba8::rgb(242, 238, 224),
        audio_video_marker: Rgba8::rgb(242, 196, 68),
        audio_karaoke_label: Rgba8::rgb(46, 54, 62),

        spectrum_stops: [
            Rgba8::rgb(18, 22, 30),
            Rgba8::rgb(28, 66, 118),
            Rgba8::rgb(42, 146, 148),
            Rgba8::rgb(214, 164, 54),
            Rgba8::rgb(244, 236, 204),
        ],

        overlay_handle: Rgba8::rgb(52, 168, 236),
        overlay_cross: Rgba8::rgb(242, 196, 68),
        overlay_clip_outline: Rgba8::rgb(236, 96, 156),

        warning: Rgba8::rgb(154, 92, 0),
        error: Rgba8::rgb(182, 32, 38),
    }
}

/// Calm, crisp Dark palette (zero pure `#000000`/`#FFFFFF`, chroma-capped surfaces).
fn base_dark() -> Palette {
    Palette {
        window_bg: Rgba8::rgb(28, 30, 34),
        panel_bg: Rgba8::rgb(33, 35, 40),
        menu_bg: Rgba8::rgb(25, 27, 31),
        toolbar_bg: Rgba8::rgb(30, 32, 36),
        dialog_bg: Rgba8::rgb(35, 37, 42),
        border: Rgba8::rgb(64, 68, 76),
        hover: Rgba8::rgb(48, 52, 60),
        focus_ring: Rgba8::rgb(106, 174, 238),

        text_primary: Rgba8::rgb(212, 214, 218),
        text_secondary: Rgba8::rgb(162, 166, 174),
        text_disabled: Rgba8::rgb(122, 126, 134),
        text_inverse: Rgba8::rgb(26, 28, 32),

        selection_bg: Rgba8::rgb(42, 74, 64),
        selection_text: Rgba8::rgb(236, 240, 236),
        active_line_bg: Rgba8::rgb(42, 54, 68),

        grid_bg: Rgba8::rgb(30, 32, 36),
        grid_zebra_bg: Rgba8::rgb(37, 39, 44),
        grid_comment_bg: Rgba8::rgb(42, 46, 62),
        grid_selected_bg: Rgba8::rgb(42, 74, 64),
        grid_active_bg: Rgba8::rgb(44, 56, 72),

        grid_cps_ok: Rgba8::rgb(108, 214, 168),
        grid_cps_warn: Rgba8::rgb(238, 186, 86),
        grid_cps_error: Rgba8::rgb(242, 114, 114),
        grid_overlap: Rgba8::rgb(198, 148, 242),

        syntax_plain: Rgba8::rgb(212, 214, 218),
        syntax_brace: Rgba8::rgb(194, 152, 232),
        syntax_tag_name: Rgba8::rgb(116, 182, 242),
        syntax_tag_value: Rgba8::rgb(124, 212, 158),
        syntax_karaoke: Rgba8::rgb(238, 176, 94),
        syntax_escape: Rgba8::rgb(228, 146, 204),
        syntax_comment: Rgba8::rgb(146, 152, 162),
        syntax_drawing: Rgba8::rgb(102, 206, 214),
        syntax_error: Rgba8::rgb(244, 116, 116),

        audio_bg: Rgba8::rgb(22, 24, 28),
        audio_waveform: Rgba8::rgb(86, 194, 138),
        audio_waveform_selected: Rgba8::rgb(242, 188, 86),
        audio_selection: Rgba8::rgba(82, 162, 128, 72),
        audio_inactive: Rgba8::rgba(136, 142, 152, 52),
        audio_keyframe: Rgba8::rgb(232, 112, 204),
        audio_line_boundary: Rgba8::rgb(128, 162, 224),
        audio_play_cursor: Rgba8::rgb(238, 234, 220),
        audio_video_marker: Rgba8::rgb(242, 198, 78),
        audio_karaoke_label: Rgba8::rgb(42, 48, 56),

        spectrum_stops: [
            Rgba8::rgb(18, 20, 26),
            Rgba8::rgb(34, 76, 132),
            Rgba8::rgb(52, 156, 158),
            Rgba8::rgb(222, 174, 66),
            Rgba8::rgb(244, 238, 210),
        ],

        overlay_handle: Rgba8::rgb(78, 182, 244),
        overlay_cross: Rgba8::rgb(244, 202, 84),
        overlay_clip_outline: Rgba8::rgb(242, 112, 168),

        warning: Rgba8::rgb(236, 178, 76),
        error: Rgba8::rgb(242, 116, 116),
    }
}

fn apply_warm_to_palette(p: &mut Palette, strength: u8) {
    let w = |c: Rgba8| warm(c, strength);
    p.window_bg = w(p.window_bg);
    p.panel_bg = w(p.panel_bg);
    p.menu_bg = w(p.menu_bg);
    p.toolbar_bg = w(p.toolbar_bg);
    p.dialog_bg = w(p.dialog_bg);
    p.border = w(p.border);
    p.hover = w(p.hover);
    p.focus_ring = w(p.focus_ring);
    p.text_primary = w(p.text_primary);
    p.text_secondary = w(p.text_secondary);
    p.text_disabled = w(p.text_disabled);
    p.text_inverse = w(p.text_inverse);
    p.selection_bg = w(p.selection_bg);
    p.selection_text = w(p.selection_text);
    p.active_line_bg = w(p.active_line_bg);
    p.grid_bg = w(p.grid_bg);
    p.grid_zebra_bg = w(p.grid_zebra_bg);
    p.grid_comment_bg = w(p.grid_comment_bg);
    p.grid_selected_bg = w(p.grid_selected_bg);
    p.grid_active_bg = w(p.grid_active_bg);
    p.grid_overlap = w(p.grid_overlap);
    p.grid_cps_ok = w(p.grid_cps_ok);
    p.grid_cps_warn = w(p.grid_cps_warn);
    p.grid_cps_error = w(p.grid_cps_error);
    p.syntax_plain = w(p.syntax_plain);
    p.syntax_brace = w(p.syntax_brace);
    p.syntax_tag_name = w(p.syntax_tag_name);
    p.syntax_tag_value = w(p.syntax_tag_value);
    p.syntax_karaoke = w(p.syntax_karaoke);
    p.syntax_escape = w(p.syntax_escape);
    p.syntax_comment = w(p.syntax_comment);
    p.syntax_drawing = w(p.syntax_drawing);
    p.syntax_error = w(p.syntax_error);
    p.audio_bg = w(p.audio_bg);
    p.audio_waveform = w(p.audio_waveform);
    p.audio_waveform_selected = w(p.audio_waveform_selected);
    p.audio_selection = w(p.audio_selection);
    p.audio_inactive = w(p.audio_inactive);
    p.audio_keyframe = w(p.audio_keyframe);
    p.audio_line_boundary = w(p.audio_line_boundary);
    p.audio_play_cursor = w(p.audio_play_cursor);
    p.audio_video_marker = w(p.audio_video_marker);
    p.audio_karaoke_label = w(p.audio_karaoke_label);
    for stop in &mut p.spectrum_stops {
        *stop = w(*stop);
    }
    p.overlay_handle = w(p.overlay_handle);
    p.overlay_cross = w(p.overlay_cross);
    p.overlay_clip_outline = w(p.overlay_clip_outline);
    p.warning = w(p.warning);
    p.error = w(p.error);
}

/// Enforces every readability, chroma, zebra, and CVD-separation invariant via CIELAB nudging.
#[allow(clippy::too_many_lines)]
fn enforce_rules(mut p: Palette, base: BaseTheme, eye_comfort: bool, strength: u8) -> Palette {
    // 1. Cap large surfaces chroma <= 11.5 (strictly <= 12.0) and avoid pure white/black.
    for surf in [
        &mut p.window_bg,
        &mut p.panel_bg,
        &mut p.menu_bg,
        &mut p.toolbar_bg,
        &mut p.dialog_bg,
        &mut p.grid_bg,
        &mut p.grid_zebra_bg,
        &mut p.audio_bg,
    ] {
        *surf = cap_chroma(*surf, 11.2);
    }

    // 2. Enforce grid zebra ΔL* in [2.3, 3.7] (strictly inside 2..=4).
    let g_lab = rgba_to_lab(p.grid_bg);
    let target_zebra_l = match base {
        BaseTheme::Light => (g_lab.l - 3.0).max(60.0),
        BaseTheme::Dark => (g_lab.l + 3.0).min(40.0),
    };
    let z_lab = rgba_to_lab(p.grid_zebra_bg);
    p.grid_zebra_bg = cap_chroma(
        lab_to_rgba(
            Lab {
                l: target_zebra_l,
                a: z_lab.a,
                b: z_lab.b,
            },
            255,
        ),
        11.2,
    );

    // 3. Body text contrast band: 7..=13 normal, 7..=10 in EyeComfort.
    let body_max = if eye_comfort { 9.8 } else { 12.5 };
    p.text_primary = nudge_contrast(p.text_primary, p.window_bg, 7.2, body_max);
    p.text_primary = nudge_contrast(p.text_primary, p.grid_bg, 7.1, body_max);
    p.syntax_plain = p.text_primary;
    p.text_secondary = nudge_contrast(p.text_secondary, p.window_bg, 4.7, 7.5);
    p.text_disabled = nudge_contrast(p.text_disabled, p.window_bg, 3.2, 4.4);
    p.selection_text = nudge_contrast(p.selection_text, p.selection_bg, 7.2, 13.0);
    p.selection_text = nudge_contrast(p.selection_text, p.grid_selected_bg, 7.2, 13.0);

    // 4. All syntax tokens & accents >= 3.2:1 on their background.
    for tok in [
        &mut p.syntax_brace,
        &mut p.syntax_tag_name,
        &mut p.syntax_tag_value,
        &mut p.syntax_karaoke,
        &mut p.syntax_escape,
        &mut p.syntax_comment,
        &mut p.syntax_drawing,
        &mut p.syntax_error,
    ] {
        *tok = nudge_contrast(*tok, p.panel_bg, 3.3, 12.0);
    }
    p.focus_ring = nudge_contrast(p.focus_ring, p.window_bg, 3.3, 12.0);
    p.warning = nudge_contrast(p.warning, p.window_bg, 3.3, 12.0);
    p.error = nudge_contrast(p.error, p.window_bg, 3.3, 12.0);

    // 5. CVD-safe lightness + blue-yellow (b*) separation for CPS ok / warn / error / overlap:
    // Every pair differs by >= 20 ΔE under normal vision, deuteranopia, AND protanopia while
    // keeping contrast >= 3.1:1 against `grid_bg`.
    let (ok_lab, warn_lab, err_lab, over_lab) = match base {
        BaseTheme::Light => (
            Lab {
                l: 48.0,
                a: -24.0,
                b: -16.0,
            },
            Lab {
                l: 52.0,
                a: 10.0,
                b: 54.0,
            },
            Lab {
                l: 24.0,
                a: 42.0,
                b: 18.0,
            },
            Lab {
                l: 16.0,
                a: 22.0,
                b: -30.0,
            },
        ),
        BaseTheme::Dark => (
            Lab {
                l: 78.0,
                a: -28.0,
                b: -16.0,
            },
            Lab {
                l: 82.0,
                a: 8.0,
                b: 52.0,
            },
            Lab {
                l: 54.0,
                a: 46.0,
                b: 22.0,
            },
            Lab {
                l: 46.0,
                a: 24.0,
                b: -34.0,
            },
        ),
    };
    let to_status = |lab: Lab| {
        let raw = lab_to_rgba(lab, 255);
        if eye_comfort && strength > 0 {
            warm(raw, strength / 2)
        } else {
            raw
        }
    };
    p.grid_cps_ok = to_status(ok_lab);
    p.grid_cps_warn = to_status(warn_lab);
    p.grid_cps_error = to_status(err_lab);
    p.grid_overlap = to_status(over_lab);

    // Ensure all four status colors also pass >= 3.0:1 on `grid_bg` (or keep >= 20 ΔE).
    // On Light (grid_bg L*~96), L* 52 gives contrast ~4.6:1; on Dark (grid_bg L*~13), L* 46 gives ~3.3:1.
    // Verify the highest-L* Light rung / lowest-L* Dark rung meets >= 3.0:1:
    if contrast(p.grid_cps_ok, p.grid_bg) < 3.05 {
        p.grid_cps_ok = nudge_contrast(p.grid_cps_ok, p.grid_bg, 3.1, 12.0);
    }
    if contrast(p.grid_overlap, p.grid_bg) < 3.05 {
        p.grid_overlap = nudge_contrast(p.grid_overlap, p.grid_bg, 3.1, 12.0);
    }

    p
}

#[cfg(test)]
mod tests {
    use super::*;

    fn assert_palette_rules(spec: ThemeSpec, eye_comfort: bool) {
        let p = palette(spec);

        // No pure white / pure black surfaces or body text
        assert_ne!(p.window_bg, Rgba8::rgb(255, 255, 255));
        assert_ne!(p.window_bg, Rgba8::rgb(0, 0, 0));
        assert_ne!(p.text_primary, Rgba8::rgb(0, 0, 0));
        assert_ne!(p.text_primary, Rgba8::rgb(255, 255, 255));

        // WCAG contrast rules
        let body_c = contrast(p.text_primary, p.window_bg);
        let body_hi = if eye_comfort { 10.0 } else { 13.0 };
        assert!(
            (7.0..=body_hi).contains(&body_c),
            "{spec:?}: body contrast {body_c:.2} not in 7..={body_hi}"
        );
        let sec_c = contrast(p.text_secondary, p.window_bg);
        assert!(
            sec_c >= 4.5,
            "{spec:?}: secondary contrast {sec_c:.2} < 4.5"
        );
        let dis_c = contrast(p.text_disabled, p.window_bg);
        assert!(dis_c >= 3.0, "{spec:?}: disabled contrast {dis_c:.2} < 3.0");
        let selection_contrast = contrast(p.selection_text, p.selection_bg);
        assert!(
            selection_contrast >= 7.0,
            "{spec:?}: selection contrast {selection_contrast:.2} < 7.0"
        );

        for (name, tok) in [
            ("syntax_brace", p.syntax_brace),
            ("syntax_tag_name", p.syntax_tag_name),
            ("syntax_tag_value", p.syntax_tag_value),
            ("syntax_karaoke", p.syntax_karaoke),
            ("syntax_escape", p.syntax_escape),
            ("syntax_comment", p.syntax_comment),
            ("syntax_drawing", p.syntax_drawing),
            ("syntax_error", p.syntax_error),
        ] {
            let c = contrast(tok, p.panel_bg);
            assert!(c >= 3.0, "{spec:?}: {name} contrast {c:.2} < 3.0");
        }

        // Large surface chroma cap C* <= 12
        for (name, surf) in [
            ("window_bg", p.window_bg),
            ("panel_bg", p.panel_bg),
            ("menu_bg", p.menu_bg),
            ("toolbar_bg", p.toolbar_bg),
            ("dialog_bg", p.dialog_bg),
            ("grid_bg", p.grid_bg),
            ("grid_zebra_bg", p.grid_zebra_bg),
            ("audio_bg", p.audio_bg),
        ] {
            let ch = rgba_to_lab(surf).chroma();
            assert!(ch <= 12.0, "{spec:?}: {name} chroma {ch:.2} > 12");
        }

        // Grid zebra ΔL* in 2..=4
        let dl = (rgba_to_lab(p.grid_bg).l - rgba_to_lab(p.grid_zebra_bg).l).abs();
        assert!((2.0..=4.0).contains(&dl), "{spec:?}: zebra dL* = {dl:.2}");

        // CVD safety: CPS ok/warn/error + overlap pairwise ΔE >= 20 under normal, deuteranopia, protanopia
        let statuses = [
            p.grid_cps_ok,
            p.grid_cps_warn,
            p.grid_cps_error,
            p.grid_overlap,
        ];
        for i in 0..statuses.len() {
            for j in (i + 1)..statuses.len() {
                let de_norm = delta_e_lab(statuses[i], statuses[j]);
                let de_deut = delta_e_lab(
                    simulate_cvd(statuses[i], CvdKind::Deuteranopia),
                    simulate_cvd(statuses[j], CvdKind::Deuteranopia),
                );
                let de_prot = delta_e_lab(
                    simulate_cvd(statuses[i], CvdKind::Protanopia),
                    simulate_cvd(statuses[j], CvdKind::Protanopia),
                );
                assert!(de_norm >= 20.0, "{spec:?}: normal dE({i},{j})={de_norm:.1}");
                assert!(de_deut >= 20.0, "{spec:?}: deut dE({i},{j})={de_deut:.1}");
                assert!(de_prot >= 20.0, "{spec:?}: prot dE({i},{j})={de_prot:.1}");
            }
        }
    }

    #[test]
    fn all_themes_and_eye_comfort_strengths_pass_every_rule() {
        assert_palette_rules(ThemeSpec::Light, false);
        let lp = palette(ThemeSpec::Light);
        let dp = palette(ThemeSpec::Dark);
        println!(
            "THEME_STATS|Light|body={:.2}|sec={:.2}|dis={:.2}|sel={:.2}|blue=0.0%",
            contrast(lp.text_primary, lp.window_bg),
            contrast(lp.text_secondary, lp.window_bg),
            contrast(lp.text_disabled, lp.window_bg),
            contrast(lp.selection_text, lp.selection_bg)
        );
        println!(
            "THEME_STATS|Dark|body={:.2}|sec={:.2}|dis={:.2}|sel={:.2}|blue=0.0%",
            contrast(dp.text_primary, dp.window_bg),
            contrast(dp.text_secondary, dp.window_bg),
            contrast(dp.text_disabled, dp.window_bg),
            contrast(dp.selection_text, dp.selection_bg)
        );
        assert_palette_rules(ThemeSpec::Dark, false);

        for base in [BaseTheme::Light, BaseTheme::Dark] {
            let base_p = palette(match base {
                BaseTheme::Light => ThemeSpec::Light,
                BaseTheme::Dark => ThemeSpec::Dark,
            });
            let base_blue = base_p.mean_linear_blue();

            for strength in [0u8, 30, 60, 100] {
                let spec = ThemeSpec::EyeComfort { base, strength };
                assert_palette_rules(spec, true);
                let ep = palette(spec);
                let red = (1.0 - ep.mean_linear_blue() / base_blue) * 100.0;
                println!("THEME_STATS|EyeComfort({base:?},{strength})|body={:.2}|sec={:.2}|dis={:.2}|sel={:.2}|blue=-{red:.1}%", contrast(ep.text_primary, ep.window_bg), contrast(ep.text_secondary, ep.window_bg), contrast(ep.text_disabled, ep.window_bg), contrast(ep.selection_text, ep.selection_bg));
                if strength == 100 {
                    let warm_blue = palette(spec).mean_linear_blue();
                    assert!(
                        warm_blue <= base_blue * 0.50,
                        "{spec:?}: linear blue {warm_blue:.4} > 50% of base {base_blue:.4}"
                    );
                }
            }
        }
    }

    #[test]
    fn helpers_schedule_true_color_and_oklab_roundtrip() {
        assert!(is_true_colour_token(TokenId::VideoPreview));
        assert!(is_true_colour_token(TokenId::ColorSwatch));
        assert!(is_true_colour_token(TokenId::ColorPicker));
        assert!(!is_true_colour_token(TokenId::Grid));

        // Overnight schedule 20:00 (1200) -> 06:00 (360)
        assert!(eye_comfort_active(1300, 1200, 360));
        assert!(eye_comfort_active(120, 1200, 360));
        assert!(!eye_comfort_active(720, 1200, 360));

        let c = Rgba8::rgb(120, 180, 90);
        let rt = oklab_to_rgba(rgba_to_oklab(c), 255);
        assert!(c.r.abs_diff(rt.r) <= 1 && c.g.abs_diff(rt.g) <= 1 && c.b.abs_diff(rt.b) <= 1);
    }
}
