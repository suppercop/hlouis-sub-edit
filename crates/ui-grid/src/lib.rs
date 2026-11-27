//! Virtualised high-performance subtitle grid widget (`egui 0.37`) built on [`grid_model::GridModel`],
//! [`syntax_highlight::GridDisplayConfig`], [`theme::Palette`], and [`i18n_catalog::Catalog`].
//!
//! Mirrors Aegisub 3.2.2 / 3.5.1 `src/base_grid.cpp`:
//! - Virtualised rendering (`ScrollArea::show_rows`): only visible rows are laid out (< 1 ms CPU
//!   even with 200,000 lines scrolled to the middle).
//! - Classic Aegisub density; Actor & Margin columns hidden by default; column visibility, widths,
//!   and order are customizable and persistable.
//! - Comment / Selected / Active / Overlap / Visible-on-Video-Frame colouring and CPS badges all
//!   sourced strictly from [`theme::Palette`].
//! - Full Aegisub grid context menu including the `HLouis` submenu (`Select by Style`,
//!   `Clean Whitespace`).

#![forbid(unsafe_code)]
#![cfg_attr(not(test), deny(clippy::unwrap_used, clippy::expect_used))]

use ass_core::{AssEvent, AssTime, EventKind, ScriptFile};
use egui::{Align, FontId, Key, Layout, Pos2, Rect, ScrollArea, Sense, Stroke, Ui, Vec2};
use grid_model::{ColumnVisibility, GridColumn, GridModel, KeyNav, RowFlags, RowTiming};
use i18n_catalog::Catalog;
use std::fmt::Write as _;
use syntax_highlight::{GridDisplayConfig, format_grid_text_into};
use theme::Palette;
use ui_theme::{one_px_stroke, rgba, snap_px};

/// Action emitted by the subtitle grid (mouse/keyboard selection or context menu).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum GridAction {
    SelectionChanged { active: Option<usize> },
    Command(&'static str),
}

#[derive(Debug, Clone)]
struct CachedRow {
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
    cached_rows: Vec<CachedRow>,
    visible_buf: Vec<RowFlags>,
    time_buf_start: String,
    time_buf_end: String,
    num_buf: String,
}

impl Default for SubtitleGridState {
    fn default() -> Self {
        Self {
            model: GridModel::new(),
            display_cfg: GridDisplayConfig::default(),
            row_height: 20.0,
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
            // Default widths matching Aegisub 3.2.2 proportions.
            column_widths: [
                44.0,  // LineNumber
                36.0,  // Layer
                82.0,  // Start
                82.0,  // End
                36.0,  // Cps
                96.0,  // Style
                84.0,  // Actor (hidden by default)
                84.0,  // Effect (hidden by default)
                38.0,  // MarginL (hidden by default)
                38.0,  // MarginR (hidden by default)
                38.0,  // MarginV (hidden by default)
                480.0, // Text (fills remaining)
            ],
            cps_warn_threshold: 18,
            cps_error_threshold: 25,
            scroll_to_active_requested: false,
            cached_rows: Vec::new(),
            visible_buf: Vec::with_capacity(128),
            time_buf_start: String::with_capacity(16),
            time_buf_end: String::with_capacity(16),
            num_buf: String::with_capacity(16),
        }
    }
}

impl SubtitleGridState {
    /// Syncs the underlying [`GridModel`] timings and O(1) render cache from `script`.
    pub fn sync_from_script(&mut self, script: &ScriptFile) {
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
    // chars / (dur_cs / 100) = (visible_chars * 100 + dur_cs/2) / dur_cs
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

struct RowH(f32);
impl grid_model::FontMetrics for RowH {
    fn row_height(&self) -> f32 {
        self.0
    }
}

/// Renders the virtualised subtitle grid into `ui`, returning any user-triggered [`GridAction`].
///
/// Only rows intersecting the visible scroll viewport are fetched and painted (< 1 ms CPU for
/// 200,000 lines scrolled to the middle).
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
    let font_body = FontId::proportional(13.0);
    let font_mono = FontId::monospace(12.0);

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

    // 2. Column header row (with right-click column visibility toggle menu).
    let header_h = state.row_height + 2.0;
    let (header_rect, header_resp) = ui.allocate_exact_size(
        Vec2::new(ui.available_width().max(100.0), header_h),
        Sense::click(),
    );
    let painter = ui.painter_at(header_rect);
    painter.rect_filled(header_rect, 0.0, rgba(palette.toolbar_bg));
    let y_bot = snap_px(header_rect.bottom(), ppp);
    painter.line_segment(
        [
            Pos2::new(header_rect.left(), y_bot),
            Pos2::new(header_rect.right(), y_bot),
        ],
        border_stroke,
    );

    let mut x = header_rect.left();
    for &col in &state.column_order {
        if !SubtitleGridState::is_column_visible(&state.model.columns, col) {
            continue;
        }
        let col_idx = SubtitleGridState::column_index(col);
        let w = if col == GridColumn::Text {
            (header_rect.right() - x).max(120.0)
        } else {
            state.column_widths[col_idx]
        };
        let label = catalog
            .get(SubtitleGridState::column_header_key(col))
            .unwrap_or("?");
        painter.text(
            Pos2::new(x + 4.0, header_rect.center().y),
            egui::Align2::LEFT_CENTER,
            label,
            font_body.clone(),
            rgba(palette.text_secondary),
        );
        x += w;
        let sx = snap_px(x, ppp);
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

            // Background colour strictly from Palette.
            let bg = if is_active {
                palette.grid_active_bg
            } else if is_selected {
                palette.grid_selected_bg
            } else if is_comment {
                palette.grid_comment_bg
            } else if row_idx % 2 == 1 {
                palette.grid_zebra_bg
            } else {
                palette.grid_bg
            };

            let p = ui.painter_at(row_rect);
            p.rect_filled(row_rect, 0.0, rgba(bg));

            // Left accent bar for overlap / visible-on-video-frame.
            if is_overlap {
                let bar = Rect::from_min_size(row_rect.min, Vec2::new(3.0, row_height));
                p.rect_filled(bar, 0.0, rgba(palette.grid_overlap));
            } else if is_vis_frame {
                let bar = Rect::from_min_size(row_rect.min, Vec2::new(3.0, row_height));
                p.rect_filled(bar, 0.0, rgba(palette.focus_ring));
            }

            let text_col = if is_selected || is_active {
                rgba(palette.selection_text)
            } else if is_comment {
                rgba(palette.text_secondary)
            } else {
                rgba(palette.text_primary)
            };

            let mut cx = row_rect.left();
            let cy = row_rect.center().y;
            for &col in &state.column_order {
                if !SubtitleGridState::is_column_visible(&state.model.columns, col) {
                    continue;
                }
                let col_idx = SubtitleGridState::column_index(col);
                let w = if col == GridColumn::Text {
                    (row_rect.right() - cx).max(120.0)
                } else {
                    state.column_widths[col_idx]
                };

                match col {
                    GridColumn::LineNumber => {
                        state.num_buf.clear();
                        let _ = write!(state.num_buf, "{}", row_idx + 1);
                        p.text(
                            Pos2::new(cx + 5.0, cy),
                            egui::Align2::LEFT_CENTER,
                            &state.num_buf,
                            font_mono.clone(),
                            text_col,
                        );
                    }
                    GridColumn::Layer => {
                        state.num_buf.clear();
                        let _ = write!(state.num_buf, "{}", row.layer);
                        p.text(
                            Pos2::new(cx + 4.0, cy),
                            egui::Align2::LEFT_CENTER,
                            &state.num_buf,
                            font_mono.clone(),
                            text_col,
                        );
                    }
                    GridColumn::Start => {
                        format_ass_time_into(
                            AssTime::from_cs(row.start_cs),
                            &mut state.time_buf_start,
                        );
                        p.text(
                            Pos2::new(cx + 4.0, cy),
                            egui::Align2::LEFT_CENTER,
                            &state.time_buf_start,
                            font_mono.clone(),
                            text_col,
                        );
                    }
                    GridColumn::End => {
                        format_ass_time_into(AssTime::from_cs(row.end_cs), &mut state.time_buf_end);
                        p.text(
                            Pos2::new(cx + 4.0, cy),
                            egui::Align2::LEFT_CENTER,
                            &state.time_buf_end,
                            font_mono.clone(),
                            text_col,
                        );
                    }
                    GridColumn::Cps => {
                        let cps = row.cps;
                        let cps_col = if cps >= state.cps_error_threshold {
                            rgba(palette.grid_cps_error)
                        } else if cps >= state.cps_warn_threshold {
                            rgba(palette.grid_cps_warn)
                        } else {
                            rgba(palette.grid_cps_ok)
                        };
                        state.num_buf.clear();
                        let _ = write!(state.num_buf, "{cps}");
                        p.text(
                            Pos2::new(cx + 4.0, cy),
                            egui::Align2::LEFT_CENTER,
                            &state.num_buf,
                            font_mono.clone(),
                            cps_col,
                        );
                    }
                    GridColumn::Style => {
                        p.text(
                            Pos2::new(cx + 4.0, cy),
                            egui::Align2::LEFT_CENTER,
                            &row.style,
                            font_body.clone(),
                            text_col,
                        );
                    }
                    GridColumn::Actor => {
                        p.text(
                            Pos2::new(cx + 4.0, cy),
                            egui::Align2::LEFT_CENTER,
                            &row.actor,
                            font_body.clone(),
                            text_col,
                        );
                    }
                    GridColumn::Effect => {
                        p.text(
                            Pos2::new(cx + 4.0, cy),
                            egui::Align2::LEFT_CENTER,
                            &row.effect,
                            font_body.clone(),
                            text_col,
                        );
                    }
                    GridColumn::MarginL => {
                        state.num_buf.clear();
                        let _ = write!(state.num_buf, "{}", row.margin_l);
                        p.text(
                            Pos2::new(cx + 4.0, cy),
                            egui::Align2::LEFT_CENTER,
                            &state.num_buf,
                            font_mono.clone(),
                            text_col,
                        );
                    }
                    GridColumn::MarginR => {
                        state.num_buf.clear();
                        let _ = write!(state.num_buf, "{}", row.margin_r);
                        p.text(
                            Pos2::new(cx + 4.0, cy),
                            egui::Align2::LEFT_CENTER,
                            &state.num_buf,
                            font_mono.clone(),
                            text_col,
                        );
                    }
                    GridColumn::MarginV => {
                        state.num_buf.clear();
                        let _ = write!(state.num_buf, "{}", row.margin_v);
                        p.text(
                            Pos2::new(cx + 4.0, cy),
                            egui::Align2::LEFT_CENTER,
                            &state.num_buf,
                            font_mono.clone(),
                            text_col,
                        );
                    }
                    GridColumn::Text => {
                        p.text(
                            Pos2::new(cx + 4.0, cy),
                            egui::Align2::LEFT_CENTER,
                            &row.display_text,
                            font_body.clone(),
                            text_col,
                        );
                    }
                }
                cx += w;
            }

            // Crisp 1-px row divider.
            let ry = snap_px(row_rect.bottom(), ppp);
            p.line_segment(
                [
                    Pos2::new(row_rect.left(), ry),
                    Pos2::new(row_rect.right(), ry),
                ],
                Stroke::new(1.0 / ppp.max(0.5), rgba(palette.border)),
            );

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
                    r"{\an5}Hello subtitle world",
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
        assert!(!state.model.columns.actor);
        assert!(!state.model.columns.margin_l);

        let pal = theme::palette(ThemeSpec::Dark);
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
}
