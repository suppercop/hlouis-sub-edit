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

use ass_core::{AssEvent, AssTime, EditCommand, EventKind, ScriptFile};
use document::{CommitType, DocumentController};
use edit_box_ops::{
    ShiftEnterMode, TextSelection, autocomplete_actors, autocomplete_effects, insert_hard_space,
    toggle_bold, toggle_italic, toggle_strikeout, toggle_underline,
};
use egui::text::{LayoutJob, TextFormat};
use egui::{Color32, FontId, Galley, Key, Pos2, RichText, Sense, Stroke, TextEdit, Ui, Vec2};
use i18n_catalog::Catalog;
use keyframes::{TimeType, Timecodes};
use std::fmt::Write as _;
use std::sync::Arc;
use syntax_highlight::{IncrementalTokenizer, TokenKind, TokenSpan, tokenize_into};
use theme::Palette;
use ui_theme::rgba;

/// Edit-box text galley kept across frames; valid only for the [`ui_theme::GalleyEpoch`]
/// generation it was laid out in.
#[derive(Clone)]
struct CachedEditGalley {
    text: String,
    wrap_width_bits: u32,
    use_monospace: bool,
    pal_sig: [u32; 9],
    generation: u64,
    galley: Arc<Galley>,
}

impl std::fmt::Debug for CachedEditGalley {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("CachedEditGalley")
            .field("len", &self.text.len())
            .field("wrap_width_bits", &self.wrap_width_bits)
            .finish_non_exhaustive()
    }
}

/// "AB" colour-button galley kept across frames (same generation rule).
#[derive(Clone)]
struct CachedAbGalley {
    color: Color32,
    generation: u64,
    galley: Arc<Galley>,
}

impl std::fmt::Debug for CachedAbGalley {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("CachedAbGalley")
            .field("color", &self.color)
            .finish_non_exhaustive()
    }
}

#[inline]
const fn rgba_u32(c: theme::Rgba8) -> u32 {
    u32::from_le_bytes([c.r, c.g, c.b, c.a])
}

#[inline]
const fn palette_syntax_sig(p: &Palette) -> [u32; 9] {
    [
        rgba_u32(p.syntax_plain),
        rgba_u32(p.syntax_brace),
        rgba_u32(p.syntax_tag_name),
        rgba_u32(p.syntax_tag_value),
        rgba_u32(p.syntax_karaoke),
        rgba_u32(p.syntax_escape),
        rgba_u32(p.syntax_comment),
        rgba_u32(p.syntax_drawing),
        rgba_u32(p.syntax_error),
    ]
}

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
    /// Set to `true` to focus the subtitle text field on the next frame (e.g. after insert).
    pub request_focus: bool,
    /// 4 active ASS colour swatches (Primary, Secondary, Outline, Shadow) rendered as `"AB"`.
    pub colour_swatches: [Color32; 4],
    cached_event: Option<AssEvent>,
    cached_event_key: Option<(u64, usize)>,
    cached_doc_rev: Option<u64>,
    event_cache_refreshes: u64,
    event_lookups: u64,
    max_len_buf: String,
    last_max_len: Option<usize>,
    span_scratch: Vec<TokenSpan>,
    galley_epoch: ui_theme::GalleyEpoch,
    galley_cache: Option<CachedEditGalley>,
    ab_galleys: [Option<CachedAbGalley>; 4],
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
            request_focus: false,
            colour_swatches: [
                Color32::from_rgb(20, 90, 210),
                Color32::from_rgb(210, 120, 16),
                Color32::from_rgb(40, 150, 60),
                Color32::from_rgb(90, 60, 140),
            ],
            cached_event: None,
            cached_event_key: None,
            cached_doc_rev: None,
            event_cache_refreshes: 0,
            event_lookups: 0,
            max_len_buf: String::with_capacity(12),
            last_max_len: None,
            span_scratch: Vec::with_capacity(128),
            galley_epoch: ui_theme::GalleyEpoch::default(),
            galley_cache: None,
            ab_galleys: [None, None, None, None],
        }
    }
}

impl EditBoxState {
    /// Synchronizes buffers when the active line or document revision changes, reusing the cached
    /// [`AssEvent`] snapshot on idle frames and during in-place same-line text amends.
    pub fn sync_active_line(&mut self, doc: &DocumentController, timecodes: Option<&Timecodes>) {
        let active = doc.active_line();
        let doc_rev = doc.revision();
        let non_text_rev = doc.non_text_revision();

        if self.active_line == active && self.cached_doc_rev == Some(doc_rev) {
            return;
        }

        let Some(idx) = active else {
            self.active_line = None;
            self.cached_event = None;
            self.cached_event_key = None;
            self.cached_doc_rev = Some(doc_rev);
            self.text_buf.clear();
            self.original_snapshot.clear();
            self.style_buf.clear();
            self.actor_buf.clear();
            self.effect_buf.clear();
            return;
        };

        // Fast path: same active line and only an in-place `amend_text` bumped `doc_rev`.
        if self.active_line == Some(idx)
            && self.cached_event_key == Some((non_text_rev, idx))
            && self.cached_event.is_some()
        {
            if let Some(text) = doc.effective_event_text(idx)
                && self.text_buf != text
            {
                self.text_buf.clear();
                self.text_buf.push_str(text);
                self.tokenizer.set_text(&self.text_buf);
            }
            self.cached_doc_rev = Some(doc_rev);
            self.event_cache_refreshes = self.event_cache_refreshes.saturating_add(1);
            return;
        }

        self.active_line = Some(idx);
        self.cached_doc_rev = Some(doc_rev);
        self.text_buf.clear();
        self.original_snapshot.clear();
        self.style_buf.clear();
        self.actor_buf.clear();
        self.effect_buf.clear();

        self.event_lookups = self.event_lookups.saturating_add(1);
        if let Some(ev) = doc
            .script()
            .events_section()
            .and_then(|s| s.get_event(idx))
            .cloned()
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
            let style_opt = doc
                .script()
                .styles()
                .find(|st| st.name().eq_ignore_ascii_case(ev.style()))
                .or_else(|| doc.script().styles().next());
            if let Some(st) = style_opt {
                self.colour_swatches = [
                    parse_ass_colour_rgb(st.primary_colour()).unwrap_or(Color32::WHITE),
                    parse_ass_colour_rgb(st.secondary_colour())
                        .unwrap_or(Color32::from_rgb(255, 0, 0)),
                    parse_ass_colour_rgb(st.outline_colour()).unwrap_or(Color32::BLACK),
                    parse_ass_colour_rgb(st.back_colour()).unwrap_or(Color32::BLACK),
                ];
            }
            self.cached_event = Some(ev);
            self.cached_event_key = Some((non_text_rev, idx));
            self.event_cache_refreshes = self.event_cache_refreshes.saturating_add(1);
        } else {
            self.cached_event = None;
            self.cached_event_key = None;
        }
    }

    /// Number of times the edit-box active-line cache was refreshed (including fast-path amends).
    #[must_use]
    pub const fn event_cache_refreshes(&self) -> u64 {
        self.event_cache_refreshes
    }

    /// Number of times the edit box called `EventsSection::get_event` to load the active line.
    #[must_use]
    pub const fn event_lookups(&self) -> u64 {
        self.event_lookups
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

/// Parses an ASS `&HAABBGGRR` or `&HBBGGRR` colour string into an opaque [`Color32`].
#[must_use]
pub fn parse_ass_colour_rgb(raw: &str) -> Option<Color32> {
    let trimmed = raw.trim().trim_end_matches('&');
    let hex = trimmed
        .strip_prefix("&H")
        .or_else(|| trimmed.strip_prefix("&h"))
        .unwrap_or(trimmed);
    let val = u32::from_str_radix(hex, 16).ok()?;
    #[allow(clippy::cast_possible_truncation)]
    let r = (val & 0xFF) as u8;
    #[allow(clippy::cast_possible_truncation)]
    let g = ((val >> 8) & 0xFF) as u8;
    #[allow(clippy::cast_possible_truncation)]
    let b = ((val >> 16) & 0xFF) as u8;
    Some(Color32::from_rgb(r, g, b))
}

/// Paints a classic Aegisub `"AB"` colour-picker button: bold `"AB"` in the active style's actual
/// colour (primary, secondary, outline, shadow) on a normal white button.
fn paint_ab_colour_button(
    ui: &mut Ui,
    fg_color: Color32,
    cached: &mut Option<CachedAbGalley>,
    generation: u64,
    pal: &Palette,
    tooltip: &str,
) -> bool {
    let (rect, resp) = ui.allocate_exact_size(Vec2::new(22.0, 18.0), Sense::click());
    let is_light =
        u16::from(pal.grid_bg.r) + u16::from(pal.grid_bg.g) + u16::from(pal.grid_bg.b) > 384;
    let bg = if resp.hovered() {
        if is_light {
            Color32::from_rgb(238, 244, 252)
        } else {
            Color32::from_rgb(224, 228, 236)
        }
    } else if is_light {
        Color32::WHITE
    } else {
        Color32::from_rgb(238, 238, 240)
    };
    let galley = match cached {
        Some(entry) if entry.generation == generation && entry.color == fg_color => {
            Arc::clone(&entry.galley)
        }
        slot => {
            let g = ui.fonts_mut(|f| {
                f.layout_no_wrap("AB".to_owned(), FontId::proportional(10.5), fg_color)
            });
            *slot = Some(CachedAbGalley {
                color: fg_color,
                generation,
                galley: Arc::clone(&g),
            });
            g
        }
    };
    let p = ui.painter();
    p.rect_filled(rect, 2.0, bg);
    p.rect_stroke(
        rect,
        2.0,
        Stroke::new(1.0, rgba(pal.border)),
        egui::StrokeKind::Inside,
    );
    let lum = (u32::from(fg_color.r()) * 299
        + u32::from(fg_color.g()) * 587
        + u32::from(fg_color.b()) * 114)
        / 1000;
    if lum > 200 {
        p.rect_filled(rect.shrink(2.0), 1.5, Color32::from_rgb(76, 82, 92));
    }
    let gsize = galley.size();
    p.galley(
        Pos2::new(
            rect.center().x - gsize.x * 0.5,
            rect.center().y - gsize.y * 0.5,
        ),
        galley,
        fg_color,
    );
    resp.on_hover_text(tooltip).clicked()
}

/// Paints a vector green checkmark commit button (avoids Unicode `"✓"` glyph lookup).
fn paint_commit_check_button(ui: &mut Ui, pal: &Palette) -> bool {
    let (rect, resp) = ui.allocate_exact_size(Vec2::new(20.0, 18.0), Sense::click());
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
    let c = rect.center();
    let st = Stroke::new(1.8, Color32::from_rgb(38, 168, 84));
    p.line_segment(
        [
            Pos2::new(c.x - 4.5, c.y + 0.5),
            Pos2::new(c.x - 1.5, c.y + 3.5),
        ],
        st,
    );
    p.line_segment(
        [
            Pos2::new(c.x - 1.5, c.y + 3.5),
            Pos2::new(c.x + 4.5, c.y - 3.5),
        ],
        st,
    );
    resp.on_hover_text("Commit changes (Enter)").clicked()
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
            let arrow_st = Stroke::new(1.2, fg);
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
            let uc = up_r.center();
            p.line_segment(
                [
                    Pos2::new(uc.x - 2.5, uc.y + 1.2),
                    Pos2::new(uc.x, uc.y - 1.5),
                ],
                arrow_st,
            );
            p.line_segment(
                [
                    Pos2::new(uc.x, uc.y - 1.5),
                    Pos2::new(uc.x + 2.5, uc.y + 1.2),
                ],
                arrow_st,
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
            let dc = dn_r.center();
            p.line_segment(
                [
                    Pos2::new(dc.x - 2.5, dc.y - 1.2),
                    Pos2::new(dc.x, dc.y + 1.5),
                ],
                arrow_st,
            );
            p.line_segment(
                [
                    Pos2::new(dc.x, dc.y + 1.5),
                    Pos2::new(dc.x + 2.5, dc.y - 1.2),
                ],
                arrow_st,
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

/// Renders an editable combo (`TextEdit` + compact dropdown arrow whose suggestions are computed
/// lazily ONLY when the popup is open, avoiding 200k-line script scans per frame).
fn editable_combo(
    ui: &mut Ui,
    id_salt: (&str, usize),
    buf: &mut String,
    hint: &str,
    width: f32,
    script: &ScriptFile,
    suggest_fn: fn(&ScriptFile, &str, usize) -> Vec<String>,
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
                let suggestions = suggest_fn(script, buf, 8);
                if suggestions.is_empty() {
                    ui.weak("(none)");
                } else {
                    for s in &suggestions {
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
    // Cached galleys are only reused while egui has not invalidated its layouts (atlas reset,
    // new fonts / pixels-per-point); see `GalleyEpoch`.
    let generation = state.galley_epoch.sync(ui.ctx());
    let Some(line_idx) = state.active_line else {
        return false;
    };
    let Some(ev_snapshot) = state.cached_event.clone() else {
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

        let style_w = (ui.available_width() * 0.28).clamp(90.0, 190.0);
        let prev_style = state.style_buf.clone();
        egui::ComboBox::from_id_salt(("edit_style_combo", line_idx))
            .width(style_w)
            .selected_text(&state.style_buf)
            .show_ui(ui, |ui| {
                let mut any_style = false;
                for st in doc.script().styles() {
                    any_style = true;
                    let name = st.name();
                    if ui.selectable_label(state.style_buf == name, name).clicked() {
                        state.style_buf.clear();
                        state.style_buf.push_str(name);
                    }
                }
                if !any_style {
                    ui.selectable_value(&mut state.style_buf, "Default".to_owned(), "Default");
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

        // Stretch Actor (~190 px cap) and Effect (remaining width) so max line length sits flush
        // at the far right edge with no empty trailing gap.
        let len_box_w = 28.0;
        let rem_for_combos = (ui.available_width() - len_box_w - 50.0).max(80.0);
        let actor_w = (rem_for_combos * 0.48).clamp(40.0, 190.0);
        let effect_w = (rem_for_combos - actor_w).max(40.0);

        if editable_combo(
            ui,
            ("edit_actor", line_idx),
            &mut state.actor_buf,
            catalog.get("grid-col-actor").unwrap_or("Actor"),
            actor_w,
            doc.script(),
            autocomplete_actors,
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

        if editable_combo(
            ui,
            ("edit_effect", line_idx),
            &mut state.effect_buf,
            catalog.get("grid-col-effect").unwrap_or("Effect"),
            effect_w,
            doc.script(),
            autocomplete_effects,
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
        if state.last_max_len != Some(max_line_len) {
            state.max_len_buf.clear();
            let _ = write!(state.max_len_buf, "{max_line_len}");
            state.last_max_len = Some(max_line_len);
        }
        ui.add_enabled(
            false,
            TextEdit::singleline(&mut state.max_len_buf).desired_width(28.0),
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
            if paint_ab_colour_button(
                ui,
                state.colour_swatches[idx],
                &mut state.ab_galleys[idx],
                generation,
                palette,
                tip,
            ) {
                let pos = state.last_cursor_byte.min(state.text_buf.len());
                state.text_buf.insert_str(pos, tag);
                let _ = doc.amend_text(line_idx, "colour", &state.text_buf);
                changed = true;
            }
        }

        if paint_commit_check_button(ui, palette) {
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
    let use_monospace = state.use_monospace;
    let edit_font = if use_monospace {
        FontId::monospace(12.5)
    } else {
        FontId::proportional(13.0)
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
    let pal_sig = palette_syntax_sig(palette);
    let mut galley_cache = state.galley_cache.take();
    let mut span_scratch = std::mem::take(&mut state.span_scratch);
    let output = {
        let mut layouter = |ui: &Ui, text: &dyn egui::TextBuffer, wrap_width: f32| {
            let s = text.as_str();
            let wrap_bits = wrap_width.to_bits();
            if let Some(ref c) = galley_cache
                && c.generation == generation
                && c.wrap_width_bits == wrap_bits
                && c.use_monospace == use_monospace
                && c.pal_sig == pal_sig
                && c.text == s
            {
                return Arc::clone(&c.galley);
            }
            tokenize_into(s, &mut span_scratch);
            let job =
                build_highlighted_layout_job(s, &span_scratch, &pal_clone, &edit_font, wrap_width);
            let g = ui.fonts_mut(|f| f.layout_job(job));
            let cached_text = if let Some(mut old) = galley_cache.take() {
                old.text.clear();
                old.text.push_str(s);
                old.wrap_width_bits = wrap_bits;
                old.use_monospace = use_monospace;
                old.pal_sig = pal_sig;
                old.generation = generation;
                old.galley = Arc::clone(&g);
                old
            } else {
                CachedEditGalley {
                    text: s.to_owned(),
                    wrap_width_bits: wrap_bits,
                    use_monospace,
                    pal_sig,
                    generation,
                    galley: Arc::clone(&g),
                }
            };
            galley_cache = Some(cached_text);
            g
        };

        TextEdit::multiline(&mut state.text_buf)
            .id_salt(("edit_box", line_idx))
            .background_color(bg_fill)
            .min_size(Vec2::new(rem_w, rem_h))
            .desired_width(f32::INFINITY)
            .layouter(&mut layouter)
            .show(ui)
    };
    state.galley_cache = galley_cache;
    state.span_scratch = span_scratch;

    if state.request_focus {
        output.response.request_focus();
        state.request_focus = false;
    }

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
    use ass_core::ScriptFile;
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

    /// Regression: the edit box (text layouter and the four "AB" colour buttons) must never
    /// paint galleys cached across a font-atlas rebuild.
    #[test]
    fn edit_box_galleys_are_fresh_after_atlas_rebuild() {
        let mut script = ScriptFile::new_default();
        script.ensure_events_section_mut().insert_event(
            0,
            AssEvent::dialogue(AssTime::ZERO, AssTime::from_cs(250), r"{\an5}Hello\Nworld"),
        );
        let mut doc = DocumentController::new(script);
        let mut state = EditBoxState::default();
        let pal = theme::palette(ThemeSpec::Light);
        let cat = Catalog::for_locale(Locale::En);
        let ctx = egui::Context::default();
        ctx.set_fonts(ui_theme::build_font_definitions(
            &ui_theme::FontConfig::default(),
        ));
        let frame =
            |ctx: &egui::Context, doc: &mut DocumentController, state: &mut EditBoxState| {
                let mut out = ctx.run_ui(egui::RawInput::default(), |ui| {
                    let _ = show_edit_box(ui, doc, state, None, &pal, &cat);
                });
                out.textures_delta.clear();
                let galleys = ui_theme::galley_check::text_galleys(&out.shapes);
                assert!(
                    galleys.iter().any(|g| g.job.text == "AB"),
                    "AB buttons must be painted"
                );
                ui_theme::galley_check::count_stale(ctx, &out.shapes)
            };
        assert_eq!(frame(&ctx, &mut doc, &mut state), 0);
        assert_eq!(frame(&ctx, &mut doc, &mut state), 0);
        let _ = doc.amend_text(0, "test", "Tiếng Việt ắặẳẵ ứừửữự 日本語の字幕");
        state.sync_active_line(&doc, None);
        assert_eq!(frame(&ctx, &mut doc, &mut state), 0);
        ctx.set_pixels_per_point(1.5);
        assert_eq!(frame(&ctx, &mut doc, &mut state), 0);
        assert_eq!(frame(&ctx, &mut doc, &mut state), 0);
        // Rebuilt fonts (new atlas) invalidate every galley, including the cached AB buttons.
        ctx.set_fonts(egui::FontDefinitions::default());
        assert_eq!(frame(&ctx, &mut doc, &mut state), 0);
        assert_eq!(frame(&ctx, &mut doc, &mut state), 0);
    }

    #[test]
    fn edit_box_renders_with_default_fonts_and_reuses_cached_event() {
        let mut script = ScriptFile::new_default();
        script.ensure_events_section_mut().insert_event(
            0,
            AssEvent::dialogue(
                AssTime::ZERO,
                AssTime::from_cs(250),
                r"{\an5\blur2}Cached\Nevent",
            ),
        );
        let mut doc = DocumentController::new(script);
        let mut state = EditBoxState::default();
        let pal = theme::palette(ThemeSpec::Light);
        let cat = Catalog::for_locale(Locale::En);
        let lookups = std::rc::Rc::new(std::cell::Cell::new(0u64));
        let lookups_c = lookups.clone();

        let mut harness = Harness::builder()
            .with_accessibility_check(false)
            .build_ui(move |ui| {
                let _ = show_edit_box(ui, &mut doc, &mut state, None, &pal, &cat);
                lookups_c.set(state.event_lookups());
            });
        for _ in 0..5 {
            harness.step();
        }
        assert_eq!(
            lookups.get(),
            1,
            "edit-box active event should be looked up once across 5 unchanged frames"
        );
    }

    #[test]
    fn edit_box_event_cache_refreshes_on_amend_undo_and_line_change_only() {
        let mut script = ScriptFile::new_default();
        let sec = script.ensure_events_section_mut();
        sec.insert_event(
            0,
            AssEvent::dialogue(AssTime::ZERO, AssTime::from_cs(200), "Original 0"),
        );
        sec.insert_event(
            1,
            AssEvent::dialogue(AssTime::from_cs(200), AssTime::from_cs(400), "Original 1"),
        );
        let mut doc = DocumentController::new(script);
        let mut state = EditBoxState::default();

        // Initial load fetches event 0 once.
        state.sync_active_line(&doc, None);
        assert_eq!(state.text_buf, "Original 0");
        assert_eq!(state.event_cache_refreshes(), 1);
        assert_eq!(state.event_lookups(), 1);

        // Idle frames must not refresh or call get_event.
        for _ in 0..5 {
            state.sync_active_line(&doc, None);
        }
        assert_eq!(state.event_cache_refreshes(), 1);
        assert_eq!(state.event_lookups(), 1);

        // Same-line amend_text refreshes text_buf in O(1) without calling get_event.
        doc.amend_text(0, "typing", "Amended 0").expect("amend");
        state.sync_active_line(&doc, None);
        assert_eq!(state.text_buf, "Amended 0");
        assert_eq!(state.event_cache_refreshes(), 2);
        assert_eq!(state.event_lookups(), 1);

        // Undo refreshes cached event and restores original text.
        doc.undo().expect("undo");
        state.sync_active_line(&doc, None);
        assert_eq!(state.text_buf, "Original 0");
        assert_eq!(state.event_cache_refreshes(), 3);
        assert_eq!(state.event_lookups(), 2);

        // Active-line change refreshes cached event for line 1.
        doc.set_selection(Some(1), &[1]);
        state.sync_active_line(&doc, None);
        assert_eq!(state.text_buf, "Original 1");
        assert_eq!(state.event_cache_refreshes(), 4);
        assert_eq!(state.event_lookups(), 3);

        // Subsequent idle frames stay cached.
        state.sync_active_line(&doc, None);
        assert_eq!(state.event_cache_refreshes(), 4);
        assert_eq!(state.event_lookups(), 3);
    }

    #[test]
    fn parse_ass_colour_and_sync_active_style_swatches() {
        assert_eq!(
            parse_ass_colour_rgb("&H00FFE080"),
            Some(Color32::from_rgb(0x80, 0xE0, 0xFF))
        );
        assert_eq!(
            parse_ass_colour_rgb("&H000000FF&"),
            Some(Color32::from_rgb(0xFF, 0x00, 0x00))
        );
        let raw = "[V4+ Styles]
Format: Name, Fontname, Fontsize, PrimaryColour, SecondaryColour, OutlineColour, BackColour, Bold, Italic, Underline, StrikeOut, ScaleX, ScaleY, Spacing, Angle, BorderStyle, Outline, Shadow, Alignment, MarginL, MarginR, MarginV, Encoding
Style: Sign,Arial,42,&H00FFE080,&H000000FF,&H00102030,&H00000000,-1,0,0,0,100,100,0,0,1,2,0,8,10,10,10,1
[Events]
Format: Layer, Start, End, Style, Name, MarginL, MarginR, MarginV, Effect, Text
Dialogue: 0,0:00:00.00,0:00:02.00,Sign,,0,0,0,,Test
";
        let doc = DocumentController::new(ass_core::parse_ass(raw).value);
        let mut state = EditBoxState::default();
        state.sync_active_line(&doc, None);
        assert_eq!(
            state.colour_swatches[0],
            Color32::from_rgb(0x80, 0xE0, 0xFF)
        );
        assert_eq!(
            state.colour_swatches[1],
            Color32::from_rgb(0xFF, 0x00, 0x00)
        );
        assert_eq!(
            state.colour_swatches[2],
            Color32::from_rgb(0x30, 0x20, 0x10)
        );
        assert_eq!(
            state.colour_swatches[3],
            Color32::from_rgb(0x00, 0x00, 0x00)
        );
    }
}
