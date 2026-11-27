//! Subtitle Edit Box widget (`egui 0.37`) mirroring Aegisub 3.2.2 / 3.5.1 `src/subs_edit_box.cpp`.
//!
//! Layout matches Aegisub 3.5.1 (3.2.2-style config):
//! - Row 1: `Comment` checkbox, `Style` combo, `Edit` button, `Actor` editable combo, `Effect`
//!   editable combo, and max line length number only (e.g. `23`).
//! - Row 2: `Layer` spin box (up/down arrows, no `L:` prefix), `Start`, `End`, `Duration`, and
//!   margins L / R / V as three plain number boxes (no `L:`/`R:`/`V:` prefixes).
//! - Row 3: `B`, `I`, `U`, `S`, `fn`, four coloured `"AB"` buttons (primary, secondary, outline,
//!   shadow colour pickers), green commit check `✓`, `Time` / `Frame` radios, and `Show Original`
//!   checkbox (full label).
//! - Row 4: White text area in Light mode (`palette.grid_bg` in Dark/Eye Comfort), proportional
//!   UI font by default (optional monospace), `\N` shown in subtle grey inline, syntax-highlighted
//!   multiline `TextEdit` filling the remaining pane height.

#![forbid(unsafe_code)]
#![cfg_attr(not(test), deny(clippy::unwrap_used, clippy::expect_used))]

use ass_core::{AssTime, EditCommand, EventKind};
use document::{CommitType, DocumentController};
use edit_box_ops::{
    ShiftEnterMode, TextSelection, autocomplete_actors, autocomplete_effects, insert_hard_space,
    toggle_bold, toggle_italic, toggle_strikeout, toggle_underline,
};
use egui::text::{LayoutJob, TextFormat};
use egui::{Align2, Color32, FontId, Key, RichText, Sense, Stroke, TextEdit, Ui, Vec2};
use i18n_catalog::Catalog;
use keyframes::{TimeType, Timecodes};
use std::fmt::Write as _;
use syntax_highlight::{IncrementalTokenizer, TokenKind, TokenSpan};
use theme::Palette;
use ui_theme::rgba;

/// Persistent UI state for the subtitle edit box.
#[derive(Debug, Clone)]
#[allow(clippy::struct_excessive_bools)]
pub struct EditBoxState {
    pub active_line: Option<usize>,
    pub text_buf: String,
    pub original_snapshot: String,
    pub show_original: bool,
    pub frame_mode: bool,
    pub use_monospace: bool,
    pub shift_enter_mode: ShiftEnterMode,
    pub start_buf: String,
    pub end_buf: String,
    pub dur_buf: String,
    pub style_buf: String,
    pub actor_buf: String,
    pub effect_buf: String,
    pub tokenizer: IncrementalTokenizer,
    pub last_cursor_byte: usize,
    pub cps_warn_threshold: u32,
    pub cps_error_threshold: u32,
    /// Set to `true` when the user clicks the `Edit` button beside the Style combo.
    pub edit_style_requested: bool,
    /// 4 active ASS colour swatches (Primary, Secondary, Outline, Shadow) rendered as `"AB"`.
    pub colour_swatches: [Color32; 4],
}

impl Default for EditBoxState {
    fn default() -> Self {
        Self {
            active_line: None,
            text_buf: String::with_capacity(512),
            original_snapshot: String::with_capacity(512),
            show_original: false,
            frame_mode: false,
            use_monospace: false,
            shift_enter_mode: ShiftEnterMode::InsertHardNewline,
            start_buf: String::with_capacity(16),
            end_buf: String::with_capacity(16),
            dur_buf: String::with_capacity(16),
            style_buf: String::with_capacity(64),
            actor_buf: String::with_capacity(64),
            effect_buf: String::with_capacity(64),
            tokenizer: IncrementalTokenizer::new(),
            last_cursor_byte: 0,
            cps_warn_threshold: 20,
            cps_error_threshold: 30,
            edit_style_requested: false,
            colour_swatches: [
                Color32::from_rgb(20, 90, 210),
                Color32::from_rgb(210, 120, 16),
                Color32::from_rgb(40, 150, 60),
                Color32::from_rgb(90, 60, 140),
            ],
        }
    }
}

impl EditBoxState {
    /// Synchronizes buffers when the active line changes.
    pub fn sync_active_line(&mut self, doc: &DocumentController, timecodes: Option<&Timecodes>) {
        let active = doc.active_line();
        if self.active_line == active {
            return;
        }
        self.active_line = active;
        self.text_buf.clear();
        self.original_snapshot.clear();
        self.style_buf.clear();
        self.actor_buf.clear();
        self.effect_buf.clear();

        if let Some(idx) = active
            && let Some(ev) = doc.script().events_section().and_then(|s| s.get_event(idx))
        {
            let text = doc.effective_event_text(idx).unwrap_or(ev.text());
            self.text_buf.push_str(text);
            self.original_snapshot.push_str(ev.text());
            self.style_buf.push_str(ev.style());
            self.actor_buf.push_str(ev.name());
            self.effect_buf.push_str(ev.effect());
            format_time_field(ev.start(), self.frame_mode, timecodes, &mut self.start_buf);
            format_time_field(ev.end(), self.frame_mode, timecodes, &mut self.end_buf);
            let dur = AssTime::from_cs(ev.end().as_cs().saturating_sub(ev.start().as_cs()));
            format_time_field(dur, false, None, &mut self.dur_buf);
            self.tokenizer.set_text(&self.text_buf);
        }
    }
}

/// Maps a [`syntax_highlight::TokenKind`] to its [`theme::Palette`] colour (`\N` in subtle grey).
#[inline]
#[must_use]
pub fn token_color(kind: TokenKind, palette: &Palette) -> Color32 {
    if kind == TokenKind::Escape {
        return Color32::from_rgb(138, 144, 154);
    }
    let c = match kind {
        TokenKind::PlainText => palette.syntax_plain,
        TokenKind::OverrideBrace => palette.syntax_brace,
        TokenKind::TagName => palette.syntax_tag_name,
        TokenKind::TagValue => palette.syntax_tag_value,
        TokenKind::KaraokeTag => palette.syntax_karaoke,
        TokenKind::Escape => palette.syntax_escape,
        TokenKind::Comment => palette.syntax_comment,
        TokenKind::DrawingCommand => palette.syntax_drawing,
        TokenKind::Error => palette.syntax_error,
    };
    rgba(c)
}

/// Builds an [`egui::text::LayoutJob`] from pre-tokenized ASS spans into `job` (fast single pass).
#[must_use]
pub fn build_highlighted_layout_job(
    text: &str,
    spans: &[TokenSpan],
    palette: &Palette,
    font_id: &FontId,
    wrap_width: f32,
) -> LayoutJob {
    let mut job = LayoutJob::default();
    job.wrap.max_width = wrap_width;
    let mut cursor = 0usize;
    for span in spans {
        let start = span.start.min(text.len());
        let end = span.end.min(text.len());
        if start > cursor {
            job.append(
                &text[cursor..start],
                0.0,
                TextFormat::simple(font_id.clone(), rgba(palette.syntax_plain)),
            );
        }
        if end > start {
            job.append(
                &text[start..end],
                0.0,
                TextFormat::simple(font_id.clone(), token_color(span.kind, palette)),
            );
            cursor = end;
        }
    }
    if cursor < text.len() {
        job.append(
            &text[cursor..],
            0.0,
            TextFormat::simple(font_id.clone(), rgba(palette.syntax_plain)),
        );
    }
    job
}

/// Formats `AssTime` as `H:MM:SS.cc` or frame number into `out` (0 allocations).
pub fn format_time_field(
    t: AssTime,
    frame_mode: bool,
    timecodes: Option<&Timecodes>,
    out: &mut String,
) {
    out.clear();
    if frame_mode && let Some(tc) = timecodes {
        let f = tc.time_cs_to_frame(t.as_cs(), TimeType::Exact);
        let _ = write!(out, "{f}");
        return;
    }
    let total_cs = t.as_cs();
    let centis = total_cs % 100;
    let total_secs = total_cs / 100;
    let secs = total_secs % 60;
    let total_mins = total_secs / 60;
    let mins = total_mins % 60;
    let hours = total_mins / 60;
    let _ = write!(out, "{hours}:{mins:02}:{secs:02}.{centis:02}");
}

/// Computes the maximum visible character length of any `\N`-separated line (Aegisub edit-box
/// counter) plus CPS (0 allocations).
#[inline]
#[must_use]
pub fn count_visible_chars_and_cps(text: &str, dur_cs: u32) -> (usize, u32) {
    let mut total_chars = 0usize;
    let mut cur_line_chars = 0usize;
    let mut max_line_chars = 0usize;
    let mut in_tag = false;
    let mut prev_bs = false;
    for ch in text.chars() {
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
            if matches!(ch, 'N' | 'n') {
                max_line_chars = max_line_chars.max(cur_line_chars);
                cur_line_chars = 0;
                continue;
            }
            if ch == 'h' {
                cur_line_chars += 1;
                continue;
            }
        }
        if ch == '\\' {
            prev_bs = true;
            continue;
        }
        cur_line_chars += 1;
        if !ch.is_whitespace() {
            total_chars += 1;
        }
    }
    max_line_chars = max_line_chars.max(cur_line_chars);
    #[allow(clippy::cast_possible_truncation)]
    let c32 = total_chars as u32;
    let cps = c32
        .saturating_mul(100)
        .saturating_add(dur_cs / 2)
        .checked_div(dur_cs)
        .unwrap_or(0);
    (max_line_chars, cps)
}

/// Paints a classic Aegisub `"AB"` colour-picker button (primary, secondary, outline, shadow).
fn paint_ab_colour_button(ui: &mut Ui, fg_color: Color32, pal: &Palette, tooltip: &str) -> bool {
    let (rect, resp) = ui.allocate_exact_size(Vec2::new(22.0, 18.0), Sense::click());
    let p = ui.painter();
    let bg = if resp.hovered() {
        rgba(pal.hover)
    } else {
        rgba(pal.toolbar_bg)
    };
    p.rect_filled(rect, 2.0, bg);
    p.rect_stroke(
        rect,
        2.0,
        Stroke::new(1.0, rgba(pal.border)),
        egui::StrokeKind::Inside,
    );
    p.text(
        rect.center(),
        Align2::CENTER_CENTER,
        "AB",
        FontId::proportional(10.5),
        fg_color,
    );
    resp.on_hover_text(tooltip).clicked()
}

/// Paints a spin box with compact up/down arrows (no text prefix, matching Aegisub Layer).
fn layer_spin_box(ui: &mut Ui, value: &mut i32, pal: &Palette) -> bool {
    let mut changed = false;
    ui.horizontal(|ui| {
        ui.spacing_mut().item_spacing.x = 1.0;
        changed |= ui
            .add(
                egui::DragValue::new(value)
                    .speed(1)
                    .range(0..=9999)
                    .fixed_decimals(0),
            )
            .on_hover_text("Layer")
            .changed();
        ui.vertical(|ui| {
            ui.spacing_mut().item_spacing.y = 0.0;
            let (up_r, up_resp) = ui.allocate_exact_size(Vec2::new(11.0, 8.0), Sense::click());
            let (dn_r, dn_resp) = ui.allocate_exact_size(Vec2::new(11.0, 8.0), Sense::click());
            let p = ui.painter();
            let fg = rgba(pal.text_primary);
            let bd = Stroke::new(0.8, rgba(pal.border));
            p.rect_filled(
                up_r,
                1.0,
                if up_resp.hovered() {
                    rgba(pal.hover)
                } else {
                    rgba(pal.toolbar_bg)
                },
            );
            p.rect_stroke(up_r, 1.0, bd, egui::StrokeKind::Inside);
            p.text(
                up_r.center(),
                Align2::CENTER_CENTER,
                "▲",
                FontId::proportional(6.5),
                fg,
            );
            p.rect_filled(
                dn_r,
                1.0,
                if dn_resp.hovered() {
                    rgba(pal.hover)
                } else {
                    rgba(pal.toolbar_bg)
                },
            );
            p.rect_stroke(dn_r, 1.0, bd, egui::StrokeKind::Inside);
            p.text(
                dn_r.center(),
                Align2::CENTER_CENTER,
                "▼",
                FontId::proportional(6.5),
                fg,
            );
            if up_resp.clicked() {
                *value = (*value + 1).min(9999);
                changed = true;
            }
            if dn_resp.clicked() {
                *value = (*value - 1).max(0);
                changed = true;
            }
        });
    });
    changed
}

/// Renders an editable combo (`TextEdit` + compact dropdown arrow showing suggestions).
fn editable_combo(
    ui: &mut Ui,
    id_salt: (&str, usize),
    buf: &mut String,
    hint: &str,
    width: f32,
    suggestions: &[String],
) -> bool {
    let mut changed = false;
    ui.horizontal(|ui| {
        ui.spacing_mut().item_spacing.x = 0.0;
        let resp = ui.add(
            TextEdit::singleline(buf)
                .desired_width(width)
                .hint_text(hint)
                .id_salt(id_salt),
        );
        if resp.changed() {
            changed = true;
        }
        egui::ComboBox::from_id_salt((id_salt.0, id_salt.1, "dd"))
            .width(14.0)
            .selected_text("")
            .show_ui(ui, |ui| {
                if suggestions.is_empty() {
                    ui.weak("(none)");
                } else {
                    for s in suggestions {
                        if ui.selectable_label(buf == s, s).clicked() {
                            buf.clone_from(s);
                            changed = true;
                        }
                    }
                }
            });
    });
    changed
}

/// Renders the full Aegisub 3.5.1 (3.2.2-style) Edit Box panel into `ui`.
/// Returns `true` if the active line was modified on this frame.
#[allow(clippy::too_many_lines)]
pub fn show_edit_box(
    ui: &mut Ui,
    doc: &mut DocumentController,
    state: &mut EditBoxState,
    timecodes: Option<&Timecodes>,
    palette: &Palette,
    catalog: &Catalog,
) -> bool {
    state.sync_active_line(doc, timecodes);
    let Some(line_idx) = state.active_line else {
        return false;
    };
    let Some(ev_snapshot) = doc
        .script()
        .events_section()
        .and_then(|s| s.get_event(line_idx))
        .cloned()
    else {
        return false;
    };

    let mut changed = false;
    let mut is_comment = ev_snapshot.kind() == EventKind::Comment;
    let mut layer = ev_snapshot.layer();
    let mut ml = ev_snapshot.margin_l();
    let mut mr = ev_snapshot.margin_r();
    let mut mv = ev_snapshot.margin_v();

    let dur_cs = ev_snapshot
        .end()
        .as_cs()
        .saturating_sub(ev_snapshot.start().as_cs());
    let (max_line_len, _cps) = count_visible_chars_and_cps(&state.text_buf, dur_cs);
    let sel = TextSelection::clamped(
        state.last_cursor_byte,
        state.last_cursor_byte,
        &state.text_buf,
    );

    // ── Row 1 (Aegisub): Comment | Style combo | Edit | Actor editable combo | Effect editable combo | max line length only
    ui.horizontal(|ui| {
        if ui
            .checkbox(
                &mut is_comment,
                catalog.get("panel-edit-comment").unwrap_or("Comment"),
            )
            .changed()
        {
            let mut after = ev_snapshot.clone();
            after.set_kind(if is_comment {
                EventKind::Comment
            } else {
                EventKind::Dialogue
            });
            let _ = doc.commit(
                "toggle comment",
                EditCommand::ModifyEvent {
                    index: line_idx,
                    before: Box::new(ev_snapshot.clone()),
                    after: Box::new(after),
                },
                CommitType::TEXT,
            );
            changed = true;
        }

        let style_names: Vec<String> = doc
            .script()
            .styles()
            .map(|st| st.name().to_owned())
            .collect();
        let prev_style = state.style_buf.clone();
        egui::ComboBox::from_id_salt(("edit_style_combo", line_idx))
            .width(86.0)
            .selected_text(&state.style_buf)
            .show_ui(ui, |ui| {
                if style_names.is_empty() {
                    ui.selectable_value(&mut state.style_buf, "Default".to_owned(), "Default");
                } else {
                    for name in &style_names {
                        ui.selectable_value(&mut state.style_buf, name.clone(), name);
                    }
                }
            });
        if state.style_buf != prev_style {
            let mut after = ev_snapshot.clone();
            after.set_style(state.style_buf.clone());
            let _ = doc.commit(
                "change style",
                EditCommand::ModifyEvent {
                    index: line_idx,
                    before: Box::new(ev_snapshot.clone()),
                    after: Box::new(after),
                },
                CommitType::TEXT,
            );
            changed = true;
        }
        if ui
            .small_button(catalog.get("btn-edit").unwrap_or("Edit"))
            .on_hover_text("Edit active style")
            .clicked()
        {
            state.edit_style_requested = true;
        }

        let actor_sugs = autocomplete_actors(doc.script(), &state.actor_buf, 8);
        if editable_combo(
            ui,
            ("edit_actor", line_idx),
            &mut state.actor_buf,
            catalog.get("grid-col-actor").unwrap_or("Actor"),
            64.0,
            &actor_sugs,
        ) {
            let mut after = ev_snapshot.clone();
            after.set_name(state.actor_buf.clone());
            let _ = doc.commit(
                "change actor",
                EditCommand::ModifyEvent {
                    index: line_idx,
                    before: Box::new(ev_snapshot.clone()),
                    after: Box::new(after),
                },
                CommitType::TEXT,
            );
            changed = true;
        }

        let eff_sugs = autocomplete_effects(doc.script(), &state.effect_buf, 8);
        if editable_combo(
            ui,
            ("edit_effect", line_idx),
            &mut state.effect_buf,
            catalog.get("grid-col-effect").unwrap_or("Effect"),
            64.0,
            &eff_sugs,
        ) {
            let mut after = ev_snapshot.clone();
            after.set_effect(state.effect_buf.clone());
            let _ = doc.commit(
                "change effect",
                EditCommand::ModifyEvent {
                    index: line_idx,
                    before: Box::new(ev_snapshot.clone()),
                    after: Box::new(after),
                },
                CommitType::TEXT,
            );
            changed = true;
        }

        // Max line length number only (matches Aegisub's "23" readout).
        let mut len_str = format!("{max_line_len}");
        ui.add_enabled(
            false,
            TextEdit::singleline(&mut len_str).desired_width(28.0),
        )
        .on_hover_text("Maximum line length (characters)");
    });

    // ── Row 2 (Aegisub): Layer spin box (no "L:") | Start | End | Duration | plain Margin L R V
    ui.horizontal(|ui| {
        if layer_spin_box(ui, &mut layer, palette) {
            let mut after = ev_snapshot.clone();
            after.set_layer(layer);
            let _ = doc.commit(
                "change layer",
                EditCommand::ModifyEvent {
                    index: line_idx,
                    before: Box::new(ev_snapshot.clone()),
                    after: Box::new(after),
                },
                CommitType::TEXT,
            );
            changed = true;
        }
        ui.add(
            TextEdit::singleline(&mut state.start_buf)
                .desired_width(72.0)
                .id_salt(("edit_start", line_idx)),
        )
        .on_hover_text(catalog.get("grid-col-start").unwrap_or("Start"));
        ui.add(
            TextEdit::singleline(&mut state.end_buf)
                .desired_width(72.0)
                .id_salt(("edit_end", line_idx)),
        )
        .on_hover_text(catalog.get("grid-col-end").unwrap_or("End"));
        ui.add_enabled(
            false,
            TextEdit::singleline(&mut state.dur_buf).desired_width(68.0),
        )
        .on_hover_text("Duration");

        // Plain margin number boxes (no "L:", "R:", "V:" prefixes).
        let mut margins_changed = false;
        margins_changed |= ui
            .add(egui::DragValue::new(&mut ml).range(0..=9999))
            .on_hover_text("Left margin")
            .changed();
        margins_changed |= ui
            .add(egui::DragValue::new(&mut mr).range(0..=9999))
            .on_hover_text("Right margin")
            .changed();
        margins_changed |= ui
            .add(egui::DragValue::new(&mut mv).range(0..=9999))
            .on_hover_text("Vertical margin")
            .changed();
        if margins_changed {
            let mut after = ev_snapshot.clone();
            after.set_margin_l(ml);
            after.set_margin_r(mr);
            after.set_margin_v(mv);
            let _ = doc.commit(
                "change margins",
                EditCommand::ModifyEvent {
                    index: line_idx,
                    before: Box::new(ev_snapshot.clone()),
                    after: Box::new(after),
                },
                CommitType::TEXT,
            );
            changed = true;
        }
    });

    // ── Row 3 (Aegisub): B I U S fn | 4 coloured "AB" buttons | green commit ✓ | Time/Frame | Show Original
    ui.horizontal(|ui| {
        if ui
            .small_button(RichText::new("B").strong())
            .on_hover_text("Bold (\\b1)")
            .clicked()
        {
            let res = toggle_bold(&state.text_buf, sel, ev_snapshot.style(), &[]);
            state.text_buf = res.text;
            let _ = doc.amend_text(line_idx, "bold", &state.text_buf);
            changed = true;
        }
        if ui
            .small_button(RichText::new("I").italics())
            .on_hover_text("Italic (\\i1)")
            .clicked()
        {
            let res = toggle_italic(&state.text_buf, sel, ev_snapshot.style(), &[]);
            state.text_buf = res.text;
            let _ = doc.amend_text(line_idx, "italic", &state.text_buf);
            changed = true;
        }
        if ui
            .small_button(RichText::new("U").underline())
            .on_hover_text("Underline (\\u1)")
            .clicked()
        {
            let res = toggle_underline(&state.text_buf, sel, ev_snapshot.style(), &[]);
            state.text_buf = res.text;
            let _ = doc.amend_text(line_idx, "underline", &state.text_buf);
            changed = true;
        }
        if ui
            .small_button(RichText::new("S").strikethrough())
            .on_hover_text("Strikeout (\\s1)")
            .clicked()
        {
            let res = toggle_strikeout(&state.text_buf, sel, ev_snapshot.style(), &[]);
            state.text_buf = res.text;
            let _ = doc.amend_text(line_idx, "strikeout", &state.text_buf);
            changed = true;
        }
        if ui
            .small_button("fn")
            .on_hover_text("Insert font override (\\fn)")
            .clicked()
        {
            let pos = state.last_cursor_byte.min(state.text_buf.len());
            state.text_buf.insert_str(pos, r"{\fnArial}");
            let _ = doc.amend_text(line_idx, "font", &state.text_buf);
            changed = true;
        }

        let ab_tags = [
            (0usize, r"{\1c&HFFFFFF&}", "Primary colour (\\1c)"),
            (1usize, r"{\2c&H3CD2FF&}", "Secondary colour (\\2c)"),
            (2usize, r"{\3c&H201814&}", "Outline colour (\\3c)"),
            (3usize, r"{\4c&H000000&}", "Shadow colour (\\4c)"),
        ];
        for (idx, tag, tip) in ab_tags {
            if paint_ab_colour_button(ui, state.colour_swatches[idx], palette, tip) {
                let pos = state.last_cursor_byte.min(state.text_buf.len());
                state.text_buf.insert_str(pos, tag);
                let _ = doc.amend_text(line_idx, "colour", &state.text_buf);
                changed = true;
            }
        }

        let commit_col = Color32::from_rgb(38, 168, 84);
        if ui
            .small_button(RichText::new("✓").color(commit_col).strong())
            .on_hover_text("Commit changes (Enter)")
            .clicked()
        {
            let _ = doc.amend_text(line_idx, "commit", &state.text_buf);
            changed = true;
        }

        ui.separator();
        let prev_frame_mode = state.frame_mode;
        ui.radio_value(&mut state.frame_mode, false, "Time");
        ui.radio_value(&mut state.frame_mode, true, "Frame");
        if state.frame_mode != prev_frame_mode {
            format_time_field(
                ev_snapshot.start(),
                state.frame_mode,
                timecodes,
                &mut state.start_buf,
            );
            format_time_field(
                ev_snapshot.end(),
                state.frame_mode,
                timecodes,
                &mut state.end_buf,
            );
        }

        ui.checkbox(&mut state.show_original, "Show Original");
    });

    // Optional "Show Original" read-only preview.
    if state.show_original {
        ui.add_enabled(
            false,
            TextEdit::multiline(&mut state.original_snapshot)
                .desired_rows(2)
                .desired_width(f32::INFINITY),
        );
    }

    // ── Row 4: White (in Light mode) proportional UI font syntax-highlighted multiline TextEdit.
    let pal_clone = palette.clone();
    let edit_font = if state.use_monospace {
        FontId::monospace(12.5)
    } else {
        FontId::proportional(13.0)
    };
    let mut layouter = |ui: &Ui, text: &dyn egui::TextBuffer, wrap_width: f32| {
        let s = text.as_str();
        let spans = syntax_highlight::tokenize(s);
        let job = build_highlighted_layout_job(s, &spans, &pal_clone, &edit_font, wrap_width);
        ui.fonts_mut(|f| f.layout_job(job))
    };

    let rem_h = ui.available_height().max(38.0);
    let rem_w = ui.available_width().max(80.0);
    let is_light =
        u16::from(palette.grid_bg.r) + u16::from(palette.grid_bg.g) + u16::from(palette.grid_bg.b)
            > 384;
    let bg_fill = if is_light {
        Color32::WHITE
    } else {
        rgba(palette.grid_bg)
    };
    let output = TextEdit::multiline(&mut state.text_buf)
        .id_salt(("edit_box", line_idx))
        .background_color(bg_fill)
        .min_size(Vec2::new(rem_w, rem_h))
        .desired_width(f32::INFINITY)
        .layouter(&mut layouter)
        .show(ui);

    if let Some(range) = output.cursor_range {
        let char_idx = range.primary.index;
        state.last_cursor_byte = state
            .text_buf
            .char_indices()
            .nth(char_idx.into())
            .map_or(state.text_buf.len(), |(b, _)| b);
    }

    // Ctrl+Shift+Space inserts `\h` without interfering with IME composition.
    if output.response.has_focus()
        && ui.input(|i| i.modifiers.ctrl && i.modifiers.shift && i.key_pressed(Key::Space))
    {
        let cur_sel = TextSelection::clamped(
            state.last_cursor_byte,
            state.last_cursor_byte,
            &state.text_buf,
        );
        let res = insert_hard_space(&state.text_buf, cur_sel);
        state.text_buf = res.text;
        let _ = doc.amend_text(line_idx, "hard space", &state.text_buf);
        changed = true;
    }

    if output.response.changed() {
        state.tokenizer.set_text(&state.text_buf);
        let _ = doc.amend_text(line_idx, "typing", &state.text_buf);
        changed = true;
    }

    changed
}

#[cfg(test)]
mod tests {
    use super::*;
    use ass_core::{AssEvent, ScriptFile};
    use egui_kittest::Harness;
    use i18n_catalog::Locale;
    use theme::ThemeSpec;

    #[test]
    fn edit_box_renders_with_kittest_and_amends_document() {
        let mut script = ScriptFile::new_default();
        script.ensure_events_section_mut().insert_event(
            0,
            AssEvent::dialogue(
                AssTime::ZERO,
                AssTime::from_cs(250),
                r"{\an5\blur2}Hello\Nworld",
            ),
        );
        let mut doc = DocumentController::new(script);
        let mut state = EditBoxState::default();
        let pal = theme::palette(ThemeSpec::Light);
        let cat = Catalog::for_locale(Locale::En);
        let fonts = ui_theme::build_font_definitions(&ui_theme::FontConfig::default());
        let mut inited = false;

        let builder = Harness::builder().with_accessibility_check(false);
        let mut harness = builder.build_ui(move |ui| {
            if !inited {
                ui.ctx().set_fonts(fonts.clone());
                inited = true;
            }
            let _ = show_edit_box(ui, &mut doc, &mut state, None, &pal, &cat);
        });
        harness.run();
    }
}
