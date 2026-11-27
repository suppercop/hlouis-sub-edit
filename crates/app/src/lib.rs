//! Shared modules for the `hlouis-sub-edit` application binary and benchmarks.

pub mod grid;
pub mod sample_ass;
pub mod syntax;
pub mod theme;

use crate::theme::ThemePalette;
use eframe::egui;
use egui_extras::{Column, TableBuilder, TableRow};
use std::sync::Arc;

const ROW_CACHE_SLOTS: usize = 256;
const SHARED_TEXT_SLOTS: usize = 128;

type CachedCellEntry = Option<(i32, Arc<egui::Galley>)>;
type CachedRowCols = [CachedCellEntry; 6];

#[derive(Clone)]
struct CachedRowGalleys {
    row_idx: usize,
    version: u64,
    palette_sig: u64,
    dpi_bits: u32,
    cols: CachedRowCols,
}

#[derive(Clone, Copy, PartialEq, Eq)]
struct SharedCellKey {
    col_idx: usize,
    width_key: i32,
    palette_sig: u64,
    dpi_bits: u32,
    color: egui::Color32,
}

#[derive(Clone)]
struct SharedTextEntry {
    key: SharedCellKey,
    text: String,
    galley: Arc<egui::Galley>,
}

/// Direct-mapped ring cache of shaped cell galleys for recently visible grid rows
/// plus a shared string-level cache for repeated values (`Style`, `CPS`).
#[derive(Clone, Default)]
pub struct RowGalleyCache {
    slots: Vec<Option<CachedRowGalleys>>,
    shared: Vec<Option<SharedTextEntry>>,
}

impl RowGalleyCache {
    #[inline]
    fn row_slot_mut(
        &mut self,
        row_idx: usize,
        version: u64,
        palette_sig: u64,
        dpi_bits: u32,
    ) -> &mut CachedRowCols {
        if self.slots.is_empty() {
            self.slots.resize(ROW_CACHE_SLOTS, None);
        }
        let slot = &mut self.slots[row_idx & (ROW_CACHE_SLOTS - 1)];
        let matches = slot.as_ref().is_some_and(|e| {
            e.row_idx == row_idx
                && e.version == version
                && e.palette_sig == palette_sig
                && e.dpi_bits == dpi_bits
        });
        if !matches {
            *slot = Some(CachedRowGalleys {
                row_idx,
                version,
                palette_sig,
                dpi_bits,
                cols: [None, None, None, None, None, None],
            });
        }
        match slot {
            Some(entry) => &mut entry.cols,
            None => unreachable!(),
        }
    }

    #[inline]
    fn lookup_shared(&self, key: SharedCellKey, text: &str) -> Option<Arc<egui::Galley>> {
        if self.shared.is_empty() {
            return None;
        }
        let h = hash_shared_key(key.col_idx, key.width_key, text);
        let slot = self.shared[h & (SHARED_TEXT_SLOTS - 1)].as_ref()?;
        (slot.key == key && slot.text == text).then(|| Arc::clone(&slot.galley))
    }

    #[inline]
    fn insert_shared(&mut self, key: SharedCellKey, text: &str, galley: Arc<egui::Galley>) {
        if self.shared.is_empty() {
            self.shared.resize(SHARED_TEXT_SLOTS, None);
        }
        let h = hash_shared_key(key.col_idx, key.width_key, text);
        self.shared[h & (SHARED_TEXT_SLOTS - 1)] = Some(SharedTextEntry {
            key,
            text: text.to_owned(),
            galley,
        });
    }
}

#[inline]
fn hash_shared_key(col_idx: usize, width_key: i32, text: &str) -> usize {
    let w = usize::try_from(width_key).unwrap_or(0);
    let mut h = 0x517c_c1b7_2722_0a95_usize ^ col_idx.wrapping_mul(0x9e37_79b9) ^ w;
    for &b in text.as_bytes() {
        h = (h ^ usize::from(b)).wrapping_mul(0x0100_0193);
    }
    h
}

struct RowPaintMeta {
    palette_sig: u64,
    dpi_bits: u32,
}

#[derive(Clone, Copy)]
struct CellStyle {
    monospace: bool,
    truncate: bool,
    color: egui::Color32,
}

#[derive(Clone, Copy)]
struct RowVisualState {
    is_selected: bool,
    is_active: bool,
}

struct CellPaintSpec<'a> {
    col_idx: usize,
    text: &'a str,
    line_version: u64,
    style: CellStyle,
    vis: RowVisualState,
    palette: &'a ThemePalette,
    meta: Option<&'a RowPaintMeta>,
}

fn paint_cell_text(
    row: &mut TableRow<'_, '_>,
    spec: &CellPaintSpec<'_>,
    mut cache: Option<&mut RowGalleyCache>,
) {
    let idx = row.index();
    let style = spec.style;
    let vis = spec.vis;
    row.col(|ui| {
        paint_row_bg(ui, vis.is_selected, vis.is_active, idx, spec.palette);
        let rect = ui.max_rect();
        let width_key = if style.truncate {
            #[allow(clippy::cast_possible_truncation)]
            {
                rect.width().max(1.0).round() as i32
            }
        } else {
            0
        };

        let cached_galley = if let (Some(c), Some(m)) = (cache.as_deref_mut(), spec.meta) {
            if matches!(spec.col_idx, 3 | 4) {
                let key = SharedCellKey {
                    col_idx: spec.col_idx,
                    width_key,
                    palette_sig: m.palette_sig,
                    dpi_bits: m.dpi_bits,
                    color: style.color,
                };
                c.lookup_shared(key, spec.text)
            } else {
                let cols = c.row_slot_mut(idx, spec.line_version, m.palette_sig, m.dpi_bits);
                cols[spec.col_idx]
                    .as_ref()
                    .filter(|(w, _)| *w == width_key)
                    .map(|(_, g)| Arc::clone(g))
            }
        } else {
            None
        };

        let galley = if let Some(hit) = cached_galley {
            hit
        } else {
            let mut rich = egui::RichText::new(spec.text).color(style.color);
            if style.monospace {
                rich = rich.monospace();
            }
            let (wrap_mode, max_w) = if style.truncate {
                (egui::TextWrapMode::Truncate, rect.width().max(1.0))
            } else {
                (egui::TextWrapMode::Extend, f32::INFINITY)
            };
            let built = egui::WidgetText::from(rich).into_galley(
                ui,
                Some(wrap_mode),
                max_w,
                egui::TextStyle::Body,
            );
            if let (Some(c), Some(m)) = (cache, spec.meta) {
                if matches!(spec.col_idx, 3 | 4) {
                    let key = SharedCellKey {
                        col_idx: spec.col_idx,
                        width_key,
                        palette_sig: m.palette_sig,
                        dpi_bits: m.dpi_bits,
                        color: style.color,
                    };
                    c.insert_shared(key, spec.text, Arc::clone(&built));
                } else {
                    let cols = c.row_slot_mut(idx, spec.line_version, m.palette_sig, m.dpi_bits);
                    cols[spec.col_idx] = Some((width_key, Arc::clone(&built)));
                }
            }
            built
        };

        let pos = egui::pos2(rect.left(), rect.center().y - galley.size().y * 0.5);
        ui.painter()
            .with_clip_rect(rect)
            .galley(pos, galley, style.color);
    });
}

fn palette_signature(p: &ThemePalette) -> u64 {
    let mut h = 0xcbf2_9ce4_8422_2325_u64 ^ u64::from(p.is_dark);
    for color in [
        p.text_primary,
        p.text_muted,
        p.cps_normal,
        p.cps_warning,
        p.cps_critical,
    ] {
        for b in color.to_array() {
            h ^= u64::from(b);
            h = h.wrapping_mul(0x0100_0000_01b3);
        }
    }
    h
}

/// Paints a single virtualized subtitle grid row and captures click/selection events.
pub fn render_grid_row(
    row: &mut TableRow<'_, '_>,
    line: &grid::SubtitleLine,
    is_selected: bool,
    is_active: bool,
    palette: &ThemePalette,
    clicked_row: &mut Option<(usize, bool, bool)>,
) {
    render_grid_row_inner(
        row,
        line,
        RowVisualState {
            is_selected,
            is_active,
        },
        palette,
        None,
        None,
        clicked_row,
    );
}

fn render_grid_row_inner(
    row: &mut TableRow<'_, '_>,
    line: &grid::SubtitleLine,
    vis: RowVisualState,
    palette: &ThemePalette,
    mut cache: Option<&mut RowGalleyCache>,
    meta: Option<&RowPaintMeta>,
    clicked_row: &mut Option<(usize, bool, bool)>,
) {
    let idx = row.index();
    row.set_selected(vis.is_selected);

    let cps_color = if line.cps >= 25.0 {
        palette.cps_critical
    } else if line.cps >= 18.0 {
        palette.cps_warning
    } else {
        palette.cps_normal
    };

    let cols: [(&str, CellStyle); 6] = [
        (
            &line.number_str,
            CellStyle {
                monospace: true,
                truncate: false,
                color: palette.text_muted,
            },
        ),
        (
            &line.start_str,
            CellStyle {
                monospace: true,
                truncate: false,
                color: palette.text_primary,
            },
        ),
        (
            &line.end_str,
            CellStyle {
                monospace: true,
                truncate: false,
                color: palette.text_primary,
            },
        ),
        (
            &line.cps_str,
            CellStyle {
                monospace: true,
                truncate: false,
                color: cps_color,
            },
        ),
        (
            &line.style,
            CellStyle {
                monospace: false,
                truncate: true,
                color: palette.text_muted,
            },
        ),
        (
            &line.text,
            CellStyle {
                monospace: false,
                truncate: true,
                color: palette.text_primary,
            },
        ),
    ];

    for (col_idx, (text, style)) in cols.into_iter().enumerate() {
        let spec = CellPaintSpec {
            col_idx,
            text,
            line_version: line.version,
            style,
            vis,
            palette,
            meta,
        };
        paint_cell_text(row, &spec, cache.as_deref_mut());
    }

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
    let meta = RowPaintMeta {
        palette_sig: palette_signature(palette),
        dpi_bits: ui.ctx().pixels_per_point().to_bits(),
    };

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

    let lines = &grid.lines;
    let selected = &grid.selected;
    let active_row = grid.active_row;
    let galley_cache = &mut grid.galley_cache;

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
                render_grid_row_inner(
                    &mut row,
                    &lines[idx],
                    RowVisualState {
                        is_selected: selected.contains(&idx),
                        is_active: active_row == idx,
                    },
                    palette,
                    Some(galley_cache),
                    Some(&meta),
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
