//! Karaoke Templater engine compatible with Aegisub 3.5.1's `kara-templater.lua` semantics,
//! powered by sandboxed vendored `LuaJIT` (`mlua`) and `karaskel` preprocessing.
//!
//! Reference: Aegisub `automation/autoload/kara-templater.lua` (classes `code`/`template`/`mixin`,
//! modifiers, inline `$variables`, inline `!expressions!`, `retime`/`relayer`/`restyle`/`maxloop`,
//! `fxgroup`, `"fx"` effect tagging, and regeneration).

#![forbid(unsafe_code)]
#![cfg_attr(not(test), deny(clippy::unwrap_used, clippy::expect_used))]

use std::cell::RefCell;
use std::collections::{HashMap, HashSet};
use std::fmt::Write as _;
use std::rc::Rc;
use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};
use std::sync::Arc;
use std::time::{Duration, Instant};

use ass_core::{AssEvent, AssTime, EventKind, ScriptFile};
use karaskel::{
    collect_head, preprocess_script_into, KaraskelOutput, KaraskelSyllable, PreprocessedLine,
    ScriptMeta, TextExtents,
};
use mlua::{Function, HookTriggers, Lua, Table, Value, VmState};
use template_doctor::{lint_script, Diagnostic, Severity};
use thiserror::Error;

/// Configuration & sandbox limits for [`KaraTemplater`].
#[derive(Debug, Clone)]
pub struct EngineConfig {
    /// Maximum `LuaJIT` heap memory in bytes (default: 64 MiB).
    pub memory_limit_bytes: usize,
    /// Maximum Lua VM instructions before aborting (default: 5,000,000).
    pub max_instructions: u64,
    /// Wall-clock execution timeout (default: 2 s).
    pub timeout: Duration,
    /// Run `template-doctor` before execution and abort on `Severity::Error` diagnostics.
    pub abort_on_lint_errors: bool,
    /// Generate furigana syllables during `karaskel` preprocessing.
    pub generate_furigana: bool,
}

impl Default for EngineConfig {
    fn default() -> Self {
        Self {
            memory_limit_bytes: 64 * 1024 * 1024,
            max_instructions: 5_000_000,
            timeout: Duration::from_secs(2),
            abort_on_lint_errors: false,
            generate_furigana: true,
        }
    }
}

/// Errors produced by [`KaraTemplater`].
#[derive(Debug, Error)]
pub enum KaraError {
    /// `template-doctor` found blocking errors before execution.
    #[error("template-doctor found {0} error(s) before execution")]
    LintErrors(usize, Vec<Diagnostic>),

    /// Error executing a `code` block or `!expr!` on a specific template line.
    #[error("template line {line_index}: {message}")]
    LuaError {
        /// 0-based event index of the offending template/code line.
        line_index: usize,
        /// Human-readable error message.
        message: String,
    },

    /// Sandbox initialization error.
    #[error("failed to initialize `LuaJIT` sandbox: {0}")]
    SandboxInit(String),
}

/// Result of running [`KaraTemplater::apply`].
#[derive(Debug, Clone)]
pub struct RunReport {
    /// Diagnostics returned by `template-doctor` pre-flight check.
    pub lint_diagnostics: Vec<Diagnostic>,
    /// Number of previously generated `fx` lines stripped before regeneration.
    pub stripped_fx_lines: usize,
    /// Number of new `fx` lines generated.
    pub generated_fx_lines: usize,
    /// Number of karaoke source lines processed.
    pub processed_karaoke_lines: usize,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum RuleKind {
    Code,
    Template,
    Mixin,
}

/// Extended template modifiers parsed from the ASS `Effect` field.
#[derive(Debug, Clone)]
#[allow(clippy::struct_excessive_bools)]
struct ParsedTemplate {
    line_index: usize,
    kind: RuleKind,
    style: String,
    layer: i32,
    text: String,
    once: bool,
    is_line: bool,
    is_pre_line: bool,
    is_syl: bool,
    is_char: bool,
    is_furi: bool,
    is_multi: bool,
    noblank: bool,
    notext: bool,
    keeptags: bool,
    repeat: bool,
    nokeepbreak: bool,
    all: bool,
    loops: usize,
    fx: Option<String>,
    fxgroup: Option<String>,
    compiled_segments: Vec<TemplateSegment>,
}

#[derive(Debug, Clone)]
enum TemplateSegment {
    Literal(String),
    Var(String),
    Expr { expr_index: usize, raw: String },
}

/// Mutable per-output-line state manipulated by `retime()`, `relayer()`, `restyle()`, `maxloop()`.
#[derive(Debug, Clone)]
struct LineControl {
    start_ms: i64,
    end_ms: i64,
    layer: i32,
    style: String,
    max_loops: usize,
    line_start_ms: i64,
    line_end_ms: i64,
    syl_start_ms: i64,
    syl_end_ms: i64,
}

/// Removes all previously generated `Effect == "fx"` lines and restores commented-out `karaoke`
/// source lines back to `Dialogue` (matching Aegisub 3.5.1 regeneration).
pub fn strip_fx_lines(script: &mut ScriptFile) -> usize {
    let Some(ev_sec) = script.events_section_mut() else {
        return 0;
    };
    let mut removed = 0usize;
    let mut i = ev_sec.event_count();
    while i > 0 {
        i -= 1;
        if let Some(ev) = ev_sec.get_event(i) {
            let eff = ev.effect().trim();
            if eff.eq_ignore_ascii_case("fx") {
                ev_sec.remove_event(i);
                removed += 1;
                continue;
            }
            if ev.kind() == EventKind::Comment && eff.eq_ignore_ascii_case("karaoke") {
                if let Some(ev_mut) = ev_sec.get_event_mut(i) {
                    ev_mut.set_kind(EventKind::Dialogue);
                }
            }
        }
    }
    removed
}

/// Evaluates `inline_var` into `out` without allocating (fast path used by tests & engine).
#[allow(clippy::too_many_arguments, clippy::too_many_lines)]
fn write_inline_var(
    var: &str,
    out: &mut String,
    line: &PreprocessedLine,
    syl: Option<&KaraskelSyllable>,
    ctl: &LineControl,
    j: usize,
    maxj: usize,
    ci: usize,
    cn: usize,
    char_text: Option<&str>,
    meta: &ScriptMeta,
    karaskel_out: &KaraskelOutput,
) -> bool {
    let s_start = syl.map_or(line.start_ms, |s| s.start_time);
    let s_end = syl.map_or(line.end_ms, |s| s.end_time);
    let s_dur = syl.map_or(line.duration_ms, |s| s.duration);
    let s_mid = s_start + s_dur / 2;
    let l_mid = line.start_ms + line.duration_ms / 2;

    match var {
        "start" => {
            let _ = write!(out, "{}", ctl.start_ms);
        }
        "end" => {
            let _ = write!(out, "{}", ctl.end_ms);
        }
        "dur" => {
            let _ = write!(out, "{}", ctl.end_ms - ctl.start_ms);
        }
        "mid" => {
            let _ = write!(out, "{}", i64::midpoint(ctl.start_ms, ctl.end_ms));
        }
        "lstart" => {
            let _ = write!(out, "{}", line.start_ms);
        }
        "lend" => {
            let _ = write!(out, "{}", line.end_ms);
        }
        "ldur" => {
            let _ = write!(out, "{}", line.duration_ms);
        }
        "lmid" => {
            let _ = write!(out, "{l_mid}");
        }
        "sstart" => {
            let _ = write!(out, "{s_start}");
        }
        "send" => {
            let _ = write!(out, "{s_end}");
        }
        "sdur" => {
            let _ = write!(out, "{s_dur}");
        }
        "smid" => {
            let _ = write!(out, "{s_mid}");
        }
        "kdur" | "skdur" => {
            let _ = write!(out, "{}", s_dur / 10);
        }
        "si" => {
            let _ = write!(out, "{}", syl.map_or(0, |s| s.i));
        }
        "syln" => {
            let _ = write!(out, "{}", karaskel_out.line_syllables(line).len());
        }
        "ci" => {
            let _ = write!(out, "{ci}");
        }
        "cn" => {
            let _ = write!(out, "{cn}");
        }
        "j" => {
            let _ = write!(out, "{j}");
        }
        "maxj" => {
            let _ = write!(out, "{maxj}");
        }
        "layer" => {
            let _ = write!(out, "{}", ctl.layer);
        }
        "style" => {
            out.push_str(&ctl.style);
        }
        "x" | "center" => {
            let v = syl.map_or(line.center, |s| line.left + s.center);
            format_num(out, v);
        }
        "y" | "middle" | "ly" | "lmiddle" => {
            format_num(out, line.middle);
        }
        "left" => {
            let v = syl.map_or(line.left, |s| line.left + s.left);
            format_num(out, v);
        }
        "right" => {
            let v = syl.map_or(line.right, |s| line.left + s.right);
            format_num(out, v);
        }
        "top" | "ltop" => {
            format_num(out, line.top);
        }
        "bottom" | "lbottom" => {
            format_num(out, line.bottom);
        }
        "width" => {
            let v = syl.map_or(line.width, |s| s.width);
            format_num(out, v);
        }
        "height" => {
            let v = syl.map_or(line.height, |s| s.height);
            format_num(out, v);
        }
        "lx" | "lcenter" => {
            format_num(out, line.center);
        }
        "lleft" => {
            format_num(out, line.left);
        }
        "lright" => {
            format_num(out, line.right);
        }
        "lwidth" => {
            format_num(out, line.width);
        }
        "lheight" => {
            format_num(out, line.height);
        }
        "resx" => {
            format_num(out, meta.res_x);
        }
        "resy" => {
            format_num(out, meta.res_y);
        }
        "text" => {
            if let Some(ch) = char_text {
                out.push_str(ch);
            } else if let Some(s) = syl {
                out.push_str(karaskel_out.syllable_text(s));
            } else {
                out.push_str(karaskel_out.line_text(line));
            }
        }
        _ => return false,
    }
    true
}

fn format_num(out: &mut String, v: f64) {
    let rounded = v.round();
    if (v - rounded).abs() < 1e-6 {
        #[allow(clippy::cast_possible_truncation)]
        let i = rounded as i64;
        let _ = write!(out, "{i}");
    } else {
        let _ = write!(out, "{v:.2}");
    }
}

/// Main Karaoke Templater execution engine.
pub struct KaraTemplater {
    config: EngineConfig,
}

impl KaraTemplater {
    /// Creates a new engine with `config`.
    #[must_use]
    pub const fn new(config: EngineConfig) -> Self {
        Self { config }
    }

    /// Runs `template-doctor`, strips old `"fx"` lines, executes all `code`/`template`/`mixin`
    /// lines inside a sandboxed `LuaJIT` VM, and inserts the generated `"fx"` lines into `script`.
    ///
    /// # Errors
    /// Returns [`KaraError::LintErrors`] when `abort_on_lint_errors` is enabled and `template-doctor`
    /// reports errors, or [`KaraError::LuaError`] / [`KaraError::SandboxInit`] on Lua/sandbox errors.
    #[allow(clippy::too_many_lines)]
    pub fn apply(
        &self,
        script: &mut ScriptFile,
        extents: &dyn TextExtents,
    ) -> Result<RunReport, KaraError> {
        let lint_diagnostics = lint_script(script);
        if self.config.abort_on_lint_errors {
            let err_count = lint_diagnostics
                .iter()
                .filter(|d| d.severity == Severity::Error)
                .count();
            if err_count > 0 {
                return Err(KaraError::LintErrors(err_count, lint_diagnostics));
            }
        }

        let stripped_fx_lines = strip_fx_lines(script);
        let meta = collect_head(script, None);

        // 1. Collect templates, code lines, and mixins.
        let mut parsed_templates: Vec<ParsedTemplate> = Vec::new();
        let mut expr_sources: Vec<(usize, String)> = Vec::new();
        if let Some(ev_sec) = script.events_section() {
            for (idx, ev) in ev_sec.events().enumerate() {
                if let Some(pt) = parse_template_line(idx, ev, &mut expr_sources) {
                    parsed_templates.push(pt);
                }
            }
        }

        // 2. Preprocess karaoke lines via `karaskel`.
        let mut karaskel_out = KaraskelOutput::default();
        preprocess_script_into(
            script,
            extents,
            self.config.generate_furigana,
            &mut karaskel_out,
        );

        // 3. Create sandboxed `LuaJIT` VM.
        let lua = create_sandbox(&self.config)?;
        let ctl_state = Rc::new(RefCell::new(LineControl {
            start_ms: 0,
            end_ms: 0,
            layer: 0,
            style: String::new(),
            max_loops: 1,
            line_start_ms: 0,
            line_end_ms: 0,
            syl_start_ms: 0,
            syl_end_ms: 0,
        }));
        install_tenv_helpers(&lua, &meta, &ctl_state)?;

        // 4. Pre-compile every inline `!expr!` once.
        let mut compiled_exprs: Vec<Function> = Vec::with_capacity(expr_sources.len());
        for (line_idx, expr) in &expr_sources {
            let normalized = expr
                .replace("$ci", "ci")
                .replace("$cn", "cn")
                .replace("$j", "j")
                .replace("$maxj", "maxj");
            let chunk_src = format!("return ({normalized})");
            let func = lua
                .load(&chunk_src)
                .into_function()
                .map_err(|e| KaraError::LuaError {
                    line_index: *line_idx,
                    message: format!("compile error in `!{expr}!`: {e}"),
                })?;
            compiled_exprs.push(func);
        }

        // Pre-compile `code` lines once.
        let mut compiled_codes: HashMap<usize, Function> = HashMap::new();
        for pt in &parsed_templates {
            if pt.kind == RuleKind::Code {
                let func = lua
                    .load(&pt.text)
                    .into_function()
                    .map_err(|e| KaraError::LuaError {
                        line_index: pt.line_index,
                        message: format!("compile error in code line: {e}"),
                    })?;
                compiled_codes.insert(pt.line_index, func);
            }
        }

        // 5. Execute `code once` lines.
        for pt in &parsed_templates {
            if pt.kind == RuleKind::Code && pt.once {
                if let Some(f) = compiled_codes.get(&pt.line_index) {
                    f.call::<()>(()).map_err(|e| KaraError::LuaError {
                        line_index: pt.line_index,
                        message: e.to_string(),
                    })?;
                }
            }
        }

        // 6. Execute templates for each karaoke line.
        let mut generated: Vec<AssEvent> = Vec::new();
        let mut processed_karaoke_lines = 0usize;
        let mut karaoke_source_indices: HashSet<usize> = HashSet::new();
        let mut out_buf = String::with_capacity(256);

        let lines_snapshot: Vec<PreprocessedLine> = karaskel_out.lines.clone();
        for line in &lines_snapshot {
            let Some(orig_ev) = script
                .events_section()
                .and_then(|s| s.get_event(line.line_index))
                .cloned()
            else {
                continue;
            };
            if orig_ev.kind() != EventKind::Dialogue {
                continue;
            }
            let eff_trim = orig_ev.effect().trim();
            let is_kara_line = eff_trim.is_empty() || eff_trim.eq_ignore_ascii_case("karaoke");
            if !is_kara_line {
                continue;
            }

            let mut matched_any = false;
            set_line_globals(&lua, line, &orig_ev, &karaskel_out)?;

            // Run matching `code line` blocks.
            for pt in &parsed_templates {
                if pt.kind == RuleKind::Code && pt.is_line && style_matches(pt, orig_ev.style()) {
                    if let Some(f) = compiled_codes.get(&pt.line_index) {
                        f.call::<()>(()).map_err(|e| KaraError::LuaError {
                            line_index: pt.line_index,
                            message: e.to_string(),
                        })?;
                    }
                }
            }

            // Collect mixins for this style.
            let mixins: Vec<&ParsedTemplate> = parsed_templates
                .iter()
                .filter(|m| m.kind == RuleKind::Mixin && style_matches(m, orig_ev.style()))
                .collect();

            // Run matching `template` lines.
            for pt in &parsed_templates {
                if pt.kind != RuleKind::Template || !style_matches(pt, orig_ev.style()) {
                    continue;
                }
                if !fxgroup_enabled(&lua, pt.fxgroup.as_deref()) {
                    continue;
                }

                if pt.is_line || pt.is_pre_line {
                    matched_any = true;
                    run_template_unit(
                        &lua,
                        pt,
                        &mixins,
                        &compiled_exprs,
                        &ctl_state,
                        line,
                        &orig_ev,
                        None,
                        0,
                        0,
                        None,
                        &meta,
                        &karaskel_out,
                        &mut out_buf,
                        &mut generated,
                    )?;
                } else if pt.is_syl || pt.is_char || pt.is_furi {
                    let syls: Vec<KaraskelSyllable> = karaskel_out.line_syllables(line).to_vec();
                    for syl in &syls {
                        let syl_text = karaskel_out.syllable_text(syl);
                        if pt.noblank && syl_text.trim().is_empty() {
                            continue;
                        }
                        if let Some(ref req_fx) = pt.fx {
                            if karaskel_out.syllable_fx(syl) != req_fx {
                                continue;
                            }
                        }
                        set_syl_globals(&lua, line, syl, syl_text)?;

                        // Run `code syl` blocks.
                        for cp in &parsed_templates {
                            if cp.kind == RuleKind::Code
                                && cp.is_syl
                                && style_matches(cp, orig_ev.style())
                            {
                                if let Some(f) = compiled_codes.get(&cp.line_index) {
                                    f.call::<()>(()).map_err(|e| KaraError::LuaError {
                                        line_index: cp.line_index,
                                        message: e.to_string(),
                                    })?;
                                }
                            }
                        }

                        if pt.is_char {
                            let chars: Vec<char> = syl_text.chars().collect();
                            let cn = chars.len();
                            for (cidx, ch) in chars.iter().enumerate() {
                                let mut ch_buf = [0u8; 4];
                                let ch_str = ch.encode_utf8(&mut ch_buf);
                                if pt.noblank && ch_str.trim().is_empty() {
                                    continue;
                                }
                                matched_any = true;
                                run_template_unit(
                                    &lua,
                                    pt,
                                    &mixins,
                                    &compiled_exprs,
                                    &ctl_state,
                                    line,
                                    &orig_ev,
                                    Some(syl),
                                    cidx + 1,
                                    cn,
                                    Some(ch_str),
                                    &meta,
                                    &karaskel_out,
                                    &mut out_buf,
                                    &mut generated,
                                )?;
                            }
                        } else if pt.is_furi {
                            let furis = karaskel_out.syllable_furigana(syl);
                            for furi in furis {
                                let ft = karaskel_out.furigana_text(furi);
                                if pt.noblank && ft.trim().is_empty() {
                                    continue;
                                }
                                matched_any = true;
                                run_template_unit(
                                    &lua,
                                    pt,
                                    &mixins,
                                    &compiled_exprs,
                                    &ctl_state,
                                    line,
                                    &orig_ev,
                                    Some(syl),
                                    0,
                                    0,
                                    Some(ft),
                                    &meta,
                                    &karaskel_out,
                                    &mut out_buf,
                                    &mut generated,
                                )?;
                            }
                        } else {
                            matched_any = true;
                            run_template_unit(
                                &lua,
                                pt,
                                &mixins,
                                &compiled_exprs,
                                &ctl_state,
                                line,
                                &orig_ev,
                                Some(syl),
                                0,
                                0,
                                None,
                                &meta,
                                &karaskel_out,
                                &mut out_buf,
                                &mut generated,
                            )?;
                        }
                    }
                }
            }

            if matched_any {
                processed_karaoke_lines += 1;
                karaoke_source_indices.insert(line.line_index);
            }
        }

        // 7. Comment out processed karaoke lines & append generated `"fx"` lines.
        let generated_fx_lines = generated.len();
        if let Some(ev_sec) = script.events_section_mut() {
            for idx in karaoke_source_indices {
                if let Some(ev_mut) = ev_sec.get_event_mut(idx) {
                    ev_mut.set_kind(EventKind::Comment);
                    ev_mut.set_effect("karaoke");
                }
            }
            for fx_ev in generated {
                let end = ev_sec.event_count();
                ev_sec.insert_event(end, fx_ev);
            }
        }

        Ok(RunReport {
            lint_diagnostics,
            stripped_fx_lines,
            generated_fx_lines,
            processed_karaoke_lines,
        })
    }
}

fn style_matches(pt: &ParsedTemplate, event_style: &str) -> bool {
    pt.all || pt.style == event_style
}

fn fxgroup_enabled(lua: &Lua, group: Option<&str>) -> bool {
    let Some(g) = group else {
        return true;
    };
    let Ok(tbl) = lua.globals().get::<Table>("fxgroup") else {
        return true;
    };
    match tbl.get::<Value>(g) {
        Ok(Value::Boolean(b)) => b,
        _ => true,
    }
}

#[allow(clippy::too_many_arguments, clippy::too_many_lines)]
fn run_template_unit(
    lua: &Lua,
    pt: &ParsedTemplate,
    mixins: &[&ParsedTemplate],
    compiled_exprs: &[Function],
    ctl_state: &Rc<RefCell<LineControl>>,
    line: &PreprocessedLine,
    orig_ev: &AssEvent,
    syl: Option<&KaraskelSyllable>,
    ci: usize,
    cn: usize,
    char_text: Option<&str>,
    meta: &ScriptMeta,
    karaskel_out: &KaraskelOutput,
    out_buf: &mut String,
    generated: &mut Vec<AssEvent>,
) -> Result<(), KaraError> {
    let mut j = 1usize;
    let mut maxj = pt.loops.max(1);
    while j <= maxj {
        {
            let mut ctl = ctl_state.borrow_mut();
            ctl.start_ms = line.start_ms;
            ctl.end_ms = line.end_ms;
            ctl.layer = pt.layer;
            orig_ev.style().clone_into(&mut ctl.style);
            ctl.max_loops = maxj;
            ctl.line_start_ms = line.start_ms;
            ctl.line_end_ms = line.end_ms;
            ctl.syl_start_ms = syl.map_or(line.start_ms, |s| s.start_time);
            ctl.syl_end_ms = syl.map_or(line.end_ms, |s| s.end_time);
        }

        let globals = lua.globals();
        let _ = globals.set("j", j);
        let _ = globals.set("maxj", maxj);
        let _ = globals.set("ci", ci);
        let _ = globals.set("cn", cn);

        out_buf.clear();
        evaluate_segments(
            pt,
            compiled_exprs,
            ctl_state,
            line,
            syl,
            j,
            maxj,
            ci,
            cn,
            char_text,
            meta,
            karaskel_out,
            out_buf,
        )?;

        // Apply matching mixins (prepend their evaluated tags).
        for m in mixins {
            if !fxgroup_enabled(lua, m.fxgroup.as_deref()) {
                continue;
            }
            let applies = (m.is_line && pt.is_line)
                || (m.is_syl && (pt.is_syl || pt.is_char))
                || (m.is_furi && pt.is_furi)
                || (!m.is_line && !m.is_syl && !m.is_furi);
            if applies {
                let mut m_buf = String::new();
                evaluate_segments(
                    m,
                    compiled_exprs,
                    ctl_state,
                    line,
                    syl,
                    j,
                    maxj,
                    ci,
                    cn,
                    char_text,
                    meta,
                    karaskel_out,
                    &mut m_buf,
                )?;
                out_buf.push_str(&m_buf);
            }
        }

        // Append syllable / line / char text unless `notext` is set.
        if !pt.notext {
            if let Some(ch) = char_text {
                out_buf.push_str(ch);
            } else if let Some(s) = syl {
                out_buf.push_str(karaskel_out.syllable_text(s));
            } else if pt.keeptags {
                out_buf.push_str(orig_ev.text());
            } else {
                out_buf.push_str(karaskel_out.line_text(line));
            }
        }

        let ctl = ctl_state.borrow().clone();
        maxj = ctl.max_loops.max(1);

        let start_cs = u32::try_from((ctl.start_ms.max(0) + 5) / 10).unwrap_or(0);
        let end_cs = u32::try_from((ctl.end_ms.max(0) + 5) / 10).unwrap_or(start_cs);
        let mut ev = AssEvent::dialogue(
            AssTime::from_cs(start_cs),
            AssTime::from_cs(end_cs),
            out_buf.as_str(),
        );
        ev.set_layer(ctl.layer);
        ev.set_style(&ctl.style);
        ev.set_effect("fx");
        generated.push(ev);

        j += 1;
    }
    Ok(())
}

#[allow(clippy::too_many_arguments)]
fn evaluate_segments(
    pt: &ParsedTemplate,
    compiled_exprs: &[Function],
    ctl_state: &Rc<RefCell<LineControl>>,
    line: &PreprocessedLine,
    syl: Option<&KaraskelSyllable>,
    j: usize,
    maxj: usize,
    ci: usize,
    cn: usize,
    char_text: Option<&str>,
    meta: &ScriptMeta,
    karaskel_out: &KaraskelOutput,
    out_buf: &mut String,
) -> Result<(), KaraError> {
    for seg in &pt.compiled_segments {
        match seg {
            TemplateSegment::Literal(lit) => out_buf.push_str(lit),
            TemplateSegment::Var(v) => {
                let ctl = ctl_state.borrow();
                if !write_inline_var(
                    v,
                    out_buf,
                    line,
                    syl,
                    &ctl,
                    j,
                    maxj,
                    ci,
                    cn,
                    char_text,
                    meta,
                    karaskel_out,
                ) {
                    out_buf.push('$');
                    out_buf.push_str(v);
                }
            }
            TemplateSegment::Expr { expr_index, raw } => {
                if let Some(func) = compiled_exprs.get(*expr_index) {
                    let val: Value = func.call(()).map_err(|e| KaraError::LuaError {
                        line_index: pt.line_index,
                        message: format!("runtime error in `!{raw}!`: {e}"),
                    })?;
                    match val {
                        Value::Boolean(b) => {
                            let _ = write!(out_buf, "{b}");
                        }
                        Value::Integer(n) => {
                            let _ = write!(out_buf, "{n}");
                        }
                        Value::Number(n) => format_num(out_buf, n),
                        Value::String(s) => {
                            if let Ok(st) = s.to_str() {
                                out_buf.push_str(&st);
                            }
                        }
                        _ => {}
                    }
                }
            }
        }
    }
    Ok(())
}

fn create_sandbox(config: &EngineConfig) -> Result<Lua, KaraError> {
    let lua = Lua::new();
    let _ = lua.load("if jit then jit.off() end").exec();
    lua.set_memory_limit(config.memory_limit_bytes)
        .map_err(|e| KaraError::SandboxInit(e.to_string()))?;

    let globals = lua.globals();
    for forbidden in ["io", "os", "debug", "dofile", "loadfile"] {
        let _ = globals.set(forbidden, Value::Nil);
    }
    if let Ok(pkg) = globals.get::<Table>("package") {
        let _ = pkg.set("loadlib", Value::Nil);
    }

    let inst_counter = Arc::new(AtomicU64::new(0));
    let timed_out = Arc::new(AtomicBool::new(false));
    let start_time = Instant::now();
    let max_inst = config.max_instructions;
    let timeout = config.timeout;

    lua.set_hook(
        HookTriggers::new().every_nth_instruction(10_000),
        move |_lua, _debug| {
            let count = inst_counter.fetch_add(10_000, Ordering::Relaxed) + 10_000;
            if count > max_inst || start_time.elapsed() > timeout {
                timed_out.store(true, Ordering::Relaxed);
                return Err(mlua::Error::RuntimeError(
                    "`LuaJIT` sandbox limit exceeded (timeout or instruction budget)".to_owned(),
                ));
            }
            Ok(VmState::Continue)
        },
    );

    Ok(lua)
}

#[allow(clippy::too_many_lines, clippy::similar_names)]
fn install_tenv_helpers(
    lua: &Lua,
    meta: &ScriptMeta,
    ctl_state: &Rc<RefCell<LineControl>>,
) -> Result<(), KaraError> {
    let globals = lua.globals();
    let meta_tbl = lua
        .create_table()
        .map_err(|e| KaraError::SandboxInit(e.to_string()))?;
    let _ = meta_tbl.set("res_x", meta.res_x);
    let _ = meta_tbl.set("res_y", meta.res_y);
    let _ = meta_tbl.set("video_x_correct_factor", meta.video_x_correct_factor);
    let _ = globals.set("meta", meta_tbl);

    let fxgroup_tbl = lua
        .create_table()
        .map_err(|e| KaraError::SandboxInit(e.to_string()))?;
    let _ = globals.set("fxgroup", fxgroup_tbl);

    // `retime(mode, start_adj, end_adj)`
    let ctl_retime = Rc::clone(ctl_state);
    let retime_fn = lua
        .create_function(
            move |_, (mode, s_adj, e_adj): (String, Option<f64>, Option<f64>)| {
                let mut ctl = ctl_retime.borrow_mut();
                #[allow(clippy::cast_possible_truncation)]
                let sa = s_adj.unwrap_or(0.0).round() as i64;
                #[allow(clippy::cast_possible_truncation)]
                let ea = e_adj.unwrap_or(0.0).round() as i64;
                match mode.as_str() {
                    "abs" | "set" => {
                        ctl.start_ms = sa;
                        ctl.end_ms = ea;
                    }
                    "preline" => {
                        ctl.start_ms = ctl.line_start_ms + sa;
                        ctl.end_ms = ctl.line_start_ms + ea;
                    }
                    "line" => {
                        ctl.start_ms = ctl.line_start_ms + sa;
                        ctl.end_ms = ctl.line_end_ms + ea;
                    }
                    "start2syl" => {
                        ctl.start_ms = ctl.line_start_ms + sa;
                        ctl.end_ms = ctl.line_start_ms + ctl.syl_start_ms + ea;
                    }
                    "presyl" => {
                        let base = ctl.line_start_ms + ctl.syl_start_ms;
                        ctl.start_ms = base + sa;
                        ctl.end_ms = base + ea;
                    }
                    "syl" => {
                        ctl.start_ms = ctl.line_start_ms + ctl.syl_start_ms + sa;
                        ctl.end_ms = ctl.line_start_ms + ctl.syl_end_ms + ea;
                    }
                    "postsyl" => {
                        let base = ctl.line_start_ms + ctl.syl_end_ms;
                        ctl.start_ms = base + sa;
                        ctl.end_ms = base + ea;
                    }
                    "syl2end" => {
                        ctl.start_ms = ctl.line_start_ms + ctl.syl_end_ms + sa;
                        ctl.end_ms = ctl.line_end_ms + ea;
                    }
                    "postline" => {
                        ctl.start_ms = ctl.line_end_ms + sa;
                        ctl.end_ms = ctl.line_end_ms + ea;
                    }
                    "sylpct" => {
                        #[allow(clippy::cast_precision_loss)]
                        let sdur = (ctl.syl_end_ms - ctl.syl_start_ms) as f64;
                        #[allow(clippy::cast_possible_truncation)]
                        let p0 = (sdur * s_adj.unwrap_or(0.0) / 100.0).round() as i64;
                        #[allow(clippy::cast_possible_truncation)]
                        let p1 = (sdur * e_adj.unwrap_or(100.0) / 100.0).round() as i64;
                        ctl.start_ms = ctl.line_start_ms + ctl.syl_start_ms + p0;
                        ctl.end_ms = ctl.line_start_ms + ctl.syl_start_ms + p1;
                    }
                    _ => {}
                }
                Ok("")
            },
        )
        .map_err(|e| KaraError::SandboxInit(e.to_string()))?;
    let _ = globals.set("retime", retime_fn);

    // `relayer(layer)`
    let ctl_relayer = Rc::clone(ctl_state);
    let relayer_fn = lua
        .create_function(move |_, layer: i32| {
            ctl_relayer.borrow_mut().layer = layer;
            Ok("")
        })
        .map_err(|e| KaraError::SandboxInit(e.to_string()))?;
    let _ = globals.set("relayer", relayer_fn);

    // `restyle(style)`
    let ctl_restyle = Rc::clone(ctl_state);
    let restyle_fn = lua
        .create_function(move |_, style: String| {
            ctl_restyle.borrow_mut().style = style;
            Ok("")
        })
        .map_err(|e| KaraError::SandboxInit(e.to_string()))?;
    let _ = globals.set("restyle", restyle_fn);

    // `maxloop(n)`
    let ctl_maxloop = Rc::clone(ctl_state);
    let maxloop_fn = lua
        .create_function(move |_, n: usize| {
            ctl_maxloop.borrow_mut().max_loops = n.max(1);
            Ok("")
        })
        .map_err(|e| KaraError::SandboxInit(e.to_string()))?;
    let _ = globals.set("maxloop", maxloop_fn);

    // Install LuaJIT-compatible `bit` library when running under vendored PUC Lua 5.1.
    if globals.get::<Table>("bit").is_err() {
        let bit_tbl = lua
            .create_table()
            .map_err(|e| KaraError::SandboxInit(e.to_string()))?;
        let tobit = lua
            .create_function(|_, x: f64| {
                #[allow(clippy::cast_possible_truncation)]
                Ok(x as i32)
            })
            .map_err(|e| KaraError::SandboxInit(e.to_string()))?;
        let bnot = lua
            .create_function(|_, x: i32| Ok(!x))
            .map_err(|e| KaraError::SandboxInit(e.to_string()))?;
        let band = lua
            .create_function(|_, args: mlua::Variadic<i32>| {
                Ok(args.into_iter().fold(!0i32, |acc, v| acc & v))
            })
            .map_err(|e| KaraError::SandboxInit(e.to_string()))?;
        let bor = lua
            .create_function(|_, args: mlua::Variadic<i32>| {
                Ok(args.into_iter().fold(0i32, |acc, v| acc | v))
            })
            .map_err(|e| KaraError::SandboxInit(e.to_string()))?;
        let bxor = lua
            .create_function(|_, args: mlua::Variadic<i32>| {
                Ok(args.into_iter().fold(0i32, |acc, v| acc ^ v))
            })
            .map_err(|e| KaraError::SandboxInit(e.to_string()))?;
        let lshift = lua
            .create_function(|_, (x, n): (i32, u32)| Ok(x.wrapping_shl(n & 31)))
            .map_err(|e| KaraError::SandboxInit(e.to_string()))?;
        let rshift = lua
            .create_function(|_, (x, n): (i32, u32)| {
                #[allow(clippy::cast_sign_loss, clippy::cast_possible_wrap)]
                Ok(((x as u32).wrapping_shr(n & 31)) as i32)
            })
            .map_err(|e| KaraError::SandboxInit(e.to_string()))?;
        let arshift = lua
            .create_function(|_, (x, n): (i32, u32)| Ok(x.wrapping_shr(n & 31)))
            .map_err(|e| KaraError::SandboxInit(e.to_string()))?;
        let tohex = lua
            .create_function(|_, (x, n): (i32, Option<i32>)| {
                #[allow(clippy::cast_sign_loss)]
                let u = x as u32;
                let width = n.unwrap_or(8);
                let upper = width < 0;
                let digits = usize::try_from(width.unsigned_abs())
                    .unwrap_or(8)
                    .clamp(1, 8);
                let full = if upper {
                    format!("{u:08X}")
                } else {
                    format!("{u:08x}")
                };
                Ok(full[8 - digits..].to_owned())
            })
            .map_err(|e| KaraError::SandboxInit(e.to_string()))?;
        let _ = bit_tbl.set("tobit", tobit);
        let _ = bit_tbl.set("bnot", bnot);
        let _ = bit_tbl.set("band", band);
        let _ = bit_tbl.set("bor", bor);
        let _ = bit_tbl.set("bxor", bxor);
        let _ = bit_tbl.set("lshift", lshift);
        let _ = bit_tbl.set("rshift", rshift);
        let _ = bit_tbl.set("arshift", arshift);
        let _ = bit_tbl.set("tohex", tohex);
        let _ = globals.set("bit", bit_tbl);
    }

    Ok(())
}

fn set_line_globals(
    lua: &Lua,
    line: &PreprocessedLine,
    orig_ev: &AssEvent,
    karaskel_out: &KaraskelOutput,
) -> Result<(), KaraError> {
    let tbl = lua
        .create_table()
        .map_err(|e| KaraError::SandboxInit(e.to_string()))?;
    let _ = tbl.set("start_time", line.start_ms);
    let _ = tbl.set("end_time", line.end_ms);
    let _ = tbl.set("duration", line.duration_ms);
    let _ = tbl.set("left", line.left);
    let _ = tbl.set("center", line.center);
    let _ = tbl.set("right", line.right);
    let _ = tbl.set("top", line.top);
    let _ = tbl.set("middle", line.middle);
    let _ = tbl.set("bottom", line.bottom);
    let _ = tbl.set("width", line.width);
    let _ = tbl.set("height", line.height);
    let _ = tbl.set("style", orig_ev.style());
    let _ = tbl.set("text_stripped", karaskel_out.line_text(line));
    let _ = tbl.set("layer", orig_ev.layer());
    let globals = lua.globals();
    let _ = globals.set("line", tbl.clone());
    let _ = globals.set("orgline", tbl);
    Ok(())
}

fn set_syl_globals(
    lua: &Lua,
    line: &PreprocessedLine,
    syl: &KaraskelSyllable,
    syl_text: &str,
) -> Result<(), KaraError> {
    let tbl = lua
        .create_table()
        .map_err(|e| KaraError::SandboxInit(e.to_string()))?;
    let _ = tbl.set("i", syl.i);
    let _ = tbl.set("start_time", syl.start_time);
    let _ = tbl.set("end_time", syl.end_time);
    let _ = tbl.set("duration", syl.duration);
    let _ = tbl.set("left", syl.left);
    let _ = tbl.set("center", syl.center);
    let _ = tbl.set("right", syl.right);
    let _ = tbl.set("width", syl.width);
    let _ = tbl.set("height", syl.height);
    let _ = tbl.set("x", line.left + syl.center);
    let _ = tbl.set("y", line.middle);
    let _ = tbl.set("text_stripped", syl_text);
    let globals = lua.globals();
    let _ = globals.set("syl", tbl.clone());
    let _ = globals.set("basesyl", tbl);
    Ok(())
}

fn parse_template_line(
    line_index: usize,
    ev: &AssEvent,
    expr_sources: &mut Vec<(usize, String)>,
) -> Option<ParsedTemplate> {
    if ev.kind() != EventKind::Comment {
        return None;
    }
    let first = ev.effect().split_whitespace().next()?;
    let kind = if first.eq_ignore_ascii_case("code") {
        RuleKind::Code
    } else if first.eq_ignore_ascii_case("template") {
        RuleKind::Template
    } else if first.eq_ignore_ascii_case("mixin") {
        RuleKind::Mixin
    } else {
        return None;
    };
    let mut pt = ParsedTemplate {
        line_index,
        kind,
        style: ev.style().to_owned(),
        layer: ev.layer(),
        text: ev.text().to_owned(),
        once: false,
        is_line: false,
        is_pre_line: false,
        is_syl: false,
        is_char: false,
        is_furi: false,
        is_multi: false,
        noblank: false,
        notext: false,
        keeptags: false,
        repeat: false,
        nokeepbreak: false,
        all: false,
        loops: 1,
        fx: None,
        fxgroup: None,
        compiled_segments: Vec::new(),
    };

    let tokens: Vec<&str> = ev.effect().split_whitespace().collect();
    let mut i = 1usize;
    while i < tokens.len() {
        match tokens[i].to_ascii_lowercase().as_str() {
            "once" => pt.once = true,
            "line" => pt.is_line = true,
            "pre-line" => pt.is_pre_line = true,
            "syl" => pt.is_syl = true,
            "char" => pt.is_char = true,
            "furi" => pt.is_furi = true,
            "multi" => pt.is_multi = true,
            "noblank" => pt.noblank = true,
            "notext" => pt.notext = true,
            "keeptags" => pt.keeptags = true,
            "repeat" => pt.repeat = true,
            "nokeepbreak" => pt.nokeepbreak = true,
            "all" => pt.all = true,
            "loop" => {
                if let Some(next) = tokens.get(i + 1) {
                    if let Ok(n) = next.parse::<usize>() {
                        pt.loops = n.max(1);
                        i += 1;
                    }
                }
            }
            "fx" => {
                if let Some(next) = tokens.get(i + 1) {
                    pt.fx = Some((*next).to_owned());
                    i += 1;
                }
            }
            "fxgroup" => {
                if let Some(next) = tokens.get(i + 1) {
                    pt.fxgroup = Some((*next).to_owned());
                    i += 1;
                }
            }
            _ => {}
        }
        i += 1;
    }

    if pt.kind == RuleKind::Template
        && !pt.is_line
        && !pt.is_pre_line
        && !pt.is_syl
        && !pt.is_char
        && !pt.is_furi
    {
        pt.is_syl = true;
    }

    if pt.kind != RuleKind::Code {
        pt.compiled_segments = parse_template_segments(ev.text(), line_index, expr_sources);
    }

    Some(pt)
}

fn parse_template_segments(
    text: &str,
    line_index: usize,
    expr_sources: &mut Vec<(usize, String)>,
) -> Vec<TemplateSegment> {
    let mut segs = Vec::new();
    let bytes = text.as_bytes();
    let mut lit_start = 0usize;
    let mut i = 0usize;

    while i < bytes.len() {
        if bytes[i] == b'!' {
            if let Some(rel_end) = text[i + 1..].find('!') {
                if i > lit_start {
                    segs.push(TemplateSegment::Literal(text[lit_start..i].to_owned()));
                }
                let raw = text[i + 1..i + 1 + rel_end].to_owned();
                let expr_index = expr_sources.len();
                expr_sources.push((line_index, raw.clone()));
                segs.push(TemplateSegment::Expr { expr_index, raw });
                i = i + 2 + rel_end;
                lit_start = i;
                continue;
            }
        } else if bytes[i] == b'$' {
            let mut end = i + 1;
            while end < bytes.len() && (bytes[end].is_ascii_alphanumeric() || bytes[end] == b'_') {
                end += 1;
            }
            if end > i + 1 {
                if i > lit_start {
                    segs.push(TemplateSegment::Literal(text[lit_start..i].to_owned()));
                }
                segs.push(TemplateSegment::Var(text[i + 1..end].to_owned()));
                i = end;
                lit_start = i;
                continue;
            }
        }
        i += 1;
    }

    if lit_start < bytes.len() {
        segs.push(TemplateSegment::Literal(text[lit_start..].to_owned()));
    }
    segs
}

#[cfg(test)]
mod tests {
    use super::*;
    use ass_core::{parse_ass, AssStyle};
    use karaskel::TextMetric;

    struct MonoExtents;
    impl TextExtents for MonoExtents {
        fn extents(&self, _style: &AssStyle, text: &str) -> TextMetric {
            #[allow(clippy::cast_precision_loss)]
            let width = (text.chars().count() as f64) * 20.0;
            TextMetric {
                width,
                height: 40.0,
                descent: 8.0,
                ext_lead: 0.0,
            }
        }
    }

    fn run_ass(ass: &str) -> (ScriptFile, RunReport) {
        let mut script = parse_ass(ass).value;
        let engine = KaraTemplater::new(EngineConfig::default());
        let report = engine
            .apply(&mut script, &MonoExtents)
            .expect("kara-templater apply");
        (script, report)
    }

    fn fx_lines(script: &ScriptFile) -> Vec<&AssEvent> {
        script
            .events()
            .filter(|e| e.kind() == EventKind::Dialogue && e.effect() == "fx")
            .collect()
    }

    fn base_header() -> &'static str {
        "[Script Info]\nScriptType: v4.00+\nPlayResX: 1920\nPlayResY: 1080\n\n\
         [V4+ Styles]\nFormat: Name, Fontname, Fontsize, PrimaryColour, SecondaryColour, OutlineColour, BackColour, Bold, Italic, Underline, StrikeOut, ScaleX, ScaleY, Spacing, Angle, BorderStyle, Outline, Shadow, Alignment, MarginL, MarginR, MarginV, Encoding\n\
         Style: Default,Arial,40,&H00FFFFFF,&H000000FF,&H00000000,&H00000000,0,0,0,0,100,100,0,0,1,2,2,2,10,10,10,1\n\
         Style: Romaji,Arial,40,&H00FFFFFF,&H000000FF,&H00000000,&H00000000,0,0,0,0,100,100,0,0,1,2,2,2,10,10,10,1\n\n\
         [Events]\nFormat: Layer, Start, End, Style, Name, MarginL, MarginR, MarginV, Effect, Text\n"
    }

    #[test]
    #[allow(clippy::similar_names, clippy::too_many_lines)]
    fn corpus_of_16_classic_karaoke_templates() {
        // 1. Classic syl position + inline vars
        let ass1 = format!(
            "{}Comment: 0,0:00:00.00,0:00:00.00,Default,,0,0,0,template syl,{{\\an5\\pos($x,$y)\\t($sstart,$send,\\fscx120)}}\n\
             Dialogue: 0,0:00:01.00,0:00:03.00,Default,,0,0,0,karaoke,{{\\k50}}ka{{\\k50}}ra\n",
            base_header()
        );
        let (s1, r1) = run_ass(&ass1);
        assert_eq!(r1.generated_fx_lines, 2);
        assert!(fx_lines(&s1)[0].text().contains("\\t(0,500,\\fscx120)}ka"));
        assert!(fx_lines(&s1)[1]
            .text()
            .contains("\\t(500,1000,\\fscx120)}ra"));

        // 2. retime("syl", 0, 0) + fade
        let ass2 = format!(
            "{}Comment: 0,0:00:00.00,0:00:00.00,Default,,0,0,0,template syl,!retime(\"syl\",0,0)!{{\\fad(50,50)}}\n\
             Dialogue: 0,0:00:02.00,0:00:04.00,Default,,0,0,0,karaoke,{{\\k60}}A{{\\k40}}B\n",
            base_header()
        );
        let (s2, _) = run_ass(&ass2);
        let f2 = fx_lines(&s2);
        assert_eq!(f2[0].start().as_cs(), 200);
        assert_eq!(f2[0].end().as_cs(), 260);
        assert_eq!(f2[1].start().as_cs(), 260);
        assert_eq!(f2[1].end().as_cs(), 300);

        // 3. retime("preline", -300, 0) + retime("postline", 0, 300)
        let ass3 = format!(
            "{}Comment: 0,0:00:00.00,0:00:00.00,Default,,0,0,0,template line,!retime(\"preline\",-300,0)!{{\\fad(300,0)}}\n\
             Comment: 0,0:00:00.00,0:00:00.00,Default,,0,0,0,template line,!retime(\"postline\",0,300)!{{\\fad(0,300)}}\n\
             Dialogue: 0,0:00:05.00,0:00:08.00,Default,,0,0,0,karaoke,{{\\k100}}Hello\n",
            base_header()
        );
        let (s3, r3) = run_ass(&ass3);
        assert_eq!(r3.generated_fx_lines, 2);
        assert_eq!(fx_lines(&s3)[0].start().as_cs(), 470);
        assert_eq!(fx_lines(&s3)[0].end().as_cs(), 500);
        assert_eq!(fx_lines(&s3)[1].start().as_cs(), 800);
        assert_eq!(fx_lines(&s3)[1].end().as_cs(), 830);

        // 4. char-by-char wave with $ci / $cn
        let ass4 = format!(
            "{}Comment: 0,0:00:00.00,0:00:00.00,Default,,0,0,0,template char,{{\\bord!$ci*2!}}\n\
             Dialogue: 0,0:00:01.00,0:00:02.00,Default,,0,0,0,karaoke,{{\\k50}}abc\n",
            base_header()
        );
        let (s4, r4) = run_ass(&ass4);
        assert_eq!(r4.generated_fx_lines, 3);
        assert_eq!(fx_lines(&s4)[2].text(), "{\\bord6}c");

        // 5. loop 4 particles with !j! and !maxj!
        let ass5 = format!(
            "{}Comment: 0,0:00:00.00,0:00:00.00,Default,,0,0,0,template syl loop 4,{{\\blur!j!}}\n\
             Dialogue: 0,0:00:01.00,0:00:02.00,Default,,0,0,0,karaoke,{{\\k50}}hi\n",
            base_header()
        );
        let (s5, r5) = run_ass(&ass5);
        assert_eq!(r5.generated_fx_lines, 4);
        assert_eq!(fx_lines(&s5)[3].text(), "{\\blur4}hi");

        // 6. maxloop(3) dynamic loop count
        let ass6 = format!(
            "{}Comment: 0,0:00:00.00,0:00:00.00,Default,,0,0,0,template syl,!maxloop(3)!{{\\frz!j*10!}}\n\
             Dialogue: 0,0:00:01.00,0:00:02.00,Default,,0,0,0,karaoke,{{\\k50}}yo\n",
            base_header()
        );
        let (_, r6) = run_ass(&ass6);
        assert_eq!(r6.generated_fx_lines, 3);

        // 7. code once helper function + expression call
        let ass7 = format!(
            "{}Comment: 0,0:00:00.00,0:00:00.00,Default,,0,0,0,code once,function dbl(v) return v * 2 end\n\
             Comment: 0,0:00:00.00,0:00:00.00,Default,,0,0,0,template syl,{{\\fs!dbl(24)!}}\n\
             Dialogue: 0,0:00:01.00,0:00:02.00,Default,,0,0,0,karaoke,{{\\k50}}ok\n",
            base_header()
        );
        let (s7, _) = run_ass(&ass7);
        assert_eq!(fx_lines(&s7)[0].text(), "{\\fs48}ok");

        // 8. relayer + restyle
        let ass8 = format!(
            "{}Comment: 0,0:00:00.00,0:00:00.00,Default,,0,0,0,template syl,!relayer(5)!!restyle(\"Romaji\")!{{\\b1}}\n\
             Dialogue: 0,0:00:01.00,0:00:02.00,Default,,0,0,0,karaoke,{{\\k50}}x\n",
            base_header()
        );
        let (s8, _) = run_ass(&ass8);
        assert_eq!(fx_lines(&s8)[0].layer(), 5);
        assert_eq!(fx_lines(&s8)[0].style(), "Romaji");

        // 9. fxgroup conditional gating
        let ass9 = format!(
            "{}Comment: 0,0:00:00.00,0:00:00.00,Default,,0,0,0,code once,fxgroup.off = false; fxgroup.on = true\n\
             Comment: 0,0:00:00.00,0:00:00.00,Default,,0,0,0,template syl fxgroup off,{{\\b1}}\n\
             Comment: 0,0:00:00.00,0:00:00.00,Default,,0,0,0,template syl fxgroup on,{{\\i1}}\n\
             Dialogue: 0,0:00:01.00,0:00:02.00,Default,,0,0,0,karaoke,{{\\k50}}z\n",
            base_header()
        );
        let (s9, r9) = run_ass(&ass9);
        assert_eq!(r9.generated_fx_lines, 1);
        assert_eq!(fx_lines(&s9)[0].text(), "{\\i1}z");

        // 10. noblank skips whitespace syllables
        let ass10 = format!(
            "{}Comment: 0,0:00:00.00,0:00:00.00,Default,,0,0,0,template syl noblank,{{\\b1}}\n\
             Dialogue: 0,0:00:01.00,0:00:03.00,Default,,0,0,0,karaoke,{{\\k30}}a{{\\k20}} {{\\k30}}b\n",
            base_header()
        );
        let (_, r10) = run_ass(&ass10);
        assert_eq!(r10.generated_fx_lines, 2);

        // 11. notext for vector drawing particles
        let ass11 = format!(
            "{}Comment: 0,0:00:00.00,0:00:00.00,Default,,0,0,0,template syl notext,{{\\p1}}m 0 0 l 10 0 10 10 0 10\n\
             Dialogue: 0,0:00:01.00,0:00:02.00,Default,,0,0,0,karaoke,{{\\k50}}hidden\n",
            base_header()
        );
        let (s11, _) = run_ass(&ass11);
        assert!(!fx_lines(&s11)[0].text().contains("hidden"));

        // 12. mixin appends extra tags to template syl
        let ass12 = format!(
            "{}Comment: 0,0:00:00.00,0:00:00.00,Default,,0,0,0,template syl,{{\\an5}}\n\
             Comment: 0,0:00:00.00,0:00:00.00,Default,,0,0,0,mixin syl,{{\\blur2}}\n\
             Dialogue: 0,0:00:01.00,0:00:02.00,Default,,0,0,0,karaoke,{{\\k50}}m\n",
            base_header()
        );
        let (s12, _) = run_ass(&ass12);
        assert_eq!(fx_lines(&s12)[0].text(), "{\\an5}{\\blur2}m");

        // 13. template all matches any style
        let ass13 = format!(
            "{}Comment: 0,0:00:00.00,0:00:00.00,Default,,0,0,0,template line all,{{\\fad(100,100)}}\n\
             Dialogue: 0,0:00:01.00,0:00:02.00,Romaji,,0,0,0,karaoke,{{\\k50}}r\n",
            base_header()
        );
        let (_, r13) = run_ass(&ass13);
        assert_eq!(r13.generated_fx_lines, 1);

        // 14. keeptags preserves original line tags
        let ass14 = format!(
            "{}Comment: 0,0:00:00.00,0:00:00.00,Default,,0,0,0,template line keeptags,{{\\an8}}\n\
             Dialogue: 0,0:00:01.00,0:00:02.00,Default,,0,0,0,karaoke,{{\\i1\\k50}}hi\n",
            base_header()
        );
        let (s14, _) = run_ass(&ass14);
        assert!(fx_lines(&s14)[0].text().contains("\\i1"));

        // 15. retime("start2syl", 0, 0) + retime("syl2end", 0, 0)
        let ass15 = format!(
            "{}Comment: 0,0:00:00.00,0:00:00.00,Default,,0,0,0,template syl,!retime(\"start2syl\",0,0)!{{\\1c&HFFFFFF&}}\n\
             Comment: 0,0:00:00.00,0:00:00.00,Default,,0,0,0,template syl,!retime(\"syl2end\",0,0)!{{\\1c&H00FFFF&}}\n\
             Dialogue: 0,0:00:01.00,0:00:03.00,Default,,0,0,0,karaoke,{{\\k50}}a{{\\k50}}b\n",
            base_header()
        );
        let (s15, r15) = run_ass(&ass15);
        assert_eq!(r15.generated_fx_lines, 4);
        assert_eq!(fx_lines(&s15)[3].end().as_cs(), 300);

        // 16. Idempotent regeneration (strip old fx + restore karaoke + re-apply)
        let mut s16 = s1;
        let engine = KaraTemplater::new(EngineConfig::default());
        let r16 = engine.apply(&mut s16, &MonoExtents).expect("regen");
        assert_eq!(r16.stripped_fx_lines, 2);
        assert_eq!(r16.generated_fx_lines, 2);
        assert_eq!(fx_lines(&s16).len(), 2);
    }

    #[test]
    fn sandbox_blocks_io_os_loadlib_and_infinite_loops() {
        // Blocked io/os
        let ass_io = format!(
            "{}Comment: 0,0:00:00.00,0:00:00.00,Default,,0,0,0,code once,io.open(\"secret\")\n\
             Dialogue: 0,0:00:01.00,0:00:02.00,Default,,0,0,0,karaoke,{{\\k50}}a\n",
            base_header()
        );
        let mut s = parse_ass(&ass_io).value;
        let err = KaraTemplater::new(EngineConfig::default())
            .apply(&mut s, &MonoExtents)
            .expect_err("io must be blocked");
        assert!(matches!(err, KaraError::LuaError { line_index: 0, .. }));

        // Infinite loop aborted by instruction budget
        let ass_loop = format!(
            "{}Comment: 0,0:00:00.00,0:00:00.00,Default,,0,0,0,code once,while true do end\n\
             Dialogue: 0,0:00:01.00,0:00:02.00,Default,,0,0,0,karaoke,{{\\k50}}a\n",
            base_header()
        );
        let mut s2 = parse_ass(&ass_loop).value;
        let cfg = EngineConfig {
            max_instructions: 50_000,
            ..EngineConfig::default()
        };
        let err2 = KaraTemplater::new(cfg)
            .apply(&mut s2, &MonoExtents)
            .expect_err("infinite loop must time out");
        assert!(matches!(err2, KaraError::LuaError { line_index: 0, .. }));
    }

    #[test]
    fn bit_library_compat_works_under_lua51_and_luajit() {
        let ass = format!(
            "{}{}",
            base_header(),
            r"Comment: 0,0:00:00.00,0:00:00.00,Default,,0,0,0,template syl,{\1c&H!bit.tohex(bit.bor(bit.lshift(0x12,8),0x34), -4)!&}
Dialogue: 0,0:00:01.00,0:00:02.00,Default,,0,0,0,karaoke,{\k50}bit
"
        );
        let (script, report) = run_ass(&ass);
        assert_eq!(report.generated_fx_lines, 1);
        assert_eq!(fx_lines(&script)[0].text(), r"{\1c&H1234&}bit");
    }
}
