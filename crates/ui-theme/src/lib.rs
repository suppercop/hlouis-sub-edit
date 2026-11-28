//! `theme::Palette` → `egui::Visuals` / `egui::Style` mapping, font fallback setup (Windows UI
//! and monospace edit-box + CJK/Vietnamese chain), DPI pixel-snapping helpers, and Aegisub 3.2.2
//! compact density for `hlouis-sub-edit`.
//!
//! All colours come strictly from [`theme::Palette`] (never hard-coded). Caret blink is off by
//! default; animations are capped at `<= 150 ms` (`0.12 s`) and triggered only by user actions.

#![forbid(unsafe_code)]
#![cfg_attr(not(test), deny(clippy::unwrap_used, clippy::expect_used))]

use egui::style::{Selection, TextCursorStyle, WidgetVisuals, Widgets};
use egui::{
    Color32, CornerRadius, FontData, FontDefinitions, FontFamily, FontId, Margin, Stroke, Style,
    TextStyle, Vec2, Visuals,
};
use settings::{AppearancePrefs, ThemeMode};
use std::path::PathBuf;
use std::sync::Arc;
use theme::{BaseTheme, Palette, Rgba8, ThemeSpec, eye_comfort_active};

/// Maximum animation duration (seconds) allowed across the UI (`<= 150 ms`).
pub const MAX_ANIMATION_SECS: f32 = 0.12;

/// Converts [`theme::Rgba8`] into [`egui::Color32`] with zero allocations.
#[inline]
#[must_use]
pub fn rgba(c: Rgba8) -> Color32 {
    Color32::from_rgba_unmultiplied(c.r, c.g, c.b, c.a)
}

/// Snaps a logical coordinate to the nearest physical pixel center for crisp 1-px lines at any DPI.
#[inline]
#[must_use]
pub fn snap_px(coord: f32, pixels_per_point: f32) -> f32 {
    let ppp = pixels_per_point.max(0.5);
    (coord * ppp).round() / ppp
}

/// Returns a crisp 1-physical-pixel [`Stroke`] at `pixels_per_point`.
#[inline]
#[must_use]
pub fn one_px_stroke(color: Rgba8, pixels_per_point: f32) -> Stroke {
    let ppp = pixels_per_point.max(0.5);
    Stroke::new(1.0 / ppp, rgba(color))
}

/// Configurable UI, grid, and edit-box font sizes plus optional system font search paths.
#[derive(Debug, Clone, PartialEq)]
pub struct FontConfig {
    pub ui_size: f32,
    pub grid_size: f32,
    pub edit_box_size: f32,
    pub small_size: f32,
    pub heading_size: f32,
    /// Optional explicit font files to load (UI proportional, monospace, CJK/Vietnamese fallback).
    pub proportional_paths: Vec<PathBuf>,
    pub monospace_paths: Vec<PathBuf>,
    pub fallback_paths: Vec<PathBuf>,
}

impl Default for FontConfig {
    fn default() -> Self {
        Self {
            ui_size: 12.0,
            grid_size: 12.0,
            edit_box_size: 13.0,
            small_size: 10.5,
            heading_size: 14.0,
            proportional_paths: default_windows_ui_font_candidates(),
            monospace_paths: default_windows_mono_font_candidates(),
            fallback_paths: default_cjk_vi_fallback_candidates(),
        }
    }
}

/// Default Windows 10/11 proportional UI font search order (cleanly skipped on non-Windows).
#[must_use]
pub fn default_windows_ui_font_candidates() -> Vec<PathBuf> {
    vec![
        PathBuf::from(r"C:\Windows\Fonts\segoeui.ttf"),
        PathBuf::from("/usr/share/fonts/truetype/noto/NotoSans-Regular.ttf"),
        PathBuf::from("/usr/share/fonts/truetype/dejavu/DejaVuSans.ttf"),
    ]
}

/// Default Windows 10/11 monospace edit-box font search order.
#[must_use]
pub fn default_windows_mono_font_candidates() -> Vec<PathBuf> {
    vec![
        PathBuf::from(r"C:\Windows\Fonts\CascadiaMono.ttf"),
        PathBuf::from(r"C:\Windows\Fonts\consola.ttf"),
        PathBuf::from("/usr/share/fonts/truetype/dejavu/DejaVuSansMono.ttf"),
    ]
}

/// Default CJK + Vietnamese fallback font chain.
#[must_use]
pub fn default_cjk_vi_fallback_candidates() -> Vec<PathBuf> {
    vec![
        PathBuf::from(r"C:\Windows\Fonts\meiryo.ttc"),
        PathBuf::from(r"C:\Windows\Fonts\YuGothR.ttc"),
        PathBuf::from(r"C:\Windows\Fonts\malgun.ttf"),
        PathBuf::from(r"C:\Windows\Fonts\arial.ttf"),
        PathBuf::from("/usr/share/fonts/opentype/noto/NotoSansCJK-Regular.ttc"),
        PathBuf::from("/usr/share/fonts/truetype/noto/NotoSans-Regular.ttf"),
    ]
}

/// Builds [`FontDefinitions`] with proportional, monospace, and CJK/Vietnamese fallback chains,
/// skipping any system paths that are not present on the current machine.
#[must_use]
pub fn build_font_definitions(cfg: &FontConfig) -> FontDefinitions {
    let mut defs = FontDefinitions::default();
    let mut insert_first_available =
        |name: &str, paths: &[PathBuf], family: FontFamily, front: bool| {
            for p in paths {
                if let Ok(bytes) = std::fs::read(p) {
                    defs.font_data
                        .insert(name.to_owned(), Arc::new(FontData::from_owned(bytes)));
                    if let Some(list) = defs.families.get_mut(&family) {
                        if front {
                            list.insert(0, name.to_owned());
                        } else {
                            list.push(name.to_owned());
                        }
                    }
                    break;
                }
            }
        };

    insert_first_available(
        "hlouis_ui",
        &cfg.proportional_paths,
        FontFamily::Proportional,
        true,
    );
    insert_first_available(
        "hlouis_mono",
        &cfg.monospace_paths,
        FontFamily::Monospace,
        true,
    );
    insert_first_available(
        "hlouis_cjk_vi_p",
        &cfg.fallback_paths,
        FontFamily::Proportional,
        false,
    );
    insert_first_available(
        "hlouis_cjk_vi_m",
        &cfg.fallback_paths,
        FontFamily::Monospace,
        false,
    );
    defs
}

/// Resolves [`AppearancePrefs`] + system polarity + current minute-of-day (`0..1440`) into a
/// concrete [`ThemeSpec`] (zero allocations).
#[inline]
#[must_use]
pub const fn resolve_theme_spec(
    prefs: &AppearancePrefs,
    system_is_dark: bool,
    now_minutes: u16,
) -> ThemeSpec {
    let base = match prefs.theme_mode {
        ThemeMode::Light => BaseTheme::Light,
        ThemeMode::Dark => BaseTheme::Dark,
        ThemeMode::System => {
            if system_is_dark {
                BaseTheme::Dark
            } else {
                BaseTheme::Light
            }
        }
        ThemeMode::EyeComfortLight => {
            return ThemeSpec::EyeComfort {
                base: BaseTheme::Light,
                strength: prefs.eye_comfort_strength,
            };
        }
        ThemeMode::EyeComfortDark => {
            return ThemeSpec::EyeComfort {
                base: BaseTheme::Dark,
                strength: prefs.eye_comfort_strength,
            };
        }
    };

    let scheduled = prefs.eye_comfort_schedule_enabled
        && eye_comfort_active(
            now_minutes,
            prefs.eye_comfort_from_min,
            prefs.eye_comfort_to_min,
        );
    if scheduled {
        ThemeSpec::EyeComfort {
            base,
            strength: prefs.eye_comfort_strength,
        }
    } else {
        match base {
            BaseTheme::Light => ThemeSpec::Light,
            BaseTheme::Dark => ThemeSpec::Dark,
        }
    }
}

/// Returns `true` if `spec` is a dark polarity theme.
#[inline]
#[must_use]
pub const fn is_dark_spec(spec: ThemeSpec) -> bool {
    matches!(
        spec,
        ThemeSpec::Dark
            | ThemeSpec::EyeComfort {
                base: BaseTheme::Dark,
                ..
            }
    )
}

/// Maps a [`theme::Palette`] into [`egui::Visuals`] with Aegisub 3.2.2 compact widget geometry,
/// crisp 1-px borders, and caret blink disabled by default.
#[must_use]
pub fn build_visuals(pal: &Palette, dark: bool) -> Visuals {
    let mut v = if dark {
        Visuals::dark()
    } else {
        Visuals::light()
    };

    let border_stroke = Stroke::new(1.0, rgba(pal.border));
    let focus_stroke = Stroke::new(1.0, rgba(pal.focus_ring));
    let rounding = CornerRadius::same(2);

    v.panel_fill = rgba(pal.panel_bg);
    v.window_fill = rgba(pal.dialog_bg);
    v.faint_bg_color = rgba(pal.grid_zebra_bg);
    v.extreme_bg_color = rgba(pal.grid_bg);
    v.code_bg_color = rgba(pal.panel_bg);
    v.window_stroke = border_stroke;
    v.window_corner_radius = CornerRadius::same(3);
    v.menu_corner_radius = CornerRadius::same(2);
    v.warn_fg_color = rgba(pal.warning);
    v.error_fg_color = rgba(pal.error);
    v.override_text_color = Some(rgba(pal.text_primary));

    v.selection = Selection {
        bg_fill: rgba(pal.selection_bg),
        stroke: Stroke::new(1.0, rgba(pal.selection_text)),
    };

    v.text_cursor = TextCursorStyle {
        stroke: Stroke::new(1.5, rgba(pal.focus_ring)),
        preview: false,
        blink: false, // Caret blink OFF by default per UI spec
        ..Default::default()
    };

    let noninteractive = WidgetVisuals {
        bg_fill: rgba(pal.panel_bg),
        weak_bg_fill: rgba(pal.toolbar_bg),
        bg_stroke: border_stroke,
        corner_radius: rounding,
        fg_stroke: Stroke::new(1.0, rgba(pal.text_primary)),
        expansion: 0.0,
    };
    let inactive = WidgetVisuals {
        bg_fill: rgba(pal.toolbar_bg),
        weak_bg_fill: rgba(pal.toolbar_bg),
        bg_stroke: border_stroke,
        corner_radius: rounding,
        fg_stroke: Stroke::new(1.0, rgba(pal.text_primary)),
        expansion: 0.0,
    };
    let hovered = WidgetVisuals {
        bg_fill: rgba(pal.hover),
        weak_bg_fill: rgba(pal.hover),
        bg_stroke: focus_stroke,
        corner_radius: rounding,
        fg_stroke: Stroke::new(1.0, rgba(pal.text_primary)),
        expansion: 0.0,
    };
    let active = WidgetVisuals {
        bg_fill: rgba(pal.selection_bg),
        weak_bg_fill: rgba(pal.selection_bg),
        bg_stroke: focus_stroke,
        corner_radius: rounding,
        fg_stroke: Stroke::new(1.0, rgba(pal.selection_text)),
        expansion: 0.0,
    };
    let open = WidgetVisuals {
        bg_fill: rgba(pal.menu_bg),
        weak_bg_fill: rgba(pal.menu_bg),
        bg_stroke: focus_stroke,
        corner_radius: rounding,
        fg_stroke: Stroke::new(1.0, rgba(pal.text_primary)),
        expansion: 0.0,
    };

    v.widgets = Widgets {
        noninteractive,
        inactive,
        hovered,
        active,
        open,
    };
    v
}

/// Maps [`Palette`] + [`FontConfig`] into a full [`egui::Style`] matching Aegisub 3.2.2 density.
#[must_use]
pub fn build_style(pal: &Palette, dark: bool, fonts: &FontConfig) -> Style {
    let mut style = Style {
        visuals: build_visuals(pal, dark),
        animation_time: MAX_ANIMATION_SECS,
        ..Style::default()
    };

    // Classic Aegisub 3.2.2 compact spacing & density.
    style.spacing.item_spacing = Vec2::new(3.0, 2.0);
    style.spacing.button_padding = Vec2::new(4.0, 1.5);
    style.spacing.window_margin = Margin::same(6);
    style.spacing.menu_margin = Margin::same(4);
    style.spacing.indent = 12.0;
    style.spacing.interact_size = Vec2::new(24.0, 17.0);
    style.spacing.scroll.bar_width = 10.0;

    style
        .text_styles
        .insert(TextStyle::Body, FontId::proportional(fonts.ui_size));
    style
        .text_styles
        .insert(TextStyle::Button, FontId::proportional(fonts.ui_size));
    style
        .text_styles
        .insert(TextStyle::Small, FontId::proportional(fonts.small_size));
    style
        .text_styles
        .insert(TextStyle::Heading, FontId::proportional(fonts.heading_size));
    style
        .text_styles
        .insert(TextStyle::Monospace, FontId::monospace(fonts.edit_box_size));

    style
}

/// Live theme state controller caching the resolved [`ThemeSpec`] and [`Palette`] so egui style
/// is only re-applied when the spec or font sizes actually change.
#[derive(Debug, Clone)]
pub struct ThemeController {
    spec: ThemeSpec,
    palette: Palette,
    fonts: FontConfig,
    revision: u64,
}

impl Default for ThemeController {
    fn default() -> Self {
        Self::new(ThemeSpec::Dark, FontConfig::default())
    }
}

impl ThemeController {
    /// Creates a new controller initialized to `spec`.
    #[must_use]
    pub fn new(spec: ThemeSpec, fonts: FontConfig) -> Self {
        let palette = theme::palette(spec);
        Self {
            spec,
            palette,
            fonts,
            revision: 1,
        }
    }

    #[inline]
    #[must_use]
    pub const fn spec(&self) -> ThemeSpec {
        self.spec
    }

    #[inline]
    #[must_use]
    pub const fn palette(&self) -> &Palette {
        &self.palette
    }

    #[inline]
    #[must_use]
    pub const fn fonts(&self) -> &FontConfig {
        &self.fonts
    }

    #[inline]
    #[must_use]
    pub const fn revision(&self) -> u64 {
        self.revision
    }

    /// Updates the theme specification if changed, recomputing the palette. Returns `true` if
    /// the palette changed.
    pub fn set_spec(&mut self, spec: ThemeSpec) -> bool {
        if self.spec == spec {
            return false;
        }
        self.spec = spec;
        self.palette = theme::palette(spec);
        self.revision += 1;
        true
    }

    /// Evaluates [`AppearancePrefs`] + system polarity + time of day, updating live if needed.
    pub fn sync_from_prefs(
        &mut self,
        prefs: &AppearancePrefs,
        system_is_dark: bool,
        now_minutes: u16,
    ) -> bool {
        let next = resolve_theme_spec(prefs, system_is_dark, now_minutes);
        self.set_spec(next)
    }

    /// Applies the current theme style to `ctx`.
    pub fn apply_style(&self, ctx: &egui::Context) {
        let style = build_style(&self.palette, is_dark_spec(self.spec), &self.fonts);
        ctx.set_global_style(style);
    }

    /// Installs fonts into `ctx` (call once at startup or when font config changes).
    pub fn install_fonts(&self, ctx: &egui::Context) {
        ctx.set_fonts(build_font_definitions(&self.fonts));
    }
}

/// Renders a representative sample panel showing surfaces, buttons, grid rows, CPS badges, and
/// syntax-coloured text for visual inspection and snapshot testing.
pub fn show_theme_sample_panel(ui: &mut egui::Ui, pal: &Palette) {
    ui.vertical(|ui| {
        ui.heading("HLouis Sub Edit — Theme Preview");
        ui.horizontal(|ui| {
            let _ = ui.button("Open");
            let _ = ui.button("Save");
            let _ = ui.selectable_label(true, "Active Tool");
            ui.colored_label(rgba(pal.grid_cps_ok), "CPS 14");
            ui.colored_label(rgba(pal.grid_cps_warn), "CPS 21");
            ui.colored_label(rgba(pal.grid_cps_error), "CPS 29");
        });
        ui.separator();
        ui.horizontal(|ui| {
            ui.colored_label(rgba(pal.syntax_brace), "{");
            ui.colored_label(rgba(pal.syntax_tag_name), r"\pos");
            ui.colored_label(rgba(pal.syntax_tag_value), "(960,540)");
            ui.colored_label(rgba(pal.syntax_brace), "}");
            ui.colored_label(rgba(pal.syntax_plain), "Xin chào thế giới");
        });
    });
}

#[cfg(test)]
mod tests {
    use super::*;
    use egui_kittest::Harness;

    #[test]
    fn renders_sample_panel_in_light_dark_and_eye_comfort() {
        let specs = [
            ThemeSpec::Light,
            ThemeSpec::Dark,
            ThemeSpec::EyeComfort {
                base: BaseTheme::Dark,
                strength: 60,
            },
            ThemeSpec::EyeComfort {
                base: BaseTheme::Light,
                strength: 50,
            },
        ];

        for spec in specs {
            let ctrl = ThemeController::new(spec, FontConfig::default());
            let pal = ctrl.palette().clone();
            let style = build_style(&pal, is_dark_spec(spec), ctrl.fonts());
            assert!(!style.visuals.text_cursor.blink);
            assert!(style.animation_time <= 0.15);

            let fonts_defs = build_font_definitions(ctrl.fonts());
            let mut inited = false;
            let mut harness = Harness::new_ui(move |ui| {
                if !inited {
                    ui.ctx().set_fonts(fonts_defs.clone());
                    ui.ctx().set_global_style(style.clone());
                    inited = true;
                }
                show_theme_sample_panel(ui, &pal);
            });
            harness.run();
        }
    }
}

/// Helpers for tests that guard against stale `Arc<Galley>` values (galleys embed font-atlas
/// UVs and the pixels-per-point they were laid out at, so keeping one across frames breaks
/// as soon as egui grows or rebuilds its font atlas).
pub mod galley_check {
    use egui::epaint::{ClippedShape, Shape};
    use egui::{Context, Galley};
    use std::sync::Arc;

    fn collect(shape: &Shape, out: &mut Vec<Arc<Galley>>) {
        match shape {
            Shape::Text(t) => out.push(Arc::clone(&t.galley)),
            Shape::Vec(v) => {
                for s in v {
                    collect(s, out);
                }
            }
            _ => {}
        }
    }

    /// Every text galley painted by `shapes`.
    #[must_use]
    pub fn text_galleys(shapes: &[ClippedShape]) -> Vec<Arc<Galley>> {
        let mut out = Vec::new();
        for cs in shapes {
            collect(&cs.shape, &mut out);
        }
        out
    }

    fn uvs(g: &Galley) -> Vec<[u32; 2]> {
        g.rows
            .iter()
            .flat_map(|r| r.row.visuals.mesh.vertices.iter())
            .map(|v| [v.uv.x.to_bits(), v.uv.y.to_bits()])
            .collect()
    }

    /// Number of painted galleys that differ from a fresh layout of the same job at the
    /// current pixels-per-point (wrong scale or stale atlas UVs). Must be `0`.
    #[must_use]
    pub fn count_stale(ctx: &Context, shapes: &[ClippedShape]) -> usize {
        let ppp = ctx.pixels_per_point();
        text_galleys(shapes)
            .iter()
            .filter(|g| {
                if (g.pixels_per_point - ppp).abs() > f32::EPSILON {
                    return true;
                }
                let fresh = ctx.fonts_mut(|f| f.layout_job((*g.job).clone()));
                !Arc::ptr_eq(g, &fresh) && uvs(g) != uvs(&fresh)
            })
            .count()
    }
}
