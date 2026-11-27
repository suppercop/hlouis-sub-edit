//! Shared modules for the `hlouis-sub-edit` application binary and benchmarks.

pub mod grid;
pub mod sample_ass;
pub mod syntax;
pub mod theme;

use crate::theme::ThemePalette;
use eframe::egui;
use egui_extras::{Column, TableBuilder, TableRow};

/// Paints a single virtualized subtitle grid row and captures click/selection events.
pub fn render_grid_row(
    row: &mut TableRow<'_, '_>,
    line: &grid::SubtitleLine,
    is_selected: bool,
    is_active: bool,
    palette: &ThemePalette,
    clicked_row: &mut Option<(usize, bool, bool)>,
) {
    let idx = row.index();
    row.set_selected(is_selected);
    row.col(|ui| {
        paint_row_bg(ui, is_selected, is_active, idx, palette);
        ui.label(
            egui::RichText::new(line.number.to_string())
                .monospace()
                .color(palette.text_muted),
        );
    });
    row.col(|ui| {
        paint_row_bg(ui, is_selected, is_active, idx, palette);
        ui.label(
            egui::RichText::new(&line.start_str)
                .monospace()
                .color(palette.text_primary),
        );
    });
    row.col(|ui| {
        paint_row_bg(ui, is_selected, is_active, idx, palette);
        ui.label(
            egui::RichText::new(&line.end_str)
                .monospace()
                .color(palette.text_primary),
        );
    });
    row.col(|ui| {
        paint_row_bg(ui, is_selected, is_active, idx, palette);
        let cps_color = if line.cps >= 25.0 {
            palette.cps_critical
        } else if line.cps >= 18.0 {
            palette.cps_warning
        } else {
            palette.cps_normal
        };
        ui.label(
            egui::RichText::new(&line.cps_str)
                .monospace()
                .color(cps_color),
        );
    });
    row.col(|ui| {
        paint_row_bg(ui, is_selected, is_active, idx, palette);
        ui.label(egui::RichText::new(&line.style).color(palette.text_muted));
    });
    row.col(|ui| {
        paint_row_bg(ui, is_selected, is_active, idx, palette);
        ui.add(
            egui::Label::new(egui::RichText::new(&line.text).color(palette.text_primary))
                .truncate(),
        );
    });

    if row.response().clicked() {
        let (shift, ctrl) = row
            .response()
            .ctx
            .input(|i| (i.modifiers.shift, i.modifiers.command || i.modifiers.ctrl));
        *clicked_row = Some((idx, shift, ctrl));
    }
}

/// Renders the virtualized subtitle grid table over `grid`.
pub fn render_grid_table(
    ui: &mut egui::Ui,
    grid: &mut grid::GridState,
    palette: &ThemePalette,
    headers: [&str; 6],
) {
    let row_height = 22.0;
    let total_rows = grid.lines.len();
    let mut clicked_row: Option<(usize, bool, bool)> = None;

    ui.style_mut().interaction.selectable_labels = false;
    let mut table = TableBuilder::new(ui)
        .striped(false)
        .resizable(true)
        .cell_layout(egui::Layout::left_to_right(egui::Align::Center))
        .sense(egui::Sense::click())
        .column(Column::initial(64.0).at_least(44.0))
        .column(Column::initial(92.0).at_least(72.0))
        .column(Column::initial(92.0).at_least(72.0))
        .column(Column::initial(56.0).at_least(44.0))
        .column(Column::initial(88.0).at_least(60.0))
        .column(Column::remainder().at_least(180.0));

    if grid.scroll_to_active {
        grid.scroll_to_active = false;
        table = table.scroll_to_row(grid.active_row, None);
    }

    table
        .header(24.0, |mut header| {
            for title in headers {
                header.col(|ui| {
                    ui.strong(title);
                });
            }
        })
        .body(|body| {
            body.rows(row_height, total_rows, |mut row| {
                let idx = row.index();
                render_grid_row(
                    &mut row,
                    &grid.lines[idx],
                    grid.selected.contains(&idx),
                    grid.active_row == idx,
                    palette,
                    &mut clicked_row,
                );
            });
        });

    if let Some((idx, shift, ctrl)) = clicked_row {
        grid.click_row(idx, shift, ctrl);
    }
}

fn paint_row_bg(
    ui: &egui::Ui,
    is_selected: bool,
    is_active: bool,
    row_idx: usize,
    palette: &ThemePalette,
) {
    let rect = ui.max_rect();
    let bg = if is_selected {
        palette.row_selected
    } else if row_idx.is_multiple_of(2) {
        palette.row_even
    } else {
        palette.row_odd
    };
    ui.painter().rect_filled(rect, 0.0, bg);
    if is_active {
        ui.painter().line_segment(
            [rect.left_bottom(), rect.right_bottom()],
            egui::Stroke::new(1.0, palette.row_active_border),
        );
    }
}
