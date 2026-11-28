//! Virtualised high-performance subtitle grid widget (`egui 0.37`) built on [`grid_model::GridModel`],
//! [`syntax_highlight::GridDisplayConfig`], [`theme::Palette`], and [`i18n_catalog::Catalog`].
//!
//! Mirrors Aegisub 3.2.2 / 3.5.1 `src/base_grid.cpp`:
//! - Default visible columns: `#`, `Start`, `End`, `CPS`, `Style`, `Text` (`Layer`, `Actor`,
//!   `Effect`, and `Margins` hidden by default, matching the owner's Aegisub 3.2.2-style config).
//! - Classic Aegisub Light styling: light-blue header row, light-green `#` column, white normal
//!   rows, Aegisub blue selection, proportional UI font for times and text, compact 16 px rows.
//! - Plain CPS numbers with subtle background warning tint above threshold (Aegisub 3.5 style).
//! - Literal `\N` preserved in Text column; override `{...}` blocks simplified with `'☀'` only
//!   when `display_cfg.mode == TagDisplayMode::Simplify`.
//! - Virtualised rendering (`ScrollArea::show_rows`): `< 1 ms` CPU on 200,000 lines.

#![forbid(unsafe_code)]
#![cfg_attr(not(test), deny(clippy::unwrap_used, clippy::expect_used))]

use ass_core::{AssEvent, AssTime, EventKind, ScriptFile};
use egui::{
    Align, Color32, FontId, Galley, Key, Layout, Pos2, Rect, ScrollArea, Sense, Stroke, Ui, Vec2,
};
use grid_model::{ColumnVisibility, GridColumn, GridModel, KeyNav, RowFlags, RowTiming};
use i18n_catalog::Catalog;
use std::fmt::Write as _;
use std::sync::Arc;
use syntax_highlight::{GridDisplayConfig, format_grid_text_into};
use theme::Palette;
use ui_theme::{one_px_stroke, rgba, snap_px};

/// Header label galley kept across frames; valid only for the [`ui_theme::GalleyEpoch`]
/// generation it was laid out in.
#[derive(Clone)]
struct CachedHeaderGalley {
    label: String,
    font_bits: u32,
    color: Color32,
    generation: u64,
    galley: Arc<Galley>,
}

impl std::fmt::Debug for CachedHeaderGalley {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("CachedHeaderGalley")
            .field("label", &self.label)
            .finish_non_exhaustive()
    }
}

/// Cell galleys of one row kept across frames (same generation rule as the header).
#[derive(Clone)]
struct CachedGridRowGalleys {
    row_idx: usize,
    rev: u64,
    text_col: Color32,
    font_bits: u32,
    generation: u64,
    cols: [Option<Arc<Galley>>; 12],
}

impl Default for CachedGridRowGalleys {
    fn default() -> Self {
        Self {
            row_idx: usize::MAX,
            rev: 0,
            text_col: Color32::TRANSPARENT,
            font_bits: 0,
            generation: 0,
            cols: std::array::from_fn(|_| None),
        }
    }
}

impl std::fmt::Debug for CachedGridRowGalleys {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("CachedGridRowGalleys")
            .field("row_idx", &self.row_idx)
            .field("rev", &self.rev)
            .finish_non_exhaustive()
    }
}

/// Action emitted by the subtitle grid (mouse/keyboard selection or context menu).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum GridAction {
    SelectionChanged { active: Option<usize> },
    Command(&'static str),
}

#[derive(Debug, Clone)]
struct CachedRow {
    rev: u64,
    is_comment: bool,
    layer: i32,
    start_cs: u32,
    end_cs: u32,
    cps: u32,
    margin_l: i32,
    margin_r: i32,
    margin_v: i32,
    style: String,
    actor: String,
    effect: String,
    display_text: String,
}

/// Persistent UI state + reusable scratch buffers for the virtualised subtitle grid.
#[derive(Debug, Clone)]
pub struct SubtitleGridState {
    pub model: GridModel,
    pub display_cfg: GridDisplayConfig,
    pub row_height: f32,
    pub column_order: Vec<GridColumn>,
    pub column_widths: [f32; 12],
    pub cps_warn_threshold: u32,
    pub cps_error_threshold: u32,
    pub scroll_to_active_requested: bool,
    /// When `true` and the active palette is light, apply classic Aegisub 3.2.2 grid colours
    /// (light-blue header, light-green `#` column, white rows, Aegisub blue selection).
    pub classic_light_colors: bool,
    grid_rev: u64,
    cached_rows: Vec<CachedRow>,
    visible_buf: Vec<RowFlags>,
    time_buf_start: String,
    time_buf_end: String,
    num_buf: String,
    galley_epoch: ui_theme::GalleyEpoch,
    header_galleys: [Option<CachedHeaderGalley>; 12],
    row_galley_cache: Vec<CachedGridRowGalleys>,
}

impl Default for SubtitleGridState {
    fn default() -> Self {
        let mut model = GridModel::new();
        // Owner's Aegisub 3.2.2-style default: #, Start, End, CPS, Style, Text
        // (Layer, Actor, Effect, Margins hidden by default).
        model.columns.layer = false;
        model.columns.actor = false;
        model.columns.effect = false;
        model.columns.margin_l = false;
        model.columns.margin_r = false;
        model.columns.margin_v = false;
        Self {
            model,
            display_cfg: GridDisplayConfig {
                simplify_marker: '☀',
                ..GridDisplayConfig::default()
            },
            row_height: 16.0,
            column_order: vec![
                GridColumn::LineNumber,
                GridColumn::Layer,
                GridColumn::Start,
                GridColumn::End,
                GridColumn::Cps,
                GridColumn::Style,
                GridColumn::Actor,
                GridColumn::Effect,
                GridColumn::MarginL,
                GridColumn::MarginR,
                GridColumn::MarginV,
                GridColumn::Text,
            ],
            column_widths: [
                40.0,  // LineNumber
                34.0,  // Layer (hidden by default)
                74.0,  // Start
                74.0,  // End
                34.0,  // Cps
                92.0,  // Style
                84.0,  // Actor (hidden by default)
                84.0,  // Effect (hidden by default)
                38.0,  // MarginL (hidden by default)
                38.0,  // MarginR (hidden by default)
                38.0,  // MarginV (hidden by default)
                480.0, // Text (fills remaining)
            ],
            cps_warn_threshold: 20,
            cps_error_threshold: 30,
            scroll_to_active_requested: false,
            classic_light_colors: true,
            grid_rev: 1,
            cached_rows: Vec::new(),
            visible_buf: Vec::with_capacity(128),
            time_buf_start: String::with_capacity(16),
            time_buf_end: String::with_capacity(16),
            num_buf: String::with_capacity(16),
            galley_epoch: ui_theme::GalleyEpoch::default(),
            header_galleys: std::array::from_fn(|_| None),
            row_galley_cache: vec![CachedGridRowGalleys::default(); 128],
        }
    }
}

impl SubtitleGridState {
    /// Syncs the underlying [`GridModel`] timings and O(1) render cache from `script`.
    pub fn sync_from_script(&mut self, script: &ScriptFile) {
        self.grid_rev = self.grid_rev.wrapping_add(1);
        let rev = self.grid_rev;
        let count = script
            .events_section()
            .map_or(0, ass_core::EventsSection::event_count);
        let mut rows = Vec::with_capacity(count);
        self.cached_rows.clear();
        self.cached_rows.reserve(count);
        let mut tmp = String::with_capacity(128);
        for ev in script.events() {
            let is_comment = ev.kind() == EventKind::Comment;
            rows.push(RowTiming {
                start_cs: ev.start().as_cs(),
                end_cs: ev.end().as_cs(),
                is_comment,
            });
            format_grid_text_into(ev.text(), &self.display_cfg, &mut tmp);
            self.cached_rows.push(CachedRow {
                rev,
                is_comment,
                layer: ev.layer(),
                start_cs: ev.start().as_cs(),
                end_cs: ev.end().as_cs(),
                cps: compute_event_cps(ev),
                margin_l: ev.margin_l(),
                margin_r: ev.margin_r(),
                margin_v: ev.margin_v(),
                style: ev.style().to_owned(),
                actor: ev.name().to_owned(),
                effect: ev.effect().to_owned(),
                display_text: tmp.clone(),
            });
        }
        self.model.load_rows(&rows);
    }

    /// Updates a single edited row in O(1) without rescanning the script.
    pub fn sync_row(&mut self, row_idx: usize, ev: &AssEvent) {
        self.grid_rev = self.grid_rev.wrapping_add(1);
        let rev = self.grid_rev;
        let is_comment = ev.kind() == EventKind::Comment;
        self.model.update_row_timing(
            row_idx,
            RowTiming {
                start_cs: ev.start().as_cs(),
                end_cs: ev.end().as_cs(),
                is_comment,
            },
        );
        if let Some(slot) = self.cached_rows.get_mut(row_idx) {
            slot.rev = rev;
            slot.is_comment = is_comment;
            slot.layer = ev.layer();
            slot.start_cs = ev.start().as_cs();
            slot.end_cs = ev.end().as_cs();
            slot.cps = compute_event_cps(ev);
            slot.margin_l = ev.margin_l();
            slot.margin_r = ev.margin_r();
            slot.margin_v = ev.margin_v();
            slot.style.clear();
            slot.style.push_str(ev.style());
            slot.actor.clear();
            slot.actor.push_str(ev.name());
            slot.effect.clear();
            slot.effect.push_str(ev.effect());
            format_grid_text_into(ev.text(), &self.display_cfg, &mut slot.display_text);
        }
    }

    /// Requests scrolling the viewport to the active row on the next frame.
    #[inline]
    pub fn scroll_to_active(&mut self) {
        self.scroll_to_active_requested = true;
    }

    /// Moves column at `from_idx` to `to_idx` in `column_order`.
    pub fn reorder_column(&mut self, from_idx: usize, to_idx: usize) {
        if from_idx < self.column_order.len()
            && to_idx < self.column_order.len()
            && from_idx != to_idx
        {
            let col = self.column_order.remove(from_idx);
            self.column_order.insert(to_idx, col);
        }
    }

    #[inline]
    #[must_use]
    pub const fn column_index(col: GridColumn) -> usize {
        match col {
            GridColumn::LineNumber => 0,
            GridColumn::Layer => 1,
            GridColumn::Start => 2,
            GridColumn::End => 3,
            GridColumn::Cps => 4,
            GridColumn::Style => 5,
            GridColumn::Actor => 6,
            GridColumn::Effect => 7,
            GridColumn::MarginL => 8,
            GridColumn::MarginR => 9,
            GridColumn::MarginV => 10,
            GridColumn::Text => 11,
        }
    }

    #[inline]
    #[must_use]
    pub const fn is_column_visible(vis: &ColumnVisibility, col: GridColumn) -> bool {
        match col {
            GridColumn::LineNumber => vis.line_number,
            GridColumn::Layer => vis.layer,
            GridColumn::Start => vis.start,
            GridColumn::End => vis.end,
            GridColumn::Cps => vis.cps,
            GridColumn::Style => vis.style,
            GridColumn::Actor => vis.actor,
            GridColumn::Effect => vis.effect,
            GridColumn::MarginL => vis.margin_l,
            GridColumn::MarginR => vis.margin_r,
            GridColumn::MarginV => vis.margin_v,
            GridColumn::Text => vis.text,
        }
    }

    #[inline]
    #[must_use]
    pub const fn column_header_key(col: GridColumn) -> &'static str {
        match col {
            GridColumn::LineNumber => "grid-col-num",
            GridColumn::Layer => "grid-col-layer",
            GridColumn::Start => "grid-col-start",
            GridColumn::End => "grid-col-end",
            GridColumn::Cps => "grid-col-cps",
            GridColumn::Style => "grid-col-style",
            GridColumn::Actor => "grid-col-actor",
            GridColumn::Effect => "grid-col-effect",
            GridColumn::MarginL => "grid-col-margin-l",
            GridColumn::MarginR => "grid-col-margin-r",
            GridColumn::MarginV => "grid-col-margin-v",
            GridColumn::Text => "grid-col-text",
        }
    }
}

/// Fast, zero-allocation CPS calculation for a single event (strips `{...}` tags & `\N`).
#[inline]
#[must_use]
pub fn compute_event_cps(ev: &AssEvent) -> u32 {
    let dur_cs = ev.end().as_cs().saturating_sub(ev.start().as_cs());
    if dur_cs == 0 {
        return 0;
    }
    let mut visible_chars = 0u32;
    let mut in_tag = false;
    let mut prev_bs = false;
    for ch in ev.text().chars() {
        if in_tag {
            if ch == '}' {
                in_tag = false;
            }
            continue;
        }
        if ch == '{' {
            in_tag = true;
            prev_bs = false;
            continue;
        }
        if prev_bs {
            prev_bs = false;
            if matches!(ch, 'N' | 'n' | 'h') {
                continue;
            }
        }
        if ch == '\\' {
            prev_bs = true;
            continue;
        }
        if !ch.is_whitespace() {
            visible_chars += 1;
        }
    }
    (visible_chars.saturating_mul(100) + dur_cs / 2) / dur_cs
}

/// Formats `AssTime` (`H:MM:SS.cc`) into `out` with zero heap allocations.
#[inline]
pub fn format_ass_time_into(t: AssTime, out: &mut String) {
    out.clear();
    let total_cs = t.as_cs();
    let cs = total_cs % 100;
    let secs = total_cs / 100;
    let s = secs % 60;
    let total_m = secs / 60;
    let m = total_m % 60;
    let h = total_m / 60;
    let _ = write!(out, "{h}:{m:02}:{s:02}.{cs:02}");
}

#[inline]
fn is_light_palette(pal: &Palette) -> bool {
    let lum = u16::from(pal.grid_bg.r) + u16::from(pal.grid_bg.g) + u16::from(pal.grid_bg.b);
    lum > 384
}

struct RowH(f32);
impl grid_model::FontMetrics for RowH {
    fn row_height(&self) -> f32 {
        self.0
    }
}

/// Renders the virtualised subtitle grid into `ui`, returning any user-triggered [`GridAction`].
#[allow(clippy::too_many_lines)]
pub fn show_subtitle_grid(
    ui: &mut Ui,
    script: &ScriptFile,
    state: &mut SubtitleGridState,
    video_time_cs: Option<u32>,
    palette: &Palette,
    catalog: &Catalog,
) -> Option<GridAction> {
    let mut action: Option<GridAction> = None;
    let total_rows = script
        .events_section()
        .map_or(0, ass_core::EventsSection::event_count);
    let ppp = ui.ctx().pixels_per_point();
    let border_stroke = one_px_stroke(palette.border, ppp);
    // Aegisub uses the proportional UI font for times and text alike.
    let font_body = FontId::proportional(11.5);
    let use_classic_light = state.classic_light_colors && is_light_palette(palette);

    // 1. Keyboard navigation when grid has focus or no text edit owns focus.
    if !ui.ctx().egui_wants_keyboard_input() && total_rows > 0 {
        let shift = ui.input(|i| i.modifiers.shift);
        let nav = ui.input(|i| {
            if i.key_pressed(Key::ArrowUp) {
                Some(KeyNav::Up)
            } else if i.key_pressed(Key::ArrowDown) {
                Some(KeyNav::Down)
            } else if i.key_pressed(Key::PageUp) {
                Some(KeyNav::PageUp)
            } else if i.key_pressed(Key::PageDown) {
                Some(KeyNav::PageDown)
            } else if i.key_pressed(Key::Home) {
                Some(KeyNav::Home)
            } else if i.key_pressed(Key::End) {
                Some(KeyNav::End)
            } else {
                None
            }
        });
        if let Some(dir) = nav {
            #[allow(clippy::cast_possible_truncation, clippy::cast_sign_loss)]
            let page = ((ui.available_height() / state.row_height).max(1.0)) as usize;
            state.model.key_move(dir, shift, page);
            state.scroll_to_active_requested = true;
            action = Some(GridAction::SelectionChanged {
                active: state.model.active_row(),
            });
        }
    }

    // 2. Column header row (light blue in classic Light mode, matching Aegisub).
    let header_h = state.row_height + 2.0;
    let (header_rect, header_resp) = ui.allocate_exact_size(
        Vec2::new(ui.available_width().max(100.0), header_h),
        Sense::click(),
    );
    let painter = ui.painter_at(header_rect);
    let header_bg = if use_classic_light {
        Color32::from_rgb(165, 207, 231)
    } else {
        rgba(palette.toolbar_bg)
    };
    let header_text_col = if use_classic_light {
        Color32::from_rgb(20, 24, 30)
    } else {
        rgba(palette.text_secondary)
    };
    painter.rect_filled(header_rect, 0.0, header_bg);
    let y_bot = snap_px(header_rect.bottom(), ppp);
    painter.line_segment(
        [
            Pos2::new(header_rect.left(), y_bot),
            Pos2::new(header_rect.right(), y_bot),
        ],
        border_stroke,
    );

    let ctx = ui.ctx().clone();
    // Cached galleys are only reused while egui has not invalidated its layouts (atlas reset,
    // new fonts / pixels-per-point); see `GalleyEpoch`.
    let generation = state.galley_epoch.sync(&ctx);
    let font_bits = font_body.size.to_bits();
    let mut hx = header_rect.left();
    for &col in &state.column_order {
        if !SubtitleGridState::is_column_visible(&state.model.columns, col) {
            continue;
        }
        let col_idx = SubtitleGridState::column_index(col);
        let col_w = if col == GridColumn::Text {
            (header_rect.right() - hx).max(120.0)
        } else {
            state.column_widths[col_idx]
        };
        let label = catalog
            .get(SubtitleGridState::column_header_key(col))
            .unwrap_or("?");
        let hgalley = match &mut state.header_galleys[col_idx] {
            Some(c)
                if c.generation == generation
                    && c.font_bits == font_bits
                    && c.color == header_text_col
                    && c.label == label =>
            {
                Arc::clone(&c.galley)
            }
            slot => {
                let built = ctx.fonts_mut(|f| {
                    f.layout_no_wrap(label.to_owned(), font_body.clone(), header_text_col)
                });
                *slot = Some(CachedHeaderGalley {
                    label: label.to_owned(),
                    font_bits,
                    color: header_text_col,
                    generation,
                    galley: Arc::clone(&built),
                });
                built
            }
        };
        let gh = hgalley.size().y;
        painter.galley(
            Pos2::new(hx + 4.0, header_rect.center().y - gh * 0.5),
            hgalley,
            header_text_col,
        );
        hx += col_w;
        let sx = snap_px(hx, ppp);
        painter.line_segment(
            [
                Pos2::new(sx, header_rect.top()),
                Pos2::new(sx, header_rect.bottom()),
            ],
            border_stroke,
        );
    }

    header_resp.context_menu(|ui| {
        ui.checkbox(
            &mut state.model.columns.layer,
            catalog.get("grid-col-layer").unwrap_or("Layer"),
        );
        ui.checkbox(
            &mut state.model.columns.cps,
            catalog.get("grid-col-cps").unwrap_or("CPS"),
        );
        ui.checkbox(
            &mut state.model.columns.style,
            catalog.get("grid-col-style").unwrap_or("Style"),
        );
        ui.checkbox(
            &mut state.model.columns.actor,
            catalog.get("grid-col-actor").unwrap_or("Actor"),
        );
        ui.checkbox(
            &mut state.model.columns.effect,
            catalog.get("grid-col-effect").unwrap_or("Effect"),
        );
        ui.checkbox(
            &mut state.model.columns.margin_l,
            catalog.get("grid-col-margin-l").unwrap_or("L"),
        );
        ui.checkbox(
            &mut state.model.columns.margin_r,
            catalog.get("grid-col-margin-r").unwrap_or("R"),
        );
        ui.checkbox(
            &mut state.model.columns.margin_v,
            catalog.get("grid-col-margin-v").unwrap_or("V"),
        );
    });

    // 3. Virtualised scrollable row area.
    let mut scroll_area = ScrollArea::vertical()
        .id_salt("hlouis_subtitle_grid_scroll")
        .auto_shrink([false, false]);
    if state.scroll_to_active_requested
        && let Some(active) = state.model.active_row()
    {
        #[allow(clippy::cast_precision_loss)]
        let off = (active as f32) * state.row_height;
        scroll_area = scroll_area.vertical_scroll_offset(off);
        state.scroll_to_active_requested = false;
    }

    let row_height = state.row_height;
    let avail_w = ui.available_width().max(100.0);

    state.model.visible_rows_into(
        ui.available_height().max(row_height),
        &RowH(row_height),
        video_time_cs,
        &mut state.visible_buf,
    );

    let out_scroll = scroll_area.show_rows(ui, row_height, total_rows, |ui, row_range| {
        let shift_down = ui.input(|i| i.modifiers.shift);
        let ctrl_down = ui.input(|i| i.modifiers.command || i.modifiers.ctrl);

        for row_idx in row_range {
            let Some(row) = state.cached_rows.get(row_idx) else {
                continue;
            };
            let is_comment = row.is_comment;
            let is_selected = state.model.is_selected(row_idx);
            let is_active = state.model.active_row() == Some(row_idx);
            let is_overlap = state
                .visible_buf
                .iter()
                .find(|f| f.row_index == row_idx)
                .is_some_and(|f| f.is_overlapping);
            let is_vis_frame =
                video_time_cs.is_some_and(|t| !is_comment && row.start_cs <= t && t < row.end_cs);

            let (row_rect, resp) =
                ui.allocate_exact_size(Vec2::new(avail_w, row_height), Sense::click());

            let bg = if use_classic_light {
                if is_active || is_selected {
                    Color32::from_rgb(206, 255, 231)
                } else if is_comment {
                    Color32::from_rgb(216, 222, 245)
                } else {
                    Color32::WHITE
                }
            } else if is_active {
                rgba(palette.grid_active_bg)
            } else if is_selected {
                rgba(palette.grid_selected_bg)
            } else if is_comment {
                rgba(palette.grid_comment_bg)
            } else if row_idx % 2 == 1 {
                rgba(palette.grid_zebra_bg)
            } else {
                rgba(palette.grid_bg)
            };

            let row_painter = ui.painter_at(row_rect);
            row_painter.rect_filled(row_rect, 0.0, bg);

            if is_overlap {
                let bar = Rect::from_min_size(row_rect.min, Vec2::new(2.5, row_height));
                row_painter.rect_filled(bar, 0.0, rgba(palette.grid_overlap));
            } else if is_vis_frame {
                let bar = Rect::from_min_size(row_rect.min, Vec2::new(2.5, row_height));
                row_painter.rect_filled(bar, 0.0, rgba(palette.focus_ring));
            }

            let text_col = if use_classic_light {
                if is_comment {
                    Color32::from_rgb(56, 70, 110)
                } else {
                    Color32::from_rgb(16, 16, 16)
                }
            } else if is_selected || is_active {
                rgba(palette.selection_text)
            } else if is_comment {
                rgba(palette.text_secondary)
            } else {
                rgba(palette.text_primary)
            };

            let mut cx = row_rect.left();
            let cy = row_rect.center().y;
            let cache_slot = &mut state.row_galley_cache[row_idx & 127];
            if cache_slot.row_idx != row_idx
                || cache_slot.rev != row.rev
                || cache_slot.text_col != text_col
                || cache_slot.font_bits != font_bits
                || cache_slot.generation != generation
            {
                cache_slot.row_idx = row_idx;
                cache_slot.rev = row.rev;
                cache_slot.text_col = text_col;
                cache_slot.font_bits = font_bits;
                cache_slot.generation = generation;
                for c in &mut cache_slot.cols {
                    *c = None;
                }
            }
            for &col in &state.column_order {
                if !SubtitleGridState::is_column_visible(&state.model.columns, col) {
                    continue;
                }
                let col_idx = SubtitleGridState::column_index(col);
                let col_w = if col == GridColumn::Text {
                    (row_rect.right() - cx).max(120.0)
                } else {
                    state.column_widths[col_idx]
                };
                let cell_rect = Rect::from_min_size(
                    Pos2::new(cx, row_rect.top()),
                    Vec2::new(col_w, row_height),
                );

                if col == GridColumn::LineNumber && use_classic_light {
                    let num_bg = if is_active || is_selected {
                        Color32::from_rgb(156, 226, 180)
                    } else {
                        Color32::from_rgb(196, 236, 201)
                    };
                    row_painter.rect_filled(cell_rect, 0.0, num_bg);
                } else if col == GridColumn::Cps {
                    let cps = row.cps;
                    if cps >= state.cps_error_threshold {
                        let tint = if use_classic_light {
                            Color32::from_rgb(255, 206, 206)
                        } else {
                            Color32::from_rgba_unmultiplied(210, 60, 60, 75)
                        };
                        row_painter.rect_filled(cell_rect, 0.0, tint);
                    } else if cps >= state.cps_warn_threshold {
                        let tint = if use_classic_light {
                            Color32::from_rgb(255, 240, 198)
                        } else {
                            Color32::from_rgba_unmultiplied(210, 165, 50, 65)
                        };
                        row_painter.rect_filled(cell_rect, 0.0, tint);
                    }
                }

                let cell_galley = if let Some(ref existing) = cache_slot.cols[col_idx] {
                    Arc::clone(existing)
                } else {
                    let cell_txt: &str = match col {
                        GridColumn::LineNumber => {
                            state.num_buf.clear();
                            let _ = write!(state.num_buf, "{}", row_idx + 1);
                            &state.num_buf
                        }
                        GridColumn::Layer => {
                            state.num_buf.clear();
                            let _ = write!(state.num_buf, "{}", row.layer);
                            &state.num_buf
                        }
                        GridColumn::Start => {
                            format_ass_time_into(
                                AssTime::from_cs(row.start_cs),
                                &mut state.time_buf_start,
                            );
                            &state.time_buf_start
                        }
                        GridColumn::End => {
                            format_ass_time_into(
                                AssTime::from_cs(row.end_cs),
                                &mut state.time_buf_end,
                            );
                            &state.time_buf_end
                        }
                        GridColumn::Cps => {
                            state.num_buf.clear();
                            let _ = write!(state.num_buf, "{}", row.cps);
                            &state.num_buf
                        }
                        GridColumn::Style => &row.style,
                        GridColumn::Actor => &row.actor,
                        GridColumn::Effect => &row.effect,
                        GridColumn::MarginL => {
                            state.num_buf.clear();
                            let _ = write!(state.num_buf, "{}", row.margin_l);
                            &state.num_buf
                        }
                        GridColumn::MarginR => {
                            state.num_buf.clear();
                            let _ = write!(state.num_buf, "{}", row.margin_r);
                            &state.num_buf
                        }
                        GridColumn::MarginV => {
                            state.num_buf.clear();
                            let _ = write!(state.num_buf, "{}", row.margin_v);
                            &state.num_buf
                        }
                        GridColumn::Text => &row.display_text,
                    };
                    let built = ctx.fonts_mut(|f| {
                        f.layout_no_wrap(cell_txt.to_owned(), font_body.clone(), text_col)
                    });
                    cache_slot.cols[col_idx] = Some(Arc::clone(&built));
                    built
                };
                let gsize = cell_galley.size();
                match col {
                    GridColumn::LineNumber => {
                        row_painter.galley(
                            Pos2::new(cx + col_w - 5.0 - gsize.x, cy - gsize.y * 0.5),
                            cell_galley,
                            text_col,
                        );
                    }
                    GridColumn::Cps => {
                        row_painter.galley(
                            Pos2::new(cx + (col_w - gsize.x) * 0.5, cy - gsize.y * 0.5),
                            cell_galley,
                            text_col,
                        );
                    }
                    GridColumn::Style
                    | GridColumn::Actor
                    | GridColumn::Effect
                    | GridColumn::Text => {
                        row_painter.with_clip_rect(cell_rect).galley(
                            Pos2::new(cx + 4.0, cy - gsize.y * 0.5),
                            cell_galley,
                            text_col,
                        );
                    }
                    GridColumn::Layer
                    | GridColumn::Start
                    | GridColumn::End
                    | GridColumn::MarginL
                    | GridColumn::MarginR
                    | GridColumn::MarginV => {
                        row_painter.galley(
                            Pos2::new(cx + 4.0, cy - gsize.y * 0.5),
                            cell_galley,
                            text_col,
                        );
                    }
                }
                cx += col_w;
                let sx = snap_px(cx, ppp);
                row_painter.line_segment(
                    [
                        Pos2::new(sx, row_rect.top()),
                        Pos2::new(sx, row_rect.bottom()),
                    ],
                    Stroke::new(1.0 / ppp.max(0.5), rgba(palette.border)),
                );
            }

            // Crisp 1-px row divider (or Aegisub blue focus frame on active row).
            let ry = snap_px(row_rect.bottom(), ppp);
            row_painter.line_segment(
                [
                    Pos2::new(row_rect.left(), ry),
                    Pos2::new(row_rect.right(), ry),
                ],
                Stroke::new(1.0 / ppp.max(0.5), rgba(palette.border)),
            );
            if is_active {
                row_painter.rect_stroke(
                    row_rect,
                    0.0,
                    Stroke::new(1.0, Color32::from_rgb(51, 130, 224)),
                    egui::StrokeKind::Inside,
                );
            }

            // Click selection (Aegisub semantics).
            if resp.clicked() {
                if shift_down {
                    state.model.shift_click(row_idx);
                } else if ctrl_down {
                    state.model.ctrl_click(row_idx);
                } else {
                    state.model.click(row_idx);
                }
                action = Some(GridAction::SelectionChanged {
                    active: state.model.active_row(),
                });
            }

            // Context menu with all Aegisub grid actions + HLouis submenu.
            resp.context_menu(|ui| {
                ui.with_layout(Layout::top_down_justified(Align::LEFT), |ui| {
                    if ui.button("Insert (before)").clicked() {
                        action = Some(GridAction::Command("subtitle/insert/before"));
                        ui.close();
                    }
                    if ui.button("Insert (after)").clicked() {
                        action = Some(GridAction::Command("subtitle/insert/after"));
                        ui.close();
                    }
                    if ui.button("Duplicate").clicked() {
                        action = Some(GridAction::Command("edit/line/duplicate"));
                        ui.close();
                    }
                    if ui.button("Delete").clicked() {
                        action = Some(GridAction::Command("edit/line/delete"));
                        ui.close();
                    }
                    ui.separator();
                    if ui.button("Cut").clicked() {
                        action = Some(GridAction::Command("edit/line/cut"));
                        ui.close();
                    }
                    if ui.button("Copy").clicked() {
                        action = Some(GridAction::Command("edit/line/copy"));
                        ui.close();
                    }
                    if ui.button("Paste").clicked() {
                        action = Some(GridAction::Command("edit/line/paste"));
                        ui.close();
                    }
                    if ui.button("Paste Over...").clicked() {
                        action = Some(GridAction::Command("edit/line/paste/over"));
                        ui.close();
                    }
                    ui.separator();
                    if ui.button("Make Times Continuous (Change Start)").clicked() {
                        action = Some(GridAction::Command("time/continuous/start"));
                        ui.close();
                    }
                    if ui.button("Make Times Continuous (Change End)").clicked() {
                        action = Some(GridAction::Command("time/continuous/end"));
                        ui.close();
                    }
                    if ui.button("Snap to Scene").clicked() {
                        action = Some(GridAction::Command("time/snap/scene"));
                        ui.close();
                    }
                    ui.separator();
                    ui.menu_button("HLouis", |ui| {
                        if ui.button("Select Lines by Style (Quick)").clicked() {
                            action = Some(GridAction::Command("hlouis/select_by_style"));
                            ui.close();
                        }
                        if ui.button("Select Lines by Style...").clicked() {
                            action = Some(GridAction::Command("hlouis/select_by_style_dialog"));
                            ui.close();
                        }
                        if ui.button("Clean Whitespace & Line Breaks...").clicked() {
                            action = Some(GridAction::Command("hlouis/clean_whitespace"));
                            ui.close();
                        }
                    });
                });
            });
        }
    });

    state.model.scroll_y = out_scroll.state.clamped_offset().y;
    action
}

#[cfg(test)]
mod tests {
    use super::*;
    use egui_kittest::Harness;
    use i18n_catalog::Locale;
    use theme::ThemeSpec;

    fn sample_script(n: usize) -> ScriptFile {
        let mut script = ScriptFile::new_default();
        let sec = script.ensure_events_section_mut();
        for i in 0..n {
            #[allow(clippy::cast_possible_truncation)]
            let s = (i as u32) * 100;
            sec.insert_event(
                i,
                AssEvent::dialogue(
                    AssTime::from_cs(s),
                    AssTime::from_cs(s + 120),
                    r"{\an5}Hello\Nsubtitle world",
                ),
            );
        }
        script
    }

    #[test]
    fn virtualised_grid_renders_with_kittest_and_default_hidden_columns() {
        let script = sample_script(1_000);
        let mut state = SubtitleGridState::default();
        state.sync_from_script(&script);
        assert!(!state.model.columns.layer);
        assert!(!state.model.columns.actor);
        assert!(!state.model.columns.effect);
        assert!(!state.model.columns.margin_l);

        let pal = theme::palette(ThemeSpec::Light);
        let cat = Catalog::for_locale(Locale::En);
        let fonts = ui_theme::build_font_definitions(&ui_theme::FontConfig::default());
        let mut inited = false;
        let mut harness = Harness::new_ui(move |ui| {
            if !inited {
                ui.ctx().set_fonts(fonts.clone());
                inited = true;
            }
            let _ = show_subtitle_grid(ui, &script, &mut state, Some(150), &pal, &cat);
        });
        harness.run();
    }

    /// Regression: cross-frame `Arc<Galley>` caches kept stale atlas UVs / pixels-per-point
    /// after egui rebuilt its font atlas (garbled header and cells in the first real build).
    #[test]
    fn grid_galleys_are_fresh_after_atlas_rebuild() {
        let mut script = sample_script(40);
        let mut state = SubtitleGridState::default();
        state.sync_from_script(&script);
        let pal = theme::palette(ThemeSpec::Light);
        let cat = Catalog::for_locale(Locale::En);
        let ctx = egui::Context::default();
        ctx.set_fonts(ui_theme::build_font_definitions(
            &ui_theme::FontConfig::default(),
        ));
        let frame = |ctx: &egui::Context, script: &ScriptFile, state: &mut SubtitleGridState| {
            let mut out = ctx.run_ui(egui::RawInput::default(), |ui| {
                let _ = show_subtitle_grid(ui, script, state, None, &pal, &cat);
            });
            out.textures_delta.clear();
            let galleys = ui_theme::galley_check::text_galleys(&out.shapes);
            assert!(
                galleys.iter().any(|g| g.job.text == "Start"),
                "header must be painted"
            );
            ui_theme::galley_check::count_stale(ctx, &out.shapes)
        };
        assert_eq!(frame(&ctx, &script, &mut state), 0);
        assert_eq!(frame(&ctx, &script, &mut state), 0);

        // New glyphs (Vietnamese + CJK) land in the atlas, then the scale changes.
        let sec = script.ensure_events_section_mut();
        for i in 0..40usize {
            sec.insert_event(
                i,
                AssEvent::dialogue(
                    AssTime::from_cs(1_000),
                    AssTime::from_cs(2_000),
                    "Tiếng Việt ắặẳẵ ứừửữự ờởỡợ 日本語の字幕テスト 漢字かな",
                ),
            );
        }
        state.sync_from_script(&script);
        assert_eq!(frame(&ctx, &script, &mut state), 0);
        ctx.set_pixels_per_point(1.5);
        assert_eq!(frame(&ctx, &script, &mut state), 0);
        assert_eq!(frame(&ctx, &script, &mut state), 0);
        ctx.set_pixels_per_point(1.0);
        assert_eq!(frame(&ctx, &script, &mut state), 0);

        // Rebuilding the fonts invalidates every galley.
        let gen_before = state.galley_epoch.sync(&ctx);
        ctx.set_fonts(egui::FontDefinitions::default());
        assert_eq!(frame(&ctx, &script, &mut state), 0);
        assert!(
            state.galley_epoch.sync(&ctx) > gen_before,
            "font rebuild not noticed"
        );

        // Fill the glyph atlas past egui's reset threshold with many large glyph sizes.
        let mut filled = false;
        for round in 0..400u32 {
            let size = 40.0 + round as f32 * 0.37;
            let mut out = ctx.run_ui(egui::RawInput::default(), |ui| {
                let _ = ui.fonts_mut(|f| {
                    f.layout_no_wrap(
                        "ABCDEFGHIJKLMNOPQRSTUVWXYZabcdefghijklmnopqrstuvwxyz".to_owned(),
                        FontId::proportional(size),
                        Color32::WHITE,
                    )
                });
                // The method path form does not satisfy the higher-ranked closure bound.
                #[allow(clippy::redundant_closure_for_method_calls)]
                let ratio = ui.fonts(|f| f.font_atlas_fill_ratio());
                filled = ratio > 0.8;
            });
            out.textures_delta.clear();
            if filled {
                break;
            }
        }
        assert!(filled, "could not fill the font atlas for the reset test");
        let gen_before = state.galley_epoch.sync(&ctx);
        assert_eq!(
            frame(&ctx, &script, &mut state),
            0,
            "stale galleys after atlas reset"
        );
        assert!(
            state.galley_epoch.sync(&ctx) > gen_before,
            "atlas reset not noticed by the galley epoch"
        );
    }
}
