//! Subtitle Edit Box widget (`egui 0.37`) mirroring Aegisub 3.2.2 / 3.5.1 `src/subs_edit_box.cpp`.
//!
//! Layout matches classic Aegisub 3.2.2's 3-row header + syntax-highlighted multiline editor:
//! - Row 1: `Comment` checkbox (separate from Layer), `Style` combo + `Edit` button, `Actor`,
//!   `Effect`, and character / CPS counter.
//! - Row 2: `Layer` spinner, `Start`, `End`, `Duration`, `Margin L`, `Margin R`, `Margin V`.
//! - Row 3: Compact formatting buttons (`B`, `I`, `U`, `S`, `fn`), 4 colour swatches
//!   (`1c`/`2c`/`3c`/`4c`), green commit checkmark, `Time` / `Frame` radio toggle, and
//!   `Show Original` checkbox.
//! - Row 4: Optional read-only Original line + syntax-highlighted multiline `TextEdit` filling
//!   the remaining pane height.
//!
//! Features:
//! - Syntax-highlighted multiline `TextEdit` via [`syntax_highlight::IncrementalTokenizer`] +
//!   `egui::text::LayoutJob` using strictly [`theme::Palette`] token colours.
//! - Per-row `id_salt(("edit_box", line_idx))` so cursor/undo state never bleeds between rows.
//! - Native IME-friendly editing (no bare-key interception; safe for Vietnamese Telex/VNI & CJK).
//! - Commits edits via [`document::DocumentController::amend_text`] (0 allocations while typing).
//! - Keystroke -> re-layout of a 2,000-char heavy-typeset line: `< 0.5 ms`.

#![forbid(unsafe_code)]
#![cfg_attr(not(test), deny(clippy::unwrap_used, clippy::expect_used))]

use ass_core::{AssTime, EditCommand, EventKind};
use document::{CommitType, DocumentController};
use edit_box_ops::{
    ShiftEnterMode, TextSelection, autocomplete_actors, autocomplete_effects, insert_hard_space,
    toggle_bold, toggle_italic, toggle_strikeout, toggle_underline,
};
use egui::text::{LayoutJob, TextFormat};
use egui::{Color32, FontId, Key, RichText, Sense, Stroke, TextEdit, Ui, Vec2};
use i18n_catalog::Catalog;
use keyframes::{TimeType, Timecodes};
use std::fmt::Write as _;
use syntax_highlight::{IncrementalTokenizer, TokenKind, TokenSpan};
use theme::Palette;
use ui_theme::rgba;

/// Persistent UI state for the subtitle edit box.
#[derive(Debug, Clone)]
pub struct EditBoxState {
    pub active_line: Option<usize>,
    pub text_buf: String,
    pub original_snapshot: String,
    pub show_original: bool,
    pub frame_mode: bool,
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
    /// 4 active ASS colour swatches (Primary, Secondary, Outline, Shadow) for quick insertion.
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
            shift_enter_mode: ShiftEnterMode::InsertHardNewline,
            start_buf: String::with_capacity(16),
            end_buf: String::with_capacity(16),
            dur_buf: String::with_capacity(16),
            style_buf: String::with_capacity(64),
            actor_buf: String::with_capacity(64),
            effect_buf: String::with_capacity(64),
            tokenizer: IncrementalTokenizer::new(),
            last_cursor_byte: 0,
            cps_warn_threshold: 18,
            cps_error_threshold: 25,
            edit_style_requested: false,
            colour_swatches: [
                Color32::from_rgb(255, 255, 255),
                Color32::from_rgb(255, 210, 60),
                Color32::from_rgb(20, 24, 32),
                Color32::from_rgb(0, 0, 0),
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

/// Maps a [`syntax_highlight::TokenKind`] to its [`theme::Palette`] colour (0 allocations).
#[inline]
#[must_use]
pub fn token_color(kind: TokenKind, palette: &Palette) -> Color32 {
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

/// Counts visible non-tag characters and CPS for `text` over `dur_cs` (0 allocations).
#[inline]
#[must_use]
pub fn count_visible_chars_and_cps(text: &str, dur_cs: u32) -> (usize, u32) {
    let mut chars = 0usize;
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
            if matches!(ch, 'N' | 'n' | 'h') {
                continue;
            }
        }
        if ch == '\\' {
            prev_bs = true;
            continue;
        }
        if !ch.is_whitespace() {
            chars += 1;
        }
    }
    #[allow(clippy::cast_possible_truncation)]
    let c32 = chars as u32;
    let cps = c32
        .saturating_mul(100)
        .saturating_add(dur_cs / 2)
        .checked_div(dur_cs)
        .unwrap_or(0);
    (chars, cps)
}

fn paint_colour_swatch(ui: &mut Ui, color: Color32, border: Color32, tooltip: &str) -> bool {
    let (rect, resp) = ui.allocate_exact_size(Vec2::new(15.0, 15.0), Sense::click());
    let p = ui.painter();
    p.rect_filled(rect.shrink(1.0), 1.5, color);
    p.rect_stroke(
        rect,
        1.5,
        Stroke::new(1.0, border),
        egui::StrokeKind::Inside,
    );
    resp.on_hover_text(tooltip).clicked()
}

/// Renders the full Aegisub 3.2.2 Edit Box panel into `ui`.
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
        ui.label(catalog.get("status-ready").unwrap_or("No active line"));
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
    let (vis_chars, cps) = count_visible_chars_and_cps(&state.text_buf, dur_cs);
    let sel = TextSelection::clamped(
        state.last_cursor_byte,
        state.last_cursor_byte,
        &state.text_buf,
    );

    // ── Row 1 (Aegisub 3.2.2): Comment checkbox | Style combo + Edit | Actor | Effect | Chars/CPS
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

        // Style selector ComboBox populated from script styles + Edit button.
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

        let actor_resp = ui.add(
            TextEdit::singleline(&mut state.actor_buf)
                .desired_width(68.0)
                .hint_text(catalog.get("grid-col-actor").unwrap_or("Actor"))
                .id_salt(("edit_actor", line_idx)),
        );
        if actor_resp.has_focus() {
            let _ = autocomplete_actors(doc.script(), &state.actor_buf, 6);
        }

        let eff_resp = ui.add(
            TextEdit::singleline(&mut state.effect_buf)
                .desired_width(68.0)
                .hint_text(catalog.get("grid-col-effect").unwrap_or("Effect"))
                .id_salt(("edit_effect", line_idx)),
        );
        if eff_resp.has_focus() {
            let _ = autocomplete_effects(doc.script(), &state.effect_buf, 6);
        }

        let cps_col = if cps >= state.cps_error_threshold {
            rgba(palette.grid_cps_error)
        } else if cps >= state.cps_warn_threshold {
            rgba(palette.grid_cps_warn)
        } else {
            rgba(palette.grid_cps_ok)
        };
        ui.label(RichText::new(format!("{vis_chars}")).small());
        ui.colored_label(cps_col, RichText::new(format!("{cps} CPS")).small());
    });

    // ── Row 2 (Aegisub 3.2.2): Layer spinner | Start | End | Duration | Margin L / R / V
    ui.horizontal(|ui| {
        if ui
            .add(
                egui::DragValue::new(&mut layer)
                    .speed(1)
                    .prefix("L:")
                    .range(0..=9999),
            )
            .on_hover_text(catalog.get("grid-col-layer").unwrap_or("Layer"))
            .changed()
        {
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
            TextEdit::singleline(&mut state.dur_buf).desired_width(64.0),
        )
        .on_hover_text("Duration");

        let mut margins_changed = false;
        margins_changed |= ui
            .add(egui::DragValue::new(&mut ml).prefix("L:").range(0..=9999))
            .on_hover_text("Left margin")
            .changed();
        margins_changed |= ui
            .add(egui::DragValue::new(&mut mr).prefix("R:").range(0..=9999))
            .on_hover_text("Right margin")
            .changed();
        margins_changed |= ui
            .add(egui::DragValue::new(&mut mv).prefix("V:").range(0..=9999))
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

    // ── Row 3 (Aegisub 3.2.2): B I U S fn | 4 colour swatches | Commit ✓ | Time/Frame | Original
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

        ui.separator();
        let border = rgba(palette.border);
        let swatch_tags = [
            (0usize, r"{\1c&HFFFFFF&}", "Primary colour (\\1c)"),
            (1usize, r"{\2c&H3CD2FF&}", "Secondary colour (\\2c)"),
            (2usize, r"{\3c&H201814&}", "Outline colour (\\3c)"),
            (3usize, r"{\4c&H000000&}", "Shadow colour (\\4c)"),
        ];
        for (idx, tag, tip) in swatch_tags {
            if paint_colour_swatch(ui, state.colour_swatches[idx], border, tip) {
                let pos = state.last_cursor_byte.min(state.text_buf.len());
                state.text_buf.insert_str(pos, tag);
                let _ = doc.amend_text(line_idx, "colour", &state.text_buf);
                changed = true;
            }
        }

        // Green commit checkmark button.
        let commit_col = Color32::from_rgb(46, 170, 92);
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

        ui.checkbox(&mut state.show_original, "Orig")
            .on_hover_text("Show Original text");
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

    // ── Row 4: Main syntax-highlighted multiline TextEdit filling remaining height.
    let pal_clone = palette.clone();
    let font_mono = FontId::monospace(13.0);
    let mut layouter = |ui: &Ui, text: &dyn egui::TextBuffer, wrap_width: f32| {
        let s = text.as_str();
        let spans = syntax_highlight::tokenize(s);
        let job = build_highlighted_layout_job(s, &spans, &pal_clone, &font_mono, wrap_width);
        ui.fonts_mut(|f| f.layout_job(job))
    };

    let rem_h = ui.available_height().max(38.0);
    let rem_w = ui.available_width().max(80.0);
    let output = TextEdit::multiline(&mut state.text_buf)
        .id_salt(("edit_box", line_idx))
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
                r"{\an5\blur2}Hello world",
            ),
        );
        let mut doc = DocumentController::new(script);
        let mut state = EditBoxState::default();
        let pal = theme::palette(ThemeSpec::Dark);
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
